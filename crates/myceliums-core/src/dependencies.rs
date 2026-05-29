use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet, VecDeque};

/// File-level dependency entry.
#[derive(Debug, Clone)]
pub struct FileDependency {
    pub file_path: String,
    pub direct_deps: Vec<String>,
    pub transitive_deps: Vec<String>,
    pub dependents: Vec<String>,
}

/// Module-level coupling metrics (Robert Martin's Ca/Ce/I).
#[derive(Debug, Clone)]
pub struct ModuleCoupling {
    pub module_path: String,
    /// Afferent coupling: number of other modules that depend on this one
    pub afferent: u32,
    /// Efferent coupling: number of other modules this one depends on
    pub efferent: u32,
    /// Instability: Ce / (Ca + Ce), range [0,1]. 1 = maximally unstable
    pub instability: f64,
}

/// Compute file-level dependencies from IMPORTS relationships.
///
/// For a given file, returns direct imports, transitive closure, and reverse dependents.
pub fn compute_file_dependencies(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    file_path: &str,
    max_depth: Option<u32>,
) -> FileDependency {
    // Build file-level import graph: file_a -> file_b if any symbol in file_a imports from file_b
    let uid_to_file: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.file_path.as_str()))
        .collect();

    let mut file_deps: HashMap<&str, HashSet<&str>> = HashMap::new(); // file -> files it imports
    let mut file_dependents: HashMap<&str, HashSet<&str>> = HashMap::new(); // file -> files that import it

    for rel in relationships {
        if rel.kind == RelationshipKind::Imports {
            let src_file = uid_to_file.get(rel.source_uid.as_str());
            let tgt_file = uid_to_file.get(rel.target_uid.as_str());
            if let (Some(&src), Some(&tgt)) = (src_file, tgt_file) {
                if src != tgt {
                    file_deps.entry(src).or_default().insert(tgt);
                    file_dependents.entry(tgt).or_default().insert(src);
                }
            }
        }
    }

    // Direct deps
    let direct_deps: Vec<String> = file_deps
        .get(file_path)
        .map(|s| s.iter().map(|p| p.to_string()).collect())
        .unwrap_or_default();

    // Transitive deps via BFS
    let depth_limit = max_depth.unwrap_or(u32::MAX);
    let mut transitive: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(&str, u32)> = VecDeque::new();
    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(file_path);

    if let Some(deps) = file_deps.get(file_path) {
        for dep in deps {
            queue.push_back((dep, 1));
        }
    }

    while let Some((path, dist)) = queue.pop_front() {
        if visited.contains(path) || dist > depth_limit {
            continue;
        }
        visited.insert(path);
        transitive.insert(path.to_string());

        if dist < depth_limit {
            if let Some(deps) = file_deps.get(path) {
                for dep in deps {
                    if !visited.contains(dep) {
                        queue.push_back((dep, dist + 1));
                    }
                }
            }
        }
    }

    // Dependents (reverse)
    let dependents: Vec<String> = file_dependents
        .get(file_path)
        .map(|s| s.iter().map(|p| p.to_string()).collect())
        .unwrap_or_default();

    FileDependency {
        file_path: file_path.to_string(),
        direct_deps,
        transitive_deps: transitive.into_iter().collect(),
        dependents,
    }
}

/// Compute module-level coupling metrics for all files.
pub fn compute_module_coupling(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    group_by_directory: bool,
) -> Vec<ModuleCoupling> {
    let uid_to_file: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.file_path.as_str()))
        .collect();

    // Build file-level import graph
    let mut outgoing: HashMap<String, HashSet<String>> = HashMap::new();
    let mut incoming: HashMap<String, HashSet<String>> = HashMap::new();

    for rel in relationships {
        if rel.kind == RelationshipKind::Imports {
            let src_file = uid_to_file.get(rel.source_uid.as_str());
            let tgt_file = uid_to_file.get(rel.target_uid.as_str());
            if let (Some(&src), Some(&tgt)) = (src_file, tgt_file) {
                let src_mod = if group_by_directory {
                    directory_of(src)
                } else {
                    src.to_string()
                };
                let tgt_mod = if group_by_directory {
                    directory_of(tgt)
                } else {
                    tgt.to_string()
                };
                if src_mod != tgt_mod {
                    outgoing
                        .entry(src_mod.clone())
                        .or_default()
                        .insert(tgt_mod.clone());
                    incoming.entry(tgt_mod).or_default().insert(src_mod);
                }
            }
        }
    }

    // Collect all module paths
    let mut all_modules: HashSet<String> = HashSet::new();
    for key in outgoing.keys() {
        all_modules.insert(key.clone());
    }
    for key in incoming.keys() {
        all_modules.insert(key.clone());
    }

    let mut result: Vec<ModuleCoupling> = all_modules
        .into_iter()
        .map(|module_path| {
            let afferent = incoming
                .get(&module_path)
                .map(|s| s.len() as u32)
                .unwrap_or(0);
            let efferent = outgoing
                .get(&module_path)
                .map(|s| s.len() as u32)
                .unwrap_or(0);
            let total = afferent + efferent;
            let instability = if total > 0 {
                efferent as f64 / total as f64
            } else {
                0.0
            };
            ModuleCoupling {
                module_path,
                afferent,
                efferent,
                instability,
            }
        })
        .collect();

    result.sort_by(|a, b| {
        b.instability
            .partial_cmp(&a.instability)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

fn directory_of(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        path[..pos].to_string()
    } else {
        ".".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn make_symbol(uid: &str, name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_import(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Imports,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_direct_dependencies() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/a.rs"),
            make_symbol("b", "func_b", "src/b.rs"),
            make_symbol("c", "func_c", "src/c.rs"),
        ];
        // a imports b, b imports c
        let rels = vec![make_import("a", "b"), make_import("b", "c")];

        let dep = compute_file_dependencies(&symbols, &rels, "src/a.rs", None);
        assert_eq!(dep.direct_deps, vec!["src/b.rs"]);
        assert!(dep.transitive_deps.contains(&"src/b.rs".to_string()));
        assert!(dep.transitive_deps.contains(&"src/c.rs".to_string()));
    }

    #[test]
    fn test_dependents() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/a.rs"),
            make_symbol("b", "func_b", "src/b.rs"),
        ];
        let rels = vec![make_import("a", "b")];

        let dep = compute_file_dependencies(&symbols, &rels, "src/b.rs", None);
        assert!(dep.dependents.contains(&"src/a.rs".to_string()));
    }

    #[test]
    fn test_module_coupling() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/auth/login.rs"),
            make_symbol("b", "func_b", "src/db/query.rs"),
            make_symbol("c", "func_c", "src/api/handler.rs"),
        ];
        // auth imports db, api imports auth
        let rels = vec![make_import("a", "b"), make_import("c", "a")];

        let coupling = compute_module_coupling(&symbols, &rels, true);
        assert!(!coupling.is_empty());

        // src/auth should have Ca=1 (api depends on it), Ce=1 (it depends on db)
        let auth = coupling.iter().find(|c| c.module_path == "src/auth");
        assert!(auth.is_some());
        let auth = auth.unwrap();
        assert_eq!(auth.afferent, 1);
        assert_eq!(auth.efferent, 1);
        assert!((auth.instability - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_transitive_depth_limit() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/a.rs"),
            make_symbol("b", "func_b", "src/b.rs"),
            make_symbol("c", "func_c", "src/c.rs"),
            make_symbol("d", "func_d", "src/d.rs"),
        ];
        // a → b → c → d
        let rels = vec![
            make_import("a", "b"),
            make_import("b", "c"),
            make_import("c", "d"),
        ];

        // max_depth=2: should include b and c but NOT d
        let dep = compute_file_dependencies(&symbols, &rels, "src/a.rs", Some(2));
        assert!(dep.transitive_deps.contains(&"src/b.rs".to_string()));
        assert!(dep.transitive_deps.contains(&"src/c.rs".to_string()));
        assert!(
            !dep.transitive_deps.contains(&"src/d.rs".to_string()),
            "d should be excluded at depth 3"
        );
    }

    #[test]
    fn test_circular_import_handling() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/a.rs"),
            make_symbol("b", "func_b", "src/b.rs"),
        ];
        // Circular: a → b → a
        let rels = vec![make_import("a", "b"), make_import("b", "a")];

        // Should not infinite loop
        let dep = compute_file_dependencies(&symbols, &rels, "src/a.rs", None);
        assert!(dep.transitive_deps.contains(&"src/b.rs".to_string()));
        // Should also find dependents
        assert!(dep.dependents.contains(&"src/b.rs".to_string()));
    }

    #[test]
    fn test_module_coupling_isolated_files() {
        let symbols = vec![
            make_symbol("a", "func_a", "src/a.rs"),
            make_symbol("b", "func_b", "src/b.rs"),
        ];
        // No import relationships
        let rels: Vec<Relationship> = vec![];

        let coupling = compute_module_coupling(&symbols, &rels, false);
        assert!(
            coupling.is_empty(),
            "Isolated files should produce no coupling entries"
        );
    }

    #[test]
    fn test_file_not_found() {
        let symbols = vec![make_symbol("a", "func_a", "src/a.rs")];
        let rels: Vec<Relationship> = vec![];

        let dep = compute_file_dependencies(&symbols, &rels, "src/nonexistent.rs", None);
        assert!(dep.direct_deps.is_empty());
        assert!(dep.transitive_deps.is_empty());
        assert!(dep.dependents.is_empty());
    }

    #[test]
    fn test_module_coupling_directory_grouping() {
        let symbols = vec![
            make_symbol("a1", "func_a1", "src/auth/login.rs"),
            make_symbol("a2", "func_a2", "src/auth/register.rs"),
            make_symbol("b1", "func_b1", "src/db/query.rs"),
        ];
        // Both auth files import from db
        let rels = vec![make_import("a1", "b1"), make_import("a2", "b1")];

        // Group by directory: src/auth should have Ce=1 (depends on src/db)
        let coupling = compute_module_coupling(&symbols, &rels, true);
        let auth = coupling.iter().find(|c| c.module_path == "src/auth");
        assert!(auth.is_some(), "src/auth should appear in coupling");
        let auth = auth.unwrap();
        // Two import edges but same directory pair → Ce=1
        assert_eq!(auth.efferent, 1);
    }
}
