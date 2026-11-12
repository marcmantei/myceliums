//! Rust extraction.

use super::{CallReference, ExpressionStep, ImportInfo, ParsedSymbol, SourceParser};
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
                if let Some(func_node) = node.child_by_field_name("function") {
                    let (callee, chain) = match func_node.kind() {
                        "identifier" => (self.node_text(func_node, source).to_string(), None),
                        "scoped_identifier" => {
                            // e.g., `Vec::new()` or `std::io::stdin()`
                            let name = if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            };
                            (name, None)
                        }
                        "field_expression" => {
                            // e.g., `self.method()` via call_expression
                            let chain = self.build_rust_chain(func_node, source);
                            let name = if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            };
                            (name, chain)
                        }
                        "generic_function" => {
                            // e.g., `foo::<Type>()` — turbofish
                            if let Some(func_inner) = func_node.child_by_field_name("function") {
                                let (name, chain) = match func_inner.kind() {
                                    "identifier" => {
                                        (self.node_text(func_inner, source).to_string(), None)
                                    }
                                    "scoped_identifier" => {
                                        let n = if let Some(name) =
                                            func_inner.child_by_field_name("name")
                                        {
                                            self.node_text(name, source).to_string()
                                        } else {
                                            self.node_text(func_inner, source).to_string()
                                        };
                                        (n, None)
                                    }
                                    "field_expression" => {
                                        let c = self.build_rust_chain(func_inner, source);
                                        let n = if let Some(field) =
                                            func_inner.child_by_field_name("field")
                                        {
                                            self.node_text(field, source).to_string()
                                        } else {
                                            self.node_text(func_inner, source).to_string()
                                        };
                                        (n, c)
                                    }
                                    _ => (self.node_text(func_inner, source).to_string(), None),
                                };
                                (name, chain)
                            } else {
                                (self.node_text(func_node, source).to_string(), None)
                            }
                        }
                        _ => (self.node_text(func_node, source).to_string(), None),
                    };
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain,
                            file: None,
                        });
                    }
                }
            }
            "macro_invocation" => {
                // e.g., `println!("hello")`, `vec![1, 2, 3]`
                if let Some(macro_node) = node.child_by_field_name("macro") {
                    let macro_name = self.node_text(macro_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: macro_name,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
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

    fn collect_rust_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
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
        if let Some(parent) = node.parent() {
            let cursor2 = &mut parent.walk();
            let children: Vec<_> = parent.children(cursor2).collect();
            let mut idx = children.len();
            for (i, child) in children.iter().enumerate() {
                if child.id() == node.id() {
                    idx = i;
                    break;
                }
            }
            for i in (0..idx).rev() {
                if children[i].kind() == "attribute_item" {
                    let text = self.node_text(children[i], source).trim().to_string();
                    meta.decorators.push(text);
                } else if children[i].kind() != "line_comment"
                    && children[i].kind() != "block_comment"
                {
                    break;
                }
            }
            meta.decorators.reverse();
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

#[cfg(test)]
mod tests {
    use crate::parser::{ExpressionStep, SourceLanguage, SourceParser};
    use myceliums_storage::{SymbolKind, Visibility};

    // --- Rust tests ---

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
