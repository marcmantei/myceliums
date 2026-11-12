//! TypeScript, TSX and JavaScript extraction, plus the Vue/Svelte
//! `<script>` block that is parsed as TypeScript.

use super::{CallReference, ExpressionStep, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::{SymbolKind, SymbolMetadata};
use tree_sitter::Node;

impl SourceParser {
    // --- TypeScript/TSX ---

    pub(super) fn extract_ts_symbols(
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
                        name: name.clone(),
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            "method_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Method,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                }
            }
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: self.extract_ts_metadata(node, source),
                    });
                    // Recurse into class body with parent set
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_ts_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::TypeAlias,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                // Handle: const foo = ..., export const foo = ...
                // Also handle arrow functions: const foo = () => ...
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
                            let value = child.child_by_field_name("value");
                            let is_arrow = value
                                .map(|v| {
                                    v.kind() == "arrow_function"
                                        || v.kind() == "function_expression"
                                        || v.kind() == "function"
                                })
                                .unwrap_or(false);

                            let sym_kind = if is_arrow {
                                SymbolKind::Function
                            } else if self.node_text(node, source).starts_with("const") {
                                SymbolKind::Constant
                            } else {
                                SymbolKind::Variable
                            };

                            symbols.push(ParsedSymbol {
                                name,
                                kind: sym_kind,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                    }
                }
                return; // Don't recurse further
            }
            "export_statement" => {
                // Recurse into exported declarations
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    self.extract_ts_symbols(child, source, symbols, parent);
                }
                return;
            }
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_ts_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_declaration" | "method_definition" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            "variable_declarator" => {
                let value = node.child_by_field_name("value");
                let is_fn = value
                    .map(|v| v.kind() == "arrow_function" || v.kind() == "function_expression")
                    .unwrap_or(false);
                if is_fn {
                    node.child_by_field_name("name")
                        .map(|n| self.node_text(n, source).to_string())
                } else {
                    None
                }
            }
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let (callee, chain) = match func_node.kind() {
                    "identifier" => (self.node_text(func_node, source).to_string(), None),
                    "member_expression" => {
                        let chain = self.build_ts_chain(func_node, source);
                        let name = if let Some(prop) = func_node.child_by_field_name("property") {
                            self.node_text(prop, source).to_string()
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
            self.extract_ts_calls(child, source, calls, scope);
        }
    }

    /// Build an expression chain from a TypeScript `member_expression` node.
    fn build_ts_chain(&self, node: Node, source: &[u8]) -> Option<Vec<ExpressionStep>> {
        let mut steps = Vec::new();
        self.collect_ts_chain_steps(node, source, &mut steps);
        steps.push(ExpressionStep::Call);
        if steps.len() > 1 {
            Some(steps)
        } else {
            None
        }
    }

    fn collect_ts_chain_steps(&self, node: Node, source: &[u8], steps: &mut Vec<ExpressionStep>) {
        match node.kind() {
            "member_expression" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    self.collect_ts_chain_steps(obj, source, steps);
                }
                if let Some(prop) = node.child_by_field_name("property") {
                    steps.push(ExpressionStep::Field(
                        self.node_text(prop, source).to_string(),
                    ));
                }
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.collect_ts_chain_steps(func, source, steps);
                }
                steps.push(ExpressionStep::Call);
            }
            "identifier" => {
                let text = self.node_text(node, source);
                if text == "this" {
                    steps.push(ExpressionStep::This);
                } else if text == "super" {
                    steps.push(ExpressionStep::Super);
                } else {
                    steps.push(ExpressionStep::Ident(text.to_string()));
                }
            }
            "this" => steps.push(ExpressionStep::This),
            "super" => steps.push(ExpressionStep::Super),
            _ => {
                steps.push(ExpressionStep::Ident(
                    self.node_text(node, source).to_string(),
                ));
            }
        }
    }

    pub(super) fn extract_ts_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "import_statement" {
            let source_node = node.child_by_field_name("source");
            let module = source_node
                .map(|s| {
                    let text = self.node_text(s, source);
                    text.trim_matches('\'').trim_matches('"').to_string()
                })
                .unwrap_or_default();

            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "import_clause" {
                    let clause_cursor = &mut child.walk();
                    for clause_child in child.children(clause_cursor) {
                        match clause_child.kind() {
                            "identifier" => {
                                imports.push(ImportInfo {
                                    local_name: self.node_text(clause_child, source).to_string(),
                                    source_module: module.clone(),
                                    original_name: None,
                                });
                            }
                            "named_imports" => {
                                let spec_cursor = &mut clause_child.walk();
                                for spec in clause_child.children(spec_cursor) {
                                    if spec.kind() == "import_specifier" {
                                        let name_node = spec.child_by_field_name("name");
                                        let alias_node = spec.child_by_field_name("alias");
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
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Handle CommonJS require() - for JavaScript files
        // Pattern: const foo = require('module') or var bar = require('./file')
        if node.kind() == "variable_declarator" {
            if let Some(value) = node.child_by_field_name("value") {
                if value.kind() == "call_expression" {
                    if let Some(func) = value.child_by_field_name("function") {
                        if func.kind() == "identifier" && self.node_text(func, source) == "require"
                        {
                            // Extract the module name from arguments
                            let cursor = &mut value.walk();
                            for child in value.children(cursor) {
                                if child.kind() == "arguments" {
                                    let arg_cursor = &mut child.walk();
                                    for arg in child.children(arg_cursor) {
                                        if arg.kind() == "string" {
                                            let module = self.node_text(arg, source);
                                            let module = module
                                                .trim_matches('\'')
                                                .trim_matches('"')
                                                .to_string();

                                            // Extract the local name from the declarator
                                            if let Some(name_node) =
                                                node.child_by_field_name("name")
                                            {
                                                imports.push(ImportInfo {
                                                    local_name: self
                                                        .node_text(name_node, source)
                                                        .to_string(),
                                                    source_module: module,
                                                    original_name: None,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_imports(child, source, imports);
        }
    }

    /// Extract variable aliases from TypeScript/JavaScript assignments.
    ///
    /// Detects patterns like `const handler = authenticate` or `handler = authenticate`
    /// where the RHS is a simple identifier.
    pub(super) fn extract_ts_aliases(
        &self,
        node: Node,
        source: &[u8],
        aliases: &mut Vec<(String, String)>,
    ) {
        match node.kind() {
            // const/let/var handler = authenticate
            "variable_declarator" => {
                let name_node = node.child_by_field_name("name");
                let value_node = node.child_by_field_name("value");
                if let (Some(name), Some(value)) = (name_node, value_node) {
                    if name.kind() == "identifier" && value.kind() == "identifier" {
                        let local = self.node_text(name, source).to_string();
                        let target = self.node_text(value, source).to_string();
                        aliases.push((local, target));
                    }
                    // handler = obj.method
                    if name.kind() == "identifier" && value.kind() == "member_expression" {
                        if let Some(prop) = value.child_by_field_name("property") {
                            let local = self.node_text(name, source).to_string();
                            let target = self.node_text(prop, source).to_string();
                            aliases.push((local, target));
                        }
                    }
                }
            }
            // handler = authenticate (reassignment)
            "assignment_expression" => {
                let left = node.child_by_field_name("left");
                let right = node.child_by_field_name("right");
                if let (Some(left_node), Some(right_node)) = (left, right) {
                    if left_node.kind() == "identifier" && right_node.kind() == "identifier" {
                        let local = self.node_text(left_node, source).to_string();
                        let target = self.node_text(right_node, source).to_string();
                        aliases.push((local, target));
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_ts_aliases(child, source, aliases);
        }
    }

    // ── Vue / Svelte script block extraction ─────────────────────────

    /// Extract the content of the first `<script>` block from a Vue or Svelte file.
    /// Falls back to the full source if no script block is found.
    pub(super) fn extract_script_block(source: &str) -> String {
        // Match <script ...> (with optional attributes like lang="ts", setup, etc.)
        // ASCII-only lowercasing keeps every byte offset identical to `source`;
        // full Unicode lowercasing changes lengths (e.g. 'K' KELVIN SIGN →
        // 'k'), and slicing `source` with those offsets panicked.
        let lower = source.to_ascii_lowercase();
        if let Some(tag_start) = lower.find("<script") {
            // Find the closing > of the opening tag
            if let Some(content_start) = source[tag_start..].find('>') {
                let content_start = tag_start + content_start + 1;
                // Find the closing </script>
                if let Some(end) = lower[content_start..].find("</script>") {
                    return source[content_start..content_start + end].to_string();
                }
            }
        }
        // No script block found — return empty string (will produce no symbols)
        String::new()
    }

    /// Extract TypeScript/JavaScript metadata: decorators, return type, extends/implements.
    fn extract_ts_metadata(&self, node: Node, source: &[u8]) -> Option<SymbolMetadata> {
        let mut meta = SymbolMetadata::default();

        // Decorators: look at preceding siblings
        if let Some(parent) = node.parent() {
            let cursor = &mut parent.walk();
            let children: Vec<_> = parent.children(cursor).collect();
            let mut idx = children.len();
            for (i, child) in children.iter().enumerate() {
                if child.id() == node.id() {
                    idx = i;
                    break;
                }
            }
            for i in (0..idx).rev() {
                if children[i].kind() == "decorator" {
                    let text = self.node_text(children[i], source).trim().to_string();
                    meta.decorators.push(text);
                } else if children[i].kind() != "comment" {
                    break;
                }
            }
            meta.decorators.reverse();
        }

        // Return type annotation
        if node.kind() == "function_declaration"
            || node.kind() == "method_definition"
            || node.kind() == "arrow_function"
        {
            if let Some(rt) = node.child_by_field_name("return_type") {
                let text = self.node_text(rt, source).trim().to_string();
                let cleaned = text.strip_prefix(':').map(|s| s.trim()).unwrap_or(&text);
                if !cleaned.is_empty() {
                    meta.return_type = Some(cleaned.to_string());
                }
            }
        }

        // Extends / implements for class_declaration
        if node.kind() == "class_declaration" {
            let cursor = &mut node.walk();
            for child in node.children(cursor) {
                if child.kind() == "class_heritage" {
                    let cursor2 = &mut child.walk();
                    for hchild in child.children(cursor2) {
                        if hchild.kind() == "extends_clause" || hchild.kind() == "implements_clause"
                        {
                            let cursor3 = &mut hchild.walk();
                            for gchild in hchild.children(cursor3) {
                                let gk = gchild.kind();
                                if gk == "identifier"
                                    || gk == "nested_identifier"
                                    || gk == "generic_type"
                                    || gk == "type_identifier"
                                {
                                    meta.superclasses
                                        .push(self.node_text(gchild, source).to_string());
                                }
                            }
                        }
                    }
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

        if meta.decorators.is_empty()
            && meta.return_type.is_none()
            && meta.superclasses.is_empty()
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
    use myceliums_storage::SymbolKind;

    #[test]
    fn script_block_extraction_survives_case_changing_unicode() {
        // U+212A KELVIN SIGN lowercases to a 1-byte 'k'; offsets from a
        // Unicode-lowercased copy used to land inside it and panic.
        let src = "\u{212A}<script>const a = 1;</script>";
        assert_eq!(SourceParser::extract_script_block(src), "const a = 1;");
        let src = "<p>İstanbul</p><SCRIPT lang=\"ts\">let x = 2;</SCRIPT>";
        assert_eq!(SourceParser::extract_script_block(src), "let x = 2;");
        assert_eq!(SourceParser::extract_script_block("\u{212A} no script"), "");
    }

    #[test]
    fn test_parse_typescript_function() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function greet(name: string): string { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_typescript_class() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id: string) { return null; }
    deleteUser(id: string) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_typescript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("const add = (a: number, b: number) => a + b;")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_call_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    #[test]
    fn test_import_extraction_ts() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_parse_javascript_function() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse("function greet(name) { return `Hello ${name}`; }")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "greet");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_javascript_class() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    getUser(id) { return null; }
    deleteUser(id) { }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));
    }

    #[test]
    fn test_parse_javascript_arrow() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser.parse("const add = (a, b) => a + b;").unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "add");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_import_extraction_js_esm() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
import { UserService } from './services/user.js';
import axios from 'axios';
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "UserService"));
        assert!(result.imports.iter().any(|i| i.local_name == "axios"));
    }

    #[test]
    fn test_import_extraction_js_commonjs() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
const { UserService } = require('./services/user.js');
const axios = require('axios');
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "UserService" || i.local_name == "axios"));
    }

    #[test]
    fn test_call_extraction_js() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        let result = parser
            .parse(
                r#"
function main() {
    const result = processData(input);
    console.log(result);
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "log"));
    }

    #[test]
    fn test_simple_call_has_no_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { bar(); }").unwrap();
        let call = result.calls.iter().find(|c| c.callee_name == "bar");
        assert!(call.is_some(), "should detect the bar() call");
        assert!(
            call.unwrap().chain.is_none(),
            "simple call should have no chain"
        );
    }

    #[test]
    fn test_ts_method_call_chain() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { obj.method(); }").unwrap();
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
    fn test_ts_chained_calls() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("function foo() { a.b().c(); }").unwrap();
        // The outer call is c(), with the receiver being a.b()
        let call = result.calls.iter().find(|c| c.callee_name == "c");
        assert!(call.is_some(), "should detect .c() call");
        let chain = call
            .unwrap()
            .chain
            .as_ref()
            .expect("chained call should have a chain");
        // a.b().c() => [Ident("a"), Field("b"), Call, Field("c"), Call]
        assert_eq!(chain[0], ExpressionStep::Ident("a".to_string()));
        assert_eq!(chain[1], ExpressionStep::Field("b".to_string()));
        assert_eq!(chain[2], ExpressionStep::Call);
        assert_eq!(chain[3], ExpressionStep::Field("c".to_string()));
        assert_eq!(chain[4], ExpressionStep::Call);
    }

    #[test]
    fn test_typescript_class_extends_has_superclasses() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
class Animal {
    name: string;
}

class Dog extends Animal {
    bark() {}
}
"#,
            )
            .unwrap();
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "Dog")
            .expect("should find Dog");
        let meta = cls.metadata.as_ref().expect("should have metadata");
        assert!(
            !meta.superclasses.is_empty(),
            "should have superclasses from extends"
        );
    }

    #[test]
    fn test_typescript_function_return_type() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse("function add(a: number, b: number): number { return a + b; }")
            .unwrap();
        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "add")
            .expect("should find add");
        let meta = func.metadata.as_ref().expect("should have metadata");
        assert!(meta.return_type.is_some(), "should have return type");
    }

    #[test]
    fn test_typescript_alias_extraction() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser
            .parse(
                r#"
const handler = authenticate;
let cb = service.validate;
handler = reassigned;
"#,
            )
            .unwrap();
        // Should find: handler→authenticate, cb→validate, handler→reassigned
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "authenticate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "cb" && t == "validate"));
        assert!(result
            .aliases
            .iter()
            .any(|(l, t)| l == "handler" && t == "reassigned"));
    }
}
