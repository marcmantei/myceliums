use std::collections::HashMap;

use crate::parser::ImportInfo;

/// A lightweight module graph for cross-file import resolution.
///
/// After all files have been parsed, the analyzer builds a `ModuleGraph` that
/// maps module paths to their exported symbols. The call resolver can then
/// look up imported names that would otherwise be unresolved because they
/// are defined in a different file.
pub struct ModuleGraph {
    /// Maps module path (e.g., "src/auth.py", "auth", "auth.service") to module info.
    modules: HashMap<String, ModuleInfo>,
}

/// Information about a single module (file).
pub struct ModuleInfo {
    /// File path of this module (retained for diagnostics and future use).
    #[allow(dead_code)]
    pub file_path: String,
    /// Symbol UIDs exported by this module: name -> uid.
    pub exported_symbols: HashMap<String, String>,
    /// Imports declared by this module.
    pub imports: Vec<ModuleImport>,
}

/// A single import declaration within a module.
pub struct ModuleImport {
    pub source_module: String,
    /// (original_name, local_alias) pairs. `None` alias means the name is used as-is.
    pub imported_names: Vec<(String, Option<String>)>,
    /// Whether this is a wildcard import (e.g., `from module import *`).
    #[allow(dead_code)]
    pub is_wildcard: bool,
}

impl Default for ModuleGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleGraph {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }

    /// Register a file and its exported symbol (name, uid) pairs.
    ///
    /// The file path is used to derive one or more module paths depending on
    /// the language conventions (Python dotted paths, TypeScript relative
    /// imports, Rust crate paths, etc.).
    pub fn register_module(&mut self, file_path: &str, symbols: &[(String, String)]) {
        let exported: HashMap<String, String> = symbols.iter().cloned().collect();

        let module_paths = derive_module_paths(file_path);

        for module_path in module_paths {
            let info = self
                .modules
                .entry(module_path)
                .or_insert_with(|| ModuleInfo {
                    file_path: file_path.to_string(),
                    exported_symbols: HashMap::new(),
                    imports: Vec::new(),
                });
            // Merge symbols (later registrations for the same module path
            // add to, rather than replace, the symbol set).
            for (name, uid) in &exported {
                info.exported_symbols.insert(name.clone(), uid.clone());
            }
        }
    }

    /// Register an import declaration for a file.
    pub fn register_import(&mut self, file_path: &str, import: &ImportInfo) {
        let module_paths = derive_module_paths(file_path);
        let primary_path = module_paths
            .into_iter()
            .next()
            .unwrap_or_else(|| file_path.to_string());

        let info = self
            .modules
            .entry(primary_path)
            .or_insert_with(|| ModuleInfo {
                file_path: file_path.to_string(),
                exported_symbols: HashMap::new(),
                imports: Vec::new(),
            });

        let original_name = import
            .original_name
            .clone()
            .unwrap_or_else(|| import.local_name.clone());
        let alias = if import.local_name != original_name {
            Some(import.local_name.clone())
        } else {
            None
        };

        // Check if we already have an import from this source module
        if let Some(existing) = info
            .imports
            .iter_mut()
            .find(|m| m.source_module == import.source_module)
        {
            existing.imported_names.push((original_name, alias));
        } else {
            info.imports.push(ModuleImport {
                source_module: import.source_module.clone(),
                imported_names: vec![(original_name, alias)],
                is_wildcard: false,
            });
        }
    }

    /// Resolve an imported name used in `importing_file` to a symbol UID by
    /// following the module graph.
    ///
    /// Looks at the imports declared by `importing_file`, finds the source
    /// module that provides `name`, and returns the UID of the matching
    /// exported symbol.
    pub fn resolve_import(&self, importing_file: &str, name: &str) -> Option<&str> {
        // Find the module info for the importing file. Try all derived paths.
        let importing_paths = derive_module_paths(importing_file);
        let importing_info = importing_paths.iter().find_map(|p| self.modules.get(p))?;

        // Walk the file's imports looking for `name` (either as an
        // original name or as a local alias).
        for imp in &importing_info.imports {
            for (orig, alias) in &imp.imported_names {
                let local_name = alias.as_deref().unwrap_or(orig.as_str());
                if local_name == name {
                    // `orig` is the name exported by the source module.
                    // Find the source module and look up the symbol.
                    return self.find_exported_symbol(&imp.source_module, orig);
                }
            }
        }

        None
    }

    /// Look up a symbol by name in a source module. The source module string
    /// may be a dotted path, a relative path, or a bare name, so we try
    /// several normalised forms.
    fn find_exported_symbol(&self, source_module: &str, symbol_name: &str) -> Option<&str> {
        // Direct lookup
        if let Some(info) = self.modules.get(source_module) {
            if let Some(uid) = info.exported_symbols.get(symbol_name) {
                return Some(uid.as_str());
            }
        }

        // Try normalised forms: strip leading "./" and replace "." with "/"
        let normalised = source_module
            .strip_prefix("./")
            .unwrap_or(source_module)
            .replace('.', "/");

        if let Some(info) = self.modules.get(&normalised) {
            if let Some(uid) = info.exported_symbols.get(symbol_name) {
                return Some(uid.as_str());
            }
        }

        // Try dotted form (Python-style)
        let dotted = normalised.replace('/', ".");
        if let Some(info) = self.modules.get(&dotted) {
            if let Some(uid) = info.exported_symbols.get(symbol_name) {
                return Some(uid.as_str());
            }
        }

        None
    }
}

/// Derive a set of module path aliases from a file path.
///
/// Different languages use different conventions:
/// - Python: `src/auth/service.py` -> `auth.service`, `auth/service.py`, `auth/service`
/// - TypeScript: `src/auth/service.ts` -> `./auth/service`, `auth/service`, `auth/service.ts`
/// - Rust: `src/auth/mod.rs` -> `auth`, `crate::auth`
fn derive_module_paths(file_path: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // Always include the raw file path
    paths.push(file_path.to_string());

    // Strip leading "src/" if present for deriving module-style paths
    let stripped = file_path.strip_prefix("src/").unwrap_or(file_path);

    // Determine the extension
    let ext = file_path.rsplit('.').next().unwrap_or("");

    match ext {
        "py" => {
            // Python module paths
            let without_ext = stripped.trim_end_matches(".py");
            let without_init = without_ext.strip_suffix("/__init__").unwrap_or(without_ext);

            // Dotted path: auth/service -> auth.service
            let dotted = without_init.replace('/', ".");
            paths.push(dotted);

            // Slash-based paths
            paths.push(without_init.to_string());
            if stripped != without_init {
                paths.push(stripped.to_string());
            }
        }
        "ts" | "tsx" | "js" | "jsx" => {
            // TypeScript / JavaScript module paths
            let without_ext = stripped
                .trim_end_matches(".ts")
                .trim_end_matches(".tsx")
                .trim_end_matches(".js")
                .trim_end_matches(".jsx");

            let without_index = without_ext.strip_suffix("/index").unwrap_or(without_ext);

            paths.push(format!("./{}", without_index));
            paths.push(without_index.to_string());
            paths.push(stripped.to_string());
        }
        "rs" => {
            // Rust module paths
            let without_ext = stripped.trim_end_matches(".rs");
            let without_mod = without_ext.strip_suffix("/mod").unwrap_or(without_ext);

            paths.push(without_mod.to_string());
            paths.push(format!("crate::{}", without_mod.replace('/', "::")));
        }
        _ => {
            // Generic: just add without extension
            if let Some(pos) = stripped.rfind('.') {
                paths.push(stripped[..pos].to_string());
            }
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ImportInfo;

    #[test]
    fn test_cross_file_import_resolution() {
        let mut graph = ModuleGraph::new();

        // Module A exports "authenticate"
        graph.register_module(
            "src/auth/service.py",
            &[("authenticate".to_string(), "uid-auth-1".to_string())],
        );

        // Module B imports "authenticate" from "auth.service"
        graph.register_import(
            "src/main.py",
            &ImportInfo {
                local_name: "authenticate".to_string(),
                source_module: "auth.service".to_string(),
                original_name: None,
            },
        );

        let resolved = graph.resolve_import("src/main.py", "authenticate");
        assert_eq!(resolved, Some("uid-auth-1"));
    }

    #[test]
    fn test_python_from_import_resolves() {
        let mut graph = ModuleGraph::new();

        graph.register_module(
            "src/utils/helpers.py",
            &[
                ("format_date".to_string(), "uid-fmt-1".to_string()),
                ("parse_int".to_string(), "uid-parse-1".to_string()),
            ],
        );

        // from utils.helpers import format_date as fmt
        graph.register_import(
            "src/app.py",
            &ImportInfo {
                local_name: "fmt".to_string(),
                source_module: "utils.helpers".to_string(),
                original_name: Some("format_date".to_string()),
            },
        );

        // Resolve the alias
        let resolved = graph.resolve_import("src/app.py", "fmt");
        assert_eq!(resolved, Some("uid-fmt-1"));

        // Original name should not resolve (it's aliased in this file)
        let resolved_orig = graph.resolve_import("src/app.py", "format_date");
        assert_eq!(resolved_orig, None);
    }

    #[test]
    fn test_unknown_import_returns_none() {
        let mut graph = ModuleGraph::new();

        graph.register_module(
            "src/auth.py",
            &[("login".to_string(), "uid-login-1".to_string())],
        );

        // No imports registered for consumer.py
        let resolved = graph.resolve_import("src/consumer.py", "login");
        assert_eq!(resolved, None);

        // Import registered but for a non-existent symbol
        graph.register_import(
            "src/consumer.py",
            &ImportInfo {
                local_name: "nonexistent".to_string(),
                source_module: "auth".to_string(),
                original_name: None,
            },
        );

        let resolved = graph.resolve_import("src/consumer.py", "nonexistent");
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_typescript_import_resolution() {
        let mut graph = ModuleGraph::new();

        graph.register_module(
            "src/auth/service.ts",
            &[("AuthService".to_string(), "uid-as-1".to_string())],
        );

        // import { AuthService } from './auth/service'
        graph.register_import(
            "src/main.ts",
            &ImportInfo {
                local_name: "AuthService".to_string(),
                source_module: "./auth/service".to_string(),
                original_name: None,
            },
        );

        let resolved = graph.resolve_import("src/main.ts", "AuthService");
        assert_eq!(resolved, Some("uid-as-1"));
    }

    #[test]
    fn test_derive_module_paths_python() {
        let paths = derive_module_paths("src/auth/service.py");
        assert!(paths.contains(&"auth.service".to_string()));
        assert!(paths.contains(&"auth/service".to_string()));
        assert!(paths.contains(&"src/auth/service.py".to_string()));
    }

    #[test]
    fn test_derive_module_paths_typescript() {
        let paths = derive_module_paths("src/auth/service.ts");
        assert!(paths.contains(&"./auth/service".to_string()));
        assert!(paths.contains(&"auth/service".to_string()));
        assert!(paths.contains(&"auth/service.ts".to_string()));
    }

    #[test]
    fn test_derive_module_paths_rust() {
        let paths = derive_module_paths("src/auth/mod.rs");
        assert!(paths.contains(&"auth".to_string()));
        assert!(paths.contains(&"crate::auth".to_string()));
    }
}
