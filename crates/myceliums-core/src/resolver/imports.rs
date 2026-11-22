//! Import statements → the project files and symbols they bind.
//!
//! The parsers report each import as a local name, a module specifier and
//! (for a named import) the original name. What the specifier means depends
//! on the language; this module maps it to files of the project:
//!
//! | family | specifier | resolved against |
//! |---|---|---|
//! | Python | `shop.models`, `.models` | a `.py` file or a package's `__init__.py`; relative to the importer when dotted |
//! | JS/TS | `./models`, `../x/index`, `@scope/pkg`, `~utils/x`, `@/x` | the importer's directory; a bare specifier through the importer's tsconfig `paths`/`baseUrl`, then the repository's workspace packages ([`super::JsProject`]), then `@/`, `~/` as a path suffix; any other bare name is a library |
//! | Go | `example.com/shop/store` | the project directory whose path ends with the import path's tail |
//! | Rust | `crate::store::Repo`, `model::f`, `super::x` | the longest leading path that is a module file; the rest is an item in it |
//! | Java | `shop.store.Repository`, `static shop.auth.Auth.parse` | the class's file; a static import's member |
//!
//! A specifier that matches no project file is a library: the name is bound,
//! to something outside the project ([`Bound::External`]). A specifier that
//! does name a project file binds what that file declares under the name: a
//! symbol, a module-level value (`current_app: Flask = ...`), a re-export
//! (`from .sessions import Session`, `export { jwt } from './jwt'`,
//! `export * from './x'`), a submodule — or, when the file declares nothing
//! the resolver can see, [`Bound::Unknown`], never a library. For other
//! languages the specifier is tried as a path; when that fails the import
//! is ignored and the resolver keeps searching by name.

use std::collections::{BTreeSet, HashMap};

use super::{js_project, language_family, parent_dir, CallResolver, Def, DefId, Site};
use crate::parser::ImportInfo;

/// What an import binds.
pub(super) enum Bound {
    /// Specific definitions: a function, a class, a method.
    Defs(Vec<DefId>),
    /// A module, package or namespace: its files.
    Module(Vec<String>),
    /// A module-level value of a project file (`current_app: Flask =
    /// LocalProxy(...)`): the file and the index of its binding.
    Value(String, usize),
    /// A name a project file exports but declares nothing visible for (an
    /// assignment from a call, a re-export the resolver cannot follow).
    Unknown,
    /// Something outside the project: a library or the standard library.
    External,
}

/// How deep re-exports (`from .models import User` in an `__init__.py`)
/// are followed.
const MAX_REEXPORT_DEPTH: u32 = 3;

/// Whether `import` binds every top-level name of its module.
pub(super) fn is_wildcard(family: &str, import: &ImportInfo) -> bool {
    import.local_name == "*" || (family == "go" && import.original_name.as_deref() == Some("."))
}

/// Project paths indexed by their suffixes, for module lookups.
#[derive(Default)]
pub(super) struct PathIndex {
    /// Path suffix without extension → files: `src/shop/models.py` is under
    /// `models`, `shop/models` and `src/shop/models`. A package file
    /// (`__init__.py`, `mod.rs`, `index.ts`) is also under its directory's
    /// suffixes.
    modules: HashMap<String, Vec<String>>,
    /// Directory suffix → directories.
    dirs: HashMap<String, Vec<String>>,
    /// Directory → its files.
    dir_files: HashMap<String, Vec<String>>,
}

/// The suffixes of a `/`-separated path: `a/b/c` → `c`, `b/c`, `a/b/c`.
fn suffixes(path: &str) -> impl Iterator<Item = &str> {
    std::iter::once(path).chain(path.match_indices('/').map(move |(i, _)| &path[i + 1..]))
}

/// A JS/TS path without a module file extension (`a/b.d.ts` → `a/b`); a
/// directory name with a dot (`v1.2`) is kept.
fn js_stem(path: &str) -> &str {
    const EXTENSIONS: &[&str] = &[
        ".d.ts", ".d.mts", ".d.cts", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs",
        ".vue", ".svelte", ".json",
    ];
    EXTENSIONS
        .iter()
        .find_map(|ext| path.strip_suffix(ext))
        .filter(|rest| !rest.is_empty() && !rest.ends_with('/'))
        .unwrap_or(path)
}

/// A path without its file extension.
fn stem(path: &str) -> &str {
    let file_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[file_start..].rfind('.') {
        Some(dot) if dot > 0 => &path[..file_start + dot],
        _ => path,
    }
}

impl PathIndex {
    pub(super) fn new<'a>(paths: impl Iterator<Item = &'a String>) -> Self {
        let mut index = PathIndex::default();
        let paths: BTreeSet<&String> = paths.collect();
        let mut dirs = BTreeSet::new();
        for path in paths {
            let stem = stem(path);
            let mut keys: Vec<&str> = suffixes(stem).collect();
            let file_name = stem.rsplit('/').next().unwrap_or(stem);
            if matches!(file_name, "__init__" | "mod" | "index") && stem.contains('/') {
                keys.extend(suffixes(parent_dir(stem)));
            }
            for key in keys {
                index
                    .modules
                    .entry(key.to_string())
                    .or_default()
                    .push(path.clone());
            }
            let dir = parent_dir(path);
            index
                .dir_files
                .entry(dir.to_string())
                .or_default()
                .push(path.clone());
            dirs.insert(dir.to_string());
        }
        for dir in dirs {
            for key in suffixes(&dir) {
                index
                    .dirs
                    .entry(key.to_string())
                    .or_default()
                    .push(dir.clone());
            }
        }
        index
    }

    /// Files of `family` filed under the path suffix `key`.
    fn modules(&self, key: &str, family: &str) -> Vec<String> {
        self.modules
            .get(key)
            .into_iter()
            .flatten()
            .filter(|f| language_family(f) == Some(family))
            .cloned()
            .collect()
    }

    /// Directories ending in `key` that hold files of `family`.
    fn dirs(&self, key: &str, family: &str) -> Vec<String> {
        self.dirs
            .get(key)
            .into_iter()
            .flatten()
            .filter(|dir| !self.files_in(dir, family).is_empty())
            .cloned()
            .collect()
    }

    /// Whether `dir` is a Python package: it holds an `__init__.py`.
    fn is_python_package(&self, dir: &str) -> bool {
        self.dir_files.get(dir).is_some_and(|files| {
            files
                .iter()
                .any(|f| f.rsplit('/').next() == Some("__init__.py"))
        })
    }

    pub(super) fn files_in(&self, dir: &str, family: &str) -> Vec<String> {
        self.dir_files
            .get(dir)
            .into_iter()
            .flatten()
            .filter(|f| language_family(f) == Some(family))
            .cloned()
            .collect()
    }
}

/// Of several paths found under the module path `key`, the ones an
/// absolute import means: the fewest directories in front of `key` (the
/// module at a source root, `src/flask/__init__.py`, not a test fixture's
/// `tests/apps/inner/flask.py`), then the most leading directories shared
/// with `importer` (the copy in the importer's own tree).
fn closest(importer: &str, key: &str, paths: Vec<String>) -> Vec<String> {
    let segments = |s: &str| s.split('/').filter(|x| !x.is_empty()).count();
    let depth = |path: &str| {
        let stem = stem(path);
        let matched = if stem.ends_with(key) {
            stem
        } else {
            parent_dir(stem)
        };
        segments(matched).saturating_sub(segments(key))
    };
    let shared = |path: &str| {
        parent_dir(importer)
            .split('/')
            .zip(path.split('/'))
            .take_while(|(a, b)| a == b)
            .count()
    };
    let rank = |path: &str| (depth(path), std::cmp::Reverse(shared(path)));
    let best = paths.iter().map(|p| rank(p)).min();
    paths
        .into_iter()
        .filter(|p| Some(rank(p)) == best)
        .collect()
}

/// `dir` joined with a relative path, with `.` and `..` folded.
fn join_relative(dir: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

impl CallResolver {
    /// What `import`, declared in the call's file, binds; `None` when the
    /// specifier means nothing to the resolver.
    pub(super) fn resolve_import(&self, site: &Site, import: &ImportInfo) -> Option<Bound> {
        self.bind(site.file, site.family, import, 0)
    }

    fn bind(&self, importer: &str, family: &str, import: &ImportInfo, depth: u32) -> Option<Bound> {
        match family {
            "python" => Some(self.bind_python(importer, import, depth)),
            "js" => Some(self.bind_js(importer, import, depth)),
            "go" => Some(match self.go_package(importer, &import.source_module) {
                Some(files) => Bound::Module(files),
                None => Bound::External,
            }),
            "rs" => Some(self.bind_rust(importer, import, depth)),
            "jvm" => self.bind_jvm(importer, import),
            _ => self.bind_by_path(importer, family, import),
        }
    }

    /// `module.name`: a top-level definition of the module, what the
    /// module re-exports under that name (`from .sessions import Session`
    /// in a package's `__init__.py`), or a submodule (`package.utils`).
    pub(super) fn module_member(
        &self,
        files: &[String],
        family: &str,
        name: &str,
    ) -> Option<Bound> {
        if let Some(bound) = self.defined_or_reexported(files, family, name, 0) {
            return Some(bound);
        }
        let submodules: Vec<String> = files
            .iter()
            .filter(|f| {
                let file_name = stem(f).rsplit('/').next().unwrap_or_default();
                matches!(file_name, "__init__" | "mod" | "index")
            })
            .flat_map(|package| {
                let key = format!("{}/{name}", parent_dir(package));
                self.index
                    .modules(&key, family)
                    .into_iter()
                    .filter(move |f| stem(f) == key || parent_dir(stem(f)) == key)
            })
            .collect();
        (!submodules.is_empty()).then_some(Bound::Module(submodules))
    }

    /// The module-level binding named `name` in `file` (a value declared or
    /// assigned at the top level), as its index.
    pub(super) fn module_value(&self, file: &str, name: &str) -> Option<usize> {
        let context = self.files.get(file)?;
        context
            .bindings
            .iter()
            .enumerate()
            .rev()
            .find(|(_, b)| b.owner.is_none() && b.name == name && b.scope.1 == u32::MAX)
            .map(|(index, _)| index)
    }

    /// Top-level definitions named `name` in `files`, a module-level value of
    /// that name, or what the files' own imports of `name` bind (a
    /// re-export, also through `from m import *` / `export * from`).
    fn defined_or_reexported(
        &self,
        files: &[String],
        family: &str,
        name: &str,
        depth: u32,
    ) -> Option<Bound> {
        let defs = self.top_level_in(files, name, |_| true);
        if !defs.is_empty() {
            return Some(Bound::Defs(defs));
        }
        if let Some((file, index)) = files
            .iter()
            .find_map(|f| self.module_value(f, name).map(|i| (f, i)))
        {
            return Some(Bound::Value(file.clone(), index));
        }
        if depth >= MAX_REEXPORT_DEPTH {
            return None;
        }
        let reexported = files.iter().find_map(|file| {
            let import = self
                .files
                .get(file)?
                .imports
                .iter()
                .find(|i| i.local_name == name)?;
            // A library name re-exported by a project module (requests'
            // `compat.py` re-exporting `MutableMapping`) is a library name.
            self.bind(file, family, import, depth + 1)
        });
        if reexported.is_some() {
            return reexported;
        }
        files.iter().find_map(|file| {
            self.files
                .get(file)?
                .imports
                .iter()
                .filter(|i| is_wildcard(family, i))
                .find_map(|import| match self.bind(file, family, import, depth + 1)? {
                    Bound::Module(inner) => {
                        self.defined_or_reexported(&inner, family, name, depth + 1)
                    }
                    _ => None,
                })
        })
    }

    // --- Python -----------------------------------------------------------

    fn bind_python(&self, importer: &str, import: &ImportInfo, depth: u32) -> Bound {
        let module = import.source_module.as_str();
        match import.original_name.as_deref() {
            // `from m import name [as local]`
            Some(name) => {
                if let Some(files) = self.python_module(importer, module) {
                    if let Some(bound) = self.defined_or_reexported(&files, "python", name, depth) {
                        return bound;
                    }
                }
                let submodule = if module.ends_with('.') {
                    format!("{module}{name}")
                } else {
                    format!("{module}.{name}")
                };
                match self.python_module(importer, &submodule) {
                    Some(files) => Bound::Module(files),
                    // The module is the project's but does not declare the
                    // name visibly: unknown, not a library.
                    None if self.python_module(importer, module).is_some() => Bound::Unknown,
                    None => Bound::External,
                }
            }
            // `import m`, `import m as local`; `import a.b` binds `a`.
            None => {
                let top = module.split('.').next().unwrap_or(module);
                let bound = if import.local_name == top {
                    top
                } else {
                    module
                };
                match self.python_module(importer, bound) {
                    Some(files) => Bound::Module(files),
                    None => Bound::External,
                }
            }
        }
    }

    /// The file of a Python module (`shop.models`, `.models`, `..x`).
    fn python_module(&self, importer: &str, dotted: &str) -> Option<Vec<String>> {
        let level = dotted.chars().take_while(|&c| c == '.').count();
        let path = dotted[level..].replace('.', "/");
        let files = if level > 0 {
            let mut base = parent_dir(importer).to_string();
            for _ in 1..level {
                base = parent_dir(&base).to_string();
            }
            let key = join_relative(&base, &path);
            self.index
                .modules(&key, "python")
                .into_iter()
                .filter(|f| stem(f) == key || parent_dir(stem(f)) == key)
                .collect()
        } else {
            // An absolute import names a module at a source root, not one
            // inside a package: `django/contrib/gis/geos/io.py` is
            // `django.contrib.gis.geos.io`, never the standard library's `io`.
            let top_level = self
                .index
                .modules(&path, "python")
                .into_iter()
                .filter(|f| {
                    let stem = stem(f);
                    let matched = if stem.ends_with(path.as_str()) {
                        stem
                    } else {
                        parent_dir(stem)
                    };
                    let root = matched
                        .strip_suffix(path.as_str())
                        .unwrap_or_default()
                        .trim_end_matches('/');
                    root.is_empty() || !self.index.is_python_package(root)
                })
                .collect();
            closest(importer, &path, top_level)
        };
        (!files.is_empty()).then_some(files)
    }

    // --- JavaScript / TypeScript -------------------------------------------

    fn bind_js(&self, importer: &str, import: &ImportInfo, depth: u32) -> Bound {
        let Some(files) = self.js_module(importer, &import.source_module) else {
            return Bound::External;
        };
        match import.original_name.as_deref() {
            // `import * as ns from "./m"`
            Some("*") => Bound::Module(files),
            // `import { name as local } from "./m"`
            Some(name) => self
                .defined_or_reexported(&files, "js", name, depth)
                .unwrap_or(Bound::Unknown),
            // `import Default from "./m"`, `const m = require("./m")`: the
            // default export, often named like the local; else the module.
            None => match self.top_level_in(&files, &import.local_name, |_| true) {
                defs if !defs.is_empty() => Bound::Defs(defs),
                _ => Bound::Module(files),
            },
        }
    }

    /// The file a JS/TS module specifier names, `None` for a package.
    ///
    /// A bare specifier is tried, in order, against the importer's
    /// tsconfig/jsconfig `paths` and `baseUrl`, the repository's workspace
    /// packages (#169), and the `@/x` / `~/x` source-root convention; any
    /// other bare specifier is a library.
    fn js_module(&self, importer: &str, specifier: &str) -> Option<Vec<String>> {
        let relative =
            specifier.starts_with("./") || specifier.starts_with("../") || specifier == ".";
        if !relative {
            if let Some(files) = self.js_declared_module(importer, specifier) {
                return Some(files);
            }
        }
        let key = if relative {
            join_relative(parent_dir(importer), specifier)
        } else {
            // `@/x`, `~/x`: a path alias for the source root; any other
            // bare specifier is a package.
            specifier
                .strip_prefix("@/")
                .or_else(|| specifier.strip_prefix("~/"))?
                .to_string()
        };
        // `./models.js` may name `models.ts`.
        let key = stem(&key).to_string();
        let relative = specifier.starts_with('.');
        let files: Vec<String> = self
            .index
            .modules(&key, "js")
            .into_iter()
            .filter(|f| !relative || stem(f) == key || parent_dir(stem(f)) == key)
            .collect();
        let files = closest(importer, &key, files);
        (!files.is_empty()).then_some(files)
    }

    /// A bare specifier the repository declares: a `paths` alias or a path
    /// under `baseUrl` of the importer's config file, else a workspace
    /// package (its entry, or a subpath of it).
    fn js_declared_module(&self, importer: &str, specifier: &str) -> Option<Vec<String>> {
        if let Some(aliases) = self.js.aliases_for(importer) {
            for target in aliases.targets(specifier) {
                if let Some(files) = self.js_file(&target) {
                    return Some(files);
                }
            }
            if let Some(path) = aliases
                .base_url
                .as_deref()
                .and_then(|base| js_project::join(base, specifier))
            {
                if let Some(files) = self.js_file(&path) {
                    return Some(files);
                }
            }
        }
        let (package, subpath) = self.js.package_for(importer, specifier)?;
        // A path inside the package; `None` when it climbs out of the
        // repository.
        let in_package = |path: &str| -> Option<Vec<String>> {
            self.js_file(&js_project::join(&package.dir, path)?)
        };
        // A declared file, or the source a build output was made from.
        let declared = |entry: &str| -> Option<Vec<String>> {
            match js_project::source_for_build_path(entry) {
                Some(source) => in_package(&source),
                None => in_package(entry),
            }
        };
        if subpath.is_empty() {
            // Declared entries under `src/` (or standing for a file there),
            // then `src/index`, then the declared entries as they are (a
            // root `index.js` may be a shim that loads `dist/`), then the
            // package's own `index`.
            let nested = |e: &&String| e.trim_start_matches("./").contains('/');
            return package
                .entries
                .iter()
                .filter(nested)
                .find_map(|e| declared(e))
                .or_else(|| in_package("src"))
                .or_else(|| package.entries.iter().find_map(|e| in_package(e)))
                .or_else(|| self.js_file(&package.dir));
        }
        package
            .subpath_targets(subpath)
            .iter()
            .find_map(|target| declared(target))
            .or_else(|| in_package(&format!("src/{subpath}")))
            .or_else(|| declared(subpath))
    }

    /// The indexed JS/TS file a repository-relative path names, with or
    /// without its extension (`./x.js` may be `x.ts`), or the `index` file
    /// of the directory it names.
    fn js_file(&self, path: &str) -> Option<Vec<String>> {
        let key = js_stem(path);
        let files: Vec<String> = self
            .index
            .modules(key, "js")
            .into_iter()
            .filter(|f| stem(f) == key || parent_dir(stem(f)) == key)
            .collect();
        // The file itself before a directory of the same name's index.
        let exact: Vec<String> = files.iter().filter(|f| stem(f) == key).cloned().collect();
        match (exact.is_empty(), files.is_empty()) {
            (false, _) => Some(exact),
            (true, false) => Some(files),
            (true, true) => None,
        }
    }

    // --- Go ---------------------------------------------------------------

    /// The files of the project package an import path names: the
    /// directory ending in the longest tail of the path. A one-segment
    /// match counts only for a path that starts with a domain
    /// (`example.com/shop/store`), so `strings` stays the standard library.
    fn go_package(&self, importer: &str, path: &str) -> Option<Vec<String>> {
        let segments: Vec<&str> = path.split('/').collect();
        let domain = segments.len() > 1 && segments[0].contains('.');
        for k in (1..=segments.len()).rev() {
            if k == 1 && !domain {
                break;
            }
            let key = segments[segments.len() - k..].join("/");
            let dirs = closest(importer, &key, self.index.dirs(&key, "go"));
            if let Some(dir) = dirs.first() {
                return Some(self.index.files_in(dir, "go"));
            }
        }
        None
    }

    // --- Rust -------------------------------------------------------------

    fn bind_rust(&self, importer: &str, import: &ImportInfo, depth: u32) -> Bound {
        let mut segments: Vec<&str> = import.source_module.split("::").collect();
        let wildcard = segments.last() == Some(&"*");
        if wildcard {
            segments.pop();
        }
        // `crate::`, `self::`, `super::` all name modules of this crate;
        // the suffix lookup finds them by path.
        while matches!(segments.first(), Some(&("crate" | "self" | "super"))) {
            segments.remove(0);
        }
        for k in (1..=segments.len()).rev() {
            let key = segments[..k].join("/");
            let files = closest(importer, &key, self.index.modules(&key, "rs"));
            if files.is_empty() {
                continue;
            }
            return match &segments[k..] {
                [] => Bound::Module(files),
                [item] => self
                    .defined_or_reexported(&files, "rs", item, depth)
                    .unwrap_or(Bound::Unknown),
                // `Type::assoc_fn`
                [ty, item] => {
                    let defs: Vec<DefId> = self
                        .named(item)
                        .filter(|&id| {
                            let def: &Def = &self.defs[id];
                            files.contains(&def.file) && def.container.as_deref() == Some(*ty)
                        })
                        .collect();
                    if defs.is_empty() {
                        Bound::Unknown
                    } else {
                        Bound::Defs(defs)
                    }
                }
                _ => Bound::Unknown,
            };
        }
        Bound::External
    }

    // --- Java, Kotlin, Scala ------------------------------------------------

    fn bind_jvm(&self, importer: &str, import: &ImportInfo) -> Option<Bound> {
        let source = import.source_module.as_str();
        match import.original_name.as_deref() {
            Some("package") => None,
            // `import static a.b.C.m`
            Some("static") => {
                let (class_path, member) = source.rsplit_once('.')?;
                let class = class_path.rsplit('.').next()?;
                let key = class_path.replace('.', "/");
                let files = closest(importer, &key, self.index.modules(&key, "jvm"));
                let defs: Vec<DefId> = self
                    .named(member)
                    .filter(|&id| {
                        let def = &self.defs[id];
                        files.contains(&def.file)
                            && def.container.as_deref().and_then(|c| c.rsplit('.').next())
                                == Some(class)
                    })
                    .collect();
                Some(match (defs.is_empty(), files.is_empty()) {
                    (false, _) => Bound::Defs(defs),
                    (true, true) => Bound::External,
                    (true, false) => Bound::Unknown,
                })
            }
            // `import a.b.*`
            _ if import.local_name == "*" => {
                let package = source.trim_end_matches(".*").replace('.', "/");
                let dirs = closest(importer, &package, self.index.dirs(&package, "jvm"));
                Some(match dirs.first() {
                    Some(dir) => Bound::Module(self.index.files_in(dir, "jvm")),
                    None => Bound::External,
                })
            }
            // `import a.b.C`, `import a.b.C.Nested`
            _ => {
                let class = source.rsplit('.').next()?;
                let mut path = source.replace('.', "/");
                loop {
                    let files = closest(importer, &path, self.index.modules(&path, "jvm"));
                    if !files.is_empty() {
                        let defs: Vec<DefId> = self
                            .named(class)
                            .filter(|&id| {
                                self.defs[id].is_class_like() && files.contains(&self.defs[id].file)
                            })
                            .collect();
                        return Some(if defs.is_empty() {
                            Bound::Unknown
                        } else {
                            Bound::Defs(defs)
                        });
                    }
                    match path.rsplit_once('/') {
                        Some((outer, _)) => path = outer.to_string(),
                        None => return Some(Bound::External),
                    }
                }
            }
        }
    }

    // --- other languages ----------------------------------------------------

    /// The specifier tried as a path (relative, or a dotted/`::` module
    /// path); `None` when it matches no file.
    fn bind_by_path(&self, importer: &str, family: &str, import: &ImportInfo) -> Option<Bound> {
        let specifier = import.source_module.trim_matches(['"', '\'', '<', '>']);
        let key = if specifier.starts_with("./") || specifier.starts_with("../") {
            join_relative(parent_dir(importer), specifier)
        } else {
            specifier.replace("::", "/").replace('.', "/")
        };
        let files = closest(importer, stem(&key), self.index.modules(stem(&key), family));
        if files.is_empty() {
            return None;
        }
        let name = import
            .original_name
            .as_deref()
            .unwrap_or(&import.local_name);
        let defs = self.top_level_in(&files, name, |_| true);
        Some(if defs.is_empty() {
            Bound::Module(files)
        } else {
            Bound::Defs(defs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_and_stems() {
        assert_eq!(suffixes("a/b/c").collect::<Vec<_>>(), ["a/b/c", "b/c", "c"]);
        assert_eq!(stem("src/shop/models.py"), "src/shop/models");
        assert_eq!(stem("src/.hidden"), "src/.hidden");
        assert_eq!(stem("go/store"), "go/store");
        assert_eq!(join_relative("src/a", "../b/./c"), "src/b/c");
    }

    #[test]
    fn closest_prefers_a_source_root_then_the_importers_tree() {
        let paths = vec![
            "tests/shop/models.py".to_string(),
            "src/shop/models.py".to_string(),
        ];
        assert_eq!(
            closest("src/shop/cli.py", "shop/models", paths),
            ["src/shop/models.py"]
        );
        let flask = vec![
            "tests/apps/inner/flask.py".to_string(),
            "src/flask/__init__.py".to_string(),
        ];
        assert_eq!(
            closest("tests/test_json.py", "flask", flask),
            ["src/flask/__init__.py"]
        );
    }
}
