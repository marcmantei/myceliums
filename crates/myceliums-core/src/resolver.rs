use std::collections::HashMap;

use myceliums_storage::{Relationship, RelationshipKind};
use uuid::Uuid;

use crate::module_graph::ModuleGraph;
use crate::parser::{CallReference, ExpressionStep, ImportInfo};
use crate::string_pool::{StrId, StringPool};

/// Resolves call references to actual symbol UIDs using name-based heuristics.
/// Two-pass: first collects all definitions, then resolves calls.
///
/// ## Heuristic Nature and Limitations
///
/// Call resolution uses a global `name -> uid` map (last-writer-wins), which means
/// same-named symbols from different scopes/files will collide. This is a **heuristic
/// approach** that is fast and covers most cases, but produces false positives on
/// common names:
///
/// **Example false positive:**
/// ```text
/// // File: auth.rs
/// fn parse(input: &str) -> Token { }
///
/// // File: config.rs  
/// fn parse(json: &str) -> Config { }
///
/// // File: main.rs
/// let token = parse(user_input);  // Which parse()?
/// ```
///
/// The resolver cannot distinguish between `auth::parse` and `config::parse` — it picks
/// one arbitrarily. This produces incorrect edges in the call graph, especially for common
/// names like `parse`, `handle`, `validate`, `map`, etc.
///
/// ## Mitigation Strategies
///
/// 1. Expression chains provide context: `obj.method()` is more reliable than `method()`
/// 2. Import aliases and SSA-derived aliases improve accuracy within a file
/// 3. Unique function names avoid collisions (e.g., prefer `validateUserEmail` over `validate`)
/// 4. Cross-language calls are de-ranked (less likely to be real)
///
/// For 100% precise resolution, see future precision track: per-file scoping, type
/// inference, confidence scores, and optional LSP enrichment.
///
/// Internally uses a [`StringPool`] to intern all names and UIDs,
/// replacing per-entry `String` allocations with compact [`StrId`] handles.
pub struct CallResolver {
    /// Shared string interning pool.
    pool: StringPool,
    /// Map from symbol name -> symbol UID
    name_to_uid: HashMap<StrId, StrId>,
    /// Map from qualified name -> symbol UID
    qualified_to_uid: HashMap<StrId, StrId>,
    /// Map from local import name -> original name
    import_aliases: HashMap<StrId, StrId>,
    /// Map from local variable alias -> target symbol name (SSA-derived)
    aliases: HashMap<StrId, StrId>,
    /// Optional module graph for cross-file import resolution.
    module_graph: Option<ModuleGraph>,
}

impl Default for CallResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl CallResolver {
    pub fn new() -> Self {
        Self {
            pool: StringPool::new(),
            name_to_uid: HashMap::new(),
            qualified_to_uid: HashMap::new(),
            import_aliases: HashMap::new(),
            aliases: HashMap::new(),
            module_graph: None,
        }
    }

    /// Attach a module graph for cross-file import resolution.
    pub fn set_module_graph(&mut self, graph: ModuleGraph) {
        self.module_graph = Some(graph);
    }

    /// Pass 1: Register all symbol definitions
    pub fn register_symbol(&mut self, name: &str, qualified_name: &str, uid: &str) {
        let name_id = self.pool.intern(name);
        let qualified_id = self.pool.intern(qualified_name);
        let uid_id = self.pool.intern(uid);
        self.name_to_uid.insert(name_id, uid_id);
        self.qualified_to_uid.insert(qualified_id, uid_id);
    }

    /// Register import aliases for resolution
    pub fn register_imports(&mut self, imports: &[ImportInfo]) {
        for import in imports {
            if let Some(ref original) = import.original_name {
                if import.local_name != *original {
                    let local_id = self.pool.intern(&import.local_name);
                    let original_id = self.pool.intern(original);
                    self.import_aliases.insert(local_id, original_id);
                }
            }
        }
    }

    /// Register a variable alias discovered by SSA analysis.
    ///
    /// When code contains `handler = authenticate`, this maps
    /// `"handler"` → `"authenticate"` so that `handler()` resolves
    /// to the `authenticate` symbol.
    pub fn register_alias(&mut self, local_name: &str, target_name: &str) {
        let local_id = self.pool.intern(local_name);
        let target_id = self.pool.intern(target_name);
        self.aliases.insert(local_id, target_id);
    }

    /// Pass 2: Resolve call references to relationships.
    ///
    /// When a [`CallReference`] carries an expression chain, the resolver
    /// attempts to build a qualified name from the chain's receiver type
    /// (e.g. `Foo.bar`) and looks that up before falling back to the bare
    /// callee name. SSA aliases are also consulted as a final fallback.
    ///
    /// ## Confidence Notes
    ///
    /// This is a **heuristic approach** with varying confidence levels:
    ///
    /// - **High confidence**: Expression chains (`obj.method()`), unique names, same-file calls
    /// - **Low confidence**: Bare-name calls to functions with common names (`parse`, `handle`, `validate`, `map`)
    ///
    /// See the struct-level documentation for details on false positives and mitigation.
    pub fn resolve_calls(&self, calls: &[CallReference], repo_id: &str) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        for call in calls {
            let caller_uid = self.resolve_name(&call.caller_name, call.file.as_deref());

            // Try chain-based qualified resolution first, then import/SSA aliases.
            let callee_uid = self.resolve_with_chain(call).or_else(|| {
                let callee_id = self.pool.intern_readonly(&call.callee_name);
                let resolved_name = callee_id
                    .and_then(|id| self.import_aliases.get(&id))
                    .or_else(|| callee_id.and_then(|id| self.aliases.get(&id)));

                if let Some(&target_id) = resolved_name {
                    let target_str = self.pool.get(target_id);
                    self.resolve_name(target_str, call.file.as_deref())
                } else {
                    self.resolve_name(&call.callee_name, call.file.as_deref())
                }
            });

            if let (Some(source), Some(target)) = (caller_uid, callee_uid) {
                relationships.push(Relationship {
                    uid: Uuid::new_v4().to_string(),
                    source_uid: source.to_string(),
                    target_uid: target.to_string(),
                    kind: RelationshipKind::Calls,
                    repo_id: repo_id.to_string(),
                    metadata: format!("line:{}", call.line),
                });
            }
        }

        // Deduplicate: same source+target only once
        let mut seen = std::collections::HashSet::new();
        relationships.retain(|r| {
            let key = format!("{}:{}", r.source_uid, r.target_uid);
            seen.insert(key)
        });

        relationships
    }

    /// Attempt to resolve a call using its expression chain.
    ///
    /// Extracts the receiver identifier and builds a qualified name like
    /// `Receiver.method` for lookup.
    fn resolve_with_chain(&self, call: &CallReference) -> Option<&str> {
        let chain = call.chain.as_ref()?;

        // Extract the receiver name from the chain (first Ident).
        let receiver = chain.iter().find_map(|step| match step {
            ExpressionStep::Ident(name) => Some(name.as_str()),
            _ => None,
        })?;

        // Resolve alias if present.
        let receiver = self
            .pool
            .intern_readonly(receiver)
            .and_then(|id| self.import_aliases.get(&id))
            .map(|&id| self.pool.get(id))
            .unwrap_or(receiver);

        // Try qualified name: `Receiver.method`
        let qualified = format!("{}.{}", receiver, call.callee_name);
        self.resolve_name(&qualified, call.file.as_deref())
    }

    fn resolve_name(&self, name: &str, file: Option<&str>) -> Option<&str> {
        // Intern readonly — if the string isn't in the pool, it can't match.
        if let Some(name_id) = self.pool.intern_readonly(name) {
            // First try qualified and simple name maps
            if let Some(&uid_id) = self.qualified_to_uid.get(&name_id) {
                return Some(self.pool.get(uid_id));
            }
            if let Some(&uid_id) = self.name_to_uid.get(&name_id) {
                return Some(self.pool.get(uid_id));
            }
        }

        // Fall back to the module graph for cross-file resolution
        if let (Some(graph), Some(file_path)) = (&self.module_graph, file) {
            if let Some(uid) = graph.resolve_import(file_path, name) {
                return Some(uid);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_alias_resolves_call() {
        let mut resolver = CallResolver::new();
        resolver.register_symbol("main", "mod::main", "uid-main");
        resolver.register_symbol("authenticate", "mod::authenticate", "uid-auth");
        resolver.register_alias("handler", "authenticate");

        let calls = vec![CallReference {
            caller_name: "main".to_string(),
            callee_name: "handler".to_string(),
            line: 10,
            chain: None,
            file: None,
        }];

        let rels = resolver.resolve_calls(&calls, "repo-1");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source_uid, "uid-main");
        assert_eq!(rels[0].target_uid, "uid-auth");
    }

    #[test]
    fn import_alias_takes_precedence_over_ssa_alias() {
        let mut resolver = CallResolver::new();
        resolver.register_symbol("main", "mod::main", "uid-main");
        resolver.register_symbol("connect", "db::connect", "uid-connect");
        resolver.register_symbol("other", "mod::other", "uid-other");

        // Both import alias and SSA alias point to different targets
        let import_local_id = resolver.pool.intern("db");
        let import_target_id = resolver.pool.intern("connect");
        resolver
            .import_aliases
            .insert(import_local_id, import_target_id);
        resolver.register_alias("db", "other");

        let calls = vec![CallReference {
            caller_name: "main".to_string(),
            callee_name: "db".to_string(),
            line: 5,
            chain: None,
            file: None,
        }];

        let rels = resolver.resolve_calls(&calls, "repo-1");
        assert_eq!(rels.len(), 1);
        // Import alias wins
        assert_eq!(rels[0].target_uid, "uid-connect");
    }

    #[test]
    fn multiple_aliases_to_same_target() {
        let mut resolver = CallResolver::new();
        resolver.register_symbol("main", "mod::main", "uid-main");
        resolver.register_symbol("validate", "mod::validate", "uid-validate");
        resolver.register_alias("check", "validate");
        resolver.register_alias("verify", "validate");

        let calls = vec![
            CallReference {
                caller_name: "main".to_string(),
                callee_name: "check".to_string(),
                line: 10,
                chain: None,
                file: None,
            },
            CallReference {
                caller_name: "main".to_string(),
                callee_name: "verify".to_string(),
                line: 11,
                chain: None,
                file: None,
            },
        ];

        let rels = resolver.resolve_calls(&calls, "repo-1");
        // Both resolve to validate, but dedup collapses them to one edge
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].target_uid, "uid-validate");
    }

    #[test]
    fn stringpool_stress_test_1000_symbols() {
        let mut resolver = CallResolver::new();

        // Register 1000 symbols
        for i in 0..1000 {
            resolver.register_symbol(
                &format!("func_{i}"),
                &format!("mod::func_{i}"),
                &format!("uid-{i}"),
            );
        }

        // Register aliases for every 10th symbol
        for i in (0..1000).step_by(10) {
            resolver.register_alias(&format!("alias_{i}"), &format!("func_{i}"));
        }

        // Create call references from func_0 to every other symbol
        let calls: Vec<CallReference> = (1..1000)
            .map(|i| CallReference {
                caller_name: "func_0".to_string(),
                callee_name: format!("func_{i}"),
                line: i as u32,
                chain: None,
                file: None,
            })
            .collect();

        let rels = resolver.resolve_calls(&calls, "repo-bench");
        assert_eq!(rels.len(), 999);

        // Verify alias-based calls resolve correctly
        let alias_calls: Vec<CallReference> = (0..1000)
            .step_by(10)
            .map(|i| CallReference {
                caller_name: "func_0".to_string(),
                callee_name: format!("alias_{i}"),
                line: (1000 + i) as u32,
                chain: None,
                file: None,
            })
            .collect();

        let alias_rels = resolver.resolve_calls(&alias_calls, "repo-bench");
        // alias_0 resolves to func_0, so func_0 -> func_0 is a self-call, still valid
        assert_eq!(alias_rels.len(), 100);

        for rel in &alias_rels {
            assert_eq!(rel.source_uid, "uid-0");
            assert!(rel.target_uid.starts_with("uid-"));
        }

        // Verify the pool is actually deduplicating (re-registering shouldn't grow it much)
        let pool_size_before = resolver.pool.len();
        resolver.register_symbol("func_0", "mod::func_0", "uid-0");
        assert_eq!(resolver.pool.len(), pool_size_before);
    }
}
