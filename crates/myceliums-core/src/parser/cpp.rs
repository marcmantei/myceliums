//! C++ extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- C++ ---

    pub(super) fn extract_cpp_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                // Free functions or method definitions outside class body
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
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
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "declaration" if self.cpp_declaration_is_function(node, source) => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
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
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "class_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_cpp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into struct body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_cpp_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_cpp_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "template_declaration" => {
                // Extract the inner declaration (class, function, etc.) with template info
                let template_sig = self.extract_cpp_template_prefix(node, source);
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "function_definition" => {
                            if let Some(declarator) = child.child_by_field_name("declarator") {
                                let name = self.extract_cpp_declarator_name(declarator, source);
                                if !name.is_empty() {
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
                                        signature: format!(
                                            "{} {}",
                                            template_sig,
                                            self.extract_cpp_signature(child, source)
                                        ),
                                        content: self.node_text(node, source).to_string(),
                                        parent_name: parent.map(|s| s.to_string()),
                                        metadata: None,
                                    });
                                }
                            }
                        }
                        "declaration" if self.cpp_declaration_is_function(child, source) => {
                            if let Some(declarator) = child.child_by_field_name("declarator") {
                                let name = self.extract_cpp_declarator_name(declarator, source);
                                if !name.is_empty() {
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
                                        signature: format!(
                                            "{} {}",
                                            template_sig,
                                            self.extract_cpp_signature(child, source)
                                        ),
                                        content: self.node_text(node, source).to_string(),
                                        parent_name: parent.map(|s| s.to_string()),
                                        metadata: None,
                                    });
                                }
                            }
                        }
                        "class_specifier" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                let name = self.node_text(name_node, source).to_string();
                                symbols.push(ParsedSymbol {
                                    name: name.clone(),
                                    kind: SymbolKind::Class,
                                    start_line: node.start_position().row as u32 + 1,
                                    end_line: node.end_position().row as u32 + 1,
                                    signature: format!(
                                        "{} {}",
                                        template_sig,
                                        self.extract_cpp_signature(child, source)
                                    ),
                                    content: self.node_text(node, source).to_string(),
                                    parent_name: parent.map(|s| s.to_string()),
                                    metadata: None,
                                });
                                if let Some(body) = child.child_by_field_name("body") {
                                    self.extract_cpp_symbols(body, source, symbols, Some(&name));
                                }
                            }
                        }
                        "struct_specifier" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                let name = self.node_text(name_node, source).to_string();
                                symbols.push(ParsedSymbol {
                                    name: name.clone(),
                                    kind: SymbolKind::Class,
                                    start_line: node.start_position().row as u32 + 1,
                                    end_line: node.end_position().row as u32 + 1,
                                    signature: format!(
                                        "{} {}",
                                        template_sig,
                                        self.extract_cpp_signature(child, source)
                                    ),
                                    content: self.node_text(node, source).to_string(),
                                    parent_name: parent.map(|s| s.to_string()),
                                    metadata: None,
                                });
                                if let Some(body) = child.child_by_field_name("body") {
                                    self.extract_cpp_symbols(body, source, symbols, Some(&name));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                return; // Already handled children
            }
            "type_definition" => {
                // typedef ... name;
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_cpp_declarator_name(declarator, source);
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "alias_declaration" => {
                // using Name = Type;
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if !name.is_empty() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_cpp_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_cpp_symbols(child, source, symbols, parent);
        }
    }

    fn extract_cpp_declarator_name(&self, node: Node, source: &[u8]) -> String {
        match node.kind() {
            "function_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.extract_cpp_declarator_name(decl, source)
                } else {
                    String::new()
                }
            }
            "qualified_identifier" | "scoped_identifier" => {
                // e.g., ClassName::methodName — extract just the last name
                if let Some(name) = node.child_by_field_name("name") {
                    self.node_text(name, source).to_string()
                } else {
                    self.node_text(node, source).to_string()
                }
            }
            "identifier" | "type_identifier" | "field_identifier" => {
                self.node_text(node, source).to_string()
            }
            "destructor_name" => {
                // ~ClassName
                self.node_text(node, source).to_string()
            }
            "pointer_declarator" | "reference_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.extract_cpp_declarator_name(decl, source)
                } else {
                    String::new()
                }
            }
            "parenthesized_declarator" => {
                // (*funcPtr)(args)
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    let name = self.extract_cpp_declarator_name(child, source);
                    if !name.is_empty() {
                        return name;
                    }
                }
                String::new()
            }
            _ => String::new(),
        }
    }

    fn cpp_declaration_is_function(&self, node: Node, _source: &[u8]) -> bool {
        if let Some(declarator) = node.child_by_field_name("declarator") {
            self.cpp_is_function_declarator(declarator)
        } else {
            false
        }
    }

    fn cpp_is_function_declarator(&self, node: Node) -> bool {
        match node.kind() {
            "function_declarator" => true,
            "pointer_declarator" | "reference_declarator" | "parenthesized_declarator" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    self.cpp_is_function_declarator(decl)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn extract_cpp_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    fn extract_cpp_template_prefix(&self, node: Node, source: &[u8]) -> String {
        // Extract "template<...>" part
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('>') {
            text[..=pos].trim().to_string()
        } else {
            "template<>".to_string()
        }
    }

    pub(super) fn extract_cpp_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_definition" => node
                .child_by_field_name("declarator")
                .map(|d| self.extract_cpp_declarator_name(d, source))
                .filter(|n| !n.is_empty()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = match func_node.kind() {
                        "identifier" => self.node_text(func_node, source).to_string(),
                        "field_expression" => {
                            if let Some(field) = func_node.child_by_field_name("field") {
                                self.node_text(field, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "qualified_identifier" | "scoped_identifier" => {
                            if let Some(name) = func_node.child_by_field_name("name") {
                                self.node_text(name, source).to_string()
                            } else {
                                self.node_text(func_node, source).to_string()
                            }
                        }
                        "template_function" => {
                            // e.g., make_shared<Foo>(...)
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
            "new_expression" => {
                // new Foo(...) — extract the type name
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
            self.extract_cpp_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_cpp_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "preproc_include" => {
                // #include <header> or #include "header"
                if let Some(path_node) = node.child_by_field_name("path") {
                    let path_text = self.node_text(path_node, source);
                    // Strip quotes/angle brackets
                    let module = path_text
                        .trim_start_matches(['<', '"'])
                        .trim_end_matches(['>', '"'])
                        .to_string();
                    let local_name = module
                        .rsplit('/')
                        .next()
                        .unwrap_or(&module)
                        .trim_end_matches(".h")
                        .trim_end_matches(".hpp")
                        .trim_end_matches(".hxx")
                        .to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: module,
                        original_name: None,
                    });
                }
            }
            "using_declaration" => {
                // using namespace std; or using std::vector;
                let text = self.node_text(node, source).trim().to_string();
                let module = text
                    .trim_start_matches("using")
                    .trim_start_matches("namespace")
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string();
                if !module.is_empty() {
                    let local_name = module.rsplit("::").next().unwrap_or(&module).to_string();
                    imports.push(ImportInfo {
                        local_name,
                        source_module: module,
                        original_name: None,
                    });
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_cpp_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- C++ tests ---

    #[test]
    fn test_parse_cpp_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
class Dog {
public:
    Dog(const std::string& name) : name_(name) {}

    std::string speak() const {
        return "Woof!";
    }

    void fetch(const std::string& item) {
        std::cout << item << std::endl;
    }

private:
    std::string name_;
};
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Dog"));
        assert!(names.contains(&"speak"));
        assert!(names.contains(&"fetch"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "Dog" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let speak = result.symbols.iter().find(|s| s.name == "speak").unwrap();
        assert_eq!(speak.kind, SymbolKind::Method);
        assert_eq!(speak.parent_name.as_deref(), Some("Dog"));
    }

    #[test]
    fn test_parse_cpp_struct() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
struct Point {
    double x;
    double y;

    double distance() const {
        return 0.0;
    }
};
"#,
            )
            .unwrap();
        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);

        let distance = result
            .symbols
            .iter()
            .find(|s| s.name == "distance")
            .unwrap();
        assert_eq!(distance.kind, SymbolKind::Method);
        assert_eq!(distance.parent_name.as_deref(), Some("Point"));
    }

    #[test]
    fn test_parse_cpp_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
enum class Color {
    Red,
    Green,
    Blue
};
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "Color");
        assert_eq!(result.symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_cpp_namespace() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
namespace myapp {
    void helper() {}
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"myapp"));
        let ns = result.symbols.iter().find(|s| s.name == "myapp").unwrap();
        assert_eq!(ns.kind, SymbolKind::Module);
    }

    #[test]
    fn test_parse_cpp_free_function() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
void greet(const std::string& name) {
    std::cout << "Hello, " << name << std::endl;
}
"#,
            )
            .unwrap();
        let greet = result.symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(greet.kind, SymbolKind::Function);
        assert_eq!(greet.parent_name, None);
    }

    #[test]
    fn test_parse_cpp_template_function() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
template<typename T>
T max_value(T a, T b) {
    return (a > b) ? a : b;
}
"#,
            )
            .unwrap();
        let max_val = result
            .symbols
            .iter()
            .find(|s| s.name == "max_value")
            .unwrap();
        assert_eq!(max_val.kind, SymbolKind::Function);
        assert!(max_val.signature.contains("template"));
    }

    #[test]
    fn test_parse_cpp_template_class() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
template<typename T>
class Container {
public:
    void add(const T& item) {}
    size_t size() const { return 0; }
};
"#,
            )
            .unwrap();
        let container = result
            .symbols
            .iter()
            .find(|s| s.name == "Container")
            .unwrap();
        assert_eq!(container.kind, SymbolKind::Class);
        assert!(container.signature.contains("template"));
    }

    #[test]
    fn test_parse_cpp_typedef_and_using() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
typedef unsigned long ulong;
using StringVec = std::vector<std::string>;
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"ulong"));
        assert!(names.contains(&"StringVec"));

        let ulong_sym = result.symbols.iter().find(|s| s.name == "ulong").unwrap();
        assert_eq!(ulong_sym.kind, SymbolKind::TypeAlias);

        let sv = result
            .symbols
            .iter()
            .find(|s| s.name == "StringVec")
            .unwrap();
        assert_eq!(sv.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_call_extraction_cpp() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
void doWork() {
    greet("world");
    dog.fetch("ball");
    auto p = new Point{1.0, 2.0};
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "greet"));
        assert!(result.calls.iter().any(|c| c.callee_name == "fetch"));
        assert!(result.calls.iter().any(|c| c.callee_name == "Point"));
    }

    #[test]
    fn test_import_extraction_cpp() {
        let mut parser = SourceParser::new(SourceLanguage::Cpp).unwrap();
        let result = parser
            .parse(
                r#"
#include <iostream>
#include <string>
#include "myheader.h"
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.source_module == "iostream"));
        assert!(result.imports.iter().any(|i| i.source_module == "string"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "myheader.h" && i.local_name == "myheader"));
    }

    #[test]
    fn test_cpp_extension_mapping() {
        assert_eq!(
            SourceLanguage::from_extension("cpp"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("cc"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("cxx"),
            Some(SourceLanguage::Cpp)
        );
        assert_eq!(
            SourceLanguage::from_extension("hpp"),
            Some(SourceLanguage::Cpp)
        );
    }
    // --- Ruby tests ---
}
