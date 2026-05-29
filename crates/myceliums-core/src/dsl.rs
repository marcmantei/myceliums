//! Declarative DSL for language-specific parsing rules.
//!
//! Instead of hand-coded per-language extractors, this module provides a
//! **data-driven** approach: each language is described by composable
//! [`ScopeRule`], [`ReferenceRule`], and [`ImportRule`] structs. The DSL engine
//! walks the tree-sitter CST once, matching rules in priority order.
//!
//! # Design
//!
//! Inspired by Orbit's declarative parsing architecture. All adaptations are
//! semantic/conceptual — no code was copied.

use crate::parser::{CallReference, ImportInfo, ParsedSymbol};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

// ── Rule types ───────────────────────────────────────────────────────

/// A complete set of parsing rules for one language.
#[derive(Debug, Clone)]
pub struct LanguageRules {
    /// Which language this ruleset applies to.
    pub language: &'static str,
    /// Rules that define symbols (functions, classes, etc.).
    pub scope_rules: Vec<ScopeRule>,
    /// Rules that define call references.
    pub reference_rules: Vec<ReferenceRule>,
    /// Rules that define import statements.
    pub import_rules: Vec<ImportRule>,
    /// Fully-qualified name separator ("::" for Rust, "." for Python/Go).
    pub fqn_separator: &'static str,
}

/// Predicate for conditional rule matching.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// Parent node must have one of these kinds.
    ParentKindIn(Vec<&'static str>),
    /// Parent node must NOT have one of these kinds.
    ParentKindNotIn(Vec<&'static str>),
    /// The node must have a specific field present.
    HasField(&'static str),
}

/// A rule that identifies a symbol-defining AST node.
#[derive(Debug, Clone)]
pub struct ScopeRule {
    /// Tree-sitter node kinds that trigger this rule.
    pub node_kinds: Vec<&'static str>,
    /// What kind of symbol this produces.
    pub symbol_kind: SymbolKind,
    /// Field name on the node that holds the symbol name.
    pub name_field: &'static str,
    /// Whether this node creates a nested scope (e.g. class body).
    pub creates_scope: bool,
    /// Which field name contains the scope body (if `creates_scope` is true).
    pub body_field: Option<&'static str>,
    /// If true, use parent scope as Method instead of the declared kind
    /// when inside a scope.
    pub method_when_scoped: bool,
    /// Optional condition for matching.
    pub condition: Option<Predicate>,
}

/// A rule that identifies a call/reference AST node.
#[derive(Debug, Clone)]
pub struct ReferenceRule {
    /// Tree-sitter node kinds that trigger this rule.
    pub node_kinds: Vec<&'static str>,
    /// Field on the call node that holds the function/callee expression.
    pub function_field: &'static str,
    /// For member access: the field that holds the attribute/method name.
    pub attribute_field: Option<&'static str>,
    /// Node kinds for member access expressions (e.g. "attribute", "selector_expression").
    pub member_access_kinds: Vec<&'static str>,
}

/// A rule that identifies an import statement.
#[derive(Debug, Clone)]
pub struct ImportRule {
    /// Tree-sitter node kinds that trigger this rule.
    pub node_kinds: Vec<&'static str>,
    /// How to extract imports from this node.
    pub strategy: ImportStrategy,
}

/// Strategy for extracting import information from an AST node.
#[derive(Debug, Clone)]
pub enum ImportStrategy {
    /// Python-style: `import foo` / `import foo as bar`
    /// Looks for dotted_name and aliased_import children.
    PythonImport,
    /// Python-style: `from foo import bar, baz as qux`
    /// module_name field + named children.
    PythonFromImport { module_field: &'static str },
    /// Go-style: `import "fmt"` / `import (\n "fmt" \n "os" \n)`
    GoImport,
}

// ── DSL Engine ───────────────────────────────────────────────────────

/// The DSL engine walks a tree-sitter CST once, applying rules to extract
/// symbols, calls, and imports.
pub struct DslEngine<'a> {
    rules: &'a LanguageRules,
    source: &'a [u8],
}

impl<'a> DslEngine<'a> {
    pub fn new(rules: &'a LanguageRules, source: &'a [u8]) -> Self {
        Self { rules, source }
    }

    /// Run the engine on the root node, returning extracted symbols, calls, and imports.
    pub fn extract(&self, root: Node) -> (Vec<ParsedSymbol>, Vec<CallReference>, Vec<ImportInfo>) {
        let mut symbols = Vec::new();
        let mut calls = Vec::new();
        let mut imports = Vec::new();
        self.walk_node(root, None, None, &mut symbols, &mut calls, &mut imports);
        (symbols, calls, imports)
    }

    fn node_text(&self, node: Node) -> &str {
        node.utf8_text(self.source).unwrap_or("")
    }

    fn walk_node(
        &self,
        node: Node,
        parent_scope: Option<&str>,
        current_fn: Option<&str>,
        symbols: &mut Vec<ParsedSymbol>,
        calls: &mut Vec<CallReference>,
        imports: &mut Vec<ImportInfo>,
    ) {
        let kind = node.kind();
        let mut new_scope: Option<String> = None;
        let mut new_fn: Option<String> = None;
        let mut skip_children = false;

        // Check scope rules (symbol definitions)
        for rule in &self.rules.scope_rules {
            if !rule.node_kinds.contains(&kind) {
                continue;
            }

            // Check predicate
            if let Some(ref pred) = rule.condition {
                if !self.check_predicate(pred, node) {
                    continue;
                }
            }

            if let Some(name_node) = node.child_by_field_name(rule.name_field) {
                let name = self.node_text(name_node).to_string();
                let sym_kind = if rule.method_when_scoped && parent_scope.is_some() {
                    SymbolKind::Method
                } else {
                    rule.symbol_kind.clone()
                };

                let signature = self.extract_signature(node);

                symbols.push(ParsedSymbol {
                    name: name.clone(),
                    kind: sym_kind.clone(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    signature,
                    content: self.node_text(node).to_string(),
                    parent_name: parent_scope.map(|s| s.to_string()),
                    metadata: None,
                });

                // Track function scope for call resolution
                if matches!(sym_kind, SymbolKind::Function | SymbolKind::Method) {
                    new_fn = Some(name.clone());
                }

                // If this rule creates a scope, recurse into the body with new parent
                if rule.creates_scope {
                    new_scope = Some(name);
                    if let Some(body_field) = rule.body_field {
                        if let Some(body) = node.child_by_field_name(body_field) {
                            self.walk_node(
                                body,
                                new_scope.as_deref(),
                                new_fn.as_deref().or(current_fn),
                                symbols,
                                calls,
                                imports,
                            );
                            skip_children = true;
                        }
                    }
                }

                break; // First matching rule wins
            }
        }

        // Check reference rules (calls)
        let active_fn = new_fn.as_deref().or(current_fn);
        for rule in &self.rules.reference_rules {
            if !rule.node_kinds.contains(&kind) {
                continue;
            }

            if let Some(func_node) = node.child_by_field_name(rule.function_field) {
                let callee = if rule
                    .member_access_kinds
                    .iter()
                    .any(|k| *k == func_node.kind())
                {
                    // Member access: try to get the attribute name
                    if let Some(attr_field) = rule.attribute_field {
                        func_node
                            .child_by_field_name(attr_field)
                            .map(|n| self.node_text(n).to_string())
                            .unwrap_or_else(|| self.node_text(func_node).to_string())
                    } else {
                        self.node_text(func_node).to_string()
                    }
                } else {
                    self.node_text(func_node).to_string()
                };

                if let Some(caller) = active_fn {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
        }

        // Check import rules
        for rule in &self.rules.import_rules {
            if !rule.node_kinds.contains(&kind) {
                continue;
            }
            self.extract_imports_by_strategy(node, &rule.strategy, imports);
        }

        // Recurse into children (unless we already handled the scope body)
        if !skip_children {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                self.walk_node(
                    child,
                    new_scope.as_deref().or(parent_scope),
                    active_fn,
                    symbols,
                    calls,
                    imports,
                );
            }
        }
    }

    fn check_predicate(&self, pred: &Predicate, node: Node) -> bool {
        match pred {
            Predicate::ParentKindIn(kinds) => node
                .parent()
                .map(|p| kinds.iter().any(|k| *k == p.kind()))
                .unwrap_or(false),
            Predicate::ParentKindNotIn(kinds) => node
                .parent()
                .map(|p| !kinds.iter().any(|k| *k == p.kind()))
                .unwrap_or(true),
            Predicate::HasField(field) => node.child_by_field_name(field).is_some(),
        }
    }

    fn extract_signature(&self, node: Node) -> String {
        let start = node.start_byte();
        let text = &self.source[start..];
        // Find the body delimiter
        if let Some(pos) = text.iter().position(|&b| b == b'{' || b == b':') {
            let sig = std::str::from_utf8(&text[..pos]).unwrap_or("");
            let trimmed = sig.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        let full = self.node_text(node);
        full.lines().next().unwrap_or("").trim().to_string()
    }

    fn extract_imports_by_strategy(
        &self,
        node: Node,
        strategy: &ImportStrategy,
        imports: &mut Vec<ImportInfo>,
    ) {
        match strategy {
            ImportStrategy::PythonImport => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "dotted_name" {
                        let name = self.node_text(child).to_string();
                        let local = name.split('.').next_back().unwrap_or(&name).to_string();
                        imports.push(ImportInfo {
                            local_name: local,
                            source_module: name,
                            original_name: None,
                        });
                    } else if child.kind() == "aliased_import" {
                        let name_node = child.child_by_field_name("name");
                        let alias_node = child.child_by_field_name("alias");
                        if let Some(name) = name_node {
                            let original = self.node_text(name).to_string();
                            let local = alias_node
                                .map(|a| self.node_text(a).to_string())
                                .unwrap_or_else(|| {
                                    original
                                        .split('.')
                                        .next_back()
                                        .unwrap_or(&original)
                                        .to_string()
                                });
                            imports.push(ImportInfo {
                                local_name: local,
                                source_module: original,
                                original_name: None,
                            });
                        }
                    }
                }
            }
            ImportStrategy::PythonFromImport { module_field } => {
                let module_node = node.child_by_field_name(module_field);
                let module = module_node
                    .map(|m| self.node_text(m).to_string())
                    .unwrap_or_default();

                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "dotted_name" if Some(child.id()) != module_node.map(|n| n.id()) => {
                            let name = self.node_text(child).to_string();
                            imports.push(ImportInfo {
                                local_name: name.clone(),
                                source_module: module.clone(),
                                original_name: Some(name),
                            });
                        }
                        "aliased_import" => {
                            let name_node = child.child_by_field_name("name");
                            let alias_node = child.child_by_field_name("alias");
                            if let Some(name) = name_node {
                                let original = self.node_text(name).to_string();
                                let local = alias_node
                                    .map(|a| self.node_text(a).to_string())
                                    .unwrap_or_else(|| original.clone());
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: module.clone(),
                                    original_name: Some(original),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            ImportStrategy::GoImport => {
                // Go imports: `import "pkg"` or `import ( "pkg1" \n "pkg2" )`
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    self.extract_go_import_spec(child, imports);
                }
            }
        }
    }

    fn extract_go_import_spec(&self, node: Node, imports: &mut Vec<ImportInfo>) {
        match node.kind() {
            "import_spec" => {
                if let Some(path_node) = node.child_by_field_name("path") {
                    let raw = self.node_text(path_node);
                    let module = raw.trim_matches('"').to_string();
                    let local = module.split('/').next_back().unwrap_or(&module).to_string();

                    // Check for alias
                    let alias = node.child_by_field_name("name");
                    let final_local = alias
                        .map(|a| self.node_text(a).to_string())
                        .unwrap_or(local);

                    imports.push(ImportInfo {
                        local_name: final_local,
                        source_module: module,
                        original_name: alias.map(|_| {
                            let raw2 = self.node_text(path_node);
                            raw2.trim_matches('"')
                                .split('/')
                                .next_back()
                                .unwrap_or("")
                                .to_string()
                        }),
                    });
                }
            }
            "import_spec_list" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    self.extract_go_import_spec(child, imports);
                }
            }
            _ => {}
        }
    }
}

// ── Built-in Language Rules ──────────────────────────────────────────

/// Python language rules.
pub fn python_rules() -> LanguageRules {
    LanguageRules {
        language: "python",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_definition"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["class_definition"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call"],
            function_field: "function",
            attribute_field: Some("attribute"),
            member_access_kinds: vec!["attribute"],
        }],
        import_rules: vec![
            ImportRule {
                node_kinds: vec!["import_statement"],
                strategy: ImportStrategy::PythonImport,
            },
            ImportRule {
                node_kinds: vec!["import_from_statement"],
                strategy: ImportStrategy::PythonFromImport {
                    module_field: "module_name",
                },
            },
        ],
        fqn_separator: ".",
    }
}

/// Go language rules.
pub fn go_rules() -> LanguageRules {
    LanguageRules {
        language: "go",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_declaration"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method_declaration"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("field"),
            member_access_kinds: vec!["selector_expression"],
        }],
        import_rules: vec![ImportRule {
            node_kinds: vec!["import_declaration"],
            strategy: ImportStrategy::GoImport,
        }],
        fqn_separator: ".",
    }
}

/// TypeScript / JavaScript / TSX rules (shared grammar structure).
pub fn typescript_rules() -> LanguageRules {
    LanguageRules {
        language: "typescript",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_declaration"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method_definition"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["class_declaration"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["interface_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["type_alias_declaration"],
                symbol_kind: SymbolKind::TypeAlias,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_declaration"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("property"),
            member_access_kinds: vec!["member_expression"],
        }],
        import_rules: vec![], // Complex — kept in hand-coded path
        fqn_separator: ".",
    }
}

/// Rust language rules.
pub fn rust_rules() -> LanguageRules {
    LanguageRules {
        language: "rust",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_item"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["struct_item"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_item"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["trait_item"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["impl_item"],
                symbol_kind: SymbolKind::Class,
                name_field: "type",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["mod_item"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["const_item"],
                symbol_kind: SymbolKind::Constant,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["type_item"],
                symbol_kind: SymbolKind::TypeAlias,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("field"),
            member_access_kinds: vec!["field_expression", "scoped_identifier"],
        }],
        import_rules: vec![], // Complex use-tree — kept in hand-coded path
        fqn_separator: "::",
    }
}

/// Java language rules.
pub fn java_rules() -> LanguageRules {
    LanguageRules {
        language: "java",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_declaration"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["interface_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_declaration"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method_declaration", "constructor_declaration"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["method_invocation"],
            function_field: "name",
            attribute_field: None,
            member_access_kinds: vec![],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// C# language rules.
pub fn csharp_rules() -> LanguageRules {
    LanguageRules {
        language: "csharp",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec![
                    "class_declaration",
                    "struct_declaration",
                    "record_declaration",
                ],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["interface_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_declaration"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method_declaration", "constructor_declaration"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["namespace_declaration"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["invocation_expression"],
            function_field: "function",
            attribute_field: Some("name"),
            member_access_kinds: vec!["member_access_expression"],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// C language rules.
pub fn c_rules() -> LanguageRules {
    LanguageRules {
        language: "c",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_definition"],
                symbol_kind: SymbolKind::Function,
                name_field: "declarator",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["struct_specifier"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_specifier"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("field"),
            member_access_kinds: vec!["field_expression"],
        }],
        import_rules: vec![],
        fqn_separator: "::",
    }
}

/// C++ language rules.
pub fn cpp_rules() -> LanguageRules {
    LanguageRules {
        language: "cpp",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_definition"],
                symbol_kind: SymbolKind::Function,
                name_field: "declarator",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["class_specifier", "struct_specifier"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_specifier"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["namespace_definition"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("field"),
            member_access_kinds: vec!["field_expression", "scoped_identifier"],
        }],
        import_rules: vec![],
        fqn_separator: "::",
    }
}

/// Ruby language rules.
pub fn ruby_rules() -> LanguageRules {
    LanguageRules {
        language: "ruby",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["module"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method", "singleton_method"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call", "method_call"],
            function_field: "method",
            attribute_field: None,
            member_access_kinds: vec![],
        }],
        import_rules: vec![],
        fqn_separator: "::",
    }
}

/// Kotlin language rules.
pub fn kotlin_rules() -> LanguageRules {
    LanguageRules {
        language: "kotlin",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_declaration", "object_declaration"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("class_body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["function_declaration"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: None,
            member_access_kinds: vec!["navigation_expression"],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// Swift language rules.
pub fn swift_rules() -> LanguageRules {
    LanguageRules {
        language: "swift",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_declaration"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["protocol_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["function_declaration", "protocol_function_declaration"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["typealias_declaration"],
                symbol_kind: SymbolKind::TypeAlias,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: Some("suffix"),
            member_access_kinds: vec!["navigation_expression"],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// PHP language rules.
pub fn php_rules() -> LanguageRules {
    LanguageRules {
        language: "php",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_declaration"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["interface_declaration", "trait_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_declaration"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["function_definition"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["method_declaration"],
                symbol_kind: SymbolKind::Method,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["namespace_definition"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec![
                "function_call_expression",
                "member_call_expression",
                "scoped_call_expression",
            ],
            function_field: "function",
            attribute_field: Some("name"),
            member_access_kinds: vec!["member_access_expression", "scoped_identifier"],
        }],
        import_rules: vec![],
        fqn_separator: "\\",
    }
}

/// PowerShell language rules.
pub fn powershell_rules() -> LanguageRules {
    LanguageRules {
        language: "powershell",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["function_statement"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["class_statement"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_statement"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["command_expression", "invocation_expression"],
            function_field: "name",
            attribute_field: Some("member"),
            member_access_kinds: vec!["member_access"],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// Scala language rules.
pub fn scala_rules() -> LanguageRules {
    LanguageRules {
        language: "scala",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_definition"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["object_definition"],
                symbol_kind: SymbolKind::Module,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["trait_definition"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["function_definition"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
        ],
        reference_rules: vec![ReferenceRule {
            node_kinds: vec!["call_expression"],
            function_field: "function",
            attribute_field: None,
            member_access_kinds: vec![],
        }],
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// Dart language rules.
pub fn dart_rules() -> LanguageRules {
    LanguageRules {
        language: "dart",
        scope_rules: vec![
            ScopeRule {
                node_kinds: vec!["class_definition"],
                symbol_kind: SymbolKind::Class,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["mixin_declaration"],
                symbol_kind: SymbolKind::Interface,
                name_field: "name",
                creates_scope: true,
                body_field: Some("body"),
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["enum_declaration"],
                symbol_kind: SymbolKind::Enum,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: false,
                condition: None,
            },
            ScopeRule {
                node_kinds: vec!["function_signature", "method_signature"],
                symbol_kind: SymbolKind::Function,
                name_field: "name",
                creates_scope: false,
                body_field: None,
                method_when_scoped: true,
                condition: None,
            },
        ],
        reference_rules: vec![], // Dart call detection is complex (selector chains)
        import_rules: vec![],
        fqn_separator: ".",
    }
}

/// Return DSL rules for a given language, if available.
pub fn rules_for(lang: crate::parser::SourceLanguage) -> Option<LanguageRules> {
    use crate::parser::SourceLanguage;
    match lang {
        SourceLanguage::Python => Some(python_rules()),
        SourceLanguage::Go => Some(go_rules()),
        SourceLanguage::TypeScript | SourceLanguage::Tsx | SourceLanguage::JavaScript => {
            Some(typescript_rules())
        }
        SourceLanguage::Rust => Some(rust_rules()),
        SourceLanguage::Java => Some(java_rules()),
        SourceLanguage::CSharp => Some(csharp_rules()),
        SourceLanguage::C => Some(c_rules()),
        SourceLanguage::Cpp => Some(cpp_rules()),
        SourceLanguage::Ruby => Some(ruby_rules()),
        SourceLanguage::Kotlin => Some(kotlin_rules()),
        SourceLanguage::Swift => Some(swift_rules()),
        SourceLanguage::Php => Some(php_rules()),
        SourceLanguage::PowerShell => Some(powershell_rules()),
        SourceLanguage::Scala => Some(scala_rules()),
        SourceLanguage::Dart => Some(dart_rules()),
        // Languages with complex AST patterns not yet DSL-compatible:
        // Lua, Zig, Elixir, Objective-C (require custom AST walking)
        // Vue, Svelte (use TypeScript rules after script extraction)
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_dsl_functions() {
        let source = r#"
def greet(name):
    print(name)

def farewell():
    greet("world")
"#;
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = python_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (symbols, calls, _imports) = engine.extract(tree.root_node());

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "greet");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[1].name, "farewell");
        assert_eq!(symbols[1].kind, SymbolKind::Function);

        // farewell calls greet
        assert!(calls
            .iter()
            .any(|c| c.caller_name == "farewell" && c.callee_name == "greet"));
    }

    #[test]
    fn test_python_dsl_class_with_methods() {
        let source = r#"
class UserService:
    def get_user(self, id):
        return self.db.find(id)

    def delete_user(self, id):
        self.get_user(id)
"#;
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = python_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (symbols, _calls, _imports) = engine.extract(tree.root_node());

        assert_eq!(symbols.len(), 3); // UserService, get_user, delete_user
        assert_eq!(symbols[0].name, "UserService");
        assert_eq!(symbols[0].kind, SymbolKind::Class);
        assert_eq!(symbols[1].name, "get_user");
        assert_eq!(symbols[1].kind, SymbolKind::Method);
        assert_eq!(symbols[1].parent_name, Some("UserService".to_string()));
        assert_eq!(symbols[2].name, "delete_user");
        assert_eq!(symbols[2].kind, SymbolKind::Method);
    }

    #[test]
    fn test_python_dsl_imports() {
        let source = r#"
import os
import json as j
from pathlib import Path
from typing import Optional, List
from collections import OrderedDict as OD
"#;
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = python_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (_symbols, _calls, imports) = engine.extract(tree.root_node());

        assert!(imports.iter().any(|i| i.local_name == "os"));
        assert!(imports.iter().any(|i| i.local_name == "j"));
        assert!(imports
            .iter()
            .any(|i| i.local_name == "Path" && i.source_module == "pathlib"));
        assert!(imports
            .iter()
            .any(|i| i.local_name == "OD" && i.original_name == Some("OrderedDict".to_string())));
    }

    #[test]
    fn test_go_dsl_functions() {
        let source = r#"
package main

func greet(name string) {
    fmt.Println(name)
}

func main() {
    greet("world")
}
"#;
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_go::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = go_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (symbols, calls, _imports) = engine.extract(tree.root_node());

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "greet");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[1].name, "main");

        assert!(calls
            .iter()
            .any(|c| c.caller_name == "main" && c.callee_name == "greet"));
    }

    #[test]
    fn test_go_dsl_imports() {
        let source = r#"
package main

import (
    "fmt"
    "os"
)
"#;
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_go::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = go_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (_symbols, _calls, imports) = engine.extract(tree.root_node());

        assert!(imports.iter().any(|i| i.local_name == "fmt"));
        assert!(imports.iter().any(|i| i.local_name == "os"));
    }

    /// Verify DSL output matches hand-coded extractor output for Python.
    #[test]
    fn test_dsl_matches_handcoded_python() {
        let source = r#"
def authenticate(user, password):
    result = check_password(user, password)
    return result

class AuthService:
    def login(self, credentials):
        return authenticate(credentials.user, credentials.password)
"#;
        // Hand-coded path
        let mut parser =
            crate::parser::SourceParser::new(crate::parser::SourceLanguage::Python).unwrap();
        let hand_result = parser.parse(source).unwrap();

        // DSL path
        let tree = {
            let mut ts_parser = tree_sitter::Parser::new();
            ts_parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .unwrap();
            ts_parser.parse(source, None).unwrap()
        };
        let rules = python_rules();
        let engine = DslEngine::new(&rules, source.as_bytes());
        let (dsl_symbols, dsl_calls, _dsl_imports) = engine.extract(tree.root_node());

        // Same number of symbols
        assert_eq!(
            dsl_symbols.len(),
            hand_result.symbols.len(),
            "Symbol count mismatch: DSL={}, hand={}",
            dsl_symbols.len(),
            hand_result.symbols.len()
        );

        // Same symbol names and kinds
        for (dsl, hand) in dsl_symbols.iter().zip(hand_result.symbols.iter()) {
            assert_eq!(dsl.name, hand.name, "Symbol name mismatch");
            assert_eq!(dsl.kind, hand.kind, "Symbol kind mismatch for {}", dsl.name);
            assert_eq!(
                dsl.parent_name, hand.parent_name,
                "Parent mismatch for {}",
                dsl.name
            );
        }

        // Same number of calls (DSL may find slightly different calls due to walk order,
        // but should find the same callee names)
        let dsl_callees: std::collections::HashSet<_> =
            dsl_calls.iter().map(|c| &c.callee_name).collect();
        let hand_callees: std::collections::HashSet<_> =
            hand_result.calls.iter().map(|c| &c.callee_name).collect();
        assert_eq!(
            dsl_callees, hand_callees,
            "Call targets differ: DSL={:?}, hand={:?}",
            dsl_callees, hand_callees
        );
    }
}
