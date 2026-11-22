//! What a JavaScript/TypeScript project declares about its own module names
//! (#169): the packages of its workspaces and the path aliases of its
//! tsconfig/jsconfig files. Without them a bare specifier (`@vue/shared`,
//! `~utils/strings`) is a library, and every call into another package of
//! the same repository is lost.
//!
//! | source | what it declares |
//! |---|---|
//! | `package.json` `workspaces` (array, or `{ "packages": [...] }`) | globs of package directories |
//! | `pnpm-workspace.yaml` `packages` | the same, `!` excludes |
//! | `lerna.json` `packages` (default `packages/*`) | the same |
//! | each member's `package.json` | its `name`, and its entry: `source`, `exports`, `module`, `main`, `types` |
//! | `tsconfig*.json`, `jsconfig*.json` | `compilerOptions.paths` and `baseUrl`, also through `extends` |
//!
//! A package's entry is looked for among the indexed files: a declared entry
//! under a build directory (`dist/index.js`) stands for the same path under
//! `src/` (`src/index.ts`), since build output is not indexed; then
//! `src/index.*`, then the declared entries as they are, then the package's
//! own `index.*`. A subpath goes through `exports` the way Node matches it:
//! an exact key first, then the pattern with the longest prefix.
//!
//! A config file's aliases apply to the files under its directory: the
//! nearest directory with a `tsconfig*.json`/`jsconfig*.json` decides, with
//! the `paths`/`baseUrl` its files declare or inherit through `extends` — a
//! nearer config without them gives no aliases, it does not fall back to an
//! ancestor's.
//!
//! Only names the repository declares are resolved here; any other bare
//! specifier stays a library. The files are read once per analysis; a
//! missing or unreadable file, a symlink, and a path that leaves the
//! repository are skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::Value;
use walkdir::WalkDir;

use super::parent_dir;
use crate::index_scope::IndexScope;

/// Config files larger than this are not read (a generated package.json can
/// be large, a config file is not).
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
/// How many `extends` links are followed.
const MAX_EXTENDS: usize = 8;
/// Directories a package's build writes to: a declared entry inside one is
/// looked for under `src/` instead.
const BUILD_DIRS: &[&str] = &[
    "dist", "lib", "build", "out", "esm", "cjs", "es", "umd", "types", "typings", "dts",
];

/// A workspace package of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Package {
    /// Repository-relative directory (`packages/shared`).
    pub(super) dir: String,
    /// Declared entry files, package-relative, most specific first:
    /// `source`, `exports["."]` (conditions in [`CONDITIONS`] order),
    /// `module`, `main`, `browser`, `types`, `typings`.
    pub(super) entries: Vec<String>,
    /// `exports` subpaths other than `.`: `("./memory", ["./src/memory.ts"])`;
    /// a key may hold one `*`. Matched by [`Package::subpath_targets`].
    pub(super) subpaths: Vec<(String, Vec<String>)>,
}

/// The aliases one config file gives the files under its directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PathAliases {
    /// `baseUrl`, repository-relative: bare specifiers are also tried as
    /// paths under it.
    pub(super) base_url: Option<String>,
    /// The directory `paths` targets are relative to: `baseUrl`, else the
    /// directory of the config file that declares `paths`.
    pub(super) paths_base: String,
    /// `paths`: pattern (at most one `*`) → targets in file order; the
    /// patterns are sorted by key (only ties between equally long prefixes
    /// depend on it).
    pub(super) paths: Vec<(String, Vec<String>)>,
}

impl PathAliases {
    fn is_empty(&self) -> bool {
        self.base_url.is_none() && self.paths.is_empty()
    }

    /// The `paths` targets for `specifier`, as repository-relative paths, in
    /// the order to try them. An exact pattern wins over a wildcard; of the
    /// wildcards, the one with the longest prefix (TypeScript's rule).
    pub(super) fn targets(&self, specifier: &str) -> Vec<String> {
        let mut best: Option<(usize, &str, &[String])> = None;
        for (pattern, targets) in &self.paths {
            let captured = match pattern.split_once('*') {
                None if pattern == specifier => {
                    best = Some((usize::MAX, "", targets));
                    break;
                }
                None => continue,
                Some((prefix, suffix)) => {
                    if specifier.len() < prefix.len() + suffix.len()
                        || !specifier.starts_with(prefix)
                        || !specifier.ends_with(suffix)
                    {
                        continue;
                    }
                    (
                        prefix.len(),
                        &specifier[prefix.len()..specifier.len() - suffix.len()],
                    )
                }
            };
            if best.is_none_or(|(len, _, _)| captured.0 > len) {
                best = Some((captured.0, captured.1, targets));
            }
        }
        let Some((_, captured, targets)) = best else {
            return Vec::new();
        };
        targets
            .iter()
            .filter_map(|target| join(&self.paths_base, &target.replacen('*', captured, 1)))
            .collect()
    }
}

impl Package {
    /// The `exports` targets of a subpath (`sub/x`), in the order Node
    /// matches keys: an exact key (`./sub/x`), else the pattern key with the
    /// longest prefix before its `*` (then the longest key), its `*`
    /// replaced by what it matched.
    pub(super) fn subpath_targets(&self, subpath: &str) -> Vec<String> {
        let wanted = format!("./{subpath}");
        if let Some((_, targets)) = self.subpaths.iter().find(|(key, _)| *key == wanted) {
            return targets.clone();
        }
        let best = self
            .subpaths
            .iter()
            .filter_map(|(key, targets)| {
                let (prefix, suffix) = key.split_once('*')?;
                let captured = wanted
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))?;
                Some(((prefix.len(), key.len()), captured, targets))
            })
            .max_by_key(|(rank, _, _)| *rank);
        best.map(|(_, captured, targets)| {
            targets
                .iter()
                .map(|t| t.replacen('*', captured, 1))
                .collect()
        })
        .unwrap_or_default()
    }
}

/// The module names a JS/TS repository declares for its own code.
#[derive(Debug, Clone, Default)]
pub struct JsProject {
    /// Workspace packages by `name`; several when two members share one.
    pub(super) packages: BTreeMap<String, Vec<Package>>,
    /// Directory with a config file → the aliases it gives (empty: none).
    pub(super) aliases: BTreeMap<String, PathAliases>,
    /// Config files read only because an `extends` names them (their file
    /// names are not config names: `base.json`), repository-relative.
    extra_configs: Vec<String>,
}

impl JsProject {
    /// Reads the configuration files under `root`. The walk skips what the
    /// analyzer's file walk skips (hidden directories, `node_modules`,
    /// `dist`, ...), but not the project's include/exclude globs: a
    /// tsconfig at the root serves every package.
    pub fn load(root: &Path) -> Self {
        let mut files = Vec::new();
        for entry in WalkDir::new(root)
            .sort_by_file_name()
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // The root itself may have any name (a temporary `.tmpXXXX`).
                e.depth() == 0 || !IndexScope::skips_name(&e.file_name().to_string_lossy())
            })
            .filter_map(Result::ok)
        {
            // A symlink may point out of the repository: not followed.
            if entry.path_is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if !is_config_file(&name) {
                continue;
            }
            if entry
                .metadata()
                .map_or(true, |m| m.len() > MAX_CONFIG_BYTES)
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            files.push((rel, text));
        }
        // A base config `extends` names may have any file name
        // (`@repo/typescript-config/base.json`): read it on demand, but
        // never through a symlink or outside the repository (`join` already
        // refuses a path that climbs above it).
        Self::from_files_with(files, |rel| {
            let path = root.join(rel);
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
                return None;
            }
            if Path::new(rel).components().any(|c| {
                matches!(c, std::path::Component::Normal(name)
                    if IndexScope::skips_name(&name.to_string_lossy()))
            }) {
                return None;
            }
            let canonical = path.canonicalize().ok()?;
            let root = root.canonicalize().ok()?;
            if !canonical.starts_with(&root) {
                return None;
            }
            std::fs::read_to_string(canonical).ok()
        })
    }

    /// The project described by its configuration files, given as
    /// (repository-relative path, contents). Files of other names are
    /// ignored.
    pub fn from_files(files: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::from_files_with(files, |_| None)
    }

    /// [`from_files`](Self::from_files), reading a config file `extends`
    /// names but `files` lacks through `read` (repository-relative path).
    fn from_files_with(
        files: impl IntoIterator<Item = (String, String)>,
        read: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut manifests: BTreeMap<String, Value> = BTreeMap::new();
        let mut workspaces: Vec<(String, Vec<String>)> = Vec::new();
        let mut configs: BTreeMap<String, Value> = BTreeMap::new();
        for (path, text) in files {
            let dir = parent_dir(&path).to_string();
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            match name.as_str() {
                "package.json" => {
                    let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let globs = match manifest.get("workspaces") {
                        Some(Value::Array(globs)) => strings(globs),
                        Some(Value::Object(map)) => map
                            .get("packages")
                            .and_then(Value::as_array)
                            .map(|globs| strings(globs))
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    if !globs.is_empty() {
                        workspaces.push((dir.clone(), globs));
                    }
                    manifests.insert(dir, manifest);
                }
                "pnpm-workspace.yaml" | "pnpm-workspace.yml" => {
                    let globs = serde_yaml::from_str::<serde_yaml::Value>(&text)
                        .ok()
                        .and_then(|doc| {
                            doc.get("packages")?.as_sequence().map(|globs| {
                                globs
                                    .iter()
                                    .filter_map(|g| g.as_str().map(str::to_string))
                                    .collect::<Vec<_>>()
                            })
                        })
                        .unwrap_or_default();
                    if !globs.is_empty() {
                        workspaces.push((dir, globs));
                    }
                }
                "lerna.json" => {
                    let Ok(lerna) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let globs = lerna
                        .get("packages")
                        .and_then(Value::as_array)
                        .map(|globs| strings(globs))
                        .unwrap_or_else(|| vec!["packages/*".to_string()]);
                    workspaces.push((dir, globs));
                }
                _ if is_config_file(&name) => {
                    if let Some(config) = parse_jsonc(&text) {
                        configs.insert(path, config);
                    }
                }
                _ => {}
            }
        }

        let mut project = JsProject::default();
        let members: Vec<(&String, Vec<(GlobSet, bool)>)> = workspaces
            .iter()
            .map(|(root, globs)| (root, compile_globs(globs)))
            .collect();
        for (dir, manifest) in &manifests {
            let Some(name) = manifest.get("name").and_then(Value::as_str) else {
                continue;
            };
            let member = members.iter().any(|(root, globs)| {
                let Some(rel) = relative_to(dir, root) else {
                    return false;
                };
                !rel.is_empty()
                    && globs.iter().any(|(g, include)| *include && g.is_match(rel))
                    && !globs
                        .iter()
                        .any(|(g, include)| !*include && g.is_match(rel))
            });
            if member {
                project
                    .packages
                    .entry(name.to_string())
                    .or_default()
                    .push(package(dir, manifest));
            }
        }

        // Bases that `extends` names and the walk did not read.
        let named: Vec<String> = configs.keys().cloned().collect();
        let mut pending = named.clone();
        let mut loaded = 0;
        while let Some(path) = pending.pop() {
            for base in extends_of(&configs[&path]) {
                let candidates = project.extends_candidates(parent_dir(&path), &base);
                if candidates.iter().any(|c| configs.contains_key(c)) {
                    continue;
                }
                for candidate in candidates {
                    if loaded >= MAX_EXTENDS * 8 {
                        break;
                    }
                    if let Some(config) = read(&candidate).as_deref().and_then(parse_jsonc) {
                        loaded += 1;
                        configs.insert(candidate.clone(), config);
                        project.extra_configs.push(candidate.clone());
                        pending.push(candidate);
                        break;
                    }
                }
            }
        }

        // Config files by directory, the preferred one first. A base read
        // only because `extends` names it gives its directory nothing. A
        // directory with a config file that sets no aliases still has an
        // entry: its files do not inherit an ancestor's aliases.
        let mut by_dir: BTreeMap<String, Vec<&String>> = BTreeMap::new();
        for path in &named {
            by_dir
                .entry(parent_dir(path).to_string())
                .or_default()
                .push(path);
        }
        for (dir, mut paths) in by_dir {
            paths.sort_by_key(|p| {
                let name = p.rsplit('/').next().unwrap_or(p);
                (
                    !matches!(name, "tsconfig.json"),
                    !matches!(name, "jsconfig.json"),
                    name.to_string(),
                )
            });
            let chosen = paths
                .into_iter()
                .map(|path| project.effective_aliases(path, &configs))
                .find(|aliases| !aliases.is_empty())
                .unwrap_or_default();
            project.aliases.insert(dir, chosen);
        }
        project.extra_configs.sort();
        project.extra_configs.dedup();
        project
    }

    /// Whether the repository declares nothing this resolver uses.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.aliases.values().all(PathAliases::is_empty)
    }

    /// Number of workspace packages and of directories with path aliases.
    pub fn counts(&self) -> (usize, usize) {
        (
            self.packages.values().map(Vec::len).sum(),
            self.aliases.values().filter(|a| !a.is_empty()).count(),
        )
    }

    /// Config files read only because an `extends` names them, under file
    /// names that are not config names (`packages/tsconfig/base.json`):
    /// the index records them so that editing one marks it stale.
    pub fn extra_config_files(&self) -> &[String] {
        &self.extra_configs
    }

    /// Package names two workspace members share, with their directories:
    /// an import of such a name resolves only where one of them is nearer
    /// to the importer (or the other is an example or fixture).
    pub fn duplicate_packages(&self) -> Vec<(String, Vec<String>)> {
        self.packages
            .iter()
            .filter(|(_, members)| members.len() > 1)
            .map(|(name, members)| {
                (
                    name.clone(),
                    members.iter().map(|m| m.dir.clone()).collect(),
                )
            })
            .collect()
    }

    /// The aliases that apply to `file`: those of the nearest directory
    /// above it with a config file, if that config sets any.
    pub(super) fn aliases_for(&self, file: &str) -> Option<&PathAliases> {
        let mut dir = parent_dir(file);
        loop {
            if let Some(aliases) = self.aliases.get(dir) {
                return (!aliases.is_empty()).then_some(aliases);
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent_dir(dir);
        }
    }

    /// The workspace package a bare specifier names, with the subpath after
    /// the package name (`@scope/pkg/sub/x` → `sub/x`). Of two packages of
    /// one name, the one sharing the most leading directories with
    /// `importer`, then the one outside an example, fixture or test
    /// directory; `None` when that still leaves a tie (the name is
    /// ambiguous).
    pub(super) fn package_for<'a>(
        &'a self,
        importer: &str,
        specifier: &'a str,
    ) -> Option<(&'a Package, &'a str)> {
        let segments = if specifier.starts_with('@') { 2 } else { 1 };
        let mut cut = specifier.len();
        let mut seen = 0;
        for (i, c) in specifier.char_indices() {
            if c == '/' {
                seen += 1;
                if seen == segments {
                    cut = i;
                    break;
                }
            }
        }
        let (name, rest) = specifier.split_at(cut);
        let candidates = self.packages.get(name)?;
        let shared = |p: &Package| {
            parent_dir(importer)
                .split('/')
                .zip(p.dir.split('/'))
                .take_while(|(a, b)| a == b)
                .count()
        };
        let rank = |p: &Package| (shared(p), !is_example_dir(&p.dir));
        let best = candidates.iter().max_by_key(|p| rank(p))?;
        if candidates.iter().filter(|p| rank(p) == rank(best)).count() > 1 {
            return None;
        }
        Some((best, rest.trim_start_matches('/')))
    }

    /// The `paths`/`baseUrl` a config file ends up with after `extends`.
    fn effective_aliases(&self, path: &str, configs: &BTreeMap<String, Value>) -> PathAliases {
        // The chain from the file itself to its farthest base.
        let mut chain: Vec<&String> = Vec::new();
        let mut queue = vec![path.to_string()];
        let mut seen = BTreeSet::new();
        while let Some(current) = queue.pop() {
            if chain.len() > MAX_EXTENDS || !seen.insert(current.clone()) {
                continue;
            }
            let Some((key, config)) = configs.get_key_value(&current) else {
                continue;
            };
            chain.push(key);
            // Later entries of an `extends` array win: visit them first.
            for base in extends_of(config) {
                if let Some(resolved) = self
                    .extends_candidates(parent_dir(key), &base)
                    .into_iter()
                    .find(|c| configs.contains_key(c))
                {
                    queue.push(resolved);
                }
            }
        }
        let mut aliases = PathAliases::default();
        let mut paths_dir: Option<String> = None;
        let mut paths: Option<Vec<(String, Vec<String>)>> = None;
        // The file itself first: the nearest declaration of each option wins.
        for key in &chain {
            let options = configs[*key].get("compilerOptions");
            let dir = parent_dir(key);
            if aliases.base_url.is_none() {
                if let Some(base) = options
                    .and_then(|o| o.get("baseUrl"))
                    .and_then(Value::as_str)
                {
                    aliases.base_url = join(dir, base);
                }
            }
            if paths.is_none() {
                if let Some(Value::Object(map)) = options.and_then(|o| o.get("paths")) {
                    paths = Some(
                        map.iter()
                            .map(|(pattern, targets)| {
                                let targets = match targets {
                                    Value::Array(t) => strings(t),
                                    Value::String(t) => vec![t.clone()],
                                    _ => Vec::new(),
                                };
                                (pattern.clone(), targets)
                            })
                            .collect(),
                    );
                    paths_dir = Some(dir.to_string());
                }
            }
        }
        aliases.paths = paths.unwrap_or_default();
        aliases.paths_base = aliases.base_url.clone().or(paths_dir).unwrap_or_default();
        aliases
    }

    /// The files an `extends` entry may name, in the order to try them: a
    /// relative path (with or without `.json`), or a file of a workspace
    /// package (`@repo/typescript-config/base.json`). A package from
    /// `node_modules` is not followed.
    fn extends_candidates(&self, dir: &str, base: &str) -> Vec<String> {
        if base.starts_with("./") || base.starts_with("../") {
            let Some(path) = join(dir, base) else {
                return Vec::new();
            };
            return vec![path.clone(), format!("{path}.json")];
        }
        let Some((package, rest)) = self.package_for(dir, base) else {
            return Vec::new();
        };
        let Some(path) = join(&package.dir, rest) else {
            return Vec::new();
        };
        if rest.is_empty() {
            vec![format!("{path}/tsconfig.json")]
        } else {
            vec![path.clone(), format!("{path}.json")]
        }
    }
}

/// The `extends` entries of a config file, in file order.
fn extends_of(config: &Value) -> Vec<String> {
    match config.get("extends") {
        Some(Value::String(base)) => vec![base.clone()],
        Some(Value::Array(bases)) => strings(bases),
        _ => Vec::new(),
    }
}

/// Directory names that mark example, fixture and test packages: of two
/// workspace members with one name, one inside such a directory loses.
const EXAMPLE_DIRS: &[&str] = &[
    "example",
    "examples",
    "fixture",
    "fixtures",
    "__fixtures__",
    "test",
    "tests",
    "__tests__",
    "e2e",
    "demo",
    "demos",
    "playground",
    "playgrounds",
    "sandbox",
    "sample",
    "samples",
    "template",
    "templates",
];

/// Whether a package directory lies in an example, fixture or test tree.
fn is_example_dir(dir: &str) -> bool {
    dir.split('/')
        .any(|segment| EXAMPLE_DIRS.contains(&segment))
}

/// Whether a file name is one the resolver reads to resolve bare JS/TS
/// specifiers (#169): `package.json`, `pnpm-workspace.yaml`, `lerna.json`,
/// `tsconfig*.json`, `jsconfig*.json`. Editing one can change the index.
pub(crate) fn is_config_file(name: &str) -> bool {
    matches!(
        name,
        "package.json" | "pnpm-workspace.yaml" | "pnpm-workspace.yml" | "lerna.json"
    ) || ((name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json"))
}

/// The string elements of a JSON array.
fn strings(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// Workspace globs, each with whether it includes (`!` excludes).
fn compile_globs(globs: &[String]) -> Vec<(GlobSet, bool)> {
    globs
        .iter()
        .filter_map(|glob| {
            let (pattern, include) = match glob.strip_prefix('!') {
                Some(rest) => (rest, false),
                None => (glob.as_str(), true),
            };
            let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .ok()?;
            let mut set = GlobSetBuilder::new();
            set.add(glob);
            Some((set.build().ok()?, include))
        })
        .collect()
}

/// `dir` relative to `root`, when it is inside it.
fn relative_to<'a>(dir: &'a str, root: &str) -> Option<&'a str> {
    if root.is_empty() {
        return Some(dir);
    }
    let rest = dir.strip_prefix(root)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix('/')
    }
}

/// A member's entries and exported subpaths.
fn package(dir: &str, manifest: &Value) -> Package {
    let mut entries = Vec::new();
    let push = |entries: &mut Vec<String>, value: Option<&Value>| {
        if let Some(path) = value.and_then(Value::as_str) {
            entries.push(path.to_string());
        }
    };
    push(&mut entries, manifest.get("source"));
    let mut subpaths = Vec::new();
    match manifest.get("exports") {
        Some(Value::String(entry)) => entries.push(entry.clone()),
        Some(Value::Object(map)) if map.keys().any(|k| k.starts_with('.')) => {
            for (key, target) in map {
                let mut leaves = Vec::new();
                export_leaves(target, &mut leaves);
                if key == "." {
                    entries.extend(leaves);
                } else {
                    subpaths.push((key.clone(), leaves));
                }
            }
        }
        // Conditions only (`{ "import": ..., "require": ... }`): the entry.
        Some(conditions @ Value::Object(_)) => export_leaves(conditions, &mut entries),
        _ => {}
    }
    for field in ["module", "main", "browser", "types", "typings"] {
        push(&mut entries, manifest.get(field));
    }
    Package {
        dir: dir.to_string(),
        entries,
        subpaths,
    }
}

/// The order conditions of an `exports` target are tried in. JSON objects
/// are read without their key order, so it is fixed here: the source first,
/// then what a bundler or Node would pick, types last; any other condition
/// after these, by name.
const CONDITIONS: &[&str] = &[
    "source",
    "import",
    "module",
    "default",
    "browser",
    "node",
    "require",
    "development",
    "production",
    "types",
    "typings",
];

/// The file paths an `exports` target names, conditions in [`CONDITIONS`]
/// order, array fallbacks in order.
fn export_leaves(target: &Value, out: &mut Vec<String>) {
    match target {
        Value::String(path) => out.push(path.clone()),
        Value::Array(items) => items.iter().for_each(|item| export_leaves(item, out)),
        Value::Object(conditions) => {
            let rank = |name: &str| {
                CONDITIONS
                    .iter()
                    .position(|c| *c == name)
                    .unwrap_or(CONDITIONS.len())
            };
            let mut ordered: Vec<(&String, &Value)> = conditions.iter().collect();
            ordered.sort_by(|a, b| (rank(a.0), a.0).cmp(&(rank(b.0), b.0)));
            for (_, inner) in ordered {
                export_leaves(inner, out);
            }
        }
        _ => {}
    }
}

/// `dir` joined with a relative path, `.` and `..` folded; a leading `/`
/// counts as relative to `dir` too. `None` when `..` climbs above the
/// repository root: such a path is not the repository's.
pub(super) fn join(dir: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The path a declared entry may have in the indexed source: one under a
/// build directory (`dist/index.js`) stands for `src/index.js`; `None` for
/// any other path.
pub(super) fn source_for_build_path(entry: &str) -> Option<String> {
    let entry = entry.trim_start_matches("./");
    let (first, rest) = entry.split_once('/')?;
    BUILD_DIRS.contains(&first).then(|| format!("src/{rest}"))
}

/// Parses JSON with comments and trailing commas (tsconfig's JSONC).
fn parse_jsonc(text: &str) -> Option<Value> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: a comma followed by only whitespace before `}` or `]`.
    let bytes: Vec<char> = out.chars().collect();
    let mut cleaned = String::with_capacity(out.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            cleaned.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                cleaned.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            cleaned.push(c);
        } else if c == ',' {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                cleaned.push(c);
            }
        } else {
            cleaned.push(c);
        }
        i += 1;
    }
    serde_json::from_str(&cleaned).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> JsProject {
        JsProject::from_files(
            files
                .iter()
                .map(|(path, text)| (path.to_string(), text.to_string())),
        )
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse() {
        let value = parse_jsonc(
            r#"{
              // line comment
              "a": "http://x/*not a comment*/", /* block */
              "b": [1, 2,],
              "c": {"d": "e\"//",},
            }"#,
        )
        .unwrap();
        assert_eq!(value["a"], "http://x/*not a comment*/");
        assert_eq!(value["b"], serde_json::json!([1, 2]));
        assert_eq!(value["c"]["d"], "e\"//");
    }

    #[test]
    fn workspace_members_come_from_every_kind_of_declaration() {
        let p = project(&[
            (
                "package.json",
                r#"{"name": "root", "workspaces": ["packages/*"]}"#,
            ),
            ("packages/a/package.json", r#"{"name": "@x/a"}"#),
            // Nested deeper than the glob reaches: not a member.
            ("packages/a/fixtures/b/package.json", r#"{"name": "b"}"#),
            ("tools/package.json", r#"{"name": "not-a-member"}"#),
            (
                "web/pnpm-workspace.yaml",
                "packages:\n  - 'apps/*'\n  - '!apps/legacy'\n",
            ),
            ("web/apps/site/package.json", r#"{"name": "site"}"#),
            ("web/apps/legacy/package.json", r#"{"name": "legacy"}"#),
            ("lib/lerna.json", r#"{"version": "1.0.0"}"#),
            ("lib/packages/c/package.json", r#"{"name": "c"}"#),
            (
                "yarn2/package.json",
                r#"{"workspaces": {"packages": ["mods/**"]}}"#,
            ),
            ("yarn2/mods/deep/d/package.json", r#"{"name": "d"}"#),
        ]);
        let names: Vec<&str> = p.packages.keys().map(String::as_str).collect();
        assert_eq!(names, ["@x/a", "c", "d", "site"]);
        assert_eq!(p.packages["site"][0].dir, "web/apps/site");
    }

    #[test]
    fn a_specifier_names_a_package_and_a_subpath() {
        let p = project(&[
            ("package.json", r#"{"workspaces": ["packages/*"]}"#),
            ("packages/shared/package.json", r#"{"name": "@vue/shared"}"#),
            ("packages/vue/package.json", r#"{"name": "vue"}"#),
        ]);
        let (pkg, rest) = p.package_for("a/b.ts", "@vue/shared").unwrap();
        assert_eq!((pkg.dir.as_str(), rest), ("packages/shared", ""));
        let (pkg, rest) = p.package_for("a/b.ts", "vue/jsx-runtime").unwrap();
        assert_eq!((pkg.dir.as_str(), rest), ("packages/vue", "jsx-runtime"));
        assert!(p.package_for("a/b.ts", "@vue/other").is_none());
        assert!(p.package_for("a/b.ts", "vuex").is_none());
        assert!(p.package_for("a/b.ts", "@vue").is_none());
    }

    #[test]
    fn entries_put_source_and_exports_first() {
        let p = project(&[
            ("package.json", r#"{"workspaces": ["p/*"]}"#),
            (
                "p/a/package.json",
                r#"{"name": "a", "main": "index.js", "module": "dist/a.mjs",
                    "exports": {".": {"types": "./dist/a.d.ts", "source": "./src/a.ts"},
                                "./sub": "./src/sub.ts", "./*": "./*"}}"#,
            ),
        ]);
        let a = &p.packages["a"][0];
        assert_eq!(
            a.entries,
            ["./src/a.ts", "./dist/a.d.ts", "dist/a.mjs", "index.js"]
        );
        assert_eq!(a.subpaths[0], ("./*".to_string(), vec!["./*".to_string()]));
        assert_eq!(
            a.subpaths[1],
            ("./sub".to_string(), vec!["./src/sub.ts".to_string()])
        );
        assert_eq!(
            source_for_build_path("./dist/a.mjs").as_deref(),
            Some("src/a.mjs")
        );
        assert_eq!(source_for_build_path("index.js"), None);
        assert_eq!(source_for_build_path("src/a.ts"), None);
    }

    #[test]
    fn exports_match_an_exact_key_first_then_the_longest_pattern_prefix() {
        let p = project(&[
            ("package.json", r#"{"workspaces": ["p/*"]}"#),
            (
                "p/a/package.json",
                r#"{"name": "a", "exports": {
                    "./*": "./*",
                    "./features/*": "./src/features/*.ts",
                    "./features/*.css": "./styles/*.css",
                    "./features/special": "./src/special.ts",
                    ".": {"require": "./dist/a.cjs", "import": "./dist/a.mjs", "source": "./src/a.ts"}
                }}"#,
            ),
        ]);
        let a = &p.packages["a"][0];
        // Conditions in a fixed order, whatever the file order.
        assert_eq!(a.entries, ["./src/a.ts", "./dist/a.mjs", "./dist/a.cjs"]);
        assert_eq!(a.subpath_targets("features/special"), ["./src/special.ts"]);
        assert_eq!(a.subpath_targets("features/x"), ["./src/features/x.ts"]);
        // Of two patterns with one prefix, the longer key.
        assert_eq!(a.subpath_targets("features/x.css"), ["./styles/x.css"]);
        assert_eq!(a.subpath_targets("lib/y"), ["./lib/y"]);
        assert!(project(&[]).packages.is_empty());
    }

    #[test]
    fn a_path_that_climbs_above_the_repository_is_none() {
        assert_eq!(join("a/b", "../c").as_deref(), Some("a/c"));
        assert_eq!(join("a", "../../outside"), None);
        assert_eq!(join("", "..").as_deref(), None);
        let aliases = PathAliases {
            base_url: None,
            paths_base: "app".into(),
            paths: vec![("#/*".into(), vec!["../../*".into(), "./src/*".into()])],
        };
        assert_eq!(aliases.targets("#/x"), ["app/src/x"]);
        let p = project(&[("app/tsconfig.json", r#"{"extends": "../../base.json"}"#)]);
        assert!(p.extra_config_files().is_empty());
    }

    #[test]
    fn same_named_packages_prefer_the_nearer_then_the_non_example_else_none() {
        let p = project(&[
            (
                "package.json",
                r#"{"workspaces": ["packages/*", "examples/*", "apps/*/packages/*"]}"#,
            ),
            ("packages/ui/package.json", r#"{"name": "ui"}"#),
            ("examples/ui/package.json", r#"{"name": "ui"}"#),
            ("packages/x/package.json", r#"{"name": "x"}"#),
            ("apps/b/packages/x/package.json", r#"{"name": "x"}"#),
            ("apps/c/packages/x/package.json", r#"{"name": "x"}"#),
        ]);
        let dir = |importer: &str, name: &'static str| {
            p.package_for(importer, name)
                .map(|(pkg, _)| pkg.dir.clone())
        };
        // Equally far: the example loses.
        assert_eq!(dir("src/m.ts", "ui").as_deref(), Some("packages/ui"));
        // Equally far, neither an example: ambiguous.
        assert_eq!(dir("apps/d/m.ts", "x"), None);
        assert_eq!(
            dir("apps/b/src/m.ts", "x").as_deref(),
            Some("apps/b/packages/x")
        );
        assert_eq!(p.duplicate_packages().len(), 2);
    }

    #[test]
    fn a_nearer_config_without_aliases_does_not_inherit_an_ancestors() {
        let p = project(&[
            (
                "tsconfig.json",
                r#"{"compilerOptions": {"paths": {"@/*": ["src/*"]}}}"#,
            ),
            (
                "tools/tsconfig.json",
                r#"{"compilerOptions": {"strict": true}}"#,
            ),
            (
                "apps/web/tsconfig.json",
                r#"{"extends": "../../tsconfig.json"}"#,
            ),
        ]);
        assert!(p.aliases_for("src/a.ts").is_some());
        assert!(p.aliases_for("tools/build.ts").is_none());
        // Extending the ancestor inherits its aliases, relative to it.
        let web = p.aliases_for("apps/web/src/a.ts").unwrap();
        assert_eq!(web.targets("@/x"), ["src/x"]);
    }

    #[test]
    fn load_reads_no_symlink_and_nothing_outside_the_repository() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("evil.json"),
            r#"{"compilerOptions": {"paths": {"@evil/*": ["*"]}}}"#,
        )
        .unwrap();
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        std::fs::write(
            root.join("cfg/base.json"),
            r#"{"compilerOptions": {"paths": {"~/*": ["src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("app/tsconfig.json"),
            r#"{"extends": ["../cfg/base.json", "../cfg/linked.json"]}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("evil.json"),
                root.join("cfg/linked.json"),
            )
            .unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("evil.json"),
                root.join("tsconfig.json"),
            )
            .unwrap();
        }
        let p = JsProject::load(root);
        let app = p.aliases_for("app/x.ts").unwrap();
        assert_eq!(app.targets("~/y"), ["cfg/src/y"]);
        assert!(app.targets("@evil/z").is_empty());
        assert_eq!(p.extra_config_files(), ["cfg/base.json"]);
        assert!(
            p.aliases_for("x.ts").is_none(),
            "the symlinked root config is not read"
        );
    }

    #[test]
    fn paths_follow_extends_and_resolve_against_base_url() {
        let p = project(&[
            (
                "tsconfig.base.json",
                r#"{"compilerOptions": {"baseUrl": ".", "paths": {"~u/*": ["shared/u/*"], "@c": ["shared/c"]}}}"#,
            ),
            (
                "apps/web/tsconfig.json",
                r#"{"extends": "../../tsconfig.base"}"#,
            ),
            // Own paths, no baseUrl anywhere: relative to this file.
            (
                "apps/admin/tsconfig.json",
                r##"{"compilerOptions": {"paths": {"#/*": ["./src/*"]}}}"##,
            ),
        ]);
        let web = p.aliases_for("apps/web/src/main.ts").unwrap();
        assert_eq!(web.targets("~u/strings"), ["shared/u/strings"]);
        assert_eq!(web.targets("@c"), ["shared/c"]);
        assert!(web.targets("@c/x").is_empty());
        assert_eq!(web.base_url.as_deref(), Some(""));
        let admin = p.aliases_for("apps/admin/src/x.ts").unwrap();
        assert_eq!(admin.targets("#/pages/a"), ["apps/admin/src/pages/a"]);
        assert_eq!(admin.base_url, None);
        // `tsconfig.base.json` sits at the root, so the root has aliases of
        // its own.
        assert!(p.aliases_for("scripts/build.ts").is_some());
    }

    #[test]
    fn the_longest_wildcard_prefix_wins_and_an_exact_pattern_beats_both() {
        let aliases = PathAliases {
            base_url: None,
            paths_base: "root".into(),
            paths: vec![
                ("@vue/*".into(), vec!["packages/*/src".into()]),
                ("@vue/compat".into(), vec!["packages/vue-compat/src".into()]),
                ("*".into(), vec!["types/*".into()]),
            ],
        };
        assert_eq!(aliases.targets("@vue/shared"), ["root/packages/shared/src"]);
        assert_eq!(
            aliases.targets("@vue/compat"),
            ["root/packages/vue-compat/src"]
        );
        assert_eq!(aliases.targets("lodash"), ["root/types/lodash"]);
    }

    #[test]
    fn extends_reaches_a_workspace_package_and_never_loops() {
        let files = [
            (
                "package.json",
                r#"{"workspaces": ["packages/*", "apps/*"]}"#,
            ),
            (
                "packages/typescript-config/package.json",
                r#"{"name": "@repo/typescript-config"}"#,
            ),
            (
                "apps/a/tsconfig.json",
                r#"{"extends": "@repo/typescript-config/base.json"}"#,
            ),
            ("apps/b/tsconfig.json", r#"{"extends": "./tsconfig.json"}"#),
        ];
        // A base that is not a config file by name is read when `extends`
        // names it (`load` reads it from disk).
        let p = JsProject::from_files_with(
            files.iter().map(|(p, t)| (p.to_string(), t.to_string())),
            |path| {
                (path == "packages/typescript-config/base.json").then(|| {
                    r#"{"compilerOptions": {"paths": {"@ui/*": ["../ui/src/*"]}}}"#.to_string()
                })
            },
        );
        let a = p.aliases_for("apps/a/src/x.ts").unwrap();
        assert_eq!(a.targets("@ui/button"), ["packages/ui/src/button"]);
        // A file extending itself has no aliases, and the base gives its
        // own directory none.
        assert!(p.aliases_for("apps/b/x.ts").is_none());
        assert!(p.aliases_for("packages/typescript-config/x.ts").is_none());
    }
}
