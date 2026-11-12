//! Python extraction.

use super::{CallReference, ExpressionStep, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::{SymbolKind, SymbolMetadata};
use tree_sitter::Node;

impl SourceParser {
    // --- Python ---

    pub(super) fn extract_py_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "function_definition" => {
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
                        signature: self.extract_py_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_py_metadata(node, source),
                    });
                }
            }
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_py_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_py_metadata(node, source),
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_py_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_symbols(child, source, symbols, parent);
        }
    }

    fn extract_py_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find(':') {
            // Check this is the colon before the body, not a type annotation
            let before_colon = &text[..pos];
            if before_colon.contains("def ") || before_colon.contains("class ") {
                return text[..=pos].trim().to_string();
            }
        }
        text.lines().next().unwrap_or("").trim().to_string()
    }

    pub(super) fn extract_py_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = if kind == "function_definition" {
            node.child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string())
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let (callee, chain) = match func_node.kind() {
                    "identifier" => (self.node_text(func_node, source).to_string(), None),
                    "attribute" => {
                        let chain = self.build_py_chain(func_node, source);
                        let name = if let Some(attr) = func_node.child_by_field_name("attribute") {
                            self.node_text(attr, source).to_string()
                        } else {
                            self.node_text(func_node, source).to_string()
                        };
                        (name, chain)
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

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_calls(child, source, calls, scope);
        }
    }

    /// Build an expression chain from a Python `attribute` node.
    fn build_py_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_py_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    fn collect_py_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
        match node.kind() {
            "attribute" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_py_chain_steps(obj, source, steps);
                }
                if let Some(attr) = node.child_by_field_name("attribute") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(attr, source).to_string(),
                    ));
                }
            }
            "call" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_py_chain_steps(func, source, steps);
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
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    pub(super) fn extract_py_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "import_statement" => {
                // import foo, import foo as bar
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "dotted_name" {
                        let name = self.node_text(child, source).to_string();
                        let local = name.split('.').next_back().unwrap_or(&name).to_string();
                        imports.push(ImportInfo {
                            local_name: local,
                            source_module: name,
                            original_name: None,
                        });
                    } else if child.kind() == "aliased_import" {
                        let name_node = child.child_by_field_name("name");
                        let alias_node = child.child_by_field_name("alias");
                        if let Some(name) = name_node {
                            let original = self.node_text(name, source).to_string();
                            let local = alias_node
                                .map(|a| self.node_text(a, source).to_string())
                                .unwrap_or_else(|| {
                                    original
                                        .split('.')
                                        .next_back()
                                        .unwrap_or(&original)
                                        .to_string()
                                });
                            imports.push(ImportInfo {
                                local_name: local,
                                source_module: original,
                                original_name: None,
                            });
                        }
                    }
                }
            }
            "import_from_statement" => {
                let module_node = node.child_by_field_name("module_name");
                let module = module_node
                    .map(|m| self.node_text(m, source).to_string())
                    .unwrap_or_default();

                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    match child.kind() {
                        "dotted_name" if Some(child) != module_node => {
                            // Skip the module name itself (already captured)
                            let name = self.node_text(child, source).to_string();
                            imports.push(ImportInfo {
                                local_name: name.clone(),
                                source_module: module.clone(),
                                original_name: Some(name),
                            });
                        }
                        "aliased_import" => {
                            let name_node = child.child_by_field_name("name");
                            let alias_node = child.child_by_field_name("alias");
                            if let Some(name) = name_node {
                                let original = self.node_text(name, source).to_string();
                                let local = alias_node
                                    .map(|a| self.node_text(a, source).to_string())
                                    .unwrap_or_else(|| original.clone());
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: module.clone(),
                                    original_name: Some(original),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_imports(child, source, imports);
        }
    }

    /// Extract variable aliases from Python assignment statements.
    ///
    /// Detects patterns like `handler = authenticate` where the RHS is a simple
    /// identifier, producing an alias pair ("handler", "authenticate").
    pub(super) fn extract_py_aliases(
        &self,
        node: Node,
        source: &[u8],
        aliases: &mut Vec<(String, String)>,
    ) {
        // Direct assignment node: `x = y`
        if node.kind() == "assignment" {
            self.try_extract_py_alias(node, source, aliases);
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_py_aliases(child, source, aliases);
        }
    }

    fn try_extract_py_alias(
        &self,
        assign: Node,
        source: &[u8],
        aliases: &mut Vec<(String, String)>,
    ) {
        let left = assign.child_by_field_name("left");
        let right = assign.child_by_field_name("right");

        if let (Some(left_node), Some(right_node)) = (left, right) {
            // identifier = identifier
            if left_node.kind() == "identifier" && right_node.kind() == "identifier" {
                let local = self.node_text(left_node, source).to_string();
                let target = self.node_text(right_node, source).to_string();
                aliases.push((local, target));
            }
            // identifier = module.attribute
            if left_node.kind() == "identifier" && right_node.kind() == "attribute" {
                if let Some(attr) = right_node.child_by_field_name("attribute") {
                    let local = self.node_text(left_node, source).to_string();
                    let target = self.node_text(attr, source).to_string();
                    aliases.push((local, target));
                }
            }
        }
    }

    /// Extract Python metadata: decorators, return type, superclasses.
    fn extract_py_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Decorators: check if the parent is a decorated_definition
        if let Some(parent) = node.parent() {
            if parent.kind() == "decorated_definition" {
                let cursor = &mut parent.walk();
                for child in parent.children(cursor) {
                    if child.kind() == "decorator" {
                        let text = self.node_text(child, source).trim().to_string();
                        meta.decorators.push(text);
                    }
                }
            }
        }

        // Return type annotation
        if node.kind() == "function_definition" {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                let cleaned = text.strip_prefix("->").map(|s| s.trim()).unwrap_or(&text);
                if !cleaned.is_empty() {
                    meta.return_type = Some(cleaned.to_string());
                }
            }
        }

        // Superclasses for class_definition
        if node.kind() == "class_definition" {
            if let Some(args) = node.child_by_field_name("superclasses") {
                let cursor = &mut args.walk();
                for child in args.children(cursor) {
                    let ck = child.kind();
                    if ck == "identifier" || ck == "attribute" || ck == "subscript" {
                        meta.superclasses
                            .push(self.node_text(child, source).to_string());
                    }
                }
            }
        }

        if meta.decorators.is_empty() && meta.return_type.is_none() && meta.superclasses.is_empty()
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
    use myceliums_storage::SymbolKind;

    #[test]
    fn test_parse_python_function() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse("def greet(name: str) -> str:\n    return f'Hello {name}'")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_python_class() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
class UserService:
    def get_user(self, id: str):
        return None
    def delete_user(self, id: str):
        pass
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"get_user"));
        assert!(names.contains(&"delete_user"));
    }

    #[test]
    fn test_py_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser.parse("def foo():\n    obj.method()\n").unwrap();
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

    // --- Metadata extraction tests ---

    #[test]
    fn test_python_decorated_function_has_decorators() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
@app.route("/api")
@login_required
def handle_request():
    pass
"#,
            )
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "handle_request")
            .expect("should find handle_request");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.decorators.is_empty(),
            "should have at least one decorator, got {:?}",
            meta.decorators
        );
    }

    #[test]
    fn test_python_class_with_superclasses() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
class MyView(BaseView, Mixin):
    pass
"#,
            )
            .unwrap();
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "MyView")
            .expect("should find MyView");
        let meta = cls.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.superclasses.is_empty(),
            "should have superclasses, got {:?}",
            meta.superclasses
        );
    }

    #[test]
    fn test_python_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
def greet(name: str) -> str:
    return f"Hello {name}"
"#,
            )
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "greet")
            .expect("should find greet");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
        assert_eq!(meta.return_type.as_deref().unwrap(), "str");
    }

    #[test]
    fn test_python_alias_extraction() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
handler = authenticate
callback = module.process
x = 42
"#,
            )
            .unwrap();
        // Should find 2 aliases: handler→authenticate, callback→process
        assert_eq!(result.aliases.len(), 2);
        assert!(result
            .aliases
            .contains(&("handler".to_string(), "authenticate".to_string())));
        assert!(result
            .aliases
            .contains(&("callback".to_string(), "process".to_string())));
    }

    #[test]
    fn test_alias_resolves_call() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let result = parser
            .parse(
                r#"
def authenticate(user):
    pass

def main():
    handler = authenticate
    handler()
"#,
            )
            .unwrap();
        // Aliases collected
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "authenticate"));
        // Call to handler() exists
        assert!(result.calls.iter().any(|c| c.callee_name == "handler"));
    }
}
