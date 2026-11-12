//! C-family extraction: C and Objective-C.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- C ---

    pub(super) fn extract_c_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = self.extract_c_declarator_name(declarator, source) {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Function,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "declaration" => {
                // Check if this is a function forward declaration (has function_declarator)
                // or a variable/constant declaration
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if self.c_has_function_declarator(declarator) {
                        if let Some(name) = self.extract_c_declarator_name(declarator, source) {
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_c_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    // Only emit if this has a body (definition, not just usage)
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "union_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    if node.child_by_field_name("body").is_some() {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Enum,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "type_definition" => {
                // typedef ... name;
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    let name = self.extract_c_typedef_name(declarator, source);
                    if let Some(name) = name {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_c_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
            "preproc_def" => {
                // #define NAME value
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
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
            "preproc_function_def" => {
                // #define NAME(args) body
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
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
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_symbols(child, source, symbols, parent);
        }
    }

    /// Extract the function name from a C declarator, handling pointer declarators
    /// and function declarators (e.g., `int *foo(int x)` or `void bar(void)`).
    fn extract_c_declarator_name(&self, node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_declarator_name(declarator, source)
                } else {
                    None
                }
            }
            "pointer_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_declarator_name(declarator, source)
                } else {
                    None
                }
            }
            "parenthesized_declarator" => {
                // (*func_ptr)(args) — recurse inside parens
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if let Some(name) = self.extract_c_declarator_name(child, source) {
                        return Some(name);
                    }
                }
                None
            }
            "identifier" => Some(self.node_text(node, source).to_string()),
            _ => None,
        }
    }

    /// Check if a declarator contains a function_declarator node.
    fn c_has_function_declarator(&self, node: Node) -> bool {
        if node.kind() == "function_declarator" {
            return true;
        }
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            if self.c_has_function_declarator(child) {
                return true;
            }
        }
        false
    }

    /// Extract the typedef name from a declarator node.
    fn extract_c_typedef_name(&self, node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "type_identifier" | "identifier" => Some(self.node_text(node, source).to_string()),
            "pointer_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_typedef_name(declarator, source)
                } else {
                    None
                }
            }
            "function_declarator" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    self.extract_c_typedef_name(declarator, source)
                } else {
                    None
                }
            }
            "parenthesized_declarator" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if let Some(name) = self.extract_c_typedef_name(child, source) {
                        return Some(name);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn extract_c_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_c_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "function_definition" {
            node.child_by_field_name("declarator")
                .and_then(|d| self.extract_c_declarator_name(d, source))
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
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
                    "parenthesized_expression" => {
                        // Function pointer calls: (*func_ptr)(args)
                        self.node_text(func_node, source).to_string()
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

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_c_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "preproc_include" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path_text = self.node_text(path_node, source).to_string();
                // Strip angle brackets or quotes
                let module = path_text
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim_matches('"')
                    .to_string();
                // Derive local name from filename without extension
                let local_name = module
                    .rsplit('/')
                    .next()
                    .unwrap_or(&module)
                    .split('.')
                    .next()
                    .unwrap_or(&module)
                    .to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: module,
                    original_name: None,
                });
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_c_imports(child, source, imports);
        }
    }

    // --- Objective-C ---

    fn extract_objc_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_objc_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_interface" | "interface_declaration" => {
                // @interface ClassName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    // Some grammars don't have a body field; recurse children
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "class_implementation" | "implementation_definition" => {
                // @implementation ClassName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into implementation body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "protocol_declaration" => {
                // @protocol ProtocolName
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into protocol body
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_objc_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_objc_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "method_declaration" | "method_definition" => {
                // - (void)methodName or + (id)classMethod
                if let Some(name_node) = node.child_by_field_name("selector") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_objc_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_definition" | "declaration" => {
                // C-style function definitions in ObjC files
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if declarator.kind() == "function_declarator" {
                        if let Some(name_node) = declarator.child_by_field_name("declarator") {
                            let name = self.node_text(name_node, source).to_string();
                            symbols.push(ParsedSymbol {
                                name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_objc_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_objc_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_objc_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method_declaration" | "method_definition" => node
                .child_by_field_name("selector")
                .or_else(|| node.child_by_field_name("name"))
                .map(|n| self.node_text(n, source).to_string()),
            "function_definition" => node
                .child_by_field_name("declarator")
                .and_then(|d| d.child_by_field_name("declarator"))
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "message_expression" | "message_send" => {
                // ObjC-style: [obj method] or [obj method:arg]
                if let Some(selector_node) = node.child_by_field_name("selector") {
                    let callee = self.node_text(selector_node, source).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(name_node) = node.child_by_field_name("name") {
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
            "call_expression" => {
                // C-style function calls: NSLog(@"hello"), dispatch_async(...)
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = self.node_text(func_node, source).to_string();
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
            self.extract_objc_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_objc_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "preproc_import" | "preproc_include" => {
                // #import <Foundation/Foundation.h> or #import "MyHeader.h"
                // #include <stdio.h> or #include "MyHeader.h"
                let text = self.node_text(node, source);
                if let Some(start) = text.find('<').or_else(|| text.find('"')) {
                    if let Some(end) = text[start + 1..]
                        .find('>')
                        .or_else(|| text[start + 1..].find('"'))
                    {
                        let module = text[start + 1..start + 1 + end].to_string();
                        if !module.is_empty() {
                            let local_name =
                                module.split('/').next_back().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: None,
                            });
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_objc_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- C tests ---

    #[test]
    fn test_parse_c_function() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse("int add(int a, int b) { return a + b; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_c_struct() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
struct Node {
    int value;
    struct Node *next;
};
"#,
            )
            .unwrap();
        let node_sym = result.symbols.iter().find(|s| s.name == "Node").unwrap();
        assert_eq!(node_sym.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_c_enum() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
enum Color {
    RED,
    GREEN,
    BLUE
};
"#,
            )
            .unwrap();
        let color = result.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_c_union() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
union Data {
    int i;
    float f;
    char c;
};
"#,
            )
            .unwrap();
        let data = result.symbols.iter().find(|s| s.name == "Data").unwrap();
        assert_eq!(data.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_c_typedef() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser.parse("typedef unsigned long ulong;").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "ulong");
        assert_eq!(result.symbols[0].kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_c_macro() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
#define MAX_SIZE 1024
#define SQUARE(x) ((x) * (x))
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MAX_SIZE"));
        assert!(names.contains(&"SQUARE"));
        for sym in &result.symbols {
            assert_eq!(sym.kind, SymbolKind::Constant);
        }
    }

    #[test]
    fn test_parse_c_forward_declaration() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser.parse("int init(struct Config *config);").unwrap();
        let init_sym = result.symbols.iter().find(|s| s.name == "init").unwrap();
        assert_eq!(init_sym.kind, SymbolKind::Function);
    }

    #[test]
    fn test_call_extraction_c() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
void process(int x) {
    int result = add(x, 1);
    printf("Result: %d\n", result);
    free(ptr);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "add"));
        assert!(result.calls.iter().any(|c| c.callee_name == "printf"));
        assert!(result.calls.iter().any(|c| c.callee_name == "free"));
        for call in &result.calls {
            assert_eq!(call.caller_name, "process");
        }
    }

    #[test]
    fn test_import_extraction_c() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse(
                r#"
#include <stdio.h>
#include <stdlib.h>
#include "myheader.h"
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.source_module == "stdio.h"));
        assert!(result.imports.iter().any(|i| i.source_module == "stdlib.h"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "myheader.h"));
        // Local names derived from filenames without extension
        assert!(result.imports.iter().any(|i| i.local_name == "stdio"));
        assert!(result.imports.iter().any(|i| i.local_name == "myheader"));
    }

    #[test]
    fn test_parse_c_pointer_return_function() {
        let mut parser = SourceParser::new(SourceLanguage::C).unwrap();
        let result = parser
            .parse("void *allocate(size_t size) { return malloc(size); }")
            .unwrap();
        let alloc = result
            .symbols
            .iter()
            .find(|s| s.name == "allocate")
            .unwrap();
        assert_eq!(alloc.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_c_extension() {
        assert_eq!(SourceLanguage::from_extension("c"), Some(SourceLanguage::C));
        assert_eq!(SourceLanguage::from_extension("h"), Some(SourceLanguage::C));
    }
}
