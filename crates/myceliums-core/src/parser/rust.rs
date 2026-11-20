//! Rust extraction.

use super::{
    classify, BindingValue, CallForm, CallReference, DeclaredTypes, ExpressionStep, ImportInfo,
    ParsedSymbol, SourceParser, TypeBinding, FILE_SCOPE,
};
use myceliums_storage::{SymbolKind, SymbolMetadata, Visibility};
use tree_sitter::Node;

impl SourceParser {
    // --- Rust ---

    pub(super) fn extract_rust_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_item" | "function_signature_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "struct_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "enum_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                }
            }
            "trait_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_rust_metadata(node, source),
                    });
                    // Recurse into trait body for method signatures
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_rust_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "impl_item" => {
                // Extract the type name from the impl block
                let type_name = self.extract_rust_impl_type_name(node, source);
                if let Some(ref type_name) = type_name {
                    // Recurse into impl body with the type name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_rust_symbols(body, source, symbols, Some(type_name));
                        return;
                    }
                }
            }
            "mod_item" => {
                // A `#[cfg(test)]` module is test code, and so is everything
                // in it (#146).
                let cfg_test = self
                    .rust_attributes(node, source)
                    .iter()
                    .any(|a| classify::is_rust_cfg_test(a));
                let first = symbols.len();
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
                if cfg_test {
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_rust_symbols(child, source, symbols, parent);
                    }
                    for symbol in &mut symbols[first..] {
                        symbol
                            .metadata
                            .get_or_insert_with(SymbolMetadata::default)
                            .test = true;
                    }
                    return;
                }
            }
            "const_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Constant,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "static_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Variable,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "type_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "macro_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_rust_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_symbols(child, source, symbols, parent);
        }
    }

    /// `impl Trait for Type`: the trait is a base of `Type`, which may be
    /// declared in another file (#139). A method a type gets from a trait
    /// (a default method) is then found through it.
    pub(super) fn extract_rust_trait_impls(
        &self,
        node: Node,
        source: &[u8],
        out: &mut Vec<DeclaredTypes>,
    ) {
        if node.kind() == "impl_item" {
            let ty = self.extract_rust_impl_type_name(node, source);
            let trait_node = node.child_by_field_name("trait");
            if let (Some(ty), Some(trait_node)) = (ty, trait_node) {
                let ty = ty
                    .trim_start_matches('&')
                    .rsplit("::")
                    .next()
                    .unwrap_or(&ty)
                    .trim();
                let base = self.node_text(trait_node, source);
                out.push(DeclaredTypes {
                    name: ty.to_string(),
                    line: node.start_position().row as u32 + 1,
                    return_type: None,
                    bases: vec![base.split('<').next().unwrap_or(base).trim().to_string()],
                    elsewhere: true,
                });
            }
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_trait_impls(child, source, out);
        }
    }

    /// Extract the type name from an impl block.
    /// Handles both `impl Type` and `impl Trait for Type`.
    fn extract_rust_impl_type_name(&self, node: Node, source: &[u8]) -> Option<String> {
        if let Some(type_node) = node.child_by_field_name("type") {
            let text = self.node_text(type_node, source);
            // Strip generic parameters if present (e.g., `Server<T>` -> `Server`)
            let name = text.split('<').next().unwrap_or(text).trim();
            Some(name.to_string())
        } else {
            None
        }
    }

    fn extract_rust_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Rust, the signature is everything before the opening brace
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            // For items without a body (e.g., trait method signatures ending with `;`)
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_rust_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "function_item" {
            node.child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string())
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let (Some(func_node), Some(caller)) =
                    (node.child_by_field_name("function"), scope)
                {
                    // `foo::<T>()`, `x.foo::<T>()`: the turbofish is noise here.
                    let func_node = match func_node.kind() {
                        "generic_function" => func_node
                            .child_by_field_name("function")
                            .unwrap_or(func_node),
                        _ => func_node,
                    };
                    let (callee, chain, form) = match func_node.kind() {
                        "identifier" => (
                            self.node_text(func_node, source).to_string(),
                            None,
                            CallForm::Bare,
                        ),
                        // `Type::new()`, `module::f()`, `Self::f()`: the path
                        // before the name is the receiver.
                        "scoped_identifier" => {
                            let mut steps = self.rust_path_steps(func_node, source);
                            let name = match steps.pop() {
                                Some(ExpressionStep::Field(name) | ExpressionStep::Ident(name)) => {
                                    name
                                }
                                _ => self.node_text(func_node, source).to_string(),
                            };
                            if steps.is_empty() {
                                (name, None, CallForm::Bare)
                            } else {
                                steps.push(ExpressionStep::Field(name.clone()));
                                steps.push(ExpressionStep::Call);
                                (name, Some(steps), CallForm::Member)
                            }
                        }
                        // `self.method()`, `value.method()`
                        "field_expression" => {
                            let chain = self.build_rust_chain(func_node, source);
                            let name = if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            };
                            (name, chain, CallForm::Member)
                        }
                        _ => (
                            self.node_text(func_node, source).to_string(),
                            None,
                            CallForm::Unknown,
                        ),
                    };
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: callee,
                        line: node.start_position().row as u32 + 1,
                        chain,
                        file: None,
                        form,
                        arg_count: node
                            .child_by_field_name("arguments")
                            .map(|args| args.named_child_count() as u32),
                    });
                }
            }
            "macro_invocation" => {
                // e.g., `println!("hello")`, `vec![1, 2, 3]`
                if let (Some(macro_node), Some(caller)) = (node.child_by_field_name("macro"), scope)
                {
                    let macro_name = match macro_node.child_by_field_name("name") {
                        Some(name) => self.node_text(name, source),
                        None => self.node_text(macro_node, source),
                    };
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: macro_name.to_string(),
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                        form: CallForm::Bare,
                        arg_count: None,
                    });
                    // The arguments of a macro are an unparsed token tree:
                    // `println!("{}", run(&argv))` still calls `run`.
                    if let Some(tree) = self.find_child_by_kind(node, "token_tree") {
                        self.extract_rust_token_tree_calls(tree, source, calls, caller);
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_calls(child, source, calls, scope);
        }
    }

    /// Parameters, `let` bindings and struct fields with their declared
    /// types or initialisers (#139).
    pub(super) fn extract_rust_bindings(
        &self,
        node: Node,
        source: &[u8],
        out: &mut Vec<TypeBinding>,
        function: Option<(u32, u32)>,
    ) {
        let lines = |n: Node| {
            (
                n.start_position().row as u32 + 1,
                n.end_position().row as u32 + 1,
            )
        };
        let local_scope = |n: Node| match function {
            Some((_, end)) => (lines(n).0, end),
            None => FILE_SCOPE,
        };
        let text = |n: Node| self.node_text(n, source).to_string();
        match node.kind() {
            // `static REGISTRY: Registry = ...;`, `const LIMIT: usize = 3;`
            "static_item" | "const_item" => {
                if let (Some(name), Some(ty)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("type"),
                ) {
                    out.push(TypeBinding {
                        name: text(name),
                        owner: None,
                        scope: local_scope(node),
                        value: BindingValue::Type(text(ty)),
                    });
                }
            }
            "struct_item" => {
                let name = node.child_by_field_name("name").map(text);
                let body = node.child_by_field_name("body");
                if let (Some(name), Some(body)) = (name, body) {
                    let cursor = &mut body.walk();
                    for field in body.named_children(cursor) {
                        if let (Some(field_name), Some(ty)) = (
                            field.child_by_field_name("name"),
                            field.child_by_field_name("type"),
                        ) {
                            out.push(TypeBinding {
                                name: text(field_name),
                                owner: Some(name.clone()),
                                scope: FILE_SCOPE,
                                value: BindingValue::Type(text(ty)),
                            });
                        }
                    }
                }
                return;
            }
            "function_item" | "closure_expression" => {
                let span = lines(node);
                if let Some(params) = node.child_by_field_name("parameters") {
                    let cursor = &mut params.walk();
                    for param in params.named_children(cursor) {
                        match param.kind() {
                            "parameter" => {
                                let ty = param.child_by_field_name("type").map(text);
                                if let Some(pattern) = param.child_by_field_name("pattern") {
                                    match (rust_pattern_name(pattern, source), ty) {
                                        (Some(name), Some(ty)) => out.push(TypeBinding {
                                            name,
                                            owner: None,
                                            scope: span,
                                            value: BindingValue::Type(ty),
                                        }),
                                        _ => self.rust_unknown_names(pattern, source, span, out),
                                    }
                                }
                            }
                            "self_parameter" => {}
                            _ => self.rust_unknown_names(param, source, span, out),
                        }
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.extract_rust_bindings(body, source, out, Some(span));
                }
                return;
            }
            "let_declaration" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    let value = match (
                        node.child_by_field_name("type"),
                        node.child_by_field_name("value"),
                    ) {
                        (Some(ty), _) => BindingValue::Type(text(ty)),
                        (None, Some(value)) => self.rust_binding_value(value, source),
                        _ => BindingValue::Unknown,
                    };
                    match rust_pattern_name(pattern, source) {
                        Some(name) => out.push(TypeBinding {
                            name,
                            owner: None,
                            scope: local_scope(node),
                            value,
                        }),
                        None => self.rust_unknown_names(pattern, source, local_scope(node), out),
                    }
                }
            }
            "for_expression" | "let_condition" | "match_arm" => {
                let pattern = node.child_by_field_name("pattern");
                if let Some(pattern) = pattern {
                    self.rust_unknown_names(pattern, source, local_scope(node), out);
                }
            }
            _ => {}
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_bindings(child, source, out, function);
        }
    }

    /// Every name a pattern binds, of unknown type.
    fn rust_unknown_names(
        &self,
        pattern: Node,
        source: &[u8],
        scope: (u32, u32),
        out: &mut Vec<TypeBinding>,
    ) {
        if pattern.kind() == "identifier" {
            out.push(TypeBinding {
                name: self.node_text(pattern, source).to_string(),
                owner: None,
                scope,
                value: BindingValue::Unknown,
            });
            return;
        }
        // The path of a tuple-struct or struct pattern names a type, not a binding.
        let cursor = &mut pattern.walk();
        for (i, child) in pattern.children(cursor).enumerate() {
            if pattern.field_name_for_child(i as u32) == Some("type") {
                continue;
            }
            self.rust_unknown_names(child, source, scope, out);
        }
    }

    /// What a `let` initialiser is, for a binding.
    fn rust_binding_value(&self, value: Node, source: &[u8]) -> BindingValue {
        match value.kind() {
            "call_expression"
            | "field_expression"
            | "identifier"
            | "scoped_identifier"
            | "struct_expression"
            | "reference_expression"
            | "try_expression"
            | "await_expression"
            | "parenthesized_expression"
            | "index_expression"
            | "string_literal"
            | "raw_string_literal"
            | "integer_literal"
            | "float_literal"
            | "boolean_literal"
            | "char_literal"
            | "array_expression"
            | "tuple_expression"
            | "macro_invocation"
            | "self" => {
                let mut steps = Vec::new();
                self.collect_rust_chain_steps(value, source, &mut steps);
                BindingValue::Expr(steps)
            }
            _ => BindingValue::Unknown,
        }
    }

    /// Bare calls written inside a macro's token tree: an identifier followed
    /// by a parenthesised token tree, not preceded by `.` or `::` (method and
    /// path calls are left out: without a parse their receiver is unknown).
    fn extract_rust_token_tree_calls(
        &self,
        tree: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        caller: &str,
    ) {
        let cursor = &mut tree.walk();
        let tokens: Vec<Node> = tree.children(cursor).collect();
        for (i, token) in tokens.iter().enumerate() {
            if token.kind() != "token_tree" {
                continue;
            }
            let called = i > 0
                && tokens[i - 1].kind() == "identifier"
                && self.node_text(*token, source).starts_with('(')
                && !(i > 1 && matches!(self.node_text(tokens[i - 2], source), "." | "::"));
            if called {
                let name = tokens[i - 1];
                calls.push(CallReference {
                    caller_name: caller.to_string(),
                    callee_name: self.node_text(name, source).to_string(),
                    line: name.start_position().row as u32 + 1,
                    chain: None,
                    file: None,
                    form: CallForm::Bare,
                    arg_count: None,
                });
            }
            // `self.method(...)`: the one receiver a token tree spells out.
            let on_self = i > 2
                && tokens[i - 1].kind() == "identifier"
                && self.node_text(*token, source).starts_with('(')
                && self.node_text(tokens[i - 2], source) == "."
                && tokens[i - 3].kind() == "self";
            if on_self {
                let name = self.node_text(tokens[i - 1], source).to_string();
                calls.push(CallReference {
                    caller_name: caller.to_string(),
                    callee_name: name.clone(),
                    line: tokens[i - 1].start_position().row as u32 + 1,
                    chain: Some(vec![
                        ExpressionStep::This,
                        ExpressionStep::Field(name),
                        ExpressionStep::Call,
                    ]),
                    file: None,
                    form: CallForm::Member,
                    arg_count: None,
                });
            }
            self.extract_rust_token_tree_calls(*token, source, calls, caller);
        }
    }

    /// `a::b::c` as chain steps `[Ident(a), Field(b), Field(c)]`; generic
    /// arguments (`Repo::<User>::new`) are dropped.
    pub(super) fn rust_path_steps(&self, node: Node, source: &[u8]) -> Vec<ExpressionStep> {
        let mut segments = Vec::new();
        self.rust_path_segments(node, source, &mut segments);
        segments
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                if i == 0 {
                    ExpressionStep::Ident(s)
                } else {
                    ExpressionStep::Field(s)
                }
            })
            .collect()
    }

    fn rust_path_segments(&self, node: Node, source: &[u8], segments: &mut Vec<String>) {
        match node.kind() {
            "scoped_identifier" | "scoped_type_identifier" => {
                if let Some(path) = node.child_by_field_name("path") {
                    self.rust_path_segments(path, source, segments);
                }
                if let Some(name) = node.child_by_field_name("name") {
                    segments.push(self.node_text(name, source).to_string());
                }
            }
            "generic_type" => {
                if let Some(ty) = node.child_by_field_name("type") {
                    self.rust_path_segments(ty, source, segments);
                }
            }
            "generic_function" => {
                if let Some(function) = node.child_by_field_name("function") {
                    self.rust_path_segments(function, source, segments);
                }
            }
            _ => segments.push(self.node_text(node, source).to_string()),
        }
    }

    /// Build an expression chain from a Rust `field_expression` node.
    fn build_rust_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_rust_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    pub(super) fn collect_rust_chain_steps(
        &self,
        node: Node,
        source: &[u8],
        steps: &mut Vec<ExpressionStep>,
    ) {
        match node.kind() {
            "field_expression" => {
                if let Some(obj) = node.child_by_field_name("value") {
                    self.collect_rust_chain_steps(obj, source, steps);
                }
                if let Some(field) = node.child_by_field_name("field") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(field, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_rust_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "scoped_identifier" | "scoped_type_identifier" | "generic_function" => {
                steps.extend(self.rust_path_steps(node, source))
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "self" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            "self" => steps.push(ExpressionStep::This),
            "reference_expression"
            | "parenthesized_expression"
            | "try_expression"
            | "unary_expression"
            | "await_expression" => {
                let inner = node.child_by_field_name("value").or_else(|| {
                    node.named_child(node.named_child_count().saturating_sub(1) as u32)
                });
                match inner {
                    Some(inner) => self.collect_rust_chain_steps(inner, source, steps),
                    None => steps.push(ExpressionStep::Literal),
                }
            }
            "index_expression" => {
                if let Some(value) = node.named_child(0) {
                    self.collect_rust_chain_steps(value, source, steps);
                }
                steps.push(ExpressionStep::Index);
            }
            // `Foo { .. }` builds a Foo.
            "struct_expression" => {
                let mut segments = Vec::new();
                if let Some(name) = node.child_by_field_name("name") {
                    self.rust_path_segments(name, source, &mut segments);
                }
                steps.push(ExpressionStep::New(segments.join("::")));
            }
            "string_literal" | "raw_string_literal" | "integer_literal" | "float_literal"
            | "boolean_literal" | "char_literal" | "array_expression" | "tuple_expression"
            | "macro_invocation" | "range_expression" | "closure_expression" => {
                steps.push(ExpressionStep::Literal)
            }
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    pub(super) fn extract_rust_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "use_declaration" => {
                // The use_declaration has an "argument" field containing the use tree
                if let Some(arg) = node.child_by_field_name("argument") {
                    self.extract_rust_use_tree(arg, source, imports, "");
                }
                return; // Don't recurse further for use declarations
            }
            "extern_crate_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let alias = node
                        .child_by_field_name("alias")
                        .map(|a| self.node_text(a, source).to_string());
                    imports.push(ImportInfo {
                        local_name: alias.unwrap_or_else(|| name.clone()),
                        source_module: name,
                        original_name: None,
                    });
                }
                return;
            }
            "mod_item" => {
                // `mod foo;` (no body) is a file-level import
                let has_body = node.child_by_field_name("body").is_some();
                if !has_body {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = self.node_text(name_node, source).to_string();
                        imports.push(ImportInfo {
                            local_name: name.clone(),
                            source_module: name,
                            original_name: None,
                        });
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_rust_imports(child, source, imports);
        }
    }

    /// Recursively extract imports from a Rust use tree.
    /// `prefix` accumulates the path prefix (e.g., "std::io").
    fn extract_rust_use_tree(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
        prefix: &str,
    ) {
        match node.kind() {
            "scoped_use_list" => {
                // e.g., `std::{io, fs}` or `std::io::{Read, Write}`
                // The scoped_use_list has a "path" field and a "list" field
                let path_node = node.child_by_field_name("path");
                let list_node = node.child_by_field_name("list");

                let new_prefix = if let Some(path) = path_node {
                    let path_text = self.node_text(path, source);
                    if prefix.is_empty() {
                        path_text.to_string()
                    } else {
                        format!("{}::{}", prefix, path_text)
                    }
                } else {
                    prefix.to_string()
                };

                if let Some(list) = list_node {
                    // list is a use_list containing multiple items
                    let cursor = &mut list.walk();
                    for child in list.children(cursor) {
                        self.extract_rust_use_tree(child, source, imports, &new_prefix);
                    }
                }
            }
            "use_as_clause" => {
                // e.g., `Read as IoRead`
                let path_node = node.child_by_field_name("path");
                let alias_node = node.child_by_field_name("alias");
                if let Some(path) = path_node {
                    let path_text = self.node_text(path, source).to_string();
                    let full_path = if prefix.is_empty() {
                        path_text.clone()
                    } else {
                        format!("{}::{}", prefix, path_text)
                    };
                    let local = alias_node
                        .map(|a| self.node_text(a, source).to_string())
                        .unwrap_or_else(|| {
                            path_text
                                .split("::")
                                .last()
                                .unwrap_or(&path_text)
                                .to_string()
                        });
                    imports.push(ImportInfo {
                        local_name: local,
                        source_module: full_path,
                        original_name: Some(path_text),
                    });
                }
            }
            "scoped_identifier" => {
                // e.g., `std::collections::HashMap`
                let full_text = self.node_text(node, source).to_string();
                let full_path = if prefix.is_empty() {
                    full_text.clone()
                } else {
                    format!("{}::{}", prefix, full_text)
                };
                let local_name = full_text
                    .split("::")
                    .last()
                    .unwrap_or(&full_text)
                    .to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: full_path,
                    original_name: None,
                });
            }
            "identifier" => {
                let name = self.node_text(node, source).to_string();
                let full_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{}::{}", prefix, name)
                };
                imports.push(ImportInfo {
                    local_name: name,
                    source_module: full_path,
                    original_name: None,
                });
            }
            "self" => {
                // `use std::io::{self, Read}` — the `self` imports the parent module
                let local_name = prefix.split("::").last().unwrap_or(prefix).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: prefix.to_string(),
                    original_name: Some("self".to_string()),
                });
            }
            "use_wildcard" => {
                // `use std::io::*` — glob import
                imports.push(ImportInfo {
                    local_name: "*".to_string(),
                    source_module: if prefix.is_empty() {
                        "*".to_string()
                    } else {
                        format!("{}::*", prefix)
                    },
                    original_name: None,
                });
            }
            _ => {
                // For other node types (e.g., `crate`, `super`), treat as identifier
                let text = self.node_text(node, source).to_string();
                if !text.is_empty() && text != "use" && text != "::" && text != ";" {
                    let full_path = if prefix.is_empty() {
                        text.clone()
                    } else {
                        format!("{}::{}", prefix, text)
                    };
                    let local_name = text.split("::").last().unwrap_or(&text).to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: full_path,
                        original_name: None,
                    });
                }
            }
        }
    }

    /// The attributes written directly above `node` (`#[...]`, comments
    /// between them skipped), in source order.
    fn rust_attributes(&self, node: Node, source: &[u8]) -> Vec<String> {
        let mut attributes = Vec::new();
        let Some(parent) = node.parent() else {
            return attributes;
        };
        let cursor = &mut parent.walk();
        let children: Vec<_> = parent.children(cursor).collect();
        let idx = children
            .iter()
            .position(|child| child.id() == node.id())
            .unwrap_or(children.len());
        for child in children[..idx].iter().rev() {
            if child.kind() == "attribute_item" {
                attributes.push(self.node_text(*child, source).trim().to_string());
            } else if child.kind() != "line_comment" && child.kind() != "block_comment" {
                break;
            }
        }
        attributes.reverse();
        attributes
    }

    /// Extract Rust metadata: visibility, attributes (decorators), return type.
    fn extract_rust_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Visibility modifier
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            if child.kind() == "visibility_modifier" {
                let vis_text = self.node_text(child, source).trim().to_string();
                meta.visibility = Some(if vis_text.starts_with("pub(crate)") {
                    Visibility::Internal
                } else if vis_text.starts_with("pub") {
                    Visibility::Public
                } else {
                    Visibility::Private
                });
                break;
            }
        }

        // Attributes (decorators): look at preceding siblings
        meta.decorators = self.rust_attributes(node, source);
        // A test function in a product file (#146); `#[cfg(test)]` modules
        // are marked where they are extracted.
        if meta
            .decorators
            .iter()
            .any(|a| classify::is_rust_test_attribute(a))
        {
            meta.test = true;
        }

        // Return type
        if node.kind() == "function_item" || node.kind() == "function_signature_item" {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                if !text.is_empty() {
                    meta.return_type = Some(text);
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

        if meta.visibility.is_none()
            && meta.decorators.is_empty()
            && meta.return_type.is_none()
            && meta.type_params.is_empty()
        {
            None
        } else {
            Some(meta)
        }
    }
}

/// The name a simple pattern binds: `x`, `mut x`, `ref x`; `None` for a
/// destructuring pattern.
fn rust_pattern_name(pattern: Node, source: &[u8]) -> Option<String> {
    let text = |n: Node| n.utf8_text(source).unwrap_or_default().to_string();
    match pattern.kind() {
        "identifier" => Some(text(pattern)),
        "mut_pattern" | "ref_pattern" => {
            let cursor = &mut pattern.walk();
            let inner = pattern
                .named_children(cursor)
                .find(|c| c.kind() == "identifier");
            inner.map(text)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{ExpressionStep, SourceLanguage, SourceParser};
    use myceliums_storage::{SymbolKind, Visibility};

    // --- Rust tests ---

    /// `#[cfg(test)]` modules and everything in them, and functions with a
    /// test attribute, are test code (#146); the rest of the file is not.
    #[test]
    fn rust_test_code_is_marked() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
pub fn product() {}

#[tokio::test]
async fn async_check() {}

#[cfg(not(test))]
mod real {
    pub fn shipped() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> u32 { 1 }

    struct Fixture;

    impl Fixture {
        fn build() -> Self { Fixture }
    }

    #[test]
    fn checks() { assert_eq!(helper(), 1); }
}
"#,
            )
            .unwrap();
        let is_test = |name: &str| {
            let symbol = result
                .symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol {name}"));
            symbol.metadata.as_ref().is_some_and(|m| m.test)
        };
        for name in [
            "async_check",
            "tests",
            "helper",
            "Fixture",
            "build",
            "checks",
        ] {
            assert!(is_test(name), "{name} should be test code");
        }
        for name in ["product", "real", "shipped"] {
            assert!(!is_test(name), "{name} should be product code");
        }
    }

    #[test]
    fn test_parse_rust_function() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("fn process(input: &str) -> Result<String> { Ok(input.to_string()) }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "process");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_rust_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("struct Server { port: u16, host: String }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Server");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_rust_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("enum Status { Active, Inactive, Error(String) }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Status");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_rust_trait() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
trait Handler {
    fn handle(&self, req: Request) -> Response;
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Handler"));
        assert!(names.contains(&"handle"));
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "Handler")
                .unwrap()
                .kind,
            SymbolKind::Interface
        );
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "handle")
                .unwrap()
                .kind,
            SymbolKind::Method
        );
    }

    #[test]
    fn test_parse_rust_impl_block() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
struct Server { port: u16 }

impl Server {
    fn new(port: u16) -> Self { Server { port } }
    fn start(&self) -> bool { true }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"new"));
        assert!(names.contains(&"start"));
        // Methods inside impl should have parent_name set
        let new_sym = result.symbols.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new_sym.kind, SymbolKind::Method);
        assert_eq!(new_sym.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_rust_trait_impl() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
impl Handler for Server {
    fn handle(&self) -> bool { true }
}
"#,
            )
            .unwrap();
        let handle = result.symbols.iter().find(|s| s.name == "handle").unwrap();
        assert_eq!(handle.kind, SymbolKind::Method);
        assert_eq!(handle.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_rust_const_static() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
const MAX_SIZE: usize = 1024;
static COUNTER: u32 = 0;
"#,
            )
            .unwrap();
        let max_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MAX_SIZE")
            .unwrap();
        assert_eq!(max_sym.kind, SymbolKind::Constant);
        let counter_sym = result.symbols.iter().find(|s| s.name == "COUNTER").unwrap();
        assert_eq!(counter_sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn test_parse_rust_type_alias() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("type Result<T> = std::result::Result<T, Error>;")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Result");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_rust_macro_definition() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
macro_rules! my_macro {
    ($x:expr) => { println!("{}", $x) };
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "my_macro");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_rust_module() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("mod utils { fn helper() {} }").unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"utils"));
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.name == "utils")
                .unwrap()
                .kind,
            SymbolKind::Module
        );
    }

    #[test]
    fn test_call_extraction_rust() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
fn main() {
    let result = process(&input);
    println!("done");
    let v = Vec::new();
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "process"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
        assert!(result.calls.iter().any(|c| c.callee_name == "new"));
    }

    #[test]
    fn test_import_extraction_rust() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse(
                r#"
use std::collections::HashMap;
use std::{io, fs};
use crate::parser::SourceLanguage;
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "HashMap"));
        assert!(result.imports.iter().any(|i| i.local_name == "io"));
        assert!(result.imports.iter().any(|i| i.local_name == "fs"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "SourceLanguage"));
    }

    #[test]
    fn test_import_extraction_rust_mod_declaration() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("mod parser;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "parser");
    }

    #[test]
    fn test_rust_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser.parse("fn foo() { obj.method(); }").unwrap();
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
    fn test_rust_pub_function_has_visibility() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("pub fn serve(port: u16) -> Result<()> { Ok(()) }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "serve")
            .expect("should find serve");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert_eq!(meta.visibility, Some(Visibility::Public));
    }

    #[test]
    fn test_rust_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::Rust).unwrap();
        let result = parser
            .parse("fn compute(x: i32) -> Vec<String> { vec![] }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "compute")
            .expect("should find compute");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
    }
}

/// What the resolver needs from Rust (#139).
#[cfg(test)]
mod resolution_facts {
    use crate::parser::{BindingValue, CallForm, ExpressionStep, SourceLanguage, SourceParser};

    const SOURCE: &str = r#"
pub struct UserService {
    validator: Validator,
}

impl Config {
    pub fn load(text: &str, repo: &mut Repo<User>) -> Self {
        let mut users = UserService::new(Repo::<User>::new());
        let handler = |q: &str| q.len();
        println!("{}", run(self.render(&text)));
        Self::new(parse(text))
    }
}
"#;

    fn parse() -> crate::parser::ParseResult {
        SourceParser::new(SourceLanguage::Rust)
            .unwrap()
            .parse(SOURCE)
            .unwrap()
    }

    #[test]
    fn paths_and_macro_arguments_are_calls() {
        let result = parse();
        let chain_of = |callee: &str, first: &str| {
            result
                .calls
                .iter()
                .find(|c| {
                    c.callee_name == callee
                        && c.chain.as_deref().and_then(|ch| ch.first())
                            == Some(&ExpressionStep::Ident(first.to_string()))
                })
                .map(|c| c.form)
        };
        assert_eq!(chain_of("new", "UserService"), Some(CallForm::Member));
        assert_eq!(
            chain_of("new", "Repo"),
            Some(CallForm::Member),
            "turbofish dropped"
        );
        assert_eq!(chain_of("new", "Self"), Some(CallForm::Member));
        let run = result
            .calls
            .iter()
            .find(|c| c.callee_name == "run")
            .unwrap();
        assert_eq!(run.form, CallForm::Bare, "a call inside println!");
        let render = result
            .calls
            .iter()
            .find(|c| c.callee_name == "render")
            .unwrap();
        assert_eq!(render.chain.as_deref().unwrap()[0], ExpressionStep::This);
    }

    #[test]
    fn parameters_lets_and_struct_fields_are_bound() {
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
        assert_eq!(
            find("repo", None),
            BindingValue::Type("&mut Repo<User>".into())
        );
        assert_eq!(
            find("users", None),
            BindingValue::Expr(vec![
                ExpressionStep::Ident("UserService".into()),
                ExpressionStep::Field("new".into()),
                ExpressionStep::Call
            ])
        );
        assert_eq!(find("handler", None), BindingValue::Unknown);
        assert_eq!(find("q", None), BindingValue::Type("&str".into()));
    }

    #[test]
    fn statics_and_consts_are_typed_bindings() {
        let result = SourceParser::new(SourceLanguage::Rust)
            .unwrap()
            .parse("static REGISTRY: Registry = Registry::new();\nconst LIMIT: usize = 3;\n")
            .unwrap();
        let registry = result
            .bindings
            .iter()
            .find(|b| b.name == "REGISTRY")
            .unwrap();
        assert_eq!(registry.value, BindingValue::Type("Registry".into()));
        assert_eq!(registry.scope, crate::parser::FILE_SCOPE);
        assert!(result.bindings.iter().any(|b| b.name == "LIMIT"));
    }

    #[test]
    fn a_trait_impl_records_the_trait_as_a_base_of_the_type() {
        let result = SourceParser::new(SourceLanguage::Rust)
            .unwrap()
            .parse("impl Greeter for Bot {}\nimpl<T> fmt::Display for Wrapper<T> {}\nimpl Bot {}\n")
            .unwrap();
        let declared: Vec<(&str, u32, &[String], bool)> = result
            .declared
            .iter()
            .map(|d| (d.name.as_str(), d.line, d.bases.as_slice(), d.elsewhere))
            .collect();
        assert_eq!(
            declared,
            [
                ("Bot", 1, &["Greeter".to_string()][..], true),
                ("Wrapper", 2, &["fmt::Display".to_string()][..], true),
            ]
        );
    }
}
