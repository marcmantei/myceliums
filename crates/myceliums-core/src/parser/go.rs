//! Go extraction.

use super::{
    BindingValue, CallForm, CallReference, ExpressionStep, ImportInfo, ParsedSymbol, SourceParser,
    TypeBinding, FILE_SCOPE,
};
use myceliums_storage::{SymbolKind, SymbolMetadata};
use tree_sitter::Node;

impl SourceParser {
    // --- Go ---

    pub(super) fn extract_go_symbols(
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
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_go_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.go_function_metadata(node, source),
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    // Extract receiver type as parent
                    let receiver_type = node.child_by_field_name("receiver").and_then(|recv| {
                        // receiver is a parameter_list; find the type inside
                        let cursor = &mut recv.walk();
                        for child in recv.children(cursor) {
                            if child.kind() == "parameter_declaration" {
                                if let Some(type_node) = child.child_by_field_name("type") {
                                    // `*Repo[T]` → `Repo`
                                    return Some(go_type_name(self.node_text(type_node, source)));
                                }
                            }
                        }
                        None
                    });
                    let parent_name = receiver_type.or_else(|| parent.map(|s| s.to_string()));
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_go_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name,
                        metadata: self.go_function_metadata(node, source),
                    });
                }
            }
            "type_declaration" => {
                // type_declaration contains type_spec children
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "type_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            let type_node = child.child_by_field_name("type");
                            let sym_kind = match type_node.map(|t| t.kind()) {
                                Some("struct_type") => SymbolKind::Class,
                                Some("interface_type") => SymbolKind::Interface,
                                _ => SymbolKind::TypeAlias,
                            };
                            // Embedded fields: their methods are promoted.
                            let embedded = type_node
                                .filter(|t| t.kind() == "struct_type")
                                .map(|t| self.go_embedded_types(t, source))
                                .unwrap_or_default();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: sym_kind,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self.extract_go_signature(child, source),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: (!embedded.is_empty()).then(|| SymbolMetadata {
                                    superclasses: embedded,
                                    ..SymbolMetadata::default()
                                }),
                            });
                            // An interface's method set.
                            if let Some(interface) =
                                type_node.filter(|t| t.kind() == "interface_type")
                            {
                                let cursor = &mut interface.walk();
                                for elem in interface.children(cursor) {
                                    let Some(method) = (elem.kind() == "method_elem")
                                        .then(|| elem.child_by_field_name("name"))
                                        .flatten()
                                    else {
                                        continue;
                                    };
                                    symbols.push(ParsedSymbol {
                                        name: self.node_text(method, source).to_string(),
                                        kind: SymbolKind::Method,
                                        start_line: elem.start_position().row as u32 + 1,
                                        end_line: elem.end_position().row as u32 + 1,
                                        signature: self.node_text(elem, source).trim().to_string(),
                                        content: self.node_text(elem, source).to_string(),
                                        parent_name: Some(name.clone()),
                                        metadata: self.go_function_metadata(elem, source),
                                    });
                                }
                            }
                        }
                    }
                }
                return; // Don't recurse further
            }
            "const_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "const_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(child, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            "var_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "var_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Variable,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self
                                    .node_text(child, source)
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_symbols(child, source, symbols, parent);
        }
    }

    /// The declared result of a Go function or method (`*Config`,
    /// `(*Config, error)`), if any.
    fn go_function_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let result = node.child_by_field_name("result")?;
        Some(SymbolMetadata {
            return_type: Some(self.node_text(result, source).trim().to_string()),
            ..SymbolMetadata::default()
        })
    }

    /// The types a struct embeds (`struct { *Report; ... }`).
    fn go_embedded_types(&self, struct_type: Node, source: &[u8]) -> Vec<String> {
        let mut embedded = Vec::new();
        let Some(list) = self.find_child_by_kind(struct_type, "field_declaration_list") else {
            return embedded;
        };
        let cursor = &mut list.walk();
        for field in list.children(cursor) {
            if field.kind() == "field_declaration" && field.child_by_field_name("name").is_none() {
                if let Some(ty) = field.child_by_field_name("type") {
                    embedded.push(go_type_name(self.node_text(ty, source)));
                }
            }
        }
        embedded
    }

    fn extract_go_signature(&self, node: Node, source: &[u8]) -> String {
        let start = node.start_byte();
        let text = &source[start..];
        if let Some(pos) = text.iter().position(|&b| b == b'{') {
            let sig = std::str::from_utf8(&text[..pos]).unwrap_or("");
            sig.trim().to_string()
        } else {
            let first_line = self.node_text(node, source);
            first_line.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_go_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_declaration" | "method_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        // Calls in `go f()` and `defer f()` are ordinary call_expressions
        // below those statements.
        if kind == "call_expression" {
            if let (Some(func_node), Some(caller)) = (node.child_by_field_name("function"), scope) {
                let (callee, chain, form) = match func_node.kind() {
                    "identifier" => (
                        self.node_text(func_node, source).to_string(),
                        None,
                        CallForm::Bare,
                    ),
                    // `pkg.F()`, `value.Method()`
                    "selector_expression" => {
                        let name = func_node
                            .child_by_field_name("field")
                            .map(|f| self.node_text(f, source).to_string())
                            .unwrap_or_else(|| self.node_text(func_node, source).to_string());
                        let mut steps = Vec::new();
                        if let Some(operand) = func_node.child_by_field_name("operand") {
                            self.collect_go_chain_steps(operand, source, &mut steps);
                        }
                        steps.push(ExpressionStep::Field(name.clone()));
                        steps.push(ExpressionStep::Call);
                        (name, Some(steps), CallForm::Member)
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

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_calls(child, source, calls, scope);
        }
    }

    /// Receivers, parameters, variables and struct fields with their
    /// declared types or initialisers (#139).
    pub(super) fn extract_go_bindings(
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
        let mut push =
            |name: String, owner: Option<String>, scope: (u32, u32), value: BindingValue| {
                if name != "_" {
                    out.push(TypeBinding {
                        name,
                        owner,
                        scope,
                        value,
                    });
                }
            };
        match node.kind() {
            "type_spec" => {
                let name = node.child_by_field_name("name").map(text);
                let fields = node
                    .child_by_field_name("type")
                    .filter(|t| t.kind() == "struct_type")
                    .and_then(|t| self.find_child_by_kind(t, "field_declaration_list"));
                if let (Some(name), Some(fields)) = (name, fields) {
                    let cursor = &mut fields.walk();
                    for field in fields.named_children(cursor) {
                        let Some(ty) = field.child_by_field_name("type") else {
                            continue;
                        };
                        let cursor = &mut field.walk();
                        for field_name in field.children_by_field_name("name", cursor) {
                            push(
                                text(field_name),
                                Some(name.clone()),
                                FILE_SCOPE,
                                BindingValue::Type(text(ty)),
                            );
                        }
                    }
                }
                return;
            }
            "function_declaration" | "method_declaration" | "func_literal" => {
                let span = lines(node);
                for list in ["receiver", "parameters"] {
                    let Some(params) = node.child_by_field_name(list) else {
                        continue;
                    };
                    let cursor = &mut params.walk();
                    for param in params.named_children(cursor) {
                        let ty = param.child_by_field_name("type").map(text);
                        let variadic = param.kind() == "variadic_parameter_declaration";
                        let cursor = &mut param.walk();
                        for name in param.children_by_field_name("name", cursor) {
                            let value = match (&ty, variadic) {
                                (Some(ty), false) => BindingValue::Type(ty.clone()),
                                _ => BindingValue::Unknown,
                            };
                            push(text(name), None, span, value);
                        }
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.extract_go_bindings(body, source, out, Some(span));
                }
                return;
            }
            "short_var_declaration" | "var_spec" | "range_clause" => {
                let names: Vec<Node> = match node.kind() {
                    "var_spec" => {
                        let cursor = &mut node.walk();
                        node.children_by_field_name("name", cursor).collect()
                    }
                    _ => node
                        .child_by_field_name("left")
                        .map(|left| {
                            let cursor = &mut left.walk();
                            left.named_children(cursor)
                                .filter(|n| n.kind() == "identifier")
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let values: Vec<Node> = match node.kind() {
                    "range_clause" => Vec::new(),
                    _ => node
                        .child_by_field_name(if node.kind() == "var_spec" {
                            "value"
                        } else {
                            "right"
                        })
                        .map(|right| {
                            let cursor = &mut right.walk();
                            right.named_children(cursor).collect()
                        })
                        .unwrap_or_default(),
                };
                let declared = node.child_by_field_name("type").map(text);
                for (i, name) in names.iter().enumerate() {
                    // `a, err := f()`: the first name gets the call's
                    // (first) result.
                    let value = match (&declared, values.get(i), values.len()) {
                        (Some(ty), _, _) => BindingValue::Type(ty.clone()),
                        (None, Some(value), n) if n == names.len() => {
                            self.go_binding_value(*value, source)
                        }
                        (None, _, 1) if i == 0 => self.go_binding_value(values[0], source),
                        _ => BindingValue::Unknown,
                    };
                    push(text(*name), None, local_scope(node), value);
                }
            }
            _ => {}
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_bindings(child, source, out, function);
        }
    }

    /// What an initialiser is, for a binding.
    fn go_binding_value(&self, value: Node, source: &[u8]) -> BindingValue {
        match value.kind() {
            "func_literal" => BindingValue::Unknown,
            _ => {
                let mut steps = Vec::new();
                self.collect_go_chain_steps(value, source, &mut steps);
                BindingValue::Expr(steps)
            }
        }
    }

    /// Chain steps of a Go expression used as a receiver.
    pub(super) fn collect_go_chain_steps(
        &self,
        node: Node,
        source: &[u8],
        steps: &mut Vec<ExpressionStep>,
    ) {
        match node.kind() {
            "selector_expression" => {
                if let Some(operand) = node.child_by_field_name("operand") {
                    self.collect_go_chain_steps(operand, source, steps);
                }
                if let Some(field) = node.child_by_field_name("field") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(field, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(function) = node.child_by_field_name("function") {
                    self.collect_go_chain_steps(function, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => steps.push(ExpressionStep::Ident(
                self.node_text(node, source).to_string(),
            )),
            "unary_expression" | "parenthesized_expression" => {
                match node
                    .child_by_field_name("operand")
                    .or_else(|| node.named_child(0))
                {
                    Some(inner) => self.collect_go_chain_steps(inner, source, steps),
                    None => steps.push(ExpressionStep::Literal),
                }
            }
            "index_expression" => {
                if let Some(operand) = node.child_by_field_name("operand") {
                    self.collect_go_chain_steps(operand, source, steps);
                }
                steps.push(ExpressionStep::Index);
            }
            // `T{...}` and `x.(T)` are values of type T.
            "composite_literal" | "type_assertion_expression" => {
                let ty = node.child_by_field_name("type");
                match ty.map(|t| go_type_name(self.node_text(t, source))) {
                    Some(name) => steps.push(ExpressionStep::New(name)),
                    None => steps.push(ExpressionStep::Literal),
                }
            }
            "interpreted_string_literal"
            | "raw_string_literal"
            | "int_literal"
            | "float_literal"
            | "imaginary_literal"
            | "rune_literal"
            | "true"
            | "false"
            | "nil"
            | "func_literal" => steps.push(ExpressionStep::Literal),
            _ => steps.push(ExpressionStep::Ident(
                self.node_text(node, source).to_string(),
            )),
        }
    }

    pub(super) fn extract_go_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "import_declaration" {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "import_spec" {
                    self.extract_go_import_spec(child, source, imports);
                } else if child.kind() == "import_spec_list" {
                    let spec_cursor = &mut child.walk();
                    for spec in child.children(spec_cursor) {
                        if spec.kind() == "import_spec" {
                            self.extract_go_import_spec(spec, source, imports);
                        }
                    }
                }
            }
            return;
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_imports(child, source, imports);
        }
    }

    fn extract_go_import_spec(&self, node: Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        // import_spec can have: name (alias), path (the import path string)
        let path_node = node.child_by_field_name("path");
        let name_node = node.child_by_field_name("name");

        if let Some(path) = path_node {
            let module = self.node_text(path, source).trim_matches('"').to_string();

            // Derive the default local name from the last segment of the path
            let default_local = module.rsplit('/').next().unwrap_or(&module).to_string();

            let (local_name, original_name) = if let Some(alias) = name_node {
                let alias_text = self.node_text(alias, source).to_string();
                if alias_text == "." {
                    // Dot import: import . "fmt" — symbols available without qualifier
                    (default_local.clone(), Some(".".to_string()))
                } else {
                    (alias_text, Some(default_local))
                }
            } else {
                (default_local, None)
            };

            imports.push(ImportInfo {
                local_name,
                source_module: module,
                original_name,
            });
        }
    }
}

/// The type a Go type expression names, without pointers, slices of
/// pointers or generic arguments: `*store.Repo[*model.User]` → `store.Repo`.
/// A slice, array, map, channel or function type keeps its full text, so it
/// never matches a project type.
pub(super) fn go_type_name(text: &str) -> String {
    let text = text.trim().trim_start_matches('*').trim();
    if text.starts_with('[')
        || text.starts_with("map[")
        || text.starts_with("chan ")
        || text.starts_with("func")
    {
        return text.to_string();
    }
    text.split('[').next().unwrap_or(text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- Go tests ---

    #[test]
    fn test_parse_go_function() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func greet(name string) string {
    return "Hello " + name
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_go_struct_and_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type Server struct {
    Host string
    Port int
}

func (s *Server) Start() error {
    return nil
}

func (s Server) Address() string {
    return s.Host
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"Start"));
        assert!(names.contains(&"Address"));

        let server_sym = result.symbols.iter().find(|s| s.name == "Server").unwrap();
        assert_eq!(server_sym.kind, SymbolKind::Class);

        let start_sym = result.symbols.iter().find(|s| s.name == "Start").unwrap();
        assert_eq!(start_sym.kind, SymbolKind::Method);
        assert_eq!(start_sym.parent_name.as_deref(), Some("Server"));

        let addr_sym = result.symbols.iter().find(|s| s.name == "Address").unwrap();
        assert_eq!(addr_sym.kind, SymbolKind::Method);
        assert_eq!(addr_sym.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_go_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type Handler interface {
    ServeHTTP(w ResponseWriter, r *Request)
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 2);
        assert_eq!(result.symbols[0].name, "Handler");
        assert_eq!(result.symbols[0].kind, SymbolKind::Interface);
        // The interface's method set is extracted too (#139).
        assert_eq!(result.symbols[1].name, "ServeHTTP");
        assert_eq!(result.symbols[1].kind, SymbolKind::Method);
        assert_eq!(result.symbols[1].parent_name.as_deref(), Some("Handler"));
    }

    #[test]
    fn test_parse_go_const_and_var() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

const MaxRetries = 3

var DefaultTimeout = 30
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MaxRetries"));
        assert!(names.contains(&"DefaultTimeout"));

        let const_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MaxRetries")
            .unwrap();
        assert_eq!(const_sym.kind, SymbolKind::Constant);

        let var_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "DefaultTimeout")
            .unwrap();
        assert_eq!(var_sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn test_parse_go_type_alias() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

type UserID string
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "UserID");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_go() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func main() {
    result := processData(input)
    fmt.Println(result)
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Println"));
    }

    #[test]
    fn test_call_extraction_go_goroutine_and_defer() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

func serve() {
    go handleRequest(conn)
    defer file.Close()
}
"#,
            )
            .unwrap();
        assert!(result
            .calls
            .iter()
            .any(|c| c.callee_name == "handleRequest"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Close"));
    }

    #[test]
    fn test_import_extraction_go() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

import (
    "fmt"
    "net/http"
    h "net/http"
)
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "fmt"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "http" && i.source_module == "net/http"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "h" && i.source_module == "net/http"));
    }

    #[test]
    fn test_import_extraction_go_single() {
        let mut parser = SourceParser::new(SourceLanguage::Go).unwrap();
        let result = parser
            .parse(
                r#"
package main

import "fmt"
"#,
            )
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "fmt");
        assert_eq!(result.imports[0].source_module, "fmt");
    }
}

/// What the resolver needs from Go (#139).
#[cfg(test)]
mod resolution_facts {
    use crate::parser::{BindingValue, CallForm, ExpressionStep, SourceLanguage, SourceParser};
    use myceliums_storage::{SymbolKind, SymbolMetadata};

    const SOURCE: &str = r#"
package report

type Sink interface {
	Write(text string)
}

type OrderReport struct {
	*Report
	repo *store.Repo[*model.User]
}

func (r *Repo[T]) Add(row T) int { return 0 }

func Load(text string) (*Config, error) {
	cfg, err := config.Load(text)
	user := &model.User{Name: text}
	handler := func(q string) string { return q }
	return cfg.Get("a"), err
}
"#;

    fn parse() -> crate::parser::ParseResult {
        SourceParser::new(SourceLanguage::Go)
            .unwrap()
            .parse(SOURCE)
            .unwrap()
    }

    fn metadata(result: &crate::parser::ParseResult, name: &str) -> SymbolMetadata {
        result
            .symbols
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.metadata.clone())
            .unwrap_or_default()
    }

    #[test]
    fn symbols_carry_what_receiver_typing_needs() {
        let result = parse();
        let write = result.symbols.iter().find(|s| s.name == "Write").unwrap();
        assert_eq!(
            (write.kind.clone(), write.parent_name.as_deref()),
            (SymbolKind::Method, Some("Sink"))
        );
        let add = result.symbols.iter().find(|s| s.name == "Add").unwrap();
        assert_eq!(add.parent_name.as_deref(), Some("Repo"), "generic receiver");
        assert_eq!(metadata(&result, "OrderReport").superclasses, ["Report"]);
        assert_eq!(
            metadata(&result, "Load").return_type.as_deref(),
            Some("(*Config, error)")
        );
    }

    #[test]
    fn calls_and_bindings() {
        let result = parse();
        let load = result
            .calls
            .iter()
            .find(|c| c.callee_name == "Load")
            .unwrap();
        assert_eq!(load.form, CallForm::Member);
        assert_eq!(
            load.chain.as_deref().unwrap()[0],
            ExpressionStep::Ident("config".into())
        );
        let find = |name: &str, owner: Option<&str>| {
            result
                .bindings
                .iter()
                .find(|b| b.name == name && b.owner.as_deref() == owner)
                .unwrap_or_else(|| panic!("{name} not bound: {:?}", result.bindings))
                .value
                .clone()
        };
        assert_eq!(find("r", None), BindingValue::Type("*Repo[T]".into()));
        assert_eq!(
            find("repo", Some("OrderReport")),
            BindingValue::Type("*store.Repo[*model.User]".into())
        );
        assert!(matches!(find("cfg", None), BindingValue::Expr(_)));
        assert_eq!(find("err", None), BindingValue::Unknown);
        assert_eq!(
            find("user", None),
            BindingValue::Expr(vec![ExpressionStep::New("model.User".into())])
        );
        assert_eq!(find("handler", None), BindingValue::Unknown);
    }
}
