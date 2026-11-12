//! .NET-family extraction: C# and PowerShell.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- C# ---

    pub(super) fn extract_csharp_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" | "struct_declaration" | "record_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with parent set
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_csharp_symbols(body, source, symbols, Some(&name));
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
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body for method signatures
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_csharp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
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
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "constructor_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Variable,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "delegate_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_csharp_signature(node, source),
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
            self.extract_csharp_symbols(child, source, symbols, parent);
        }
    }

    fn extract_csharp_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For C#, the signature is everything before the opening brace
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            // For items without a body (e.g., interface method signatures ending with `;`)
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_csharp_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "method_declaration" | "constructor_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "invocation_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "member_access_expression" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "member_binding_expression" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
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
            "object_creation_expression" => {
                // new Foo(...) — extract the type name as callee
                if let Some(type_node) = node.child_by_field_name("type") {
                    let callee = self.node_text(type_node, source).to_string();
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
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_csharp_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_csharp_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "using_directive" {
            // using System.Collections.Generic;
            // using Alias = Some.Namespace;
            let cursor = &mut node.walk();
            let mut module = String::new();
            let mut alias: Option<String> = None;

            for child in node.children(cursor) {
                match child.kind() {
                    "name_equals" => {
                        // This is the alias part: `using Alias = ...`
                        if let Some(name_node) = child.child_by_field_name("name") {
                            alias = Some(self.node_text(name_node, source).to_string());
                        }
                    }
                    "qualified_name" | "identifier" | "alias_qualified_name" => {
                        module = self.node_text(child, source).to_string();
                    }
                    _ => {}
                }
            }

            if !module.is_empty() {
                let local_name = if let Some(ref a) = alias {
                    a.clone()
                } else {
                    // Derive local name from the last segment of the qualified name
                    module.rsplit('.').next().unwrap_or(&module).to_string()
                };
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: alias,
                });
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_csharp_imports(child, source, imports);
        }
    }

    // --- PowerShell ---

    fn extract_powershell_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_powershell_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_statement" | "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "class_statement" | "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_powershell_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_statement" | "enum_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_powershell_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_powershell_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "function_statement" | "function_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "command_expression" | "command" => {
                // PowerShell command calls like: Get-Process, Invoke-WebRequest
                if let Some(cmd_node) = node.child_by_field_name("name") {
                    let callee = self.node_text(cmd_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(cmd_node) = node.child(0) {
                    // Fallback: first child is the command name
                    let callee = self.node_text(cmd_node, source).to_string();
                    if !callee.is_empty() {
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
            }
            "command_invocation_expression" => {
                if let Some(cmd_node) = node.child(0) {
                    let callee = self.node_text(cmd_node, source).to_string();
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
            "invocation_expression" => {
                // Method-style calls: [System.IO.File]::ReadAllText() or $obj.Method()
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
                } else if let Some(member_node) = node.child_by_field_name("member") {
                    let callee = self.node_text(member_node, source).to_string();
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
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_powershell_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "import_statement" => {
                if let Some(module_node) = node.child_by_field_name("module_name") {
                    let module = self.node_text(module_node, source).to_string();
                    if !module.is_empty() {
                        imports.push(ImportInfo {
                            local_name: module.clone(),
                            source_module: module,
                            original_name: None,
                        });
                    }
                }
                return;
            }
            "using_statement" => {
                // using module ModuleName; or using namespace System.IO;
                let text = self.node_text(node, source).to_string();
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() >= 3 {
                    let module = parts[2].trim_end_matches(';').to_string();
                    if !module.is_empty() {
                        let local_name =
                            module.split('.').next_back().unwrap_or(&module).to_string();
                        imports.push(ImportInfo {
                            local_name,
                            source_module: module,
                            original_name: Some(parts[1].to_string()),
                        });
                    }
                }
                return;
            }
            "command_expression" | "command" => {
                // Import-Module ModuleName
                if let Some(cmd_node) = node.child(0) {
                    let cmd_name = self.node_text(cmd_node, source);
                    if cmd_name == "Import-Module" {
                        // The module name is typically the second child or argument
                        if let Some(arg_node) = node.child(1) {
                            let module = self.node_text(arg_node, source).to_string();
                            let module = module.trim().to_string();
                            if !module.is_empty() {
                                imports.push(ImportInfo {
                                    local_name: module.clone(),
                                    source_module: module,
                                    original_name: None,
                                });
                            }
                        }
                        return;
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_powershell_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- C# tests ---

    #[test]
    fn test_parse_csharp_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public class UserService
{
    public string Name { get; set; }

    public UserService(string name)
    {
        Name = name;
    }

    public string GetUser(string id)
    {
        return null;
    }

    public void DeleteUser(string id)
    {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"GetUser"));
        assert!(names.contains(&"DeleteUser"));
        assert!(names.contains(&"Name"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "UserService" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let get_user = result.symbols.iter().find(|s| s.name == "GetUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));

        let name_prop = result.symbols.iter().find(|s| s.name == "Name").unwrap();
        assert_eq!(name_prop.kind, SymbolKind::Variable);
        assert_eq!(name_prop.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_csharp_interface() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public interface IHandler
{
    void Handle(Request req);
    string GetName();
}
"#,
            )
            .unwrap();
        let handler = result
            .symbols
            .iter()
            .find(|s| s.name == "IHandler")
            .unwrap();
        assert_eq!(handler.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_csharp_enum() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public enum Status
{
    Active,
    Inactive,
    Error
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Status");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_csharp_struct() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public struct Point
{
    public int X { get; set; }
    public int Y { get; set; }
}
"#,
            )
            .unwrap();
        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_csharp_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
namespace MyApp.Services
{
    public class Foo { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MyApp.Services"));
        let ns = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Services")
            .unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_csharp_delegate() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse("public delegate void EventHandler(object sender, EventArgs e);")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "EventHandler");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_csharp() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
public class App
{
    public void Run()
    {
        var result = ProcessData(input);
        Console.WriteLine(result);
        var svc = new UserService();
    }
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "ProcessData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "WriteLine"));
        assert!(result.calls.iter().any(|c| c.callee_name == "UserService"));
    }

    #[test]
    fn test_import_extraction_csharp() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse(
                r#"
using System;
using System.Collections.Generic;
using System.Linq;
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "System" && i.source_module == "System"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "Generic" && i.source_module == "System.Collections.Generic"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "Linq" && i.source_module == "System.Linq"));
    }

    #[test]
    fn test_parse_csharp_record() {
        let mut parser = SourceParser::new(SourceLanguage::CSharp).unwrap();
        let result = parser
            .parse("public record Person(string Name, int Age);")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Person");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }
}
