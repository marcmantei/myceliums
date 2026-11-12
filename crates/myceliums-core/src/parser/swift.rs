//! Swift extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- Swift ---

    /// Determine the Swift declaration kind from the `declaration_kind` field.
    /// In tree-sitter-swift, class/struct/enum/extension all use `class_declaration`
    /// with different `declaration_kind` values.
    fn swift_declaration_kind<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
        node.child_by_field_name("declaration_kind")
            .map(move |dk| self.node_text(dk, source))
    }

    pub(super) fn extract_swift_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                // tree-sitter-swift uses class_declaration for class, struct, enum, and extension
                let decl_kind = self.swift_declaration_kind(node, source).unwrap_or("class");
                match decl_kind {
                    "class" | "struct" => {
                        if let Some(name_node) = node.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Class,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_swift_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            if let Some(body) = node.child_by_field_name("body") {
                                self.extract_swift_symbols(body, source, symbols, Some(&name));
                                return;
                            }
                        }
                    }
                    "enum" => {
                        if let Some(name_node) = node.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Enum,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_swift_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            if let Some(body) = node.child_by_field_name("body") {
                                self.extract_swift_symbols(body, source, symbols, Some(&name));
                                return;
                            }
                        }
                    }
                    "extension" => {
                        // extension TypeName { ... } — extract the type being extended
                        // name field contains the user_type being extended
                        let ext_name = node
                            .child_by_field_name("name")
                            .map(|n| self.node_text(n, source).to_string());
                        let effective_parent = ext_name.as_deref().or(parent);
                        if let Some(body) = node.child_by_field_name("body") {
                            self.extract_swift_symbols(body, source, symbols, effective_parent);
                            return;
                        }
                    }
                    _ => {}
                }
            }
            "protocol_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_swift_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_declaration" => {
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
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "protocol_function_declaration" => {
                // Protocol method declarations (no body)
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_swift_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // var/let declarations — name is in a "pattern" child with field name "name"
                if let Some(pattern_node) = node.child_by_field_name("name") {
                    // pattern_node is a `pattern` node containing a `simple_identifier`
                    let name = self.node_text(pattern_node, source).to_string();
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Variable,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self
                                .node_text(node, source)
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_string(),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
                return; // Don't recurse further
            }
            "typealias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self
                            .node_text(node, source)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
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
            self.extract_swift_symbols(child, source, symbols, parent);
        }
    }

    fn extract_swift_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_swift_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            // In tree-sitter-swift, call_expression has the callee as first child
            // and call_suffix as second child
            if let Some(first_child) = node.child(0) {
                let callee = match first_child.kind() {
                    "simple_identifier" => self.node_text(first_child, source).to_string(),
                    "navigation_expression" => {
                        // obj.method() — extract the suffix's simple_identifier
                        if let Some(suffix) = first_child.child_by_field_name("suffix") {
                            // navigation_suffix contains a simple_identifier with field "suffix"
                            if let Some(id) = suffix.child_by_field_name("suffix") {
                                self.node_text(id, source).to_string()
                            } else {
                                // Fallback: last simple_identifier in the navigation expression
                                let nav_cursor = &mut first_child.walk();
                                let mut last_name = self.node_text(first_child, source).to_string();
                                for nav_child in first_child.children(nav_cursor) {
                                    if nav_child.kind() == "simple_identifier" {
                                        last_name = self.node_text(nav_child, source).to_string();
                                    }
                                }
                                last_name
                            }
                        } else {
                            self.node_text(first_child, source).to_string()
                        }
                    }
                    _ => self.node_text(first_child, source).to_string(),
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

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_swift_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_swift_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "import_declaration" {
            // import Foundation
            // import UIKit.UIViewController
            // The import_declaration has an `identifier` child with the module path
            let cursor = &mut node.walk();
            let mut module = String::new();
            for child in node.children(cursor) {
                if child.kind() == "identifier" {
                    module = self.node_text(child, source).to_string();
                }
            }
            if !module.is_empty() {
                let local_name = module.rsplit('.').next().unwrap_or(&module).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: None,
                });
            }
            return;
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_swift_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- Swift tests ---

    #[test]
    fn test_parse_swift_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    func getUser(id: String) -> String {
        return ""
    }
    func deleteUser(id: String) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_swift_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
struct Point {
    var x: Int
    var y: Int
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"x"));
        assert!(names.contains(&"y"));

        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);

        let x = result.symbols.iter().find(|s| s.name == "x").unwrap();
        assert_eq!(x.kind, SymbolKind::Variable);
        assert_eq!(x.parent_name.as_deref(), Some("Point"));
    }

    #[test]
    fn test_parse_swift_protocol() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
protocol Repository {
    func findById(id: String) -> Any?
    func save(entity: Any) -> Any
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let repo = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(repo.kind, SymbolKind::Interface);

        let find = result
            .symbols
            .iter()
            .find(|s| s.name == "findById")
            .unwrap();
        assert_eq!(find.kind, SymbolKind::Method);
        assert_eq!(find.parent_name.as_deref(), Some("Repository"));
    }

    #[test]
    fn test_parse_swift_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
enum Color {
    case red
    case green
    case blue
}
"#,
            )
            .unwrap();
        let color = result.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_swift_typealias() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser.parse("typealias UserID = String").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "UserID");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_swift_extension() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
extension UserService {
    func getAllUsers() -> [User] {
        return []
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"getAllUsers"));

        let get_all = result
            .symbols
            .iter()
            .find(|s| s.name == "getAllUsers")
            .unwrap();
        assert_eq!(get_all.kind, SymbolKind::Method);
        assert_eq!(get_all.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_swift_top_level_function() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
func main() {
    print("Hello")
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "main");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
        assert_eq!(result.symbols[0].parent_name, None);
    }

    #[test]
    fn test_call_extraction_swift() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
func main() {
    let result = processData(input)
    print(result)
    service.getUser(id: "1")
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "print"));
        assert!(result.calls.iter().any(|c| c.callee_name == "getUser"));
    }

    #[test]
    fn test_import_extraction_swift() {
        let mut parser = SourceParser::new(SourceLanguage::Swift).unwrap();
        let result = parser
            .parse(
                r#"
import Foundation
import UIKit
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "Foundation"));
        assert!(result.imports.iter().any(|i| i.source_module == "UIKit"));
    }

    #[test]
    fn test_parse_swift_file_extension() {
        let lang = SourceLanguage::from_extension("swift");
        assert_eq!(lang, Some(SourceLanguage::Swift));
    }
}
