//! TypeScript, TSX and JavaScript extraction, plus the Vue/Svelte
//! `<script>` block that is parsed as TypeScript.

use super::{
    BindingValue, CallForm, CallReference, ExpressionStep, ImportInfo, ParsedSymbol, SourceParser,
    TypeBinding, FILE_SCOPE,
};
use myceliums_storage::{SymbolKind, SymbolMetadata};
use tree_sitter::Node;

impl SourceParser {
    // --- TypeScript/TSX ---

    pub(super) fn extract_ts_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            // An abstract method's declaration is a method too: calls bind
            // to it statically.
            "method_definition" | "abstract_method_signature" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                    // Recurse into class body with parent set
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ts_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // The interface's method signatures.
                    if let Some(body) = node.child_by_field_name("body") {
                        let cursor = &mut body.walk();
                        for member in body.children(cursor) {
                            let Some(method) = (member.kind() == "method_signature")
                                .then(|| member.child_by_field_name("name"))
                                .flatten()
                            else {
                                continue;
                            };
                            let return_type = member.child_by_field_name("return_type").map(|rt| {
                                self.node_text(rt, source)
                                    .trim_start_matches(':')
                                    .trim()
                                    .to_string()
                            });
                            symbols.push(ParsedSymbol {
                                name: self.node_text(method, source).to_string(),
                                kind: SymbolKind::Method,
                                start_line: member.start_position().row as u32 + 1,
                                end_line: member.end_position().row as u32 + 1,
                                signature: self.node_text(member, source).trim().to_string(),
                                content: self.node_text(member, source).to_string(),
                                parent_name: Some(name.clone()),
                                metadata: return_type.map(|return_type| SymbolMetadata {
                                    return_type: Some(return_type),
                                    ..SymbolMetadata::default()
                                }),
                            });
                        }
                    }
                }
            }
            "type_alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                // Handle: const foo = ..., export const foo = ...
                // Also handle arrow functions: const foo = () => ...
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            let value = child.child_by_field_name("value");
                            let is_arrow = value
                                .map(|v| {
                                    v.kind() == "arrow_function"
                                        || v.kind() == "function_expression"
                                        || v.kind() == "function"
                                })
                                .unwrap_or(false);

                            let sym_kind = if is_arrow {
                                SymbolKind::Function
                            } else if self.node_text(node, source).starts_with("const") {
                                SymbolKind::Constant
                            } else {
                                SymbolKind::Variable
                            };

                            symbols.push(ParsedSymbol {
                                name,
                                kind: sym_kind,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            "export_statement" => {
                // Recurse into exported declarations
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    self.extract_ts_symbols(child, source, symbols, parent);
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_ts_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_declaration" | "method_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "variable_declarator" => {
                let value = node.child_by_field_name("value");
                let is_fn = value
                    .map(|v| v.kind() == "arrow_function" || v.kind() == "function_expression")
                    .unwrap_or(false);
                if is_fn {
                    node.child_by_field_name("name")
                        .map(|n| self.node_text(n, source).to_string())
                } else {
                    None
                }
            }
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" || kind == "new_expression" {
            let field = if kind == "new_expression" {
                "constructor"
            } else {
                "function"
            };
            if let Some(func_node) = node.child_by_field_name(field) {
                let (callee, chain, form) = match func_node.kind() {
                    "identifier" => (
                        self.node_text(func_node, source).to_string(),
                        None,
                        CallForm::Bare,
                    ),
                    "member_expression" => {
                        let chain = self.build_ts_chain(func_node, source);
                        let name = if let Some(prop) = func_node.child_by_field_name("property") {
                            self.node_text(prop, source).to_string()
                        } else {
                            self.node_text(func_node, source).to_string()
                        };
                        (name, chain, CallForm::Member)
                    }
                    "super" => ("super".to_string(), None, CallForm::SuperConstructor),
                    _ => (
                        self.node_text(func_node, source).to_string(),
                        None,
                        CallForm::Unknown,
                    ),
                };
                // `new X()` and `new ns.X()` construct X.
                let form = match (kind, form) {
                    ("new_expression", CallForm::Bare | CallForm::Member) => CallForm::New,
                    (_, form) => form,
                };
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain,
                        file: None,
                        form,
                        arg_count: None,
                    });
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_calls(child, source, calls, scope);
        }
    }

    /// Parameters, variables and class fields (declared, constructor
    /// parameter properties, `this.x = ...`) with their annotations or
    /// initialisers (#139).
    pub(super) fn extract_ts_bindings(
        &self,
        node: Node,
        source: &[u8],
        out: &mut Vec<TypeBinding>,
        function: Option<(u32, u32)>,
        class: Option<&str>,
    ) {
        let lines = |n: Node| {
            (
                n.start_position().row as u32 + 1,
                n.end_position().row as u32 + 1,
            )
        };
        let local_scope = |n: Node| match function {
            Some((_, end)) => (lines(n).0, end),
            None => (lines(n).0, u32::MAX),
        };
        let annotation = |n: Node| {
            n.child_by_field_name("type").map(|t| {
                self.node_text(t, source)
                    .trim_start_matches(':')
                    .trim()
                    .to_string()
            })
        };
        match node.kind() {
            "class_declaration" | "class" | "abstract_class_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.node_text(n, source).to_string());
                if let Some(body) = node.child_by_field_name("body") {
                    self.extract_ts_bindings(body, source, out, None, name.as_deref().or(class));
                }
                return;
            }
            "public_field_definition" => {
                if let (Some(class), Some(name)) = (class, node.child_by_field_name("name")) {
                    let value = match (annotation(node), node.child_by_field_name("value")) {
                        (Some(ty), _) => BindingValue::Type(ty),
                        (None, Some(value)) => self.ts_binding_value(value, source),
                        _ => BindingValue::Unknown,
                    };
                    out.push(TypeBinding {
                        name: self.node_text(name, source).to_string(),
                        owner: Some(class.to_string()),
                        scope: FILE_SCOPE,
                        value,
                    });
                }
            }
            "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "arrow_function"
            | "function_expression"
            | "function" => {
                let span = lines(node);
                let is_constructor = node.kind() == "method_definition"
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|n| self.node_text(n, source) == "constructor");
                if let Some(param) = node.child_by_field_name("parameter") {
                    // `x => ...`
                    out.push(TypeBinding {
                        name: self.node_text(param, source).to_string(),
                        owner: None,
                        scope: span,
                        value: BindingValue::Unknown,
                    });
                }
                if let Some(params) = node.child_by_field_name("parameters") {
                    let cursor = &mut params.walk();
                    for param in params.named_children(cursor) {
                        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
                            continue;
                        }
                        let Some(pattern) = param.child_by_field_name("pattern") else {
                            continue;
                        };
                        if pattern.kind() != "identifier" {
                            self.ts_unknown_names(pattern, source, span, out);
                            continue;
                        }
                        let name = self.node_text(pattern, source).to_string();
                        let value =
                            annotation(param).map_or(BindingValue::Unknown, BindingValue::Type);
                        // `constructor(private readonly repo: Repository<User>)`
                        // also declares a field.
                        let property = self
                            .find_child_by_kind(param, "accessibility_modifier")
                            .is_some()
                            || self
                                .node_text(param, source)
                                .trim_start()
                                .starts_with("readonly ");
                        if let (true, true, Some(class)) = (is_constructor, property, class) {
                            out.push(TypeBinding {
                                name: name.clone(),
                                owner: Some(class.to_string()),
                                scope: FILE_SCOPE,
                                value: value.clone(),
                            });
                        }
                        out.push(TypeBinding {
                            name,
                            owner: None,
                            scope: span,
                            value,
                        });
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.extract_ts_bindings(body, source, out, Some(span), class);
                }
                return;
            }
            "variable_declarator" => {
                if let Some(name) = node.child_by_field_name("name") {
                    if name.kind() == "identifier" {
                        let value = match (annotation(node), node.child_by_field_name("value")) {
                            (Some(ty), _) => BindingValue::Type(ty),
                            (None, Some(value)) => self.ts_binding_value(value, source),
                            _ => BindingValue::Unknown,
                        };
                        out.push(TypeBinding {
                            name: self.node_text(name, source).to_string(),
                            owner: None,
                            scope: local_scope(node),
                            value,
                        });
                    } else {
                        self.ts_unknown_names(name, source, local_scope(node), out);
                    }
                }
            }
            // `this.validator = new Validator(...)`
            "assignment_expression" => {
                let left = node.child_by_field_name("left");
                let field = left
                    .filter(|l| l.kind() == "member_expression")
                    .filter(|l| {
                        l.child_by_field_name("object")
                            .is_some_and(|o| o.kind() == "this")
                    })
                    .and_then(|l| l.child_by_field_name("property"));
                if let (Some(field), Some(class), Some(right)) =
                    (field, class, node.child_by_field_name("right"))
                {
                    out.push(TypeBinding {
                        name: self.node_text(field, source).to_string(),
                        owner: Some(class.to_string()),
                        scope: function.unwrap_or(FILE_SCOPE),
                        value: self.ts_binding_value(right, source),
                    });
                }
            }
            // `export { Hono as HonoBase }`: the module exports `Hono` under
            // another name, as if by `const HonoBase = Hono`.
            "export_statement" if node.child_by_field_name("source").is_none() => {
                if let Some(clause) = self.find_child_by_kind(node, "export_clause") {
                    let specs = &mut clause.walk();
                    for spec in clause.children(specs) {
                        let (Some(name), Some(alias)) = (
                            spec.child_by_field_name("name"),
                            spec.child_by_field_name("alias"),
                        ) else {
                            continue;
                        };
                        let (name, alias) =
                            (self.node_text(name, source), self.node_text(alias, source));
                        if spec.kind() != "export_specifier" || name == alias {
                            continue;
                        }
                        out.push(TypeBinding {
                            name: alias.to_string(),
                            owner: None,
                            scope: local_scope(node),
                            value: BindingValue::Expr(vec![ExpressionStep::Ident(
                                name.to_string(),
                            )]),
                        });
                    }
                }
            }
            "for_in_statement" => {
                if let Some(left) = node.child_by_field_name("left") {
                    self.ts_unknown_names(left, source, local_scope(node), out);
                }
            }
            "catch_clause" => {
                if let Some(param) = node.child_by_field_name("parameter") {
                    self.ts_unknown_names(param, source, lines(node), out);
                }
            }
            _ => {}
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_bindings(child, source, out, function, class);
        }
    }

    /// Every name a destructuring pattern binds, of unknown type.
    fn ts_unknown_names(
        &self,
        pattern: Node,
        source: &[u8],
        scope: (u32, u32),
        out: &mut Vec<TypeBinding>,
    ) {
        if matches!(
            pattern.kind(),
            "identifier" | "shorthand_property_identifier_pattern"
        ) {
            out.push(TypeBinding {
                name: self.node_text(pattern, source).to_string(),
                owner: None,
                scope,
                value: BindingValue::Unknown,
            });
            return;
        }
        // In `{ key: value }` only the value side binds.
        if pattern.kind() == "pair_pattern" {
            if let Some(value) = pattern.child_by_field_name("value") {
                self.ts_unknown_names(value, source, scope, out);
            }
            return;
        }
        let cursor = &mut pattern.walk();
        for child in pattern.named_children(cursor) {
            self.ts_unknown_names(child, source, scope, out);
        }
    }

    /// What an initialiser is, for a binding: a chain for a call, member,
    /// name, `new` or literal; unknown for a function or anything else.
    fn ts_binding_value(&self, value: Node, source: &[u8]) -> BindingValue {
        match value.kind() {
            "call_expression"
            | "member_expression"
            | "identifier"
            | "this"
            | "new_expression"
            | "subscript_expression"
            | "parenthesized_expression"
            | "non_null_expression"
            | "await_expression"
            | "as_expression"
            | "satisfies_expression"
            | "string"
            | "template_string"
            | "number"
            | "true"
            | "false"
            | "null"
            | "array"
            | "object"
            | "regex" => {
                let mut steps = Vec::new();
                self.collect_ts_chain_steps(value, source, &mut steps);
                BindingValue::Expr(steps)
            }
            _ => BindingValue::Unknown,
        }
    }

    /// Build an expression chain from a TypeScript `member_expression` node.
    fn build_ts_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_ts_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    pub(super) fn collect_ts_chain_steps(
        &self,
        node: Node,
        source: &[u8],
        steps: &mut Vec<ExpressionStep>,
    ) {
        match node.kind() {
            "string" | "template_string" | "number" | "true" | "false" | "null" | "undefined"
            | "regex" | "array" | "object" => steps.push(ExpressionStep::Literal),
            "new_expression" => {
                let name = node
                    .child_by_field_name("constructor")
                    .map(|c| match c.child_by_field_name("property") {
                        Some(prop) => self.node_text(prop, source),
                        None => self.node_text(c, source),
                    })
                    .unwrap_or_default();
                steps.push(ExpressionStep::New(name.to_string()));
            }
            "parenthesized_expression"
            | "non_null_expression"
            | "await_expression"
            | "as_expression"
            | "satisfies_expression"
            | "type_assertion" => {
                if let Some(inner) = node.named_child(0) {
                    self.collect_ts_chain_steps(inner, source, steps);
                }
            }
            "subscript_expression" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_ts_chain_steps(obj, source, steps);
                }
                steps.push(ExpressionStep::Index);
            }
            "member_expression" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_ts_chain_steps(obj, source, steps);
                }
                if let Some(prop) = node.child_by_field_name("property") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(prop, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_ts_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "this" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            "this" => steps.push(ExpressionStep::This),
            "super" => steps.push(ExpressionStep::Super),
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    pub(super) fn extract_ts_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        // A re-export is an import of what it exports: `export { a, b as c }
        // from './m'`, `export * from './m'`, `export * as ns from './m'`.
        if node.kind() == "export_statement" {
            if let Some(source_node) = node.child_by_field_name("source") {
                let module = self
                    .node_text(source_node, source)
                    .trim_matches(['\'', '"'])
                    .to_string();
                let mut named = false;
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "export_clause" => {
                            named = true;
                            let specs = &mut child.walk();
                            for spec in child.children(specs) {
                                if spec.kind() != "export_specifier" {
                                    continue;
                                }
                                let Some(name) = spec.child_by_field_name("name") else {
                                    continue;
                                };
                                let original = self.node_text(name, source).to_string();
                                let local = spec
                                    .child_by_field_name("alias")
                                    .map(|a| self.node_text(a, source).to_string())
                                    .unwrap_or_else(|| original.clone());
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: module.clone(),
                                    original_name: Some(original),
                                });
                            }
                        }
                        "namespace_export" => {
                            named = true;
                            if let Some(name) = self.find_child_by_kind(child, "identifier") {
                                imports.push(ImportInfo {
                                    local_name: self.node_text(name, source).to_string(),
                                    source_module: module.clone(),
                                    original_name: Some("*".to_string()),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if !named {
                    imports.push(ImportInfo {
                        local_name: "*".to_string(),
                        source_module: module,
                        original_name: None,
                    });
                }
            }
        }

        if node.kind() == "import_statement" {
            let source_node = node.child_by_field_name("source");
            let module = source_node
                .map(|s| {
                    let text = self.node_text(s, source);
                    text.trim_matches('\'').trim_matches('"').to_string()
                })
                .unwrap_or_default();

            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "import_clause" {
                    let clause_cursor = &mut child.walk();
                    for clause_child in child.children(clause_cursor) {
                        match clause_child.kind() {
                            "identifier" => {
                                imports.push(ImportInfo {
                                    local_name: self.node_text(clause_child, source).to_string(),
                                    source_module: module.clone(),
                                    original_name: None,
                                });
                            }
                            // `import * as ns from "./m"`: the original name
                            // `*` marks a module namespace.
                            "namespace_import" => {
                                if let Some(name) =
                                    self.find_child_by_kind(clause_child, "identifier")
                                {
                                    imports.push(ImportInfo {
                                        local_name: self.node_text(name, source).to_string(),
                                        source_module: module.clone(),
                                        original_name: Some("*".to_string()),
                                    });
                                }
                            }
                            "named_imports" => {
                                let spec_cursor = &mut clause_child.walk();
                                for spec in clause_child.children(spec_cursor) {
                                    if spec.kind() == "import_specifier" {
                                        let name_node = spec.child_by_field_name("name");
                                        let alias_node = spec.child_by_field_name("alias");
                                        if let Some(name) = name_node {
                                            let original = self.node_text(name, source).to_string();
                                            let local = alias_node
                                                .map(|a| self.node_text(a, source).to_string())
                                                .unwrap_or_else(|| original.clone());
                                            imports.push(ImportInfo {
                                                local_name: local,
                                                source_module: module.clone(),
                                                original_name: Some(original),
                                            });
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Handle CommonJS require() - for JavaScript files
        // Pattern: const foo = require('module') or var bar = require('./file')
        if node.kind() == "variable_declarator" {
            if let Some(value) = node.child_by_field_name("value") {
                if value.kind() == "call_expression" {
                    if let Some(func) = value.child_by_field_name("function") {
                        if func.kind() == "identifier" && self.node_text(func, source) == "require"
                        {
                            // Extract the module name from arguments
                            let cursor = &mut value.walk();
                            for child in value.children(cursor) {
                                if child.kind() == "arguments" {
                                    let arg_cursor = &mut child.walk();
                                    for arg in child.children(arg_cursor) {
                                        if arg.kind() == "string" {
                                            let module = self.node_text(arg, source);
                                            let module = module
                                                .trim_matches('\'')
                                                .trim_matches('"')
                                                .to_string();

                                            // Extract the local name from the declarator
                                            if let Some(name_node) =
                                                node.child_by_field_name("name")
                                            {
                                                imports.push(ImportInfo {
                                                    local_name: self
                                                        .node_text(name_node, source)
                                                        .to_string(),
                                                    source_module: module,
                                                    original_name: None,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_imports(child, source, imports);
        }
    }

    /// Extract variable aliases from TypeScript/JavaScript assignments.
    ///
    /// Detects patterns like `const handler = authenticate` or `handler = authenticate`
    /// where the RHS is a simple identifier.
    pub(super) fn extract_ts_aliases(
        &self,
        node: Node,
        source: &[u8],
        aliases: &mut Vec<(String, String)>,
    ) {
        match node.kind() {
            // const/let/var handler = authenticate
            "variable_declarator" => {
                let name_node = node.child_by_field_name("name");
                let value_node = node.child_by_field_name("value");
                if let (Some(name), Some(value)) = (name_node, value_node) {
                    if name.kind() == "identifier" && value.kind() == "identifier" {
                        let local = self.node_text(name, source).to_string();
                        let target = self.node_text(value, source).to_string();
                        aliases.push((local, target));
                    }
                    // handler = obj.method
                    if name.kind() == "identifier" && value.kind() == "member_expression" {
                        if let Some(prop) = value.child_by_field_name("property") {
                            let local = self.node_text(name, source).to_string();
                            let target = self.node_text(prop, source).to_string();
                            aliases.push((local, target));
                        }
                    }
                }
            }
            // handler = authenticate (reassignment)
            "assignment_expression" => {
                let left = node.child_by_field_name("left");
                let right = node.child_by_field_name("right");
                if let (Some(left_node), Some(right_node)) = (left, right) {
                    if left_node.kind() == "identifier" && right_node.kind() == "identifier" {
                        let local = self.node_text(left_node, source).to_string();
                        let target = self.node_text(right_node, source).to_string();
                        aliases.push((local, target));
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_aliases(child, source, aliases);
        }
    }

    // ── Vue / Svelte script block extraction ─────────────────────────

    /// Extract the content of the first `<script>` block from a Vue or Svelte file.
    /// Falls back to the full source if no script block is found.
    pub(super) fn extract_script_block(source: &str) -> String {
        // Match <script ...> (with optional attributes like lang="ts", setup, etc.)
        // ASCII-only lowercasing keeps every byte offset identical to `source`;
        // full Unicode lowercasing changes lengths (e.g. 'K' KELVIN SIGN →
        // 'k'), and slicing `source` with those offsets panicked.
        let lower = source.to_ascii_lowercase();
        if let Some(tag_start) = lower.find("<script") {
            // Find the closing > of the opening tag
            if let Some(content_start) = source[tag_start..].find('>') {
                let content_start = tag_start + content_start + 1;
                // Find the closing </script>
                if let Some(end) = lower[content_start..].find("</script>") {
                    return source[content_start..content_start + end].to_string();
                }
            }
        }
        // No script block found — return empty string (will produce no symbols)
        String::new()
    }

    /// Extract TypeScript/JavaScript metadata: decorators, return type, extends/implements.
    fn extract_ts_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Decorators: look at preceding siblings
        if let Some(parent) = node.parent() {
            let cursor = &mut parent.walk();
            let children: Vec<_> = parent.children(cursor).collect();
            let mut idx = children.len();
            for (i, child) in children.iter().enumerate() {
                if child.id() == node.id() {
                    idx = i;
                    break;
                }
            }
            for i in (0..idx).rev() {
                if children[i].kind() == "decorator" {
                    let text = self.node_text(children[i], source).trim().to_string();
                    meta.decorators.push(text);
                } else if children[i].kind() != "comment" {
                    break;
                }
            }
            meta.decorators.reverse();
        }

        // Return type annotation
        if matches!(
            node.kind(),
            "function_declaration"
                | "method_definition"
                | "abstract_method_signature"
                | "arrow_function"
        ) {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                let cleaned = text.strip_prefix(':').map(|s| s.trim()).unwrap_or(&text);
                if !cleaned.is_empty() {
                    meta.return_type = Some(cleaned.to_string());
                }
            }
        }

        // Extends / implements for class_declaration
        if matches!(
            node.kind(),
            "class_declaration" | "abstract_class_declaration"
        ) {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "class_heritage" {
                    let cursor2 = &mut child.walk();
                    for hchild in child.children(cursor2) {
                        if hchild.kind() == "extends_clause" || hchild.kind() == "implements_clause"
                        {
                            let cursor3 = &mut hchild.walk();
                            for gchild in hchild.children(cursor3) {
                                let gk = gchild.kind();
                                if gk == "identifier"
                                    || gk == "nested_identifier"
                                    || gk == "generic_type"
                                    || gk == "type_identifier"
                                {
                                    meta.superclasses
                                        .push(self.node_text(gchild, source).to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Type parameters
        if let Some(tp) = node.child_by_field_name("type_parameters") {
            let text = self.node_text(tp, source).trim().to_string();
            if !text.is_empty() {
                meta.type_params.push(text);
            }
        }

        if meta.decorators.is_empty()
            && meta.return_type.is_none()
            && meta.superclasses.is_empty()
            && meta.type_params.is_empty()
        {
            None
        } else {
            Some(meta)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{ExpressionStep, SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    #[test]
    fn script_block_extraction_survives_case_changing_unicode() {
        // U+212A KELVIN SIGN lowercases to a 1-byte 'k'; offsets from a
        // Unicode-lowercased copy used to land inside it and panic.
        let src = "\u{212A}<script>const a = 1;</script>";
        assert_eq!(SourceParser::extract_script_block(src), "const a = 1;");
        let src = "<p>İstanbul</p><SCRIPT lang=\"ts\">let x = 2;</SCRIPT>";
        assert_eq!(SourceParser::extract_script_block(src), "let x = 2;");
        assert_eq!(SourceParser::extract_script_block("\u{212A} no script"), "");
    }

    #[test]
    fn test_parse_typescript_function() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function greet(name: string): string { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_typescript_class() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id: string) { return null; }
    deleteUser(id: string) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_typescript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("const add = (a: number, b: number) => a + b;")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_call_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    #[test]
    fn test_import_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_parse_javascript_function() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse("function greet(name) { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_javascript_class() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id) { return null; }
    deleteUser(id) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_javascript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser.parse("const add = (a, b) => a + b;").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_import_extraction_js_esm() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user.js';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_import_extraction_js_commonjs() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
const { UserService } = require('./services/user.js');
const axios = require('axios');
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "UserService" || i.local_name == "axios"));
    }

    #[test]
    fn test_call_extraction_js() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    #[test]
    fn test_simple_call_has_no_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { bar(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "bar");
        assert!(call.is_some(), "should detect the bar() call");
        assert!(
            call.unwrap().chain.is_none(),
            "simple call should have no chain"
        );
    }

    #[test]
    fn test_ts_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { obj.method(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "method");
        assert!(call.is_some(), "should detect obj.method() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("method call should have a chain");
        assert_eq!(chain[0], ExpressionStep::Ident("obj".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("method".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
    }

    #[test]
    fn test_ts_chained_calls() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { a.b().c(); }").unwrap();
        // The outer call is c(), with the receiver being a.b()
        let call = result.calls.iter().find(|c| c.callee_name == "c");
        assert!(call.is_some(), "should detect .c() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("chained call should have a chain");
        // a.b().c() => [Ident("a"), Field("b"), Call, Field("c"), Call]
        assert_eq!(chain[0], ExpressionStep::Ident("a".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("b".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
        assert_eq!(chain[3], ExpressionStep::Field("c".to_string()));
        assert_eq!(chain[4], ExpressionStep::Call);
    }

    #[test]
    fn test_typescript_class_extends_has_superclasses() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class Animal {
    name: string;
}

class Dog extends Animal {
    bark() {}
}
"#,
            )
            .unwrap();
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "Dog")
            .expect("should find Dog");
        let meta = cls.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.superclasses.is_empty(),
            "should have superclasses from extends"
        );
    }

    #[test]
    fn test_typescript_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function add(a: number, b: number): number { return a + b; }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "add")
            .expect("should find add");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
    }

    #[test]
    fn test_typescript_alias_extraction() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
const handler = authenticate;
let cb = service.validate;
handler = reassigned;
"#,
            )
            .unwrap();
        // Should find: handler→authenticate, cb→validate, handler→reassigned
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "authenticate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "cb" && t == "validate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "reassigned"));
    }
}

/// What the resolver needs from TypeScript (#139).
#[cfg(test)]
mod resolution_facts {
    use crate::parser::{BindingValue, CallForm, ExpressionStep, SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    const SOURCE: &str = r#"
import * as config from "./config";

export interface Sink {
  write(text: string): void;
}

export class UserService extends Base {
  private readonly validator: Validator;

  constructor(private readonly repo: Repository<User>, size?: number) {
    super("users");
    this.validator = new Validator([isEmail]);
  }

  settings(text: string): string {
    const cfg = config.Config.load(text);
    const handler = (q: string) => q;
    return cfg.get("theme");
  }
}
"#;

    fn parse() -> crate::parser::ParseResult {
        SourceParser::new(SourceLanguage::TypeScript)
            .unwrap()
            .parse(SOURCE)
            .unwrap()
    }

    #[test]
    fn abstract_classes_and_their_abstract_methods_are_symbols() {
        let result = SourceParser::new(SourceLanguage::TypeScript)
            .unwrap()
            .parse(
                "export abstract class Processor {\n  protected abstract headers(e: E): Headers;\n  run(e: E) { return this.headers(e); }\n}\n",
            )
            .unwrap();
        let find = |name: &str| result.symbols.iter().find(|s| s.name == name).unwrap();
        assert_eq!(find("Processor").kind, SymbolKind::Class);
        assert_eq!(find("headers").parent_name.as_deref(), Some("Processor"));
        assert_eq!(find("run").parent_name.as_deref(), Some("Processor"));
    }

    #[test]
    fn new_super_and_namespace_imports_are_recorded() {
        let result = parse();
        let call = |callee: &str| {
            result
                .calls
                .iter()
                .find(|c| c.callee_name == callee)
                .unwrap()
        };
        assert_eq!(call("Validator").form, CallForm::New);
        assert_eq!(call("super").form, CallForm::SuperConstructor);
        assert_eq!(call("get").form, CallForm::Member);
        let namespace = result
            .imports
            .iter()
            .find(|i| i.local_name == "config")
            .unwrap();
        assert_eq!(namespace.original_name.as_deref(), Some("*"));
        // An interface's method signatures are symbols.
        let write = result.symbols.iter().find(|s| s.name == "write").unwrap();
        assert_eq!(write.kind, SymbolKind::Method);
        assert_eq!(write.parent_name.as_deref(), Some("Sink"));
    }

    #[test]
    fn fields_parameters_and_locals_are_bound() {
        let result = parse();
        let find = |name: &str, owner: Option<&str>| {
            result
                .bindings
                .iter()
                .find(|b| b.name == name && b.owner.as_deref() == owner)
                .unwrap_or_else(|| panic!("{name} not bound: {:?}", result.bindings))
                .value
                .clone()
        };
        assert_eq!(
            find("validator", Some("UserService")),
            BindingValue::Type("Validator".into())
        );
        // A constructor parameter property is a field and a parameter.
        assert_eq!(
            find("repo", Some("UserService")),
            BindingValue::Type("Repository<User>".into())
        );
        assert_eq!(
            find("repo", None),
            BindingValue::Type("Repository<User>".into())
        );
        assert!(result
            .bindings
            .iter()
            .all(|b| b.name != "size" || b.owner.is_none()));
        assert_eq!(
            find("cfg", None),
            BindingValue::Expr(vec![
                ExpressionStep::Ident("config".into()),
                ExpressionStep::Field("Config".into()),
                ExpressionStep::Field("load".into()),
                ExpressionStep::Call
            ])
        );
        assert_eq!(find("handler", None), BindingValue::Unknown);
    }

    #[test]
    fn re_exports_are_imports_of_what_they_export() {
        let result = SourceParser::new(SourceLanguage::TypeScript)
            .unwrap()
            .parse(
                "export { jwt, sign as signToken } from './jwt'\nexport * from './util'\nexport * as codec from './codec'\nexport class Local {}\n",
            )
            .unwrap();
        let imports: Vec<(&str, &str, Option<&str>)> = result
            .imports
            .iter()
            .map(|i| {
                (
                    i.local_name.as_str(),
                    i.source_module.as_str(),
                    i.original_name.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            imports,
            [
                ("jwt", "./jwt", Some("jwt")),
                ("signToken", "./jwt", Some("sign")),
                ("*", "./util", None),
                ("codec", "./codec", Some("*")),
            ]
        );
    }

    #[test]
    fn an_aliased_local_export_binds_the_alias_to_the_name() {
        let result = SourceParser::new(SourceLanguage::TypeScript)
            .unwrap()
            .parse("class Hono {}\nexport { Hono as HonoBase, Hono }\n")
            .unwrap();
        let module_level: Vec<(&str, &BindingValue)> = result
            .bindings
            .iter()
            .filter(|b| b.owner.is_none() && b.scope.1 == u32::MAX)
            .map(|b| (b.name.as_str(), &b.value))
            .collect();
        assert_eq!(
            module_level,
            [(
                "HonoBase",
                &BindingValue::Expr(vec![ExpressionStep::Ident("Hono".to_string())])
            )]
        );
    }
}
