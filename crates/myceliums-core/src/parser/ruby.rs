//! Ruby extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- Ruby ---

    pub(super) fn extract_ruby_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ruby_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "module" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into module body with module name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ruby_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "method" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "singleton_method" => {
                // def self.foo — class/static methods
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_ruby_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "assignment" => {
                // Top-level constant assignments like `MAX_SIZE = 100`
                if parent.is_none() {
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        if child.kind() == "constant" {
                            let name = self.node_text(child, source).to_string();
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
                                parent_name: None,
                                metadata: None,
                            });
                            break;
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
            self.extract_ruby_symbols(child, source, symbols, parent);
        }
    }

    fn extract_ruby_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // Ruby uses `end` keyword, so signature is the first line
        text.lines().next().unwrap_or("").trim().to_string()
    }

    pub(super) fn extract_ruby_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method" | "singleton_method" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call" => {
                // obj.method or method(args)
                if let Some(method_node) = node.child_by_field_name("method") {
                    let callee = self.node_text(method_node, source).to_string();
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
            "method_call" => {
                // method_call is for bare method calls like `puts "hello"`
                if let Some(method_node) = node.child_by_field_name("method") {
                    let callee = self.node_text(method_node, source).to_string();
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
            "super" => {
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: "super".to_string(),
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
            "yield" => {
                if let Some(caller) = scope {
                    calls.push(CallReference {
                        caller_name: caller.to_string(),
                        callee_name: "yield".to_string(),
                        line: node.start_position().row as u32 + 1,
                        chain: None,
                        file: None,
                    });
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ruby_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_ruby_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        // Ruby imports: require, require_relative, include, extend, prepend
        if node.kind() == "call" || node.kind() == "method_call" {
            let method_node = node.child_by_field_name("method");
            if let Some(method) = method_node {
                let method_name = self.node_text(method, source);
                match method_name {
                    "require" | "require_relative" => {
                        // Extract the argument (string)
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let cursor = &mut args.walk();
                            for arg in args.children(cursor) {
                                if arg.kind() == "string" || arg.kind() == "string_content" {
                                    let module = self.node_text(arg, source);
                                    let module =
                                        module.trim_matches('\'').trim_matches('"').to_string();
                                    if !module.is_empty() {
                                        let local_name = module
                                            .rsplit('/')
                                            .next()
                                            .unwrap_or(&module)
                                            .to_string();
                                        imports.push(ImportInfo {
                                            local_name,
                                            source_module: module,
                                            original_name: Some(method_name.to_string()),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "include" | "extend" | "prepend" => {
                        // Mixin: include ModuleName
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let cursor = &mut args.walk();
                            for arg in args.children(cursor) {
                                if arg.kind() == "constant" || arg.kind() == "scope_resolution" {
                                    let module_name = self.node_text(arg, source).to_string();
                                    imports.push(ImportInfo {
                                        local_name: module_name.clone(),
                                        source_module: module_name,
                                        original_name: Some(method_name.to_string()),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ruby_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    #[test]
    fn test_parse_ruby_class_and_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class User
  def initialize(name)
    @name = name
  end

  def greet
    puts "Hello"
  end

  def self.create(name)
    new(name)
  end
end
"#,
            )
            .unwrap();

        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"initialize"));
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"create"));

        let user = result.symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.kind, SymbolKind::Class);

        let init = result
            .symbols
            .iter()
            .find(|s| s.name == "initialize")
            .unwrap();
        assert_eq!(init.kind, SymbolKind::Method);
        assert_eq!(init.parent_name.as_deref(), Some("User"));

        let create = result.symbols.iter().find(|s| s.name == "create").unwrap();
        assert_eq!(create.kind, SymbolKind::Method);
        assert_eq!(create.parent_name.as_deref(), Some("User"));
    }

    #[test]
    fn test_parse_ruby_module() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
module Serializable
  def to_json
    "json"
  end
end
"#,
            )
            .unwrap();

        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Serializable"));
        assert!(names.contains(&"to_json"));

        let m = result
            .symbols
            .iter()
            .find(|s| s.name == "Serializable")
            .unwrap();
        assert_eq!(m.kind, SymbolKind::Module);

        let method = result.symbols.iter().find(|s| s.name == "to_json").unwrap();
        assert_eq!(method.parent_name.as_deref(), Some("Serializable"));
    }

    #[test]
    fn test_parse_ruby_constant() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser.parse("MAX_RETRIES = 3").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "MAX_RETRIES");
        assert_eq!(result.symbols[0].kind, SymbolKind::Constant);
    }

    #[test]
    fn test_parse_ruby_calls() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class Foo
  def bar
    puts "hello"
    baz.quux
  end
end
"#,
            )
            .unwrap();

        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"puts"));
        assert!(callees.contains(&"quux"));
    }

    #[test]
    fn test_parse_ruby_super_and_yield() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class Child < Parent
  def initialize
    super
  end

  def action
    yield
  end
end
"#,
            )
            .unwrap();

        let callees: Vec<&str> = result
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callees.contains(&"super"));
        assert!(callees.contains(&"yield"));
    }

    #[test]
    fn test_parse_ruby_imports() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
require 'json'
require_relative 'helpers/utils'
"#,
            )
            .unwrap();

        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "json" && i.original_name.as_deref() == Some("require")));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "helpers/utils"
                && i.original_name.as_deref() == Some("require_relative")));
    }

    #[test]
    fn test_parse_ruby_mixins() {
        let mut parser = SourceParser::new(SourceLanguage::Ruby).unwrap();
        let result = parser
            .parse(
                r#"
class User
  include Serializable
  extend ClassMethods
  prepend Logging
end
"#,
            )
            .unwrap();

        let import_names: Vec<&str> = result
            .imports
            .iter()
            .map(|i| i.local_name.as_str())
            .collect();
        assert!(import_names.contains(&"Serializable"));
        assert!(import_names.contains(&"ClassMethods"));
        assert!(import_names.contains(&"Logging"));
    }
}
