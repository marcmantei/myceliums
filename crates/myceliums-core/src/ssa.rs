//! Lightweight SSA-based alias resolver for improving call-graph accuracy.
//!
//! Tracks variable assignments like `handler = authenticate` so that
//! subsequent calls through the alias (`handler()`) resolve to the
//! original symbol.

use std::collections::HashMap;

/// A value in SSA form — represents what a name points to at a given program point.
#[derive(Debug, Clone, PartialEq)]
pub enum SsaValue {
    /// UID of a known symbol.
    Symbol(String),
    /// A module-qualified import (`module::name`).
    Import(String),
    /// A deferred alias that will be resolved later.
    Alias(String),
    /// A join-point merging values from multiple control-flow paths.
    Phi(Vec<SsaValue>),
    /// Unresolvable — analysis gave up.
    Opaque,
}

/// A single basic block in the SSA graph.
#[derive(Debug, Clone)]
pub struct SsaBlock {
    /// Variable definitions within this block: name → value.
    pub defs: HashMap<String, SsaValue>,
    /// Indices of predecessor blocks (for phi-node construction).
    pub predecessors: Vec<usize>,
    /// Whether all predecessors are known (block is sealed).
    pub sealed: bool,
}

impl SsaBlock {
    fn new() -> Self {
        Self {
            defs: HashMap::new(),
            predecessors: Vec::new(),
            sealed: false,
        }
    }
}

/// Lightweight SSA resolver that tracks variable-assigned callees.
///
/// Used to resolve patterns like:
/// ```text
/// handler = authenticate
/// handler()              // → resolves to `authenticate`
/// ```
#[derive(Debug)]
pub struct SsaResolver {
    blocks: Vec<SsaBlock>,
    current_block: usize,
}

impl Default for SsaResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SsaResolver {
    /// Create a new resolver with a single entry block.
    pub fn new() -> Self {
        Self {
            blocks: vec![SsaBlock::new()],
            current_block: 0,
        }
    }

    /// Record a variable definition in the current block.
    pub fn define(&mut self, name: &str, value: SsaValue) {
        self.blocks[self.current_block]
            .defs
            .insert(name.to_string(), value);
    }

    /// Look up a name, walking back through predecessor blocks if needed.
    pub fn lookup(&self, name: &str) -> Option<&SsaValue> {
        self.lookup_in_block(name, self.current_block)
    }

    /// Resolve a name to its terminal target, following alias chains.
    ///
    /// Returns the final non-alias `SsaValue`, or `None` if the name is
    /// undefined. Alias chains longer than 64 hops are treated as opaque.
    pub fn resolve(&self, name: &str) -> Option<SsaValue> {
        let mut current = name.to_string();
        for _ in 0..64 {
            match self.lookup(&current) {
                Some(SsaValue::Alias(target)) => {
                    current = target.clone();
                }
                Some(value) => return Some(value.clone()),
                None => return None,
            }
        }
        // Exceeded hop limit — treat as opaque.
        Some(SsaValue::Opaque)
    }

    /// Start a new basic block and return its index.
    pub fn new_block(&mut self) -> usize {
        let idx = self.blocks.len();
        self.blocks.push(SsaBlock::new());
        idx
    }

    /// Switch the current block.
    pub fn set_current_block(&mut self, idx: usize) {
        assert!(idx < self.blocks.len(), "block index out of range");
        self.current_block = idx;
    }

    /// Add a predecessor edge.
    pub fn add_predecessor(&mut self, block: usize, predecessor: usize) {
        self.blocks[block].predecessors.push(predecessor);
    }

    /// Seal a block, indicating all predecessors are known.
    pub fn seal_block(&mut self, idx: usize) {
        self.blocks[idx].sealed = true;
    }

    /// Collect all alias mappings as `(local_name, target_name)` pairs
    /// suitable for feeding into [`crate::resolver::CallResolver::register_alias`].
    pub fn collect_aliases(&self) -> Vec<(String, String)> {
        let mut aliases = Vec::new();
        for block in &self.blocks {
            for (name, value) in &block.defs {
                if let SsaValue::Alias(target) = value {
                    aliases.push((name.clone(), target.clone()));
                }
            }
        }
        aliases
    }

    // ── private helpers ──────────────────────────────────────────────

    fn lookup_in_block(&self, name: &str, block_idx: usize) -> Option<&SsaValue> {
        let block = &self.blocks[block_idx];

        // Check local definitions first.
        if let Some(val) = block.defs.get(name) {
            return Some(val);
        }

        // If only one predecessor, recurse into it (trivial phi elimination).
        if block.predecessors.len() == 1 {
            return self.lookup_in_block(name, block.predecessors[0]);
        }

        // Multiple predecessors — no phi construction in this lightweight pass.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_alias() {
        let mut ssa = SsaResolver::new();
        ssa.define("handler", SsaValue::Alias("authenticate".into()));
        ssa.define("authenticate", SsaValue::Symbol("uid-auth".into()));

        // Direct lookup returns the alias.
        assert_eq!(
            ssa.lookup("handler"),
            Some(&SsaValue::Alias("authenticate".into()))
        );
        // Resolve follows the chain.
        assert_eq!(
            ssa.resolve("handler"),
            Some(SsaValue::Symbol("uid-auth".into()))
        );
    }

    #[test]
    fn import_alias() {
        let mut ssa = SsaResolver::new();
        ssa.define("db", SsaValue::Import("database::connect".into()));

        assert_eq!(
            ssa.resolve("db"),
            Some(SsaValue::Import("database::connect".into()))
        );
    }

    #[test]
    fn multiple_aliases_same_target() {
        let mut ssa = SsaResolver::new();
        ssa.define("validate", SsaValue::Symbol("uid-validate".into()));
        ssa.define("check", SsaValue::Alias("validate".into()));
        ssa.define("verify", SsaValue::Alias("validate".into()));

        assert_eq!(
            ssa.resolve("check"),
            Some(SsaValue::Symbol("uid-validate".into()))
        );
        assert_eq!(
            ssa.resolve("verify"),
            Some(SsaValue::Symbol("uid-validate".into()))
        );
    }

    #[test]
    fn chained_aliases() {
        let mut ssa = SsaResolver::new();
        ssa.define("root", SsaValue::Symbol("uid-root".into()));
        ssa.define("a", SsaValue::Alias("root".into()));
        ssa.define("b", SsaValue::Alias("a".into()));

        assert_eq!(ssa.resolve("b"), Some(SsaValue::Symbol("uid-root".into())));
    }

    #[test]
    fn unknown_name_returns_none() {
        let ssa = SsaResolver::new();
        assert_eq!(ssa.resolve("nonexistent"), None);
    }

    #[test]
    fn predecessor_block_lookup() {
        let mut ssa = SsaResolver::new();
        // Define in the entry block.
        ssa.define("handler", SsaValue::Symbol("uid-handler".into()));

        // Create a successor block that inherits from entry.
        let b1 = ssa.new_block();
        ssa.add_predecessor(b1, 0);
        ssa.seal_block(b1);
        ssa.set_current_block(b1);

        // Should still find the definition from the predecessor.
        assert_eq!(
            ssa.resolve("handler"),
            Some(SsaValue::Symbol("uid-handler".into()))
        );
    }

    #[test]
    fn collect_aliases_returns_all() {
        let mut ssa = SsaResolver::new();
        ssa.define("a", SsaValue::Alias("target_a".into()));
        ssa.define("b", SsaValue::Alias("target_b".into()));
        ssa.define("c", SsaValue::Symbol("uid-c".into()));

        let aliases = ssa.collect_aliases();
        assert_eq!(aliases.len(), 2);
        assert!(aliases.contains(&("a".into(), "target_a".into())));
        assert!(aliases.contains(&("b".into(), "target_b".into())));
    }
}
