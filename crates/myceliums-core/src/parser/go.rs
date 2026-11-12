//! Go extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
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
                        metadata: None,
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
                                    let type_text = self.node_text(type_node, source);
                                    // Strip pointer prefix
                                    let cleaned = type_text.trim_start_matches('*');
                                    return Some(cleaned.to_string());
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
                        metadata: None,
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
                            symbols.push(ParsedSymbol {
                                name,
                                kind: sym_kind,
                                start_line: child.start_position().row as u32 + 1,
                                end_line: child.end_position().row as u32 + 1,
                                signature: self.extract_go_signature(child, source),
                                content: self.node_text(child, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
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
            "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "method_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "selector_expression" => {
                            if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    if let Some(caller) = scope {
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
            "go_statement" => {
                // go handleRequest(conn) — extract the call inside
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let callee = match func_node.kind() {
                                "identifier" => self.node_text(func_node, source).to_string(),
                                "selector_expression" => {
                                    if let Some(field) = func_node.child_by_field_name("field") {
                                        self.node_text(field, source).to_string()
                                    } else {
                                        self.node_text(func_node, source).to_string()
                                    }
                                }
                                _ => self.node_text(func_node, source).to_string(),
                            };
                            if let Some(caller) = scope {
                                calls.push(CallReference {
                                    caller_name: caller.to_string(),
                                    callee_name: callee,
                                    line: child.start_position().row as u32 + 1,
                                    chain: None,
                                    file: None,
                                });
                            }
                        }
                    }
                }
                return; // Don't recurse further (already handled the call)
            }
            "defer_statement" => {
                // defer file.Close() — extract the call inside
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let callee = match func_node.kind() {
                                "identifier" => self.node_text(func_node, source).to_string(),
                                "selector_expression" => {
                                    if let Some(field) = func_node.child_by_field_name("field") {
                                        self.node_text(field, source).to_string()
                                    } else {
                                        self.node_text(func_node, source).to_string()
                                    }
                                }
                                _ => self.node_text(func_node, source).to_string(),
                            };
                            if let Some(caller) = scope {
                                calls.push(CallReference {
                                    caller_name: caller.to_string(),
                                    callee_name: callee,
                                    line: child.start_position().row as u32 + 1,
                                    chain: None,
                                    file: None,
                                });
                            }
                        }
                    }
                }
                return; // Don't recurse further
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_go_calls(child, source, calls, scope);
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
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Handler");
        assert_eq!(result.symbols[0].kind, SymbolKind::Interface);
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
