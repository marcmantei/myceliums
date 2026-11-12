//! PHP extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- PHP ---

    pub(super) fn extract_php_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
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
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "trait_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // Extract property names from property elements
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "property_element" {
                        let inner_cursor = &mut child.walk();
                        for inner in child.children(inner_cursor) {
                            if inner.kind() == "variable_name" {
                                let name = self.node_text(inner, source).to_string();
                                // Strip leading $ from PHP variable names
                                let name = name.trim_start_matches('$').to_string();
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
                    }
                }
                return; // Don't recurse further
            }
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_php_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into namespace body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_php_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "const_declaration" => {
                // const FOO = 1; or class const FOO = 1;
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "const_element" {
                        // The name is a child node with kind "name"
                        if let Some(name_node) = self.find_child_by_kind(child, "name") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Constant,
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
                }
                return; // Don't recurse further
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_symbols(child, source, symbols, parent);
        }
    }

    fn extract_php_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_php_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "method_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "function_call_expression" => {
                // Regular function calls: foo(), \Namespace\foo()
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "name" | "qualified_name" => self.node_text(func_node, source).to_string(),
                        _ => self.node_text(func_node, source).to_string(),
                    };
                    // Extract just the function name from qualified names
                    let callee = callee.rsplit('\\').next().unwrap_or(&callee).to_string();
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
            "member_call_expression" => {
                // $obj->method()
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
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
            "scoped_call_expression" => {
                // ClassName::staticMethod()
                if let Some(name_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(name_node, source).to_string();
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
            "object_creation_expression" => {
                // new Foo()
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "name" || child.kind() == "qualified_name" {
                        let class_name = self.node_text(child, source).to_string();
                        let class_name = class_name
                            .rsplit('\\')
                            .next()
                            .unwrap_or(&class_name)
                            .to_string();
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: class_name,
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                        break;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_php_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "namespace_use_declaration" => {
                // use App\Models\User;
                // use App\Models\User as UserModel;
                // use App\Models\{User, Post};
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "namespace_use_clause" {
                        let path = self.node_text(child, source).to_string();
                        // Check for alias (as)
                        let (source_module, local_name) =
                            if let Some(alias_node) = child.child_by_field_name("alias") {
                                let alias = self.node_text(alias_node, source).to_string();
                                // Get the qualified name (everything before " as ")
                                if let Some(name_node) = child.child(0) {
                                    let module = self.node_text(name_node, source).to_string();
                                    (module, alias)
                                } else {
                                    (path.clone(), path)
                                }
                            } else {
                                let local = path.rsplit('\\').next().unwrap_or(&path).to_string();
                                (path, local)
                            };
                        imports.push(ImportInfo {
                            local_name,
                            source_module,
                            original_name: None,
                        });
                    }
                    if child.kind() == "namespace_use_group" {
                        // Grouped use: use App\Models\{User, Post};
                        // Find prefix from the namespace_use_declaration
                        let prefix = node
                            .child(1)
                            .map(|n| self.node_text(n, source).to_string())
                            .unwrap_or_default();
                        let group_cursor = &mut child.walk();
                        for clause in child.children(group_cursor) {
                            if clause.kind() == "namespace_use_clause" {
                                let name = self.node_text(clause, source).to_string();
                                let full_path = if prefix.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{}\\{}", prefix, name)
                                };
                                let local = name.rsplit('\\').next().unwrap_or(&name).to_string();
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: full_path,
                                    original_name: None,
                                });
                            }
                        }
                    }
                }
                return;
            }
            "namespace_definition" => {
                // namespace App\Models;
                if let Some(name_node) = node.child_by_field_name("name") {
                    let ns_name = self.node_text(name_node, source).to_string();
                    imports.push(ImportInfo {
                        local_name: ns_name.clone(),
                        source_module: ns_name,
                        original_name: Some("namespace".to_string()),
                    });
                }
            }
            "expression_statement" => {
                // require/include/require_once/include_once calls
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "function_call_expression" {
                        if let Some(func_node) = child.child_by_field_name("function") {
                            let func_name = self.node_text(func_node, source);
                            if matches!(
                                func_name,
                                "require" | "include" | "require_once" | "include_once"
                            ) {
                                if let Some(args) = child.child_by_field_name("arguments") {
                                    let arg_text = self.node_text(args, source);
                                    let module = arg_text
                                        .trim_matches(|c| c == '(' || c == ')')
                                        .trim()
                                        .trim_matches('\'')
                                        .trim_matches('"')
                                        .to_string();
                                    if !module.is_empty() {
                                        let local_name = module
                                            .rsplit('/')
                                            .next()
                                            .unwrap_or(&module)
                                            .to_string();
                                        imports.push(ImportInfo {
                                            local_name,
                                            source_module: module,
                                            original_name: Some(func_name.to_string()),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    // require_expression, include_expression etc. are keyword-based
                    if matches!(
                        child.kind(),
                        "require_expression"
                            | "include_expression"
                            | "require_once_expression"
                            | "include_once_expression"
                    ) {
                        // The second child is typically the path expression
                        if let Some(path_node) = child.child(1) {
                            let module = self
                                .node_text(path_node, source)
                                .trim_matches('\'')
                                .trim_matches('"')
                                .to_string();
                            if !module.is_empty() {
                                let kind_name = child.kind().replace("_expression", "");
                                let local_name =
                                    module.rsplit('/').next().unwrap_or(&module).to_string();
                                imports.push(ImportInfo {
                                    local_name,
                                    source_module: module,
                                    original_name: Some(kind_name),
                                });
                            }
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_php_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- PHP tests ---

    #[test]
    fn test_parse_php_extension() {
        let lang = SourceLanguage::from_extension("php");
        assert_eq!(lang, Some(SourceLanguage::Php));
    }

    #[test]
    fn test_parse_php_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
class UserService {
    public string $name;

    public function getUser(string $id): ?User {
        return null;
    }

    public function deleteUser(string $id): void {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"UserService"),
            "missing UserService, got: {:?}",
            names
        );
        assert!(
            names.contains(&"getUser"),
            "missing getUser, got: {:?}",
            names
        );
        assert!(
            names.contains(&"deleteUser"),
            "missing deleteUser, got: {:?}",
            names
        );
        assert!(
            names.contains(&"name"),
            "missing property name, got: {:?}",
            names
        );

        // Methods should have UserService as parent
        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_php_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
interface Cacheable {
    public function cacheKey(): string;
}
"#,
            )
            .unwrap();
        let iface = result
            .symbols
            .iter()
            .find(|s| s.name == "Cacheable")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_php_trait() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
trait Timestamps {
    public function createdAt(): string {
        return $this->created_at;
    }
}
"#,
            )
            .unwrap();
        let trait_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "Timestamps")
            .unwrap();
        assert_eq!(trait_sym.kind, SymbolKind::Interface);
        let method = result
            .symbols
            .iter()
            .find(|s| s.name == "createdAt")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.parent_name.as_deref(), Some("Timestamps"));
    }

    #[test]
    fn test_parse_php_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
enum Status: string {
    case Active = 'active';
    case Inactive = 'inactive';
}
"#,
            )
            .unwrap();
        let status = result.symbols.iter().find(|s| s.name == "Status").unwrap();
        assert_eq!(status.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_php_function() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
function findUser(string $name): ?User {
    return null;
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "findUser");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_php_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
namespace App\Models;

class User {}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"App\\Models"),
            "missing namespace, got: {:?}",
            names
        );
        let ns = result
            .symbols
            .iter()
            .find(|s| s.name == "App\\Models")
            .unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_php_const() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
const MAX_RETRIES = 3;
"#,
            )
            .unwrap();
        let c = result
            .symbols
            .iter()
            .find(|s| s.name == "MAX_RETRIES")
            .unwrap();
        assert_eq!(c.kind, SymbolKind::Constant);
    }

    #[test]
    fn test_call_extraction_php() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
function main() {
    $user = new User("Alice", 30);
    $user->greet();
    User::create("Bob", 25);
    processData($user);
}
"#,
            )
            .unwrap();
        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callees.contains(&"User"),
            "missing new User, got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"greet"),
            "missing ->greet(), got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"create"),
            "missing ::create(), got: {:?}",
            callees
        );
        assert!(
            callees.contains(&"processData"),
            "missing processData(), got: {:?}",
            callees
        );
    }

    #[test]
    fn test_import_extraction_php() {
        let mut parser = SourceParser::new(SourceLanguage::Php).unwrap();
        let result = parser
            .parse(
                r#"<?php
namespace App\Models;

use App\Contracts\Renderable;
use App\Services\Logger as AppLogger;
"#,
            )
            .unwrap();
        // Namespace import
        assert!(
            result.imports.iter().any(|i| i.local_name == "App\\Models"
                && i.original_name.as_deref() == Some("namespace")),
            "missing namespace import, got: {:?}",
            result.imports
        );
        // Use statements
        assert!(
            result.imports.iter().any(|i| i.local_name == "Renderable"),
            "missing Renderable import, got: {:?}",
            result.imports
        );
        assert!(
            result.imports.iter().any(|i| i.local_name == "AppLogger"),
            "missing AppLogger import, got: {:?}",
            result.imports
        );
    }
}
