//! OVERRIDES edges (#190): which base-class or interface methods each
//! method overrides or implements.
//!
//! A method `C.m` overrides `A.m` when `A` is a project class, interface or
//! trait among `C`'s bases, transitively, `A` declares a method of the same
//! name that the language lets `C.m` override, and the parameters agree. An
//! edge goes to every such declaration up the hierarchy, not only to the
//! nearest, so the callers of an interface method reach an implementation
//! below an abstract class in one step. Go has no inheritance: an embedded
//! struct's methods are promoted, not overridden.
//!
//! What may override what, by the modifiers in the signature:
//!
//! - everywhere: static and private methods and constructors neither
//!   override nor are overridden;
//! - Java: a `final` method is not overridden;
//! - Kotlin: a class's method is overridden only when it is `open`,
//!   `abstract` or an `override` that is not `final`, and only by an
//!   `override` (an interface's methods are open);
//! - C#: a class's method is overridden only when it is `virtual`,
//!   `abstract` or an `override` that is not `sealed`, and only by an
//!   `override`; a `new` method hides it instead (an interface's methods are
//!   implemented by any method of that name).
//!
//! Parameters: the counts must agree where both signatures give them, and
//! in Java, Kotlin, C# and TypeScript each parameter's type, compared
//! without generic arguments, package or namespace qualifiers and
//! nullability. A type parameter of the method or its class matches any
//! type (`Converter<S, T>.convert(S)` is implemented by `convert(String)`),
//! and so does a parameter whose type the signature does not give. Where
//! several overloads of a base class still fit, the ones whose types agree
//! exactly win; if that leaves more than one, each gets an edge marked
//! ambiguous, with a low confidence.
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

/// The language of a definition, as far as overriding rules go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Lang {
    Java,
    Kotlin,
    CSharp,
    Js,
    #[default]
    Other,
}

impl Lang {
    fn of(file: &str) -> Self {
        let ext = file.rsplit_once('.').map_or("", |(_, ext)| ext);
        match ext {
            "java" => Lang::Java,
            "kt" | "kts" => Lang::Kotlin,
            "cs" => Lang::CSharp,
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Lang::Js,
            _ => Lang::Other,
        }
    }
}

/// What the signature says about a definition, as far as overriding goes.
#[derive(Debug, Clone, Default)]
pub(super) struct MethodTraits {
    lang: Lang,
    /// `static` (a Rust associated function without `self`, a Python
    /// `@staticmethod`): bound by its class, never dispatched.
    is_static: bool,
    /// `private`: invisible to subclasses.
    is_private: bool,
    /// `abstract` (or `@abstractmethod`).
    is_abstract: bool,
    /// `final` (Java, Kotlin) or `sealed` (C#): not overridden further.
    is_final: bool,
    /// `virtual`, `abstract`, `override` (C#) or `open`, `abstract`,
    /// `override` (Kotlin): a class's method that may be overridden.
    is_open: bool,
    /// `override` (C#, Kotlin).
    is_override: bool,
    /// `new` (C#): hides the base class's method instead of overriding it.
    hides: bool,
    /// Has a body (the declaration ends with `}`, or is Python).
    has_body: bool,
    /// Parameters, where the signature is readable.
    arity: Option<Params>,
    /// Each parameter's type, normalised (see [`normalise_type`]); an empty
    /// string where the signature does not give it. `None` when the
    /// parameter list is unreadable or the language is not compared.
    param_types: Option<Vec<String>>,
    /// Type parameters the definition declares (`<T>` of a generic method
    /// or class).
    type_params: Vec<String>,
}

impl MethodTraits {
    pub(super) fn of(symbol: &CodeSymbol, decorators: &[String], arity: Option<Params>) -> Self {
        let lang = Lang::of(&symbol.file_path);
        let signature = symbol.signature.as_str();
        let name_at = name_position(signature, &symbol.name);
        let head = name_at.map_or(signature, |at| &signature[..at]);
        let has = |word: &str| {
            head.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|token| token == word)
        };
        let decorated = |name: &str| decorators.iter().any(|d| d.contains(name));
        let file = symbol.file_path.as_str();
        let python = file.ends_with(".py") || file.ends_with(".pyi");
        let rust = file.ends_with(".rs");
        let content = symbol.content.trim_end();
        let after_name = name_at.map(|at| &signature[at + symbol.name.len()..]);
        let mut type_params = after_name
            .and_then(|rest| rest.trim_start().strip_prefix('<'))
            .map(type_parameters)
            .unwrap_or_default();
        // `public <T> T foo(...)`, `fun <T> foo(...)`: a list before the
        // name that follows a space, not a type's own (`List<String>`).
        if let Some(open) = head
            .char_indices()
            .find(|&(i, c)| {
                c == '<'
                    && head[..i]
                        .chars()
                        .next_back()
                        .is_none_or(char::is_whitespace)
            })
            .map(|(i, _)| i)
        {
            type_params.extend(type_parameters(&head[open + 1..]));
        }
        let param_types = (lang != Lang::Other)
            .then(|| after_name.and_then(|rest| param_types(rest, lang)))
            .flatten();
        Self {
            lang,
            is_static: has("static")
                || decorated("staticmethod")
                || (rust && !signature.contains("self")),
            is_private: has("private"),
            is_abstract: has("abstract") || decorated("abstractmethod"),
            is_final: has("final") || (lang == Lang::CSharp && has("sealed")),
            is_open: has("abstract")
                || has("override")
                || match lang {
                    Lang::CSharp => has("virtual"),
                    Lang::Kotlin => has("open"),
                    _ => false,
                },
            is_override: has("override"),
            hides: lang == Lang::CSharp && has("new"),
            has_body: python || content.ends_with('}'),
            arity,
            param_types,
            type_params,
        }
    }
}

impl CallResolver {
    /// Every (method, overridden method, ambiguous) triple, sorted. Computed once per
    /// preparation; [`Self::resolve_calls`] computes it first, since a call
    /// matched by name to one method and its overrides binds to the method
    /// ([`Self::overridden_root`]).
    pub(super) fn override_pairs(&mut self) -> &[(DefId, DefId, bool)] {
        self.prepare();
        if self.override_pairs.is_none() {
            let pairs = self.compute_override_pairs();
            let mut up: HashMap<DefId, Vec<DefId>> = HashMap::new();
            // An ambiguous pair does not make the call bind the base.
            for &(method, base, _) in pairs.iter().filter(|p| !p.2) {
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

    fn compute_override_pairs(&self) -> Vec<(DefId, DefId, bool)> {
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

        let mut pairs: Vec<(DefId, DefId, bool)> = Vec::new();
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
                    for &ancestor in &ancestors {
                        let Some(declared) = methods.get(&ancestor).and_then(|m| m.get(name))
                        else {
                            continue;
                        };
                        // The overloads `method` may override, with how many
                        // parameter types agree exactly.
                        let fitting: Vec<(DefId, usize)> = declared
                            .iter()
                            .copied()
                            .filter(|&base| {
                                self.may_override(base, ancestor)
                                    && self.overridable(method, base, ancestor)
                            })
                            .filter_map(|base| {
                                self.parameters_fit(method, class, base, ancestor)
                                    .map(|exact| (base, exact))
                            })
                            .collect();
                        let best = fitting.iter().map(|&(_, exact)| exact).max();
                        let chosen: Vec<DefId> = fitting
                            .iter()
                            .filter(|&&(_, exact)| Some(exact) == best)
                            .map(|&(base, _)| base)
                            .collect();
                        let ambiguous = chosen.len() > 1;
                        pairs.extend(chosen.into_iter().map(|base| (method, base, ambiguous)));
                    }
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    /// Whether the language lets `method` override `base`, a method of
    /// `ancestor` (see the module documentation).
    fn overridable(&self, method: DefId, base: DefId, ancestor: DefId) -> bool {
        let (m, b) = (&self.defs[method].traits, &self.defs[base].traits);
        let interface = self.defs[ancestor].kind == SymbolKind::Interface;
        if b.is_final {
            return false;
        }
        match b.lang {
            Lang::Kotlin => m.is_override && (interface || b.is_open),
            Lang::CSharp if interface => !m.hides,
            Lang::CSharp => m.is_override && !m.hides && b.is_open,
            _ => true,
        }
    }

    /// Whether the parameters of `method` (of `class`) fit those of `base`
    /// (of `ancestor`): `None` when they do not, else how many parameter
    /// types agree exactly.
    fn parameters_fit(
        &self,
        method: DefId,
        class: DefId,
        base: DefId,
        ancestor: DefId,
    ) -> Option<usize> {
        let (m, b) = (&self.defs[method].traits, &self.defs[base].traits);
        if let (Some(ma), Some(ba)) = (m.arity, b.arity) {
            if ma != ba {
                return None;
            }
        }
        let (Some(mt), Some(bt)) = (&m.param_types, &b.param_types) else {
            return Some(0);
        };
        if mt.len() != bt.len() {
            return None;
        }
        let generic = |traits: &MethodTraits, owner: DefId, ty: &str| {
            ty.is_empty()
                || traits.type_params.iter().any(|p| p == ty)
                || self.defs[owner].traits.type_params.iter().any(|p| p == ty)
                || looks_like_type_parameter(ty)
        };
        let mut exact = 0;
        for (a, z) in mt.iter().zip(bt) {
            if generic(m, class, a) || generic(b, ancestor, z) {
                continue;
            }
            if a != z {
                return None;
            }
            exact += 1;
        }
        Some(exact)
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
        for &(method, base, _) in &pairs {
            if !abstract_method(method) {
                *implementations.entry(base).or_default() += 1;
            }
        }
        pairs
            .into_iter()
            .map(|(method, base, ambiguous)| {
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
                    ambiguous,
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
        let traits = &def.traits;
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

/// Where `name` stands in `signature` as the defined name: followed by
/// its parameter list or type parameters.
fn name_position(signature: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    signature.match_indices(name).map(|(at, _)| at).find(|&at| {
        let before_ok = signature[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !ident(c));
        let rest = signature[at + name.len()..].trim_start();
        before_ok && (rest.starts_with('(') || rest.starts_with('<'))
    })
}

/// The text up to the `>` closing a list opened just before `text`.
fn until_closing(text: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some(&text[..i]);
            }
            depth -= 1;
        }
    }
    None
}

/// Splits on commas outside brackets.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' | '>' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// The names a type-parameter list declares (`K, V extends Comparable<V>`,
/// `in T`, `T : Any`), given the text after its `<`.
fn type_parameters(after_open: &str) -> Vec<String> {
    let Some(list) = until_closing(after_open, '<', '>') else {
        return Vec::new();
    };
    split_top_level(list)
        .into_iter()
        .filter_map(|param| {
            param
                .split(|c: char| c.is_whitespace() || c == ':')
                .find(|word| !word.is_empty() && !matches!(*word, "in" | "out" | "reified"))
                .map(str::to_string)
        })
        .collect()
}

/// Each parameter's type from the text after a method's name (`(String
/// id, Map<K, V> m)`, `(id: String)`), normalised; `None` when the list
/// cannot be read.
fn param_types(after_name: &str, lang: Lang) -> Option<Vec<String>> {
    let rest = after_name.trim_start();
    let rest = match rest.strip_prefix('<') {
        Some(generic) => {
            let list = until_closing(generic, '<', '>')?;
            generic[list.len() + 1..].trim_start()
        }
        None => rest,
    };
    let list = until_closing(rest.strip_prefix('(')?, '(', ')')?;
    if list.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(
        split_top_level(list)
            .into_iter()
            .map(|param| normalise_type(&param_type(param, lang)))
            .collect(),
    )
}

/// The written type of one parameter, without annotations, attributes,
/// modifiers, name and default value; empty when it has none.
fn param_type(param: &str, lang: Lang) -> String {
    let param = strip_annotations(param);
    let colon_style = matches!(lang, Lang::Kotlin | Lang::Js);
    let declared = if colon_style {
        // `name: Type = default`, `...rest: T[]`, `vararg xs: T`
        let Some((_, ty)) = split_once_top_level(&param, ':') else {
            return String::new();
        };
        let ty = split_once_top_level(ty, '=').map_or(ty, |(ty, _)| ty);
        let ty = ty.trim();
        if param.trim_start().starts_with("vararg") || param.trim_start().starts_with("...") {
            format!("{ty}[]")
        } else {
            ty.to_string()
        }
    } else {
        // `final Type name`, `ref Type name = 1`, `Object... args`
        let param = split_once_top_level(&param, '=').map_or(param.as_str(), |(p, _)| p);
        let words: Vec<&str> = split_words_top_level(param)
            .into_iter()
            .filter(|w| {
                !matches!(
                    *w,
                    "final" | "ref" | "out" | "in" | "params" | "this" | "scoped" | "readonly"
                )
            })
            .collect();
        match words.as_slice() {
            [] | [_] => String::new(),
            [ty @ .., _name] => ty.join(" "),
        }
    };
    declared
}

/// `Map<String, List<User>>` → `Map`; `java.util.List<T>[]` → `List[]`;
/// `String?` → `String`; `Object...` → `Object[]`; `global::Ns.Type` →
/// `Type`.
fn normalise_type(ty: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in ty.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth > 0 => {}
            '?' | '!' => {}
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    let out = out.replace("...", "[]");
    let out = out.strip_prefix("global::").unwrap_or(&out);
    let (base, dims) = match out.find('[') {
        Some(at) => (&out[..at], &out[at..]),
        None => (out, ""),
    };
    let short = base.rsplit(['.', ':']).next().unwrap_or(base);
    format!("{short}{dims}")
}

/// A type name that reads like a type parameter: one capital letter,
/// possibly followed by digits (`T`, `E`, `T2`).
fn looks_like_type_parameter(ty: &str) -> bool {
    let mut chars = ty.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_digit())
}

/// Removes `@Annotation`, `@Annotation(...)` and C# `[Attribute]` parts.
fn strip_annotations(param: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = param.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '@' => {
                i += 1;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.'))
                {
                    i += 1;
                }
                if i < chars.len() && chars[i] == '(' {
                    let mut depth = 0;
                    while i < chars.len() {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
            }
            '[' if out.trim().is_empty() => {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Splits at the first `sep` outside brackets.
fn split_once_top_level(text: &str, sep: char) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' | '>' | ']' | '}' => depth -= 1,
            c if c == sep && depth == 0 => return Some((&text[..i], &text[i + c.len_utf8()..])),
            _ => {}
        }
    }
    None
}

/// Whitespace-separated words, keeping `Map<String, Object>` in one.
fn split_words_top_level(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, None);
    for (i, c) in text.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(s) = start.take() {
                out.push(&text[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_types_are_read_and_normalised() {
        let types = |sig: &str, lang| param_types(sig, lang).unwrap();
        assert_eq!(
            types(
                "(final @Nullable Map<String, Object> hints, String... args)",
                Lang::Java
            ),
            ["Map", "String[]"]
        );
        assert_eq!(
            types("(java.util.List<T>[] items, int n)", Lang::Java),
            ["List[]", "int"]
        );
        assert_eq!(
            types("(id: String?, vararg rest: Int = 0)", Lang::Kotlin),
            ["String", "Int[]"]
        );
        assert_eq!(
            types(
                "([FromBody] ref global::Ns.Order order, int n = 3)",
                Lang::CSharp
            ),
            ["Order", "int"]
        );
        assert_eq!(types("<T>(x?: T, y): void", Lang::Js), ["T", ""]);
        assert_eq!(types("()", Lang::Java), Vec::<String>::new());
    }

    #[test]
    fn type_parameters_and_names_are_found() {
        assert_eq!(type_parameters("K, V extends Comparable<V>> {"), ["K", "V"]);
        assert_eq!(type_parameters("in T : Any, out R>"), ["T", "R"]);
        assert_eq!(
            name_position("public <T> T convert(S source)", "convert"),
            Some(13)
        );
        assert_eq!(name_position("void reconvert()", "convert"), None);
        assert!(looks_like_type_parameter("T2"));
        assert!(!looks_like_type_parameter("To"));
    }
}
