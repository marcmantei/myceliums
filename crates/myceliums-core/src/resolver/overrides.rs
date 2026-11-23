//! OVERRIDES edges (#190): which base-class or interface methods each
//! method overrides or implements.
//!
//! A method `C.m` overrides `A.m` when `A` is a project class, interface or
//! trait among `C`'s bases, transitively, and `A` declares a method of the
//! same name that takes as many parameters (where both signatures say). An
//! edge goes to every such declaration up the hierarchy, not only to the
//! nearest, so the callers of an interface method reach an implementation
//! below an abstract class in one step. Static and private methods and
//! constructors neither override nor are overridden. Go has no
//! inheritance: an embedded struct's methods are promoted, not overridden.
//!
//! The bases are the ones the call resolver uses ([`CallResolver::bases`]).
//! Where a language's extractor binds no type names to files (C#), a base
//! that is not bound is looked up by name: in the class's file, its
//! directory, else the only class of that name in the language family.

use std::collections::{BTreeMap, HashMap, HashSet};

use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};
use uuid::Uuid;

use super::types::type_name;
use super::{CallResolution, CallResolver, DefId, Params};
use crate::dispatch::{is_universal_method, DispatchConfidence, OverriddenKind};

/// How deep the base-class walk goes.
const MAX_HIERARCHY_DEPTH: u32 = 8;

/// What the signature says about a method, as far as overriding goes.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct MethodTraits {
    /// `static` (a Rust associated function without `self`, a Python
    /// `@staticmethod`): bound by its class, never dispatched.
    is_static: bool,
    /// `private`: invisible to subclasses.
    is_private: bool,
    /// `abstract` (or `@abstractmethod`).
    is_abstract: bool,
    /// Has a body (the declaration ends with `}`, or is Python).
    has_body: bool,
    /// Parameters, where the signature is readable.
    arity: Option<Params>,
}

impl MethodTraits {
    pub(super) fn of(symbol: &CodeSymbol, decorators: &[String], arity: Option<Params>) -> Self {
        let head = symbol
            .signature
            .find(&format!("{}(", symbol.name))
            .map_or(symbol.signature.as_str(), |at| &symbol.signature[..at]);
        let has = |word: &str| {
            head.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|token| token == word)
        };
        let decorated = |name: &str| decorators.iter().any(|d| d.contains(name));
        let file = symbol.file_path.as_str();
        let python = file.ends_with(".py") || file.ends_with(".pyi");
        let rust = file.ends_with(".rs");
        let content = symbol.content.trim_end();
        Self {
            is_static: has("static")
                || decorated("staticmethod")
                || (rust && !symbol.signature.contains("self")),
            is_private: has("private"),
            is_abstract: has("abstract") || decorated("abstractmethod"),
            has_body: python || content.ends_with('}'),
            arity,
        }
    }
}

impl CallResolver {
    /// Every (method, overridden method) pair, sorted. Computed once per
    /// preparation; [`Self::resolve_calls`] computes it first, since a call
    /// matched by name to one method and its overrides binds to the method
    /// ([`Self::overridden_root`]).
    pub(super) fn override_pairs(&mut self) -> &[(DefId, DefId)] {
        self.prepare();
        if self.override_pairs.is_none() {
            let pairs = self.compute_override_pairs();
            let mut up: HashMap<DefId, Vec<DefId>> = HashMap::new();
            for &(method, base) in &pairs {
                up.entry(method).or_default().push(base);
            }
            self.overridden = up;
            self.override_pairs = Some(pairs);
        }
        self.override_pairs.as_deref().unwrap_or_default()
    }

    /// Of several candidates for a call matched by name, the one the others
    /// override, when there is exactly one such and every other candidate
    /// overrides it: the call is a call of that method, which dispatch
    /// follows to the rest (#190).
    pub(super) fn overridden_root(&self, candidates: &[(DefId, CallResolution)]) -> Option<DefId> {
        let ids: Vec<DefId> = candidates.iter().map(|&(id, _)| id).collect();
        let overrides = |method: DefId, base: DefId| {
            self.overridden
                .get(&method)
                .is_some_and(|bases| bases.contains(&base))
        };
        let roots: Vec<DefId> = ids
            .iter()
            .copied()
            .filter(|&id| !ids.iter().any(|&other| other != id && overrides(id, other)))
            .collect();
        match roots.as_slice() {
            [root] if ids.iter().all(|&id| id == *root || overrides(id, *root)) => Some(*root),
            _ => None,
        }
    }

    fn compute_override_pairs(&self) -> Vec<(DefId, DefId)> {
        // Methods per class, by name.
        let mut methods: HashMap<DefId, BTreeMap<&str, Vec<DefId>>> = HashMap::new();
        for (id, def) in self.defs.iter().enumerate() {
            if def.kind != SymbolKind::Method || def.family == "go" || def.enclosing_fn.is_some() {
                continue;
            }
            if let Some(class) = self.container_class(id) {
                methods
                    .entry(class)
                    .or_default()
                    .entry(def.name.as_str())
                    .or_default()
                    .push(id);
            }
        }
        let mut classes: Vec<DefId> = methods.keys().copied().collect();
        classes.sort_unstable();

        let mut pairs: Vec<(DefId, DefId)> = Vec::new();
        for class in classes {
            let ancestors = self.ancestors(class);
            if ancestors.is_empty() {
                continue;
            }
            for (name, own) in &methods[&class] {
                for &method in own {
                    if !self.may_override(method, class) {
                        continue;
                    }
                    for ancestor in &ancestors {
                        let Some(declared) = methods.get(ancestor).and_then(|m| m.get(name)) else {
                            continue;
                        };
                        for &base in declared {
                            if self.may_override(base, *ancestor)
                                && same_arity(self.defs[method].traits, self.defs[base].traits)
                            {
                                pairs.push((method, base));
                            }
                        }
                    }
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    /// One OVERRIDES edge per (method, overridden method) pair, with a
    /// [`DispatchConfidence`] in its metadata. Call after the symbols and
    /// files are registered.
    pub fn resolve_overrides(&mut self, repo_id: &str) -> Vec<Relationship> {
        let pairs = self.override_pairs().to_vec();
        let abstract_method = |id: DefId| -> bool {
            let def = &self.defs[id];
            def.traits.is_abstract
                || (!def.traits.has_body
                    && self
                        .container_class(id)
                        .is_some_and(|c| self.defs[c].kind == SymbolKind::Interface))
        };
        let mut implementations: HashMap<DefId, usize> = HashMap::new();
        for &(method, base) in &pairs {
            if !abstract_method(method) {
                *implementations.entry(base).or_default() += 1;
            }
        }
        pairs
            .into_iter()
            .map(|(method, base)| {
                let def = &self.defs[base];
                let record = DispatchConfidence {
                    overridden: if abstract_method(base) {
                        OverriddenKind::Abstract
                    } else {
                        OverriddenKind::Concrete
                    },
                    implementations: implementations.get(&base).copied().unwrap_or(0),
                    abstract_override: abstract_method(method),
                    universal: is_universal_method(&def.family, &def.name),
                };
                Relationship {
                    uid: Uuid::new_v4().to_string(),
                    source_uid: self.defs[method].uid.clone(),
                    target_uid: def.uid.clone(),
                    kind: RelationshipKind::Overrides,
                    repo_id: repo_id.to_string(),
                    metadata: record.to_metadata(),
                }
            })
            .collect()
    }

    /// Whether `method` of `class` takes part in overriding: not static,
    /// private or a constructor.
    fn may_override(&self, method: DefId, class: DefId) -> bool {
        let def = &self.defs[method];
        let traits = def.traits;
        !traits.is_static
            && !traits.is_private
            && def.name != self.defs[class].name
            && !matches!(def.name.as_str(), "constructor" | "__init__" | "__new__")
    }

    /// Every project class, interface or trait above `class`, nearest
    /// first, each once.
    fn ancestors(&self, class: DefId) -> Vec<DefId> {
        let mut seen: HashSet<DefId> = HashSet::from([class]);
        let mut out = Vec::new();
        let mut level = vec![class];
        for depth in 0..MAX_HIERARCHY_DEPTH {
            let mut next = Vec::new();
            for &c in &level {
                for base in self.override_bases(c, depth) {
                    if seen.insert(base) {
                        out.push(base);
                        next.push(base);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            level = next;
        }
        out
    }

    /// The direct project bases of `class`: the call resolver's, else, in
    /// a family whose extractor binds no type names to files, the class
    /// each base names (see the module documentation).
    fn override_bases(&self, class: DefId, depth: u32) -> Vec<DefId> {
        let resolved = self.bases(class, depth);
        let def = &self.defs[class];
        if binds_type_names(&def.family) {
            return resolved;
        }
        let mut out = resolved;
        for base in &def.bases {
            let file = base.at.as_ref().map_or(def.file.as_str(), |(f, _)| f);
            let Some(name) = type_name(&def.family, &base.text) else {
                continue;
            };
            let short = name.rsplit(['.', ':']).next().unwrap_or(&name);
            if let Some(found) = self.declared_type(file, short) {
                if found != class && !out.contains(&found) {
                    out.push(found);
                }
            }
        }
        out
    }
}

/// Families whose extractor resolves type names through imports or
/// packages: a base it cannot bind is outside the project.
fn binds_type_names(family: &str) -> bool {
    matches!(family, "jvm" | "js" | "python" | "rs" | "go")
}

/// Whether two methods take the same parameters, where both signatures
/// say: the count, and whether the last is varargs.
fn same_arity(a: MethodTraits, b: MethodTraits) -> bool {
    match (a.arity, b.arity) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}
