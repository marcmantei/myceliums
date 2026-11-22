//! Call resolution: which symbol does each call site call?
//!
//! The parsers report every call with its caller's name, its line, the
//! callee's name, how the call site names it ([`CallForm`]) and, for a call
//! on a receiver, the receiver's expression chain. [`CallResolver`] turns
//! them into CALLS edges.
//!
//! - **The caller** is found by position: the innermost definition in the
//!   call's file whose lines contain the call site. Never by name.
//! - **The callee** is looked up the way the language binds names, nearest
//!   scope first, and only among symbols of the caller's language family
//!   ([`language_family`]):
//!   1. a bare call (`f()`): the enclosing class's members where an
//!      unqualified call reaches them (Java), the caller's own file, the
//!      file's imports, the package (Go: the directory). In a language whose
//!      extractor records call forms nothing else is in scope, so a name
//!      found nowhere there (a built-in, a library function) gets no edge.
//!   2. a call on a receiver (`x.f()`): through the module an import binds
//!      (`config.parse()`), else any *method* of that name — never a free
//!      function — and nothing for a receiver that is a literal or a
//!      library module.
//!   3. a constructor call (`new X()`): the class `X` in scope.
//!   4. a call from a language whose extractor does not record call forms:
//!      by name, same file first, then imports, the directory and the whole
//!      language family.
//!
//!   Where several symbols remain, the call is linked to each of them,
//!   labelled `ambiguous`, as long as there are at most
//!   [`MAX_AMBIGUOUS_TARGETS`]; beyond that it gets no edge.
//!
//! Every edge records how its target was found ([`CallResolution`]).

mod imports;
mod js_project;
mod types;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use myceliums_storage::{
    CodeSymbol, Relationship, RelationshipKind, Store, SymbolKind, SymbolMetadata,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::parser::{
    CallForm, CallReference, DeclaredTypes, ExpressionStep, ImportInfo, TypeBinding,
};
use imports::Bound;
pub(crate) use js_project::is_config_file as is_resolution_config_name;
pub use js_project::JsProject;
use types::Ty;

/// How the resolver picked the target of a CALLS edge.
///
/// Recorded at extraction time in the edge's metadata (`resolved:<label>`)
/// so consumers can tell a certain edge from a guess. Edges written before
/// this existed carry no label; [`CallResolution::from_metadata`] returns
/// `None` for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallResolution {
    /// Several symbols qualify; the call is linked to each of them.
    Ambiguous,
    /// Found by name in the nearest scope that has it (the caller's file,
    /// its package, or the whole language family), where exactly one
    /// symbol qualifies.
    NameMatch,
    /// Bound by an import: an imported name, or a member of an imported
    /// module.
    Import,
    /// Determined by the call's syntax and the declarations in view: a
    /// constructor call, a method of the enclosing class, a qualified name
    /// exactly one symbol has.
    Exact,
}

impl CallResolution {
    /// The label written to and read from edge metadata.
    pub fn label(self) -> &'static str {
        match self {
            CallResolution::Exact => "exact",
            CallResolution::Import => "import",
            CallResolution::NameMatch => "name-match",
            CallResolution::Ambiguous => "ambiguous",
        }
    }

    /// Reads the label back from a CALLS edge's metadata
    /// (`line:<n>;resolved:<label>`); `None` for unlabelled edges.
    pub fn from_metadata(metadata: &str) -> Option<CallResolution> {
        let label = metadata
            .split(';')
            .find_map(|part| part.strip_prefix("resolved:"))?;
        match label {
            "exact" => Some(CallResolution::Exact),
            "import" => Some(CallResolution::Import),
            "name-match" => Some(CallResolution::NameMatch),
            "ambiguous" => Some(CallResolution::Ambiguous),
            _ => None,
        }
    }
}

/// Most symbols an ambiguous call is linked to. A call that several symbols
/// qualify for gets an edge to each, labelled `ambiguous`, up to this many;
/// beyond it the name says too little and the call gets no edge.
pub const MAX_AMBIGUOUS_TARGETS: usize = 2;

/// Why a call got no edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unlinked {
    /// More than [`MAX_AMBIGUOUS_TARGETS`] symbols qualify.
    TooManyCandidates,
    /// The target is outside the project: a library value or import, a
    /// literal, a member a typed receiver's class does not declare.
    Library,
    /// A call through a parameter, local variable or imported value.
    ThroughVariable,
    /// Nothing in scope has the name: a built-in, a global, a name the
    /// resolver cannot see.
    Unresolved,
}

/// The targets of a call, or why it has none.
type Targets = Result<Vec<(DefId, CallResolution)>, Unlinked>;

/// What became of the calls one analysis resolved: how many got an edge,
/// and why the others did not (#139). Persisted in the index (`index_meta`,
/// key [`CallResolutionStats::META_KEY`]) so the caller tools can say how
/// many call sites of a name were left unlinked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallResolutionStats {
    /// Call sites linked to at least one symbol.
    pub linked: usize,
    /// Call sites more than [`MAX_AMBIGUOUS_TARGETS`] symbols qualify for.
    pub too_many_candidates: usize,
    /// Call sites whose target is outside the project.
    pub library: usize,
    /// Call sites through a parameter, variable or callback.
    pub through_variable: usize,
    /// Call sites of a name nothing in scope has.
    pub unresolved: usize,
    /// Call sites outside every function (module-level code).
    pub no_caller: usize,
    /// Unlinked call sites (too many candidates, or a project name nothing
    /// in scope binds), per family and name: calls those symbols may
    /// receive that no edge shows. Derived from
    /// [`unlinked_by_caller`](Self::unlinked_by_caller) (its
    /// [`UnlinkedReason::TooManyCandidates`] and
    /// [`UnlinkedReason::NotInScope`] entries), so dropping a file's entries
    /// after an incremental re-index drops its call sites here too (#169).
    #[serde(default)]
    pub unlinked_by_name: BTreeMap<String, BTreeMap<String, usize>>,
    /// The call sites each caller makes that got no edge but may reach
    /// project code, per file and caller qualified name (#169): more than
    /// [`MAX_AMBIGUOUS_TARGETS`] candidates, a name a project symbol has
    /// that nothing in scope binds, a call through a variable, parameter or
    /// callback. A walk over CALLS edges does not follow them; the tools
    /// name them as places a flow may continue unseen. Library calls are
    /// not recorded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unlinked_by_caller: BTreeMap<String, BTreeMap<String, Vec<UnlinkedCall>>>,
    /// Version of the per-caller record
    /// ([`myceliums_storage::schema::CALL_RECORD_VERSION`]); `0` in a record
    /// written before [`unlinked_by_caller`](Self::unlinked_by_caller)
    /// existed, whose callers' unlinked calls are unknown, not absent.
    #[serde(default)]
    pub version: u32,
    /// Files re-indexed incrementally since the last full analysis: their
    /// entries in [`unlinked_by_caller`](Self::unlinked_by_caller) were
    /// dropped, so their callers' unlinked calls are unknown until the next
    /// full analysis (#169).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub stale_files: BTreeSet<String>,
}

/// A call site that got no CALLS edge though it may reach project code
/// (see [`CallResolutionStats::unlinked_by_caller`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlinkedCall {
    /// The name called.
    #[serde(rename = "n")]
    pub name: String,
    /// The line of the call site.
    #[serde(rename = "l")]
    pub line: u32,
    /// Why it got no edge.
    #[serde(rename = "r")]
    pub reason: UnlinkedReason,
}

/// Why an [`UnlinkedCall`] got no edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnlinkedReason {
    /// More than [`MAX_AMBIGUOUS_TARGETS`] symbols qualify.
    TooManyCandidates,
    /// A project symbol has the name, but nothing in scope binds it.
    NotInScope,
    /// A call through a parameter, local variable, field or callback.
    ThroughVariable,
}

impl UnlinkedReason {
    /// Why, in a few words, for tool output.
    pub fn describe(self) -> String {
        match self {
            UnlinkedReason::TooManyCandidates => {
                format!("more than {MAX_AMBIGUOUS_TARGETS} candidates")
            }
            UnlinkedReason::NotInScope => "name not bound in scope".to_string(),
            UnlinkedReason::ThroughVariable => "through a variable or callback".to_string(),
        }
    }

    /// The reason a per-caller record keeps for `reason`, if any.
    fn of(reason: Unlinked, has_namesake: bool) -> Option<Self> {
        match reason {
            Unlinked::TooManyCandidates => Some(UnlinkedReason::TooManyCandidates),
            Unlinked::Unresolved if has_namesake => Some(UnlinkedReason::NotInScope),
            Unlinked::ThroughVariable => Some(UnlinkedReason::ThroughVariable),
            Unlinked::Unresolved | Unlinked::Library => None,
        }
    }
}

impl CallResolutionStats {
    /// Key under which the record is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "call_resolution";

    /// Call sites that got no edge, for any reason.
    pub fn unlinked(&self) -> usize {
        self.too_many_candidates
            + self.library
            + self.through_variable
            + self.unresolved
            + self.no_caller
    }

    /// Unlinked call sites named `name` in `family` (see
    /// [`language_family`]) that may be calls of a project symbol of that
    /// name.
    pub fn unlinked_named(&self, family: &str, name: &str) -> usize {
        self.unlinked_by_name
            .get(family)
            .and_then(|names| names.get(name))
            .copied()
            .unwrap_or(0)
    }

    /// Whether the record holds every caller's unlinked calls: written by an
    /// analysis that records them ([`version`](Self::version)).
    pub fn records_unlinked_calls(&self) -> bool {
        self.version >= myceliums_storage::schema::CALL_RECORD_VERSION
    }

    /// Whether `file`'s unlinked calls are unknown: re-indexed incrementally
    /// since the last full analysis.
    pub fn is_stale(&self, file: &str) -> bool {
        self.stale_files.contains(file)
    }

    /// After re-indexing `changed` incrementally: their unlinked calls are
    /// no longer known (the incremental resolver sees only the changed
    /// files, so what it leaves unlinked is not comparable), and `deleted`
    /// files have none.
    pub fn forget_files<'a>(
        &mut self,
        changed: impl IntoIterator<Item = &'a str>,
        deleted: impl IntoIterator<Item = &'a str>,
    ) {
        for file in changed {
            self.unlinked_by_caller.remove(file);
            self.stale_files.insert(file.to_string());
        }
        for file in deleted {
            self.unlinked_by_caller.remove(file);
            self.stale_files.remove(file);
        }
        // The per-name counts follow the per-caller record; an older record
        // has no per-caller entries to derive them from.
        if self.records_unlinked_calls() {
            self.unlinked_by_name = self.names_from_callers();
        }
    }

    /// The call sites the symbol `qualified_name` in `file` makes that got
    /// no edge but may reach project code, in line order.
    pub fn unlinked_from(&self, file: &str, qualified_name: &str) -> &[UnlinkedCall] {
        self.unlinked_by_caller
            .get(file)
            .and_then(|callers| callers.get(qualified_name))
            .map_or(&[], Vec::as_slice)
    }

    /// One line for `analyze` output.
    pub fn summary(&self) -> String {
        format!(
            "{} call sites linked; {} not: {} with more than {MAX_AMBIGUOUS_TARGETS} candidates, \
             {} into libraries, {} through variables, {} with nothing in scope, {} outside any function",
            self.linked,
            self.unlinked(),
            self.too_many_candidates,
            self.library,
            self.through_variable,
            self.unresolved,
            self.no_caller
        )
    }

    fn count(&mut self, reason: Unlinked) {
        match reason {
            Unlinked::TooManyCandidates => self.too_many_candidates += 1,
            Unlinked::Library => self.library += 1,
            Unlinked::ThroughVariable => self.through_variable += 1,
            Unlinked::Unresolved => self.unresolved += 1,
        }
    }

    /// [`unlinked_by_name`](Self::unlinked_by_name) as the per-caller
    /// record implies it.
    fn names_from_callers(&self) -> BTreeMap<String, BTreeMap<String, usize>> {
        let mut names: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for (file, callers) in &self.unlinked_by_caller {
            let Some(family) = language_family(file) else {
                continue;
            };
            for call in callers.values().flatten() {
                if matches!(
                    call.reason,
                    UnlinkedReason::TooManyCandidates | UnlinkedReason::NotInScope
                ) {
                    *names
                        .entry(family.to_string())
                        .or_default()
                        .entry(call.name.clone())
                        .or_default() += 1;
                }
            }
        }
        names
    }

    /// Persist the record inside the index.
    pub async fn record(&self, store: &Store) -> crate::error::Result<()> {
        store
            .set_index_meta(Self::META_KEY, &serde_json::to_string(self)?)
            .await?;
        Ok(())
    }

    /// Read the record from an index; `None` for an index analyzed before
    /// it existed.
    pub async fn load(store: &Store) -> crate::error::Result<Option<Self>> {
        match store.get_index_meta(Self::META_KEY).await? {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }
}

/// The language family of a source file, by extension: a call is only ever
/// resolved to a symbol of its own family (#139).
///
/// | family | extensions |
/// |---|---|
/// | `js` | `ts`, `tsx`, `mts`, `cts`, `js`, `jsx`, `mjs`, `cjs`, `vue`, `svelte` |
/// | `c` | `c`, `h`, `cpp`, `cc`, `cxx`, `hpp`, `hh`, `hxx`, `m`, `mm` |
/// | `jvm` | `java`, `kt`, `kts`, `scala` |
/// | `python` | `py`, `pyi`, `ipynb` |
/// | `csharp` | `cs` |
/// | `elixir` | `ex`, `exs` |
/// | `powershell` | `ps1` |
///
/// Every other extension is a family of its own (`rs`, `go`, `rb`, `swift`,
/// `php`, `lua`, `zig`, `dart`, ...). The groups are languages that call each
/// other directly, with no binding layer: TypeScript and JavaScript (and the
/// script blocks of Vue and Svelte files), C, C++ and Objective-C through
/// shared headers, Java, Kotlin and Scala on the JVM. Calls across any other
/// boundary (FFI, generated bindings, HTTP) are not resolved.
pub fn language_family(path: &str) -> Option<&str> {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = file.rsplit_once('.')?;
    Some(match ext {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => "js",
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" => "c",
        "java" | "kt" | "kts" | "scala" => "jvm",
        "py" | "pyi" | "ipynb" => "python",
        "cs" => "csharp",
        "ex" | "exs" => "elixir",
        "ps1" => "powershell",
        other => other,
    })
}

/// Families where an unqualified call inside a class reaches the class's
/// own methods (Java's implicit `this`).
fn implicit_this(family: &str) -> bool {
    family == "jvm"
}

/// Families where every file of a directory shares one namespace (a Go
/// package; a Java package for class names).
fn package_scope(family: &str) -> bool {
    matches!(family, "go" | "jvm")
}

type DefId = usize;

/// A registered symbol.
struct Def {
    uid: String,
    name: String,
    /// The symbol's qualified name, as stored.
    qualified: String,
    kind: SymbolKind,
    file: String,
    family: String,
    start: u32,
    end: u32,
    /// The container's qualified name (`UserService` for
    /// `UserService.register`); `None` at the top level of a file.
    container: Option<String>,
    /// For a definition nested in a function or method body (a local
    /// function, a closure the extractor kept): that function's lines. It
    /// is visible only there.
    enclosing_fn: Option<(u32, u32)>,
    /// Declared return type, as written.
    return_type: Option<String>,
    /// Base classes, implemented interfaces, embedded structs, Rust traits
    /// it implements: as written, and where to resolve them.
    bases: Vec<BaseRef>,
    /// The bases its own symbol metadata names (the rest come from
    /// declarations elsewhere, merged by `prepare`).
    own_bases: Vec<BaseRef>,
    /// Number of parameters, where it selects among overloads (Java);
    /// `None` for varargs or elsewhere.
    params: Option<u32>,
}

impl Def {
    fn contains(&self, line: u32) -> bool {
        self.start <= line && line <= self.end
    }

    fn is_function_like(&self) -> bool {
        matches!(self.kind, SymbolKind::Function | SymbolKind::Method)
    }

    /// A type that can have methods. In Go any named type can
    /// (`type methodTrees []methodTree`); the extractor calls those type
    /// aliases.
    fn is_class_like(&self) -> bool {
        matches!(
            self.kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum
        ) || (self.kind == SymbolKind::TypeAlias && self.family == "go")
    }

    /// Something a call can land on: a function, a method, or a class
    /// (instantiation).
    fn is_callable(&self) -> bool {
        self.is_function_like() || self.kind == SymbolKind::Class
    }

    /// Defined at the top level of its file (not in a class, not in a
    /// function body).
    fn is_top_level(&self) -> bool {
        self.container.is_none() && self.enclosing_fn.is_none()
    }

    fn dir(&self) -> &str {
        parent_dir(&self.file)
    }
}

/// A base type as written, and where it is resolved: at the definition
/// itself (`None`), or at a `file:line` elsewhere (a Rust `impl Trait for
/// Type`, whose trait is in scope there).
#[derive(Debug, Clone, PartialEq)]
struct BaseRef {
    text: String,
    at: Option<(String, u32)>,
}

/// The directory part of a path (`""` for a file at the root).
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// What a parsed file declares beyond its symbols.
#[derive(Default)]
struct FileContext {
    imports: Vec<ImportInfo>,
    /// `handler = authenticate`: local name → aliased name.
    aliases: Vec<(String, String)>,
    /// Parameters, locals and fields with their types or initialisers.
    bindings: Vec<TypeBinding>,
    /// Return and base types of the file's definitions, where the symbol
    /// metadata does not carry them.
    declared: Vec<DeclaredTypes>,
}

/// Where a call is made (or a declaration is evaluated).
struct Site<'a> {
    file: &'a str,
    family: &'a str,
    line: u32,
    /// The function or method the site is in.
    caller: Option<DefId>,
    context: Option<&'a FileContext>,
    /// A binding not to see: the one whose initialiser is being evaluated.
    exclude: Option<usize>,
}

/// Resolves call references to symbol UIDs. Two passes: register every
/// symbol ([`register_symbol`](Self::register_symbol)) and every file's
/// imports ([`register_file`](Self::register_file)), then
/// [`resolve_calls`](Self::resolve_calls).
#[derive(Default)]
pub struct CallResolver {
    defs: Vec<Def>,
    by_name: HashMap<String, Vec<DefId>>,
    by_file: HashMap<String, Vec<DefId>>,
    files: HashMap<String, FileContext>,
    index: imports::PathIndex,
    /// Workspace packages and tsconfig path aliases (#169).
    js: JsProject,
    prepared: bool,
    base_memo: types::BaseMemo,
    stats: CallResolutionStats,
}

impl CallResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pass 1: register a symbol definition.
    pub fn register_symbol(&mut self, symbol: &CodeSymbol) {
        let id = self.defs.len();
        let container = symbol
            .qualified_name
            .strip_suffix(symbol.name.as_str())
            .and_then(|prefix| prefix.strip_suffix('.'))
            .filter(|prefix| !prefix.is_empty())
            .map(str::to_string);
        let family = language_family(&symbol.file_path)
            .unwrap_or_default()
            .to_string();
        let metadata: SymbolMetadata = symbol
            .metadata
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let params = (family == "jvm")
            .then(|| param_count(&symbol.signature, &symbol.name))
            .flatten();
        self.defs.push(Def {
            uid: symbol.uid.clone(),
            name: symbol.name.clone(),
            qualified: symbol.qualified_name.clone(),
            kind: symbol.kind.clone(),
            file: symbol.file_path.clone(),
            family,
            start: symbol.start_line,
            end: symbol.end_line,
            container,
            enclosing_fn: None,
            return_type: metadata.return_type,
            bases: Vec::new(),
            own_bases: metadata
                .superclasses
                .into_iter()
                .map(|text| BaseRef { text, at: None })
                .collect(),
            params,
        });
        self.by_name
            .entry(symbol.name.clone())
            .or_default()
            .push(id);
        self.by_file
            .entry(symbol.file_path.clone())
            .or_default()
            .push(id);
        self.prepared = false;
    }

    /// Pass 1: register what a parsed file declares beyond its symbols: its
    /// imports, the aliases SSA analysis found (`handler = authenticate`),
    /// its typed bindings and its definitions' return and base types.
    /// Register every parsed file, also one without symbols: imports are
    /// resolved to files.
    pub fn register_file(
        &mut self,
        path: &str,
        imports: &[ImportInfo],
        aliases: &[(String, String)],
        bindings: &[TypeBinding],
        declared: &[DeclaredTypes],
    ) {
        let context = self.files.entry(path.to_string()).or_default();
        context.imports.extend(imports.iter().cloned());
        context.aliases.extend(aliases.iter().cloned());
        context.bindings.extend(bindings.iter().cloned());
        context.declared.extend(declared.iter().cloned());
        self.prepared = false;
    }

    /// Pass 1: the repository's own JS/TS module names — workspace
    /// packages and tsconfig/jsconfig `paths` (see [`JsProject`]). Without
    /// it a bare specifier is always a library.
    pub fn set_js_project(&mut self, project: JsProject) {
        self.js = project;
    }

    /// Mark definitions nested in function bodies and index file paths.
    fn prepare(&mut self) {
        if self.prepared {
            return;
        }
        for ids in self.by_file.values_mut() {
            // Outer definitions first: by start line, the longer span first.
            ids.sort_by_key(|&id| (self.defs[id].start, std::cmp::Reverse(self.defs[id].end)));
            let mut open: Vec<(u32, u32)> = Vec::new();
            for &id in ids.iter() {
                let def = &self.defs[id];
                while open.last().is_some_and(|&(_, end)| end < def.start) {
                    open.pop();
                }
                let enclosing = open
                    .last()
                    .copied()
                    .filter(|&(start, end)| (start, end) != (def.start, def.end));
                let is_fn = def.is_function_like();
                let span = (def.start, def.end);
                self.defs[id].enclosing_fn = enclosing;
                if is_fn {
                    open.push(span);
                }
            }
        }
        for def in &mut self.defs {
            def.bases = def.own_bases.clone();
        }
        let mut files: Vec<&String> = self.files.keys().collect();
        files.sort();
        for file in files {
            let context = &self.files[file];
            for declared in &context.declared {
                let target = if declared.elsewhere {
                    self.declared_type(file, &declared.name)
                } else {
                    self.by_file.get(file).and_then(|ids| {
                        ids.iter().copied().find(|&id| {
                            self.defs[id].name == declared.name
                                && self.defs[id].start == declared.line
                        })
                    })
                };
                let Some(id) = target else {
                    continue;
                };
                let at = declared.elsewhere.then(|| (file.clone(), declared.line));
                let def = &mut self.defs[id];
                if def.return_type.is_none() {
                    def.return_type = declared.return_type.clone();
                }
                def.bases.extend(declared.bases.iter().map(|text| BaseRef {
                    text: text.clone(),
                    at: at.clone(),
                }));
            }
        }
        let paths = self.by_file.keys().chain(self.files.keys());
        self.index = imports::PathIndex::new(paths);
        self.base_memo = types::BaseMemo::default();
        self.prepared = true;
    }

    /// The type a declaration in `file` names (`impl Trait for Type`): the
    /// class-like definition named `name` in that file, else its directory,
    /// else the only one in its language family.
    fn declared_type(&self, file: &str, name: &str) -> Option<DefId> {
        let family = language_family(file)?;
        let types: Vec<DefId> = self
            .named(name)
            .filter(|&id| self.defs[id].is_class_like() && self.defs[id].family == family)
            .collect();
        let pick = |accept: &dyn Fn(&Def) -> bool| -> Option<DefId> {
            types.iter().copied().find(|&id| accept(&self.defs[id]))
        };
        pick(&|d| d.file == file)
            .or_else(|| pick(&|d| d.dir() == parent_dir(file)))
            .or(match types.as_slice() {
                [only] => Some(*only),
                _ => None,
            })
    }

    /// Pass 2: resolve call references to CALLS relationships.
    ///
    /// One edge per caller/callee pair; it carries the line of the call that
    /// found it with the strongest [`CallResolution`], in its metadata:
    /// `line:<n>;resolved:<label>`.
    pub fn resolve_calls(&mut self, calls: &[CallReference], repo_id: &str) -> Vec<Relationship> {
        self.prepare();
        let mut stats = CallResolutionStats::default();
        let mut edges: Vec<(DefId, DefId, CallResolution, u32)> = Vec::new();
        let mut seen: HashMap<(DefId, DefId), usize> = HashMap::new();
        for call in calls {
            let Some((site, caller)) = self.site(call) else {
                stats.no_caller += 1;
                continue;
            };
            let targets = match self.targets(&site, call) {
                Ok(targets) => targets,
                Err(reason) => {
                    let namesake = self.named(&call.callee_name).any(|id| {
                        let def = &self.defs[id];
                        def.family == site.family && def.is_callable()
                    });
                    stats.count(reason);
                    if let Some(reason) = UnlinkedReason::of(reason, namesake) {
                        let def = &self.defs[caller];
                        stats
                            .unlinked_by_caller
                            .entry(def.file.clone())
                            .or_default()
                            .entry(def.qualified.clone())
                            .or_default()
                            .push(UnlinkedCall {
                                name: call.callee_name.clone(),
                                line: call.line,
                                reason,
                            });
                    }
                    continue;
                }
            };
            stats.linked += 1;
            for (target, resolution) in targets {
                match seen.get(&(caller, target)) {
                    Some(&i) => {
                        if resolution > edges[i].2 {
                            edges[i].2 = resolution;
                            edges[i].3 = call.line;
                        }
                    }
                    None => {
                        seen.insert((caller, target), edges.len());
                        edges.push((caller, target, resolution, call.line));
                    }
                }
            }
        }
        for callers in stats.unlinked_by_caller.values_mut() {
            for calls in callers.values_mut() {
                calls.sort_by(|a, b| (a.line, &a.name).cmp(&(b.line, &b.name)));
                calls.dedup();
            }
        }
        stats.unlinked_by_name = stats.names_from_callers();
        stats.version = myceliums_storage::schema::CALL_RECORD_VERSION;
        self.stats = stats;
        edges
            .into_iter()
            .map(|(source, target, resolution, line)| Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: self.defs[source].uid.clone(),
                target_uid: self.defs[target].uid.clone(),
                kind: RelationshipKind::Calls,
                repo_id: repo_id.to_string(),
                metadata: format!("line:{line};resolved:{}", resolution.label()),
            })
            .collect()
    }

    /// What became of the calls the last [`resolve_calls`](Self::resolve_calls)
    /// resolved.
    pub fn stats(&self) -> &CallResolutionStats {
        &self.stats
    }

    /// Where `call` is made: its file, family and caller. `None` for a call
    /// without a file or outside every definition.
    fn site<'a>(&'a self, call: &'a CallReference) -> Option<(Site<'a>, DefId)> {
        let file = call.file.as_deref()?;
        let family = language_family(file)?;
        let caller = self.caller_of(file, call)?;
        let site = Site {
            file,
            family,
            line: call.line,
            caller: Some(caller),
            context: self.files.get(file),
            exclude: None,
        };
        Some((site, caller))
    }

    /// The symbol a call is made from: the innermost definition in the call's
    /// file whose lines contain the call site, preferring one with the name
    /// the extractor gave as the caller (a closure or local function inside
    /// it may be extracted as a symbol of its own). Never a lookup by name
    /// alone.
    fn caller_of(&self, file: &str, call: &CallReference) -> Option<DefId> {
        let ids = self.by_file.get(file)?;
        let containing = || {
            ids.iter()
                .copied()
                .filter(|&id| self.defs[id].contains(call.line))
        };
        let innermost =
            |&id: &DefId| (self.defs[id].end - self.defs[id].start, self.defs[id].start);
        containing()
            .filter(|&id| self.defs[id].name == call.caller_name)
            .min_by_key(innermost)
            .or_else(|| {
                containing()
                    .filter(|&id| self.defs[id].is_function_like())
                    .min_by_key(innermost)
            })
    }

    /// The symbols `call` may call, with how each was found, or why it
    /// gets no edge.
    fn targets(&self, site: &Site, call: &CallReference) -> Targets {
        let name = call.callee_name.as_str();
        let targets = match call.form {
            CallForm::Bare => self.resolve_bare(site, name),
            CallForm::Member => match call.chain.as_deref().and_then(|c| receiver_steps(c, name)) {
                Some(receiver) => self.resolve_member(site, receiver, name),
                None => self.methods_named(site, name),
            },
            // `new X()`, `new ns.X()`: the class.
            CallForm::New => {
                let receiver = call.chain.as_deref().and_then(|c| receiver_steps(c, name));
                let class = match receiver {
                    Some(receiver) => {
                        let mut steps = receiver.to_vec();
                        steps.push(ExpressionStep::Field(name.to_string()));
                        self.eval(site, &steps, 0)
                    }
                    None => self.type_ty(site, name, 0),
                };
                match class {
                    Ty::Class(class) | Ty::Instance(class) => {
                        Ok(vec![(class, CallResolution::Exact)])
                    }
                    Ty::External => Err(Unlinked::Library),
                    _ => Err(Unlinked::Unresolved),
                }
            }
            CallForm::SuperConstructor => self.super_constructor(site),
            CallForm::Unknown => self.resolve_by_name(site, call),
        }?;
        let targets = self.by_arity(targets, call.arg_count);
        match targets.len() {
            0 => Err(Unlinked::Unresolved),
            // The name says too little: no edge.
            n if n > MAX_AMBIGUOUS_TARGETS => Err(Unlinked::TooManyCandidates),
            _ => Ok(targets),
        }
    }

    /// Of several overloads (Java), the one taking `arg_count` arguments.
    fn by_arity(
        &self,
        targets: Vec<(DefId, CallResolution)>,
        arg_count: Option<u32>,
    ) -> Vec<(DefId, CallResolution)> {
        let Some(count) = arg_count.filter(|_| targets.len() > 1) else {
            return targets;
        };
        let matching: Vec<DefId> = targets
            .iter()
            .map(|&(id, _)| id)
            .filter(|&id| self.defs[id].params == Some(count))
            .collect();
        match matching.as_slice() {
            // The overloads were found exactly; only their number made the
            // call look ambiguous.
            [one] => vec![(*one, CallResolution::Exact)],
            _ => targets,
        }
    }

    /// `super(...)`: the base class's constructor, else the base class.
    fn super_constructor(&self, site: &Site) -> Targets {
        let Some(class) = self.enclosing_class(site) else {
            return Err(Unlinked::Unresolved);
        };
        // A constructor call never reaches an interface the class
        // implements: `class E extends Lib implements Iface` with `Lib`
        // outside the project calls a library constructor (#169).
        let Some(base) = self
            .bases(class, 0)
            .into_iter()
            .find(|&b| self.defs[b].kind != SymbolKind::Interface)
        else {
            return Err(Unlinked::Library);
        };
        let base_name = self.defs[base].name.clone();
        let constructor = ["constructor", "__init__", base_name.as_str()]
            .iter()
            .map(|name| self.members_of(base, name))
            .find(|found| !found.is_empty());
        Ok(match constructor {
            Some(constructors) => pick(constructors, CallResolution::Exact),
            None => vec![(base, CallResolution::Exact)],
        })
    }

    // --- bare calls -------------------------------------------------------

    /// `f()`: nearest scope first; nothing outside the scopes the language
    /// has.
    fn resolve_bare(&self, site: &Site, name: &str) -> Targets {
        let name = self.unalias(site, name);
        // A parameter or local of that name shadows everything: a call
        // through a function value, unless it holds a class (`cls()`).
        // (A module-level binding is the definition itself: `export const
        // f = () => ...`.)
        let local = self
            .binding(site, name)
            .is_some_and(|(_, _, binding)| binding.scope.1 != u32::MAX);
        if local {
            return match self.eval(site, &[ExpressionStep::Ident(name.to_string())], 0) {
                Ty::Class(class) => Ok(vec![(class, CallResolution::Exact)]),
                _ => Err(Unlinked::ThroughVariable),
            };
        }
        if implicit_this(site.family) {
            let members = self.enclosing_members(site, name);
            if !members.is_empty() {
                return Ok(pick(members, CallResolution::Exact));
            }
        }
        let local = self.in_file(site, name, Def::is_callable);
        if !local.is_empty() {
            return Ok(pick(local, CallResolution::NameMatch));
        }
        if let Some(bound) = self.imported(site, name) {
            return match bound {
                Bound::Defs(defs) => {
                    let callable: Vec<DefId> = defs
                        .into_iter()
                        .filter(|&id| self.defs[id].is_callable())
                        .collect();
                    if callable.is_empty() {
                        // An imported constant or variable, called.
                        Err(Unlinked::ThroughVariable)
                    } else {
                        Ok(pick(callable, CallResolution::Import))
                    }
                }
                Bound::Value(..) => Err(Unlinked::ThroughVariable),
                Bound::Module(_) | Bound::Unknown => Err(Unlinked::Unresolved),
                Bound::External => Err(Unlinked::Library),
            };
        }
        if package_scope(site.family) {
            let package = self.in_package(site, name, |d| d.is_callable() && d.container.is_none());
            return Ok(pick(package, CallResolution::NameMatch));
        }
        Err(Unlinked::Unresolved)
    }

    /// The target of an SSA alias in the call's file (`handler =
    /// authenticate; handler()`), else `name`.
    fn unalias<'a>(&self, site: &Site<'a>, name: &'a str) -> &'a str {
        site.context
            .and_then(|c| c.aliases.iter().rev().find(|(local, _)| local == name))
            .map_or(name, |(_, target)| target.as_str())
    }

    /// Members named `name` of the classes enclosing the caller, innermost
    /// class first.
    /// Members named `name` of the classes enclosing the caller (or of
    /// their base classes), innermost class first.
    fn enclosing_members(&self, site: &Site, name: &str) -> Vec<DefId> {
        for class in self.enclosing_classes(site) {
            let members = self.methods_on(class, name, 0);
            if !members.is_empty() {
                return members;
            }
        }
        Vec::new()
    }

    /// The classes whose bodies contain the call site, innermost first.
    fn enclosing_classes(&self, site: &Site) -> Vec<DefId> {
        let mut classes: Vec<DefId> = self
            .by_file
            .get(site.file)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&id| self.defs[id].is_class_like() && self.defs[id].contains(site.line))
            .collect();
        classes.sort_by_key(|&id| self.defs[id].end - self.defs[id].start);
        classes
    }

    /// Methods and nested definitions named `name` declared in `class` — not
    /// in another class of the same name in its directory (Hono's
    /// `hono.ts` and `hono-base.ts` both declare `class Hono`).
    fn members_of(&self, class_id: DefId, name: &str) -> Vec<DefId> {
        let class = &self.defs[class_id];
        self.named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.family == class.family
                    && def.is_callable()
                    && def
                        .container
                        .as_deref()
                        .is_some_and(|c| c.rsplit('.').next() == Some(class.name.as_str()))
                    && (def.file == class.file || def.dir() == class.dir())
                    && self.container_class(id).is_none_or(|c| c == class_id)
            })
            .collect()
    }

    /// Definitions named `name` in the caller's file that are visible from
    /// the call site: one nested in a function that contains the call (it
    /// shadows the rest), else the top-level ones.
    fn in_file(&self, site: &Site, name: &str, accept: impl Fn(&Def) -> bool) -> Vec<DefId> {
        let (nested, top): (Vec<DefId>, Vec<DefId>) = self
            .named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                // A function nested in a method may carry the class as its
                // container; nested, it is local all the same.
                def.file == site.file
                    && accept(def)
                    && (def.container.is_none() || def.enclosing_fn.is_some())
                    && def
                        .enclosing_fn
                        .is_none_or(|(start, end)| start <= site.line && site.line <= end)
            })
            .partition(|&id| self.defs[id].enclosing_fn.is_some());
        let innermost = nested
            .iter()
            .map(|&id| self.defs[id].enclosing_fn.map_or(0, |(s, e)| e - s))
            .min();
        match innermost {
            Some(span) => nested
                .into_iter()
                .filter(|&id| self.defs[id].enclosing_fn.map_or(0, |(s, e)| e - s) == span)
                .collect(),
            None => top,
        }
    }

    /// Definitions named `name` in the other files of the caller's
    /// directory.
    fn in_package(&self, site: &Site, name: &str, accept: impl Fn(&Def) -> bool) -> Vec<DefId> {
        let dir = parent_dir(site.file);
        self.named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.family == site.family
                    && def.file != site.file
                    && def.dir() == dir
                    && def.enclosing_fn.is_none()
                    && accept(def)
            })
            .collect()
    }

    /// Top-level definitions named `name` in `files`.
    fn top_level_in(
        &self,
        files: &[String],
        name: &str,
        accept: impl Fn(&Def) -> bool,
    ) -> Vec<DefId> {
        self.named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.is_top_level() && accept(def) && files.contains(&def.file)
            })
            .collect()
    }

    fn named<'a>(&'a self, name: &str) -> impl Iterator<Item = DefId> + 'a {
        self.by_name.get(name).into_iter().flatten().copied()
    }

    /// What the import of `name` in the caller's file binds, if `name` is
    /// imported.
    fn imported(&self, site: &Site, name: &str) -> Option<Bound> {
        let context = site.context?;
        if let Some(import) = context.imports.iter().find(|i| i.local_name == name) {
            if let Some(bound) = self.resolve_import(site, import) {
                return Some(bound);
            }
        }
        // A wildcard import (`from m import *`, `use m::*`, Go's `import .`)
        // binds every top-level name of its module.
        context
            .imports
            .iter()
            .filter(|i| imports::is_wildcard(site.family, i))
            .find_map(|import| match self.resolve_import(site, import)? {
                Bound::Module(files) => {
                    let defs = self.top_level_in(&files, name, |_| true);
                    (!defs.is_empty()).then_some(Bound::Defs(defs))
                }
                _ => None,
            })
    }

    /// Classes named `name` visible from the call site: nested in an
    /// enclosing class (Java), in the caller's file, imported, or (Go, Java)
    /// in the same package.
    fn classes_in_scope(&self, site: &Site, name: &str) -> Vec<DefId> {
        let local: Vec<DefId> = self
            .named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.file == site.file && def.is_class_like()
            })
            .collect();
        if !local.is_empty() {
            return local;
        }
        match self.imported(site, name) {
            Some(Bound::Defs(defs)) => {
                return defs
                    .into_iter()
                    .filter(|&id| self.defs[id].is_class_like())
                    .collect()
            }
            Some(Bound::Module(files)) => {
                return self.top_level_in(&files, name, Def::is_class_like)
            }
            Some(Bound::Value(..) | Bound::Unknown | Bound::External) => return Vec::new(),
            None => {}
        }
        if package_scope(site.family) {
            return self.in_package(site, name, Def::is_class_like);
        }
        Vec::new()
    }

    // --- calls on a receiver ---------------------------------------------

    /// `receiver.name()`: on a typed receiver the class's method (or a base
    /// class's), on a module its top-level function, on a library value
    /// nothing; on a receiver of unknown type any method of that name.
    fn resolve_member(&self, site: &Site, receiver: &[ExpressionStep], name: &str) -> Targets {
        match self.eval(site, receiver, 0) {
            // A member the class and its project bases do not declare: from
            // a library base class — unless a base is unresolved (then the
            // receiver is as good as untyped) or, in Rust, a project trait
            // declares it (the type may implement that trait elsewhere).
            Ty::Instance(class) | Ty::Class(class) => match self.methods_on(class, name, 0) {
                methods if !methods.is_empty() => Ok(pick(methods, CallResolution::Exact)),
                _ if self.open_hierarchy_of(class) => self.methods_named(site, name),
                _ if self.defs[class].family == "rs" => match self.trait_methods("rs", name) {
                    methods if methods.is_empty() => Err(Unlinked::Library),
                    methods => Ok(pick(methods, CallResolution::NameMatch)),
                },
                _ => Err(Unlinked::Library),
            },
            Ty::Module(files) => match self.module_member(&files, site.family, name) {
                Some(Bound::Defs(defs)) => Ok(pick(
                    defs.into_iter()
                        .filter(|&id| self.defs[id].is_callable())
                        .collect(),
                    CallResolution::Import,
                )),
                Some(Bound::Value(..)) => Err(Unlinked::ThroughVariable),
                Some(Bound::External) => Err(Unlinked::Library),
                _ => Err(Unlinked::Unresolved),
            },
            Ty::Function(_) | Ty::External => Err(Unlinked::Library),
            Ty::Unknown => self.methods_named(site, name),
        }
    }

    /// Any method named `name` in the caller's language family: the call is
    /// on a receiver whose type is not known.
    fn methods_named(&self, site: &Site, name: &str) -> Targets {
        let methods = self
            .named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.family == site.family && def.kind == SymbolKind::Method
            })
            .collect();
        Ok(pick(methods, CallResolution::NameMatch))
    }

    // --- languages without call forms -----------------------------------

    /// A call from an extractor that does not record call forms: by name,
    /// nearest scope first, any callable kind.
    fn resolve_by_name(&self, site: &Site, call: &CallReference) -> Targets {
        let name = self.unalias(site, &call.callee_name);
        // `Receiver.method` where exactly one symbol has that qualified name.
        if let Some(receiver) = call.chain.as_deref().and_then(|chain| {
            chain.iter().find_map(|step| match step {
                ExpressionStep::Ident(name) => Some(name.as_str()),
                _ => None,
            })
        }) {
            let qualified: Vec<DefId> = self
                .named(name)
                .filter(|&id| {
                    let def = &self.defs[id];
                    def.family == site.family
                        && def
                            .container
                            .as_deref()
                            .is_some_and(|c| c.rsplit('.').next() == Some(receiver))
                })
                .collect();
            if qualified.len() == 1 {
                return Ok(pick(qualified, CallResolution::Exact));
            }
        }
        let local = self.in_file(site, name, Def::is_callable);
        if !local.is_empty() {
            return Ok(pick(local, CallResolution::NameMatch));
        }
        if let Some(Bound::Defs(defs)) = self.imported(site, name) {
            let defs: Vec<DefId> = defs
                .into_iter()
                .filter(|&id| self.defs[id].is_callable())
                .collect();
            if !defs.is_empty() {
                return Ok(pick(defs, CallResolution::Import));
            }
        }
        let package = self.in_package(site, name, Def::is_callable);
        if !package.is_empty() {
            return Ok(pick(package, CallResolution::NameMatch));
        }
        let family = self
            .named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.family == site.family && def.is_callable()
            })
            .collect();
        Ok(pick(family, CallResolution::NameMatch))
    }
}

/// The number of parameters in a signature (`int add(T row, int copies)` →
/// 2); `None` for varargs or an unreadable signature.
fn param_count(signature: &str, name: &str) -> Option<u32> {
    let start = signature.find(&format!("{name}("))? + name.len() + 1;
    let rest = &signature[start..];
    let (mut depth, mut count, mut empty) = (0i32, 0u32, true);
    for (i, c) in rest.char_indices() {
        match c {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' if depth == 0 => {
                if rest[..i].contains("...") {
                    return None;
                }
                return Some(if empty { 0 } else { count + 1 });
            }
            ')' | '>' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => count += 1,
            c if !c.is_whitespace() => empty = false,
            _ => {}
        }
    }
    None
}

/// The receiver part of a call's chain: the steps before the final
/// `.name()`. `None` when the chain does not end in a call of `name`.
fn receiver_steps<'a>(chain: &'a [ExpressionStep], name: &str) -> Option<&'a [ExpressionStep]> {
    let chain = chain.strip_suffix(&[ExpressionStep::Call]).unwrap_or(chain);
    match chain.split_last() {
        Some((ExpressionStep::Field(last), receiver)) if last == name && !receiver.is_empty() => {
            Some(receiver)
        }
        _ => None,
    }
}

/// One candidate: an edge with the given label. Several: an `ambiguous`
/// edge to each (`CallResolver::targets` drops them beyond
/// [`MAX_AMBIGUOUS_TARGETS`]).
fn pick(mut candidates: Vec<DefId>, unique: CallResolution) -> Vec<(DefId, CallResolution)> {
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.len() {
        1 => vec![(candidates[0], unique)],
        _ => candidates
            .into_iter()
            .map(|id| (id, CallResolution::Ambiguous))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::BindingValue;

    /// A symbol `name` in `file`, lines `start..=end`; `container` is the
    /// class or type it belongs to.
    fn symbol(file: &str, qualified: &str, kind: SymbolKind, lines: (u32, u32)) -> CodeSymbol {
        let name = qualified.rsplit('.').next().unwrap();
        CodeSymbol {
            uid: format!("{file}::{qualified}"),
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: lines.0,
            end_line: lines.1,
            signature: String::new(),
            content: String::new(),
            repo_id: "repo".to_string(),
            metadata: None,
        }
    }

    fn function(file: &str, name: &str, lines: (u32, u32)) -> CodeSymbol {
        symbol(file, name, SymbolKind::Function, lines)
    }

    fn method(file: &str, qualified: &str, lines: (u32, u32)) -> CodeSymbol {
        symbol(file, qualified, SymbolKind::Method, lines)
    }

    fn class(file: &str, name: &str, lines: (u32, u32)) -> CodeSymbol {
        symbol(file, name, SymbolKind::Class, lines)
    }

    /// A call of `callee` at `file:line`, made from `caller`.
    fn call(file: &str, line: u32, caller: &str, callee: &str, form: CallForm) -> CallReference {
        CallReference {
            caller_name: caller.to_string(),
            callee_name: callee.to_string(),
            line,
            chain: None,
            file: Some(file.to_string()),
            form,
            arg_count: None,
        }
    }

    /// `receiver.callee()` with the receiver given as chain steps.
    fn member(
        file: &str,
        line: u32,
        caller: &str,
        receiver: Vec<ExpressionStep>,
        callee: &str,
    ) -> CallReference {
        let mut chain = receiver;
        chain.push(ExpressionStep::Field(callee.to_string()));
        chain.push(ExpressionStep::Call);
        CallReference {
            chain: Some(chain),
            ..call(file, line, caller, callee, CallForm::Member)
        }
    }

    fn ident(name: &str) -> ExpressionStep {
        ExpressionStep::Ident(name.to_string())
    }

    fn import(local: &str, module: &str, original: Option<&str>) -> ImportInfo {
        ImportInfo {
            local_name: local.to_string(),
            source_module: module.to_string(),
            original_name: original.map(str::to_string),
        }
    }

    /// Parameters of unknown type, visible in all of `file`, as the parser
    /// reports `def f(items, value)`.
    fn declare_unknown(resolver: &mut CallResolver, file: &str, names: &[&str]) {
        let bindings: Vec<TypeBinding> = names
            .iter()
            .map(|name| TypeBinding {
                name: name.to_string(),
                owner: None,
                scope: (1, 10_000),
                value: crate::parser::BindingValue::Unknown,
            })
            .collect();
        resolver.register_file(file, &[], &[], &bindings, &[]);
    }

    fn resolver(symbols: &[CodeSymbol]) -> CallResolver {
        let mut resolver = CallResolver::new();
        for symbol in symbols {
            resolver.register_symbol(symbol);
        }
        resolver
    }

    /// `(source uid, target uid, label)` of every edge `calls` produce.
    fn edges(
        resolver: &mut CallResolver,
        calls: &[CallReference],
    ) -> Vec<(String, String, &'static str)> {
        let mut edges: Vec<_> = resolver
            .resolve_calls(calls, "repo")
            .into_iter()
            .map(|r| {
                let label = CallResolution::from_metadata(&r.metadata)
                    .expect("labelled")
                    .label();
                (r.source_uid, r.target_uid, label)
            })
            .collect();
        edges.sort();
        edges
    }

    fn edge(source: &str, target: &str, label: &'static str) -> (String, String, &'static str) {
        (source.to_string(), target.to_string(), label)
    }

    /// Two functions named `parse` in different files: a call made inside
    /// the second one is attributed to the second one, by position, not to
    /// whichever `parse` a lookup by name returns (#139).
    #[test]
    fn the_caller_is_the_definition_containing_the_call_site() {
        let mut resolver = resolver(&[
            function("b.py", "parse", (10, 20)),
            function("b.py", "decode", (30, 31)),
            function("a.py", "parse", (1, 5)),
        ]);
        let inside = call("b.py", 12, "parse", "decode", CallForm::Bare);
        assert_eq!(
            edges(&mut resolver, std::slice::from_ref(&inside)),
            [edge("b.py::parse", "b.py::decode", "name-match")]
        );
        // Outside every definition, or without a file, a call has no caller.
        let outside = CallReference {
            line: 25,
            ..inside.clone()
        };
        let nowhere = CallReference {
            file: None,
            ..inside
        };
        assert!(edges(&mut resolver, &[outside, nowhere]).is_empty());
    }

    /// A TypeScript `.map()` never resolves to a Rust `map`, however unique
    /// the name; a `.vue` file and a `.ts` file are one family (#139).
    #[test]
    fn calls_resolve_only_within_the_callers_language_family() {
        let mut resolver = resolver(&[
            function("src/view.tsx", "render", (1, 9)),
            method("src/utils.rs", "Iter.map", (1, 3)),
            method("src/format.ts", "Fmt.format", (1, 3)),
            function("src/App.vue", "mount", (1, 3)),
        ]);
        declare_unknown(&mut resolver, "src/view.tsx", &["items", "value"]);
        let calls = [
            member("src/view.tsx", 2, "render", vec![ident("items")], "map"),
            member("src/view.tsx", 3, "render", vec![ident("value")], "format"),
            call("src/App.vue", 2, "mount", "render", CallForm::Unknown),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("src/App.vue::mount", "src/view.tsx::render", "name-match"),
                edge(
                    "src/view.tsx::render",
                    "src/format.ts::Fmt.format",
                    "name-match"
                ),
            ]
        );
    }

    #[test]
    fn language_families_group_languages_that_call_each_other_directly() {
        let family = |path| language_family(path).unwrap();
        assert_eq!(family("a/App.vue"), family("b/lib.ts"));
        assert_eq!(family("x.js"), family("y.tsx"));
        assert_eq!(family("main.c"), family("lib.hpp"));
        assert_eq!(family("View.m"), family("lib.h"));
        assert_eq!(family("Main.java"), family("Util.kt"));
        assert_ne!(family("main.rs"), family("main.go"));
        assert_ne!(family("main.py"), family("main.ts"));
        assert_eq!(language_family("Makefile"), None);
    }

    /// `parse()` in Python: the caller's own file first, then what the file
    /// imports; a method of that name is never a bare call's target, and a
    /// name in neither scope (a built-in) gets no edge.
    #[test]
    fn a_bare_call_binds_like_the_language_does() {
        let mut resolver = resolver(&[
            function("shop/auth.py", "parse", (1, 5)),
            function("shop/auth.py", "decode", (7, 8)),
            function("shop/config.py", "decode", (1, 2)),
            function("shop/validation.py", "validate", (1, 2)),
            method("shop/validation.py", "Validator.validate", (4, 5)),
            function("shop/other.py", "print", (1, 2)),
        ]);
        resolver.register_file(
            "shop/auth.py",
            &[import("validate", "shop.validation", Some("validate"))],
            &[],
            &[],
            &[],
        );
        let calls = [
            call("shop/auth.py", 2, "parse", "decode", CallForm::Bare),
            call("shop/auth.py", 3, "parse", "validate", CallForm::Bare),
            call("shop/auth.py", 4, "parse", "print", CallForm::Bare),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("shop/auth.py::parse", "shop/auth.py::decode", "name-match"),
                edge(
                    "shop/auth.py::parse",
                    "shop/validation.py::validate",
                    "import"
                ),
            ]
        );
    }

    /// An aliased import (`from shop.auth import parse as parse_token`), a
    /// module import (`from shop import config; config.parse()`) and a
    /// library module (`json.loads()`).
    #[test]
    fn imports_bind_aliases_and_modules() {
        let mut resolver = resolver(&[
            function("shop/service.py", "login", (1, 9)),
            function("shop/auth.py", "parse", (1, 2)),
            function("shop/config.py", "parse", (1, 2)),
            method("shop/json.py", "Codec.loads", (1, 2)),
        ]);
        resolver.register_file(
            "shop/service.py",
            &[
                import("parse_token", "shop.auth", Some("parse")),
                import("config", "shop", Some("config")),
                import("json", "json", None),
            ],
            &[],
            &[],
            &[],
        );
        resolver.register_file("shop/__init__.py", &[], &[], &[], &[]);
        let calls = [
            call("shop/service.py", 2, "login", "parse_token", CallForm::Bare),
            member(
                "shop/service.py",
                3,
                "login",
                vec![ident("config")],
                "parse",
            ),
            member("shop/service.py", 4, "login", vec![ident("json")], "loads"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("shop/service.py::login", "shop/auth.py::parse", "import"),
                edge("shop/service.py::login", "shop/config.py::parse", "import"),
            ]
        );
    }

    /// TypeScript: relative specifiers resolve against the importer's
    /// directory; a namespace import is a module; a package is a library.
    #[test]
    fn typescript_imports_resolve_relative_paths() {
        let mut resolver = resolver(&[
            function("src/app/main.ts", "main", (1, 9)),
            function("src/app/util.ts", "format", (1, 2)),
            function("src/lib/util.ts", "format", (1, 2)),
            function("src/lib/config.ts", "parse", (1, 2)),
            function("src/other/lodash.ts", "debounce", (1, 2)),
        ]);
        resolver.register_file(
            "src/app/main.ts",
            &[
                import("format", "../lib/util", Some("format")),
                import("config", "../lib/config", Some("*")),
                import("debounce", "lodash", Some("debounce")),
            ],
            &[],
            &[],
            &[],
        );
        let calls = [
            call("src/app/main.ts", 2, "main", "format", CallForm::Bare),
            member("src/app/main.ts", 3, "main", vec![ident("config")], "parse"),
            call("src/app/main.ts", 4, "main", "debounce", CallForm::Bare),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "src/app/main.ts::main",
                    "src/lib/config.ts::parse",
                    "import"
                ),
                edge("src/app/main.ts::main", "src/lib/util.ts::format", "import"),
            ]
        );
    }

    /// Go: a package is a directory; `pkg.F()` resolves in the imported
    /// package (never to a method), `F()` in the caller's package, and a
    /// standard-library package is a library.
    #[test]
    fn go_packages_are_directories() {
        let mut resolver = resolver(&[
            function("svc/svc.go", "Run", (1, 9)),
            function("svc/helpers.go", "helper", (1, 2)),
            function("validation/validation.go", "Validate", (1, 2)),
            method("validation/validation.go", "Validator.Validate", (4, 5)),
            function("strings/strings.go", "Join", (1, 2)),
        ]);
        resolver.register_file(
            "svc/svc.go",
            &[
                import("validation", "example.com/shop/validation", None),
                import("strings", "strings", None),
            ],
            &[],
            &[],
            &[],
        );
        let calls = [
            member(
                "svc/svc.go",
                2,
                "Run",
                vec![ident("validation")],
                "Validate",
            ),
            call("svc/svc.go", 3, "Run", "helper", CallForm::Bare),
            member("svc/svc.go", 4, "Run", vec![ident("strings")], "Join"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("svc/svc.go::Run", "svc/helpers.go::helper", "name-match"),
                edge(
                    "svc/svc.go::Run",
                    "validation/validation.go::Validate",
                    "import"
                ),
            ]
        );
    }

    /// Rust: `use crate::m::item` binds the item in module file `m.rs`;
    /// `use crate::m::{self}` binds the module; `std::` is a library.
    #[test]
    fn rust_use_paths_bind_items_and_modules() {
        let mut resolver = resolver(&[
            function("src/service.rs", "run", (1, 9)),
            function("src/auth.rs", "parse", (1, 2)),
            function("src/config.rs", "parse", (1, 2)),
        ]);
        resolver.register_file(
            "src/service.rs",
            &[
                import(
                    "parse_token",
                    "crate::auth::parse",
                    Some("crate::auth::parse"),
                ),
                import("config", "crate::config", Some("self")),
                import("HashMap", "std::collections::HashMap", None),
            ],
            &[],
            &[],
            &[],
        );
        let calls = [
            call("src/service.rs", 2, "run", "parse_token", CallForm::Bare),
            member("src/service.rs", 3, "run", vec![ident("config")], "parse"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("src/service.rs::run", "src/auth.rs::parse", "import"),
                edge("src/service.rs::run", "src/config.rs::parse", "import"),
            ]
        );
    }

    /// Java: an unqualified call reaches the enclosing class's methods (the
    /// innermost class first), then a static import.
    #[test]
    fn java_unqualified_calls_reach_the_enclosing_class() {
        let mut resolver = resolver(&[
            class("shop/Main.java", "Main", (1, 30)),
            method("shop/Main.java", "Main.handler", (2, 4)),
            class("shop/Main.java", "Main.Worker", (5, 12)),
            method("shop/Main.java", "Worker.handler", (6, 7)),
            method("shop/Main.java", "Worker.run", (8, 10)),
            method("shop/Main.java", "Main.run", (13, 20)),
            class("shop/auth/Auth.java", "Auth", (1, 9)),
            method("shop/auth/Auth.java", "Auth.parse", (2, 4)),
        ]);
        resolver.register_file(
            "shop/Main.java",
            &[import("parse", "shop.auth.Auth.parse", Some("static"))],
            &[],
            &[],
            &[],
        );
        let calls = [
            call("shop/Main.java", 9, "run", "handler", CallForm::Bare),
            call("shop/Main.java", 14, "run", "handler", CallForm::Bare),
            call("shop/Main.java", 15, "run", "parse", CallForm::Bare),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "shop/Main.java::Main.run",
                    "shop/Main.java::Main.handler",
                    "exact"
                ),
                edge(
                    "shop/Main.java::Main.run",
                    "shop/auth/Auth.java::Auth.parse",
                    "import"
                ),
                edge(
                    "shop/Main.java::Worker.run",
                    "shop/Main.java::Worker.handler",
                    "exact"
                ),
            ]
        );
    }

    /// A call on an unknown receiver may reach any method of that name, but
    /// never a free function; a literal receiver reaches nothing.
    #[test]
    fn a_method_call_never_reaches_a_free_function() {
        let mut resolver = resolver(&[
            function("src/a.ts", "main", (1, 9)),
            function("src/utils.ts", "map", (1, 2)),
            method("src/models.ts", "Order.total", (1, 2)),
        ]);
        declare_unknown(&mut resolver, "src/a.ts", &["items", "order"]);
        let calls = [
            member("src/a.ts", 2, "main", vec![ident("items")], "map"),
            member("src/a.ts", 3, "main", vec![ident("order")], "total"),
            member(
                "src/a.ts",
                4,
                "main",
                vec![ExpressionStep::Literal],
                "total",
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/a.ts::main",
                "src/models.ts::Order.total",
                "name-match"
            )]
        );
    }

    /// Several candidates: an `ambiguous` edge to each, up to
    /// [`MAX_AMBIGUOUS_TARGETS`]; more than that: none.
    #[test]
    fn ambiguous_calls_link_a_few_candidates_or_none() {
        let few = MAX_AMBIGUOUS_TARGETS.max(2);
        let mut symbols = vec![function("src/a.py", "main", (1, 9))];
        for i in 0..few {
            symbols.push(method(&format!("src/m{i}.py"), "C.get", (1, 2)));
        }
        let mut resolver = resolver(&symbols);
        declare_unknown(&mut resolver, "src/a.py", &["x"]);
        let get = member("src/a.py", 2, "main", vec![ident("x")], "get");
        let found = edges(&mut resolver, std::slice::from_ref(&get));
        if MAX_AMBIGUOUS_TARGETS >= 2 {
            assert_eq!(found.len(), few);
            assert!(found.iter().all(|(_, _, label)| *label == "ambiguous"));
        } else {
            assert!(found.is_empty());
        }

        symbols.push(method("src/too_many.py", "C.get", (1, 2)));
        for i in few..=MAX_AMBIGUOUS_TARGETS {
            symbols.push(method(&format!("src/n{i}.py"), "C.get", (1, 2)));
        }
        let mut resolver = self::resolver(&symbols);
        declare_unknown(&mut resolver, "src/a.py", &["x"]);
        assert!(edges(&mut resolver, &[get]).is_empty());
    }

    /// `handler = authenticate; handler()` resolves to `authenticate`.
    #[test]
    fn an_ssa_alias_in_the_file_is_followed() {
        let mut resolver = resolver(&[
            function("app.py", "main", (1, 9)),
            function("app.py", "authenticate", (10, 12)),
        ]);
        resolver.register_file(
            "app.py",
            &[],
            &[("handler".to_string(), "authenticate".to_string())],
            &[],
            &[],
        );
        let calls = [call("app.py", 3, "main", "handler", CallForm::Bare)];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge("app.py::main", "app.py::authenticate", "name-match")]
        );
    }

    /// A function nested in another one is visible only inside it.
    #[test]
    fn a_nested_function_is_visible_only_in_its_parent() {
        let local = CodeSymbol {
            uid: "app.ts::dispatch/handler".to_string(),
            ..function("app.ts", "handler", (2, 2))
        };
        let mut resolver = resolver(&[
            function("app.ts", "dispatch", (1, 5)),
            local,
            function("app.ts", "main", (6, 9)),
            function("app.ts", "handler", (10, 12)),
        ]);
        let calls = [
            call("app.ts", 3, "dispatch", "handler", CallForm::Bare),
            call("app.ts", 7, "main", "handler", CallForm::Bare),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("app.ts::dispatch", "app.ts::dispatch/handler", "name-match"),
                edge("app.ts::main", "app.ts::handler", "name-match"),
            ]
        );
    }

    /// Several calls between one pair make one edge, with the strongest
    /// label and that call's line.
    #[test]
    fn one_edge_per_pair_with_the_strongest_label() {
        let mut resolver = resolver(&[
            function("src/main.rs", "run", (1, 9)),
            method("src/a.rs", "Repo.get", (1, 2)),
            method("src/b.rs", "Cache.get", (1, 2)),
        ]);
        resolver.register_file(
            "src/main.rs",
            &[import("Repo", "crate::a::Repo", None)],
            &[],
            &[],
            &[],
        );
        declare_unknown(&mut resolver, "src/main.rs", &["x"]);
        let calls = [
            member("src/main.rs", 2, "run", vec![ident("x")], "get"),
            CallReference {
                form: CallForm::Unknown,
                chain: Some(vec![
                    ident("Repo"),
                    ExpressionStep::Field("get".into()),
                    ExpressionStep::Call,
                ]),
                ..call("src/main.rs", 5, "run", "get", CallForm::Unknown)
            },
        ];
        let rels = resolver.resolve_calls(&calls, "repo");
        let to_repo = rels
            .iter()
            .find(|r| r.target_uid == "src/a.rs::Repo.get")
            .unwrap();
        assert_eq!(rels.len(), 2, "line 2 links both get methods");
        assert_eq!(to_repo.metadata, "line:5;resolved:exact");
    }

    // --- receiver typing ---------------------------------------------------

    fn binding(name: &str, scope: (u32, u32), value: BindingValue) -> TypeBinding {
        TypeBinding {
            name: name.to_string(),
            owner: None,
            scope,
            value,
        }
    }

    fn field(owner: &str, name: &str, value: BindingValue) -> TypeBinding {
        TypeBinding {
            owner: Some(owner.to_string()),
            ..binding(name, crate::parser::FILE_SCOPE, value)
        }
    }

    fn with_metadata(mut symbol: CodeSymbol, metadata: SymbolMetadata) -> CodeSymbol {
        symbol.metadata = Some(serde_json::to_string(&metadata).unwrap());
        symbol
    }

    fn this(name: &str) -> Vec<ExpressionStep> {
        vec![
            ExpressionStep::This,
            ExpressionStep::Field(name.to_string()),
        ]
    }

    /// The shop domain in Python: two classes with `save`/`add`/`get`, a
    /// subclass, typed and untyped receivers.
    fn python_shop() -> CallResolver {
        let mut resolver = resolver(&[
            class("shop/models.py", "User", (1, 9)),
            method("shop/models.py", "User.save", (2, 4)),
            class("shop/models.py", "Order", (10, 19)),
            method("shop/models.py", "Order.save", (11, 13)),
            method("shop/models.py", "Order.add", (14, 15)),
            class("shop/storage.py", "Repository", (1, 9)),
            method("shop/storage.py", "Repository.add", (2, 3)),
            method("shop/storage.py", "Repository.get", (4, 5)),
            class("shop/reports.py", "Report", (1, 9)),
            method("shop/reports.py", "Report.__init__", (2, 3)),
            method("shop/reports.py", "Report.header", (4, 5)),
            method("shop/reports.py", "Report.render", (6, 8)),
            with_metadata(
                class("shop/reports.py", "OrderReport", (10, 19)),
                SymbolMetadata {
                    superclasses: vec!["Report".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("shop/reports.py", "OrderReport.__init__", (11, 12)),
            method("shop/reports.py", "OrderReport.summary", (13, 14)),
            function("shop/service.py", "register", (1, 9)),
        ]);
        resolver.register_file(
            "shop/models.py",
            &[import("Repository", "shop.storage", Some("Repository"))],
            &[],
            &[binding(
                "repo",
                (2, 4),
                BindingValue::Type("Repository".to_string()),
            )],
            &[],
        );
        resolver.register_file(
            "shop/service.py",
            &[import("User", "shop.models", Some("User"))],
            &[],
            &[
                binding(
                    "user",
                    (3, 9),
                    BindingValue::Expr(vec![ident("User"), ExpressionStep::Call]),
                ),
                binding("rows", (1, 9), BindingValue::Type("list[str]".to_string())),
                binding("thing", (1, 9), BindingValue::Unknown),
            ],
            &[],
        );
        resolver
    }

    /// `repo: Repository` makes `repo.add()` Repository.add, not Order.add;
    /// `user = User(...)` makes `user.save()` User.save; a list's `.get()`
    /// is nobody's; an untyped receiver falls back to the method name.
    #[test]
    fn declared_and_constructed_receivers_select_the_class() {
        let mut resolver = python_shop();
        let calls = [
            member("shop/models.py", 3, "save", vec![ident("repo")], "add"),
            member(
                "shop/service.py",
                4,
                "register",
                vec![ident("user")],
                "save",
            ),
            member("shop/service.py", 5, "register", vec![ident("rows")], "get"),
            member(
                "shop/service.py",
                6,
                "register",
                vec![ident("thing")],
                "add",
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "shop/models.py::User.save",
                    "shop/storage.py::Repository.add",
                    "exact"
                ),
                edge(
                    "shop/service.py::register",
                    "shop/models.py::Order.add",
                    "ambiguous"
                ),
                edge(
                    "shop/service.py::register",
                    "shop/models.py::User.save",
                    "exact"
                ),
                edge(
                    "shop/service.py::register",
                    "shop/storage.py::Repository.add",
                    "ambiguous"
                ),
            ]
        );
    }

    /// `self.header()` is the enclosing class's; `self.render()` in a
    /// subclass is the base class's; `super().__init__()` is the base
    /// constructor.
    #[test]
    fn self_and_super_follow_the_class_hierarchy() {
        let mut resolver = python_shop();
        let super_init = vec![ExpressionStep::Super, ExpressionStep::Call];
        let calls = [
            member(
                "shop/reports.py",
                7,
                "render",
                this("").into_iter().take(1).collect(),
                "header",
            ),
            member(
                "shop/reports.py",
                14,
                "summary",
                vec![ExpressionStep::This],
                "render",
            ),
            member("shop/reports.py", 12, "__init__", super_init, "__init__"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "shop/reports.py::OrderReport.__init__",
                    "shop/reports.py::Report.__init__",
                    "exact"
                ),
                edge(
                    "shop/reports.py::OrderReport.summary",
                    "shop/reports.py::Report.render",
                    "exact"
                ),
                edge(
                    "shop/reports.py::Report.render",
                    "shop/reports.py::Report.header",
                    "exact"
                ),
            ]
        );
    }

    /// `super()` in a class that extends a library class and implements a
    /// project interface calls the library constructor, not the interface
    /// (#169: Vue's `VueElement extends BaseClass implements
    /// ComponentCustomElementInterface`).
    #[test]
    fn super_never_reaches_an_implemented_interface() {
        let mut resolver = resolver(&[
            symbol("src/iface.ts", "Iface", SymbolKind::Interface, (1, 3)),
            with_metadata(
                class("src/elem.ts", "Elem", (1, 9)),
                SymbolMetadata {
                    superclasses: vec!["HTMLElement".to_string(), "Iface".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("src/elem.ts", "Elem.constructor", (2, 4)),
        ]);
        resolver.register_file(
            "src/elem.ts",
            &[import("Iface", "./iface", Some("Iface"))],
            &[],
            &[],
            &[],
        );
        let calls = [call(
            "src/elem.ts",
            3,
            "constructor",
            "super",
            CallForm::SuperConstructor,
        )];
        assert!(edges(&mut resolver, &calls).is_empty());
        assert_eq!(resolver.stats().library, 1);
    }

    /// A parameter or local named like a function shadows it: the call goes
    /// through a function value and gets no edge, unless the name holds a
    /// class (`cls()` in a classmethod).
    #[test]
    fn a_local_binding_shadows_a_function_of_that_name() {
        let mut resolver = resolver(&[
            function("cli.py", "handler", (1, 2)),
            function("cli.py", "dispatch", (3, 6)),
            class("cli.py", "Config", (7, 12)),
            method("cli.py", "Config.load", (8, 9)),
        ]);
        resolver.register_file(
            "cli.py",
            &[],
            &[],
            &[
                binding("handler", (4, 6), BindingValue::Unknown),
                binding("cls", (8, 9), BindingValue::Expr(vec![ident("Config")])),
            ],
            &[],
        );
        let calls = [
            call("cli.py", 5, "dispatch", "handler", CallForm::Bare),
            call("cli.py", 9, "load", "cls", CallForm::Bare),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge("cli.py::Config.load", "cli.py::Config", "exact")]
        );
    }

    /// A declared return type types the call's result:
    /// `config.Config.load(text).get()` reaches Config.get.
    #[test]
    fn return_types_type_call_results() {
        let mut resolver = resolver(&[
            class("src/config.ts", "Config", (1, 9)),
            with_metadata(
                method("src/config.ts", "Config.load", (2, 4)),
                SymbolMetadata {
                    return_type: Some("Config".to_string()),
                    ..SymbolMetadata::default()
                },
            ),
            method("src/config.ts", "Config.get", (5, 6)),
            class("src/storage.ts", "Repository", (1, 9)),
            method("src/storage.ts", "Repository.get", (2, 3)),
            function("src/service.ts", "settings", (1, 9)),
        ]);
        resolver.register_file(
            "src/service.ts",
            &[import("config", "./config", Some("*"))],
            &[],
            &[binding(
                "cfg",
                (2, 9),
                BindingValue::Expr(vec![
                    ident("config"),
                    ExpressionStep::Field("Config".to_string()),
                    ExpressionStep::Field("load".to_string()),
                    ExpressionStep::Call,
                ]),
            )],
            &[],
        );
        let calls = [member(
            "src/service.ts",
            3,
            "settings",
            vec![ident("cfg")],
            "get",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/service.ts::settings",
                "src/config.ts::Config.get",
                "exact"
            )]
        );
    }

    /// Go: a receiver parameter typed `*UserService`, a struct field typed
    /// `*validation.Validator` in another package.
    #[test]
    fn struct_fields_type_chained_receivers() {
        let mut resolver = resolver(&[
            class("service/service.go", "UserService", (1, 4)),
            method("service/service.go", "UserService.Register", (6, 9)),
            class("validation/validation.go", "Validator", (1, 3)),
            method("validation/validation.go", "Validator.Validate", (4, 5)),
            function("validation/validation.go", "Validate", (7, 8)),
        ]);
        resolver.register_file(
            "service/service.go",
            &[import("validation", "example.com/shop/validation", None)],
            &[],
            &[
                field(
                    "UserService",
                    "validator",
                    BindingValue::Type("*validation.Validator".to_string()),
                ),
                binding("s", (6, 9), BindingValue::Type("*UserService".to_string())),
            ],
            &[],
        );
        let receiver = vec![ident("s"), ExpressionStep::Field("validator".to_string())];
        let calls = [member(
            "service/service.go",
            7,
            "Register",
            receiver,
            "Validate",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "service/service.go::UserService.Register",
                "validation/validation.go::Validator.Validate",
                "exact"
            )]
        );
    }

    /// Rust `Self::new()` and `Type::assoc()`; Java overloads by arity;
    /// `super(...)` to the base constructor.
    #[test]
    fn paths_constructors_and_overloads() {
        let mut resolver = resolver(&[
            class("src/config.rs", "Config", (1, 3)),
            method("src/config.rs", "Config.new", (5, 6)),
            method("src/config.rs", "Config.load", (7, 9)),
            class("src/Repo.java", "Repo", (1, 20)),
            CodeSymbol {
                signature: "public int add(T row)".to_string(),
                ..method("src/Repo.java", "Repo.add", (2, 4))
            },
            CodeSymbol {
                signature: "public int add(T row, int copies)".to_string(),
                uid: "src/Repo.java::Repo.add(T, int)".to_string(),
                ..method("src/Repo.java", "Repo.add", (5, 9))
            },
            class("src/Report.java", "Report", (1, 5)),
            method("src/Report.java", "Report.Report", (2, 3)),
            with_metadata(
                class("src/OrderReport.java", "OrderReport", (1, 9)),
                SymbolMetadata {
                    superclasses: vec!["Report".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("src/OrderReport.java", "OrderReport.OrderReport", (2, 4)),
        ]);
        let calls = [
            member("src/config.rs", 8, "load", vec![ident("Self")], "new"),
            CallReference {
                arg_count: Some(1),
                ..call("src/Repo.java", 6, "add", "add", CallForm::Bare)
            },
            call(
                "src/OrderReport.java",
                3,
                "OrderReport",
                "super",
                CallForm::SuperConstructor,
            ),
            call(
                "src/OrderReport.java",
                3,
                "OrderReport",
                "Report",
                CallForm::New,
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "src/OrderReport.java::OrderReport.OrderReport",
                    "src/Report.java::Report",
                    "exact"
                ),
                edge(
                    "src/OrderReport.java::OrderReport.OrderReport",
                    "src/Report.java::Report.Report",
                    "exact"
                ),
                edge(
                    "src/Repo.java::Repo.add(T, int)",
                    "src/Repo.java::Repo.add",
                    "exact"
                ),
                edge(
                    "src/config.rs::Config.load",
                    "src/config.rs::Config.new",
                    "exact"
                ),
            ]
        );
    }

    /// Go: `cfg, err := config.Load(text)` with `func Load(...) (*Config,
    /// error)` declared beside the symbol, not in its metadata.
    #[test]
    fn declared_return_types_reach_the_resolver() {
        let mut resolver = resolver(&[
            function("config/config.go", "Load", (1, 3)),
            class("config/config.go", "Config", (5, 7)),
            method("config/config.go", "Config.Get", (9, 10)),
            method("store/store.go", "Repo.Get", (1, 2)),
            function("service/service.go", "Settings", (1, 9)),
        ]);
        resolver.register_file(
            "config/config.go",
            &[],
            &[],
            &[],
            &[DeclaredTypes {
                name: "Load".to_string(),
                line: 1,
                return_type: Some("(*Config, error)".to_string()),
                bases: Vec::new(),
                elsewhere: false,
            }],
        );
        resolver.register_file(
            "service/service.go",
            &[import("config", "example.com/shop/config", None)],
            &[],
            &[binding(
                "cfg",
                (2, 9),
                BindingValue::Expr(vec![
                    ident("config"),
                    ExpressionStep::Field("Load".to_string()),
                    ExpressionStep::Call,
                ]),
            )],
            &[],
        );
        let calls = [member(
            "service/service.go",
            3,
            "Settings",
            vec![ident("cfg")],
            "Get",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "service/service.go::Settings",
                "config/config.go::Config.Get",
                "exact"
            )]
        );
    }

    // --- review regressions (#144) ---------------------------------------

    /// `from .globals import current_app` where `globals.py` declares
    /// `current_app: Flask = LocalProxy(...)`: a project value with a declared
    /// type, not a library.
    #[test]
    fn an_imported_module_level_value_is_typed_by_its_declaration() {
        let mut resolver = resolver(&[
            class("src/flask/app.py", "Flask", (1, 9)),
            method("src/flask/app.py", "Flask.make_response", (2, 3)),
            method("src/flask/wrappers.py", "Response.make_response", (1, 2)),
            function("src/flask/helpers.py", "make_response", (5, 9)),
        ]);
        resolver.register_file(
            "src/flask/globals.py",
            &[import("Flask", ".app", Some("Flask"))],
            &[],
            &[binding(
                "current_app",
                crate::parser::FILE_SCOPE,
                BindingValue::Type("Flask".to_string()),
            )],
            &[],
        );
        resolver.register_file(
            "src/flask/helpers.py",
            &[import("current_app", ".globals", Some("current_app"))],
            &[],
            &[],
            &[],
        );
        let calls = [member(
            "src/flask/helpers.py",
            6,
            "make_response",
            vec![ident("current_app")],
            "make_response",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/flask/helpers.py::make_response",
                "src/flask/app.py::Flask.make_response",
                "exact"
            )]
        );
    }

    /// A name a project module exports without a visible declaration is
    /// unknown, not a library: a method call on it still falls back to the
    /// method name.
    #[test]
    fn an_unknown_project_name_is_not_a_library() {
        let mut resolver = resolver(&[
            method("src/shop/models.py", "Order.total", (1, 2)),
            function("src/shop/cli.py", "main", (1, 9)),
        ]);
        resolver.register_file("src/shop/registry.py", &[], &[], &[], &[]);
        resolver.register_file(
            "src/shop/cli.py",
            &[import("orders", "shop.registry", Some("orders"))],
            &[],
            &[],
            &[],
        );
        let calls = [member(
            "src/shop/cli.py",
            2,
            "main",
            vec![ident("orders")],
            "total",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/shop/cli.py::main",
                "src/shop/models.py::Order.total",
                "name-match"
            )]
        );
    }

    /// `import requests.adapters` binds `requests`; `requests.adapters.
    /// HTTPAdapter()` walks the package to its submodule.
    #[test]
    fn a_dotted_import_binds_its_first_name_and_reaches_submodules() {
        let mut resolver = resolver(&[
            class("src/requests/adapters.py", "HTTPAdapter", (1, 9)),
            function("tests/test_adapters.py", "test_it", (1, 9)),
        ]);
        resolver.register_file("src/requests/__init__.py", &[], &[], &[], &[]);
        resolver.register_file(
            "tests/test_adapters.py",
            &[import("requests", "requests.adapters", None)],
            &[],
            &[],
            &[],
        );
        let receiver = vec![
            ident("requests"),
            ExpressionStep::Field("adapters".to_string()),
        ];
        let calls = [CallReference {
            chain: Some(vec![
                ident("requests"),
                ExpressionStep::Field("adapters".to_string()),
                ExpressionStep::Field("HTTPAdapter".to_string()),
                ExpressionStep::Call,
            ]),
            ..member(
                "tests/test_adapters.py",
                2,
                "test_it",
                receiver,
                "HTTPAdapter",
            )
        }];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "tests/test_adapters.py::test_it",
                "src/requests/adapters.py::HTTPAdapter",
                "import"
            )]
        );
    }

    /// A Vue-like monorepo (#169): `@vue/shared` and `vue` are packages of
    /// the repository's pnpm workspace whose package.json names build
    /// output (`dist/`) and a root `index.js` shim; `@vue/*` is also a
    /// tsconfig `paths` alias. Both resolve to the package's `src/index.ts`
    /// barrel; a package of another name stays a library.
    #[test]
    fn workspace_packages_and_path_aliases_resolve_like_relative_imports() {
        let mut resolver = resolver(&[
            function("packages/shared/src/general.ts", "isArray", (1, 2)),
            function("packages/shared/src/general.ts", "hasOwn", (3, 4)),
            function("packages/reactivity/src/reactive.ts", "reactive", (1, 9)),
            function("packages/runtime/src/api.ts", "watch", (1, 9)),
            function("packages/app/src/main.ts", "main", (1, 9)),
            function("other/lodash.ts", "debounce", (1, 2)),
        ]);
        let shim = r#"{"name": "@vue/shared", "main": "index.js", "module": "dist/shared.esm-bundler.js", "types": "dist/shared.d.ts"}"#;
        resolver.set_js_project(JsProject::from_files([
            (
                "pnpm-workspace.yaml".to_string(),
                "packages:\n  - 'packages/*'\n".to_string(),
            ),
            ("packages/shared/package.json".to_string(), shim.to_string()),
            (
                "packages/reactivity/package.json".to_string(),
                r#"{"name": "@vue/reactivity", "main": "index.js"}"#.to_string(),
            ),
            (
                "packages/runtime/package.json".to_string(),
                r#"{"name": "@vue/runtime", "exports": {".": "./src/api.ts", "./sub/*": "./src/*.ts"}}"#
                    .to_string(),
            ),
        ]));
        for (file, imports) in [
            (
                "packages/shared/src/index.ts",
                vec![import("*", "./general", None)],
            ),
            // The root shim a bundler never sees.
            ("packages/shared/index.js", vec![]),
            (
                "packages/reactivity/src/reactive.ts",
                vec![import("isArray", "@vue/shared", Some("isArray"))],
            ),
            (
                "packages/app/src/main.ts",
                vec![
                    import("shared", "@vue/shared", Some("*")),
                    import("reactive", "@vue/reactivity/src/reactive", Some("reactive")),
                    import("watch", "@vue/runtime", Some("watch")),
                    import("debounce", "lodash", Some("debounce")),
                ],
            ),
        ] {
            resolver.register_file(file, &imports, &[], &[], &[]);
        }
        let calls = [
            call(
                "packages/reactivity/src/reactive.ts",
                2,
                "reactive",
                "isArray",
                CallForm::Bare,
            ),
            member(
                "packages/app/src/main.ts",
                2,
                "main",
                vec![ident("shared")],
                "hasOwn",
            ),
            call(
                "packages/app/src/main.ts",
                3,
                "main",
                "reactive",
                CallForm::Bare,
            ),
            call(
                "packages/app/src/main.ts",
                4,
                "main",
                "watch",
                CallForm::Bare,
            ),
            call(
                "packages/app/src/main.ts",
                5,
                "main",
                "debounce",
                CallForm::Bare,
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "packages/app/src/main.ts::main",
                    "packages/reactivity/src/reactive.ts::reactive",
                    "import"
                ),
                edge(
                    "packages/app/src/main.ts::main",
                    "packages/runtime/src/api.ts::watch",
                    "import"
                ),
                edge(
                    "packages/app/src/main.ts::main",
                    "packages/shared/src/general.ts::hasOwn",
                    "import"
                ),
                edge(
                    "packages/reactivity/src/reactive.ts::reactive",
                    "packages/shared/src/general.ts::isArray",
                    "import"
                ),
            ]
        );
        // lodash is no package of the repository: a library call.
        assert_eq!(resolver.stats().library, 1);
    }

    /// tsconfig `paths` (through `extends`) and `baseUrl` name files of the
    /// repository; they win over a workspace package of the same name, and
    /// a pattern whose targets do not exist falls through.
    #[test]
    fn tsconfig_paths_and_base_url_resolve_bare_specifiers() {
        let mut resolver = resolver(&[
            function("shared/utils/strings.ts", "slugify", (1, 2)),
            function("shared/config/index.ts", "readFlag", (1, 2)),
            function("apps/web/src/lib/date.ts", "today", (1, 2)),
            function("apps/web/src/main.ts", "start", (1, 9)),
            function("vendor/missing.ts", "gone", (1, 2)),
        ]);
        resolver.set_js_project(JsProject::from_files([
            (
                "tsconfig.base.json".to_string(),
                r#"{
                  // shared options
                  "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {"~utils/*": ["shared/utils/*"], "@shared/config": ["shared/config"], "@gone/*": ["nowhere/*"],},
                  },
                }"#
                .to_string(),
            ),
            (
                "apps/web/tsconfig.json".to_string(),
                r#"{"extends": "../../tsconfig.base.json"}"#.to_string(),
            ),
        ]));
        resolver.register_file(
            "apps/web/src/main.ts",
            &[
                import("slugify", "~utils/strings", Some("slugify")),
                import("readFlag", "@shared/config", Some("readFlag")),
                // baseUrl is the root: `apps/web/src/lib/date`.
                import("today", "apps/web/src/lib/date", Some("today")),
                import("gone", "@gone/missing", Some("gone")),
            ],
            &[],
            &[],
            &[],
        );
        let calls: Vec<CallReference> = ["slugify", "readFlag", "today", "gone"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                call(
                    "apps/web/src/main.ts",
                    i as u32 + 2,
                    "start",
                    name,
                    CallForm::Bare,
                )
            })
            .collect();
        let start = "apps/web/src/main.ts::start";
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(start, "apps/web/src/lib/date.ts::today", "import"),
                edge(start, "shared/config/index.ts::readFlag", "import"),
                edge(start, "shared/utils/strings.ts::slugify", "import"),
            ]
        );
    }

    /// A TypeScript barrel: `export { jwt } from './jwt'` and `export * from
    /// './util'` in `index.ts`, imported with `from '.'`.
    #[test]
    fn typescript_barrels_are_followed() {
        let mut resolver = resolver(&[
            function("src/mw/jwt/jwt.ts", "jwt", (1, 3)),
            function("src/mw/jwt/util.ts", "decode", (1, 3)),
            function("src/mw/jwt/handler.ts", "handle", (1, 9)),
        ]);
        resolver.register_file(
            "src/mw/jwt/index.ts",
            &[
                import("jwt", "./jwt", Some("jwt")),
                import("*", "./util", None),
            ],
            &[],
            &[],
            &[],
        );
        resolver.register_file(
            "src/mw/jwt/handler.ts",
            &[
                import("jwt", ".", Some("jwt")),
                import("decode", ".", Some("decode")),
            ],
            &[],
            &[],
            &[],
        );
        let calls = [
            call("src/mw/jwt/handler.ts", 2, "handle", "jwt", CallForm::Bare),
            call(
                "src/mw/jwt/handler.ts",
                3,
                "handle",
                "decode",
                CallForm::Bare,
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "src/mw/jwt/handler.ts::handle",
                    "src/mw/jwt/jwt.ts::jwt",
                    "import"
                ),
                edge(
                    "src/mw/jwt/handler.ts::handle",
                    "src/mw/jwt/util.ts::decode",
                    "import"
                ),
            ]
        );
    }

    /// Gin's `c.Writer.Header().Set(...)`: `Writer` is a project interface
    /// that does not declare `Header` (it comes from `http.ResponseWriter`),
    /// so the chain is a library value from there on and `Set` is not
    /// matched to the project's `Context.Set`.
    #[test]
    fn no_name_match_after_a_typed_step() {
        let mut resolver = resolver(&[
            class("context.go", "Context", (1, 5)),
            method("context.go", "Context.Set", (7, 8)),
            method("context.go", "Context.Render", (10, 19)),
            symbol(
                "response_writer.go",
                "ResponseWriter",
                SymbolKind::Interface,
                (1, 5),
            ),
            method("response_writer.go", "ResponseWriter.Status", (2, 2)),
        ]);
        resolver.register_file(
            "context.go",
            &[],
            &[],
            &[
                field(
                    "Context",
                    "Writer",
                    BindingValue::Type("ResponseWriter".to_string()),
                ),
                binding("c", (10, 19), BindingValue::Type("*Context".to_string())),
            ],
            &[],
        );
        let chain = vec![
            ident("c"),
            ExpressionStep::Field("Writer".to_string()),
            ExpressionStep::Field("Header".to_string()),
            ExpressionStep::Call,
        ];
        let calls = [member("context.go", 12, "Render", chain, "Set")];
        assert!(edges(&mut resolver, &calls).is_empty());
        assert_eq!(resolver.stats().library, 1);
    }

    /// A field assigned from an untyped parameter is declared but untyped:
    /// method calls on it still fall back to the method name.
    #[test]
    fn a_declared_untyped_field_still_falls_back_to_the_name() {
        let mut resolver = resolver(&[
            class("svc.py", "Service", (1, 9)),
            method("svc.py", "Service.__init__", (2, 3)),
            method("svc.py", "Service.run", (4, 6)),
            method("repo.py", "Repository.fetch", (1, 2)),
        ]);
        resolver.register_file(
            "svc.py",
            &[],
            &[],
            &[
                binding("repo", (2, 3), BindingValue::Unknown),
                TypeBinding {
                    owner: Some("Service".to_string()),
                    ..binding("repo", (2, 3), BindingValue::Expr(vec![ident("repo")]))
                },
            ],
            &[],
        );
        let receiver = vec![
            ExpressionStep::This,
            ExpressionStep::Field("repo".to_string()),
        ];
        let calls = [member("svc.py", 5, "run", receiver, "fetch")];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "svc.py::Service.run",
                "repo.py::Repository.fetch",
                "name-match"
            )]
        );
    }

    /// Every call is accounted for: linked, or why not, and unlinked calls
    /// of a project symbol's name are counted per name.
    #[test]
    fn unlinked_calls_are_counted_by_reason_and_name() {
        let mut symbols = vec![
            function("app.py", "main", (1, 20)),
            function("app.py", "helper", (30, 31)),
        ];
        for i in 0..3 {
            symbols.push(method(&format!("m{i}.py"), "C.get", (1, 2)));
        }
        let mut resolver = resolver(&symbols);
        resolver.register_file(
            "app.py",
            &[import("json", "json", None)],
            &[],
            &[
                binding("x", (1, 20), BindingValue::Unknown),
                binding("callback", (1, 20), BindingValue::Unknown),
            ],
            &[],
        );
        let calls = [
            call("app.py", 2, "main", "helper", CallForm::Bare),
            member("app.py", 3, "main", vec![ident("x")], "get"),
            member("app.py", 4, "main", vec![ident("json")], "loads"),
            call("app.py", 5, "main", "callback", CallForm::Bare),
            call("app.py", 6, "main", "print", CallForm::Bare),
            call("app.py", 25, "?", "helper", CallForm::Bare),
        ];
        resolver.resolve_calls(&calls, "repo");
        let stats = resolver.stats();
        assert_eq!(
            (
                stats.linked,
                stats.too_many_candidates,
                stats.library,
                stats.through_variable,
                stats.unresolved,
                stats.no_caller
            ),
            (1, 1, 1, 1, 1, 1)
        );
        assert_eq!(stats.unlinked(), 5);
        assert_eq!(stats.unlinked_named("python", "get"), 1);
        assert_eq!(
            stats.unlinked_named("python", "print"),
            0,
            "no project symbol is named print"
        );
        assert!(stats.summary().starts_with("1 call sites linked; 5 not"));
        // Per caller: the calls that may reach project code, in line order;
        // not the library call (`json.loads`) or the built-in (`print`).
        let reasons: Vec<(&str, u32, UnlinkedReason)> = stats
            .unlinked_from("app.py", "main")
            .iter()
            .map(|c| (c.name.as_str(), c.line, c.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("get", 3, UnlinkedReason::TooManyCandidates),
                ("callback", 5, UnlinkedReason::ThroughVariable),
            ]
        );
        assert!(stats.unlinked_from("app.py", "helper").is_empty());
        // The record round-trips through the index's JSON, and an older
        // record without it still reads.
        let json = serde_json::to_string(stats).unwrap();
        assert!(
            json.contains(r#""n":"get","l":3,"r":"too-many-candidates""#),
            "{json}"
        );
        let back: CallResolutionStats = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, stats);
        let old: CallResolutionStats =
            serde_json::from_str(r#"{"linked":1,"too_many_candidates":0,"library":0,"through_variable":0,"unresolved":0,"no_caller":0}"#)
                .unwrap();
        assert!(old.unlinked_by_caller.is_empty());
        // An old record says so, rather than looking complete.
        assert!(!old.records_unlinked_calls());
        assert!(stats.records_unlinked_calls());

        // Incremental re-indexing forgets what it cannot know: the file's
        // per-caller entries and its call sites in the per-name counts.
        let mut recorded = stats.clone();
        recorded.forget_files(["app.py"], []);
        assert!(recorded.unlinked_from("app.py", "main").is_empty());
        assert_eq!(recorded.unlinked_named("python", "get"), 0);
        assert!(recorded.is_stale("app.py"));
        recorded.forget_files([], ["app.py"]);
        assert!(!recorded.is_stale("app.py"));
    }

    // --- re-check of #144: bases the resolver cannot see -------------------

    fn rust_greeter(record_impl: bool) -> CallResolver {
        let mut resolver = resolver(&[
            symbol("src/greet.rs", "Greeter", SymbolKind::Interface, (1, 5)),
            method("src/greet.rs", "Greeter.hello", (2, 4)),
            class("src/bot.rs", "Bot", (2, 2)),
            method("src/bot.rs", "Bot.run", (4, 6)),
            method("src/other.rs", "Robot.hello", (1, 2)),
        ]);
        let declared = DeclaredTypes {
            name: "Bot".to_string(),
            line: 8,
            return_type: None,
            bases: vec!["Greeter".to_string()],
            elsewhere: true,
        };
        resolver.register_file(
            "src/bot.rs",
            &[import("Greeter", "crate::greet::Greeter", None)],
            &[],
            &[],
            if record_impl {
                std::slice::from_ref(&declared)
            } else {
                &[]
            },
        );
        resolver
    }

    /// `impl Greeter for Bot {}` makes the trait's default method
    /// `Greeter::hello` Bot's: `self.hello()` in `impl Bot` reaches it.
    #[test]
    fn a_rust_trait_default_method_is_found_through_the_impl() {
        let mut resolver = rust_greeter(true);
        let calls = [member(
            "src/bot.rs",
            5,
            "run",
            vec![ExpressionStep::This],
            "hello",
        )];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/bot.rs::Bot.run",
                "src/greet.rs::Greeter.hello",
                "exact"
            )]
        );
    }

    /// Without the impl in view, a project trait declaring the method is
    /// still a candidate (by name), never a library member.
    #[test]
    fn a_rust_method_a_project_trait_declares_is_not_a_library_member() {
        let mut resolver = rust_greeter(false);
        let calls = [
            member("src/bot.rs", 5, "run", vec![ExpressionStep::This], "hello"),
            member("src/bot.rs", 5, "run", vec![ExpressionStep::This], "clone"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "src/bot.rs::Bot.run",
                "src/greet.rs::Greeter.hello",
                "name-match"
            )]
        );
        assert_eq!(
            resolver.stats().library,
            1,
            "clone: no project trait declares it"
        );
    }

    /// `class Child(Base)` where `Base = declarative_base()` in a project
    /// module: the base is unresolved, so `self.save()` falls back to the
    /// method name instead of being dropped as a library call; a class
    /// whose bases are all known (`Exception`) still drops it.
    #[test]
    fn a_python_class_with_an_unresolved_project_base_falls_back_to_the_name() {
        let mut resolver = resolver(&[
            with_metadata(
                class("app/svc.py", "Child", (3, 9)),
                SymbolMetadata {
                    superclasses: vec!["Base".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("app/svc.py", "Child.run", (4, 6)),
            with_metadata(
                class("app/errors.py", "Failure", (1, 9)),
                SymbolMetadata {
                    superclasses: vec!["Exception".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("app/errors.py", "Failure.report", (2, 4)),
            method("app/models.py", "Model.save", (1, 3)),
        ]);
        resolver.register_file(
            "app/db.py",
            &[],
            &[],
            &[binding(
                "Base",
                crate::parser::FILE_SCOPE,
                BindingValue::Expr(vec![ident("declarative_base"), ExpressionStep::Call]),
            )],
            &[],
        );
        resolver.register_file(
            "app/svc.py",
            &[import("Base", "app.db", Some("Base"))],
            &[],
            &[],
            &[],
        );
        resolver.register_file("app/errors.py", &[], &[], &[], &[]);
        let calls = [
            member("app/svc.py", 5, "run", vec![ExpressionStep::This], "save"),
            member(
                "app/errors.py",
                3,
                "report",
                vec![ExpressionStep::This],
                "save",
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [edge(
                "app/svc.py::Child.run",
                "app/models.py::Model.save",
                "name-match"
            )]
        );
        assert_eq!(resolver.stats().library, 1, "Failure's bases are all known");
    }

    /// Hono: `class Hono extends HonoBase`, where `hono-base.ts` declares
    /// `class Hono` and exports it as `export { Hono as HonoBase }`. The
    /// alias resolves to the project class, so the hierarchy is closed: a
    /// method of the base is `exact`, and a member neither class declares
    /// (a field holding an arrow function) is not name-matched to an
    /// unrelated object-literal method elsewhere.
    #[test]
    fn a_base_exported_under_an_alias_is_a_project_base() {
        let mut resolver = resolver(&[
            class("src/hono-base.ts", "Hono", (1, 20)),
            method("src/hono-base.ts", "Hono.route", (2, 4)),
            method("src/hono-base.ts", "Hono.constructor", (5, 7)),
            with_metadata(
                class("src/hono.ts", "Hono", (3, 10)),
                SymbolMetadata {
                    superclasses: vec!["HonoBase<E, S>".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("src/hono.ts", "Hono.constructor", (4, 6)),
            function("src/main.ts", "main", (1, 10)),
            method("src/client.test.ts", "stub.fetch", (5, 7)),
        ]);
        resolver.register_file(
            "src/hono-base.ts",
            &[],
            &[],
            &[binding(
                "HonoBase",
                (21, u32::MAX),
                BindingValue::Export("Hono".to_string()),
            )],
            &[],
        );
        resolver.register_file(
            "src/hono.ts",
            &[import("HonoBase", "./hono-base", Some("HonoBase"))],
            &[],
            &[],
            &[],
        );
        resolver.register_file(
            "src/main.ts",
            &[import("Hono", "./hono", Some("Hono"))],
            &[],
            &[binding(
                "app",
                (2, 10),
                BindingValue::Expr(vec![ExpressionStep::New("Hono".to_string())]),
            )],
            &[],
        );
        resolver.register_file("src/client.test.ts", &[], &[], &[], &[]);
        let calls = [
            member("src/main.ts", 3, "main", vec![ident("app")], "route"),
            member("src/main.ts", 4, "main", vec![ident("app")], "fetch"),
            // `super(options)`: the base's constructor, not the subclass's
            // own, though both classes are `Hono` in one directory.
            call(
                "src/hono.ts",
                5,
                "constructor",
                "super",
                CallForm::SuperConstructor,
            ),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge(
                    "src/hono.ts::Hono.constructor",
                    "src/hono-base.ts::Hono.constructor",
                    "exact"
                ),
                edge("src/main.ts::main", "src/hono-base.ts::Hono.route", "exact"),
            ]
        );
        assert_eq!(resolver.stats().library, 1);
    }

    /// `import { Foo } from './lib'; export { Bar as Foo }`: inside the
    /// module `Foo` is still the imported class — the export alias only
    /// names `Bar` for importers (#118).
    #[test]
    fn an_export_alias_does_not_shadow_an_import_of_its_name() {
        let mut resolver = resolver(&[
            class("src/lib.ts", "Foo", (1, 5)),
            method("src/lib.ts", "Foo.run", (2, 4)),
            class("src/a.ts", "Bar", (2, 6)),
            method("src/a.ts", "Bar.run", (3, 5)),
            function("src/a.ts", "main", (8, 12)),
            function("src/b.ts", "use", (2, 6)),
        ]);
        resolver.register_file(
            "src/a.ts",
            &[import("Foo", "./lib", Some("Foo"))],
            &[],
            &[binding(
                "Foo",
                (7, u32::MAX),
                BindingValue::Export("Bar".to_string()),
            )],
            &[],
        );
        resolver.register_file(
            "src/b.ts",
            &[import("Foo", "./a", Some("Foo"))],
            &[],
            &[],
            &[],
        );
        // `Foo.run()` in each file.
        let calls = [
            member("src/a.ts", 10, "main", vec![ident("Foo")], "run"),
            member("src/b.ts", 4, "use", vec![ident("Foo")], "run"),
        ];
        assert_eq!(
            edges(&mut resolver, &calls),
            [
                edge("src/a.ts::main", "src/lib.ts::Foo.run", "exact"),
                edge("src/b.ts::use", "src/a.ts::Bar.run", "exact"),
            ]
        );
    }

    /// `from io import BytesIO` is the standard library's `io`, not the
    /// project's `pkg/geo/io.py` (a module of package `pkg.geo`): the base
    /// is a library base, and `self.seek()` stays unlinked instead of
    /// falling back to the only project method named `seek` (Django's
    /// `django/contrib/gis/geos/io.py`).
    #[test]
    fn an_absolute_import_does_not_bind_a_module_inside_a_package() {
        let mut resolver = resolver(&[
            with_metadata(
                class("pkg/text.py", "Buffer", (3, 8)),
                SymbolMetadata {
                    superclasses: vec!["BytesIO".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            method("pkg/text.py", "Buffer.read", (4, 6)),
            class("pkg/geo/io.py", "WKTReader", (1, 3)),
            method("tests/test_file.py", "TestFile.seek", (2, 3)),
        ]);
        for file in ["pkg/__init__.py", "pkg/geo/__init__.py", "pkg/geo/io.py"] {
            resolver.register_file(file, &[], &[], &[], &[]);
        }
        resolver.register_file("tests/test_file.py", &[], &[], &[], &[]);
        resolver.register_file(
            "pkg/text.py",
            &[import("BytesIO", "io", Some("BytesIO"))],
            &[],
            &[],
            &[],
        );
        let calls = [member(
            "pkg/text.py",
            5,
            "read",
            vec![ExpressionStep::This],
            "seek",
        )];
        assert_eq!(edges(&mut resolver, &calls), []);
        assert_eq!(resolver.stats().library, 1);
    }

    /// A library name a project module re-exports (`from .compat import
    /// MutableMapping`, compat importing it from `collections.abc`) is a
    /// library base: members the class does not declare stay unlinked.
    #[test]
    fn a_re_exported_library_base_is_a_library_base() {
        let mut resolver = resolver(&[
            with_metadata(
                class("req/structures.py", "CaseInsensitiveDict", (3, 9)),
                SymbolMetadata {
                    superclasses: vec!["MutableMapping[str, _VT]".to_string()],
                    ..SymbolMetadata::default()
                },
            ),
            function("req/test.py", "test_keys", (1, 9)),
            method("req/cookies.py", "Jar.keys", (1, 2)),
        ]);
        resolver.register_file(
            "req/compat.py",
            &[import(
                "MutableMapping",
                "collections.abc",
                Some("MutableMapping"),
            )],
            &[],
            &[],
            &[],
        );
        resolver.register_file(
            "req/structures.py",
            &[import("MutableMapping", ".compat", Some("MutableMapping"))],
            &[],
            &[],
            &[],
        );
        resolver.register_file(
            "req/test.py",
            &[import(
                "CaseInsensitiveDict",
                "req.structures",
                Some("CaseInsensitiveDict"),
            )],
            &[],
            &[binding(
                "cid",
                (2, 9),
                BindingValue::Expr(vec![ident("CaseInsensitiveDict"), ExpressionStep::Call]),
            )],
            &[],
        );
        let calls = [member(
            "req/test.py",
            3,
            "test_keys",
            vec![ident("cid")],
            "keys",
        )];
        assert!(edges(&mut resolver, &calls).is_empty());
        assert_eq!(resolver.stats().library, 1);
    }

    #[test]
    fn parameter_counts_come_from_signatures() {
        assert_eq!(
            param_count("public int add(T row, int copies)", "add"),
            Some(2)
        );
        assert_eq!(param_count("int size()", "size"), Some(0));
        assert_eq!(param_count("void put(Map<K, V> m)", "put"), Some(1));
        assert_eq!(
            param_count("static String join(String... cells)", "join"),
            None
        );
    }

    #[test]
    fn unlabelled_metadata_has_no_resolution() {
        assert_eq!(CallResolution::from_metadata("line:3"), None);
        assert_eq!(CallResolution::from_metadata(""), None);
        assert_eq!(
            CallResolution::from_metadata("line:3;resolved:import"),
            Some(CallResolution::Import)
        );
    }

    /// Resolution stays linear-ish: a thousand symbols and calls.
    #[test]
    fn a_thousand_symbols_and_calls() {
        let mut symbols = vec![function("src/main.py", "main", (1, 5000))];
        for i in 0..1000 {
            symbols.push(function(
                &format!("src/m{i}.py"),
                &format!("func_{i}"),
                (1, 2),
            ));
        }
        let mut resolver = resolver(&symbols);
        let calls: Vec<CallReference> = (0..1000)
            .map(|i| {
                call(
                    "src/main.py",
                    2 + i,
                    "main",
                    &format!("func_{i}"),
                    CallForm::Unknown,
                )
            })
            .collect();
        assert_eq!(resolver.resolve_calls(&calls, "repo").len(), 1000);
    }
}
