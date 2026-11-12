//! Dart extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // ── Dart ─────────────────────────────────────────────────────────

    pub(super) fn extract_dart_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "mixin_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "extension_declaration" => {
                // extension Foo on Bar { ... }
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
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
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_dart_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_signature" | "method_signature" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let kind = if parent.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_dart_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_body" => {
                // Skip — the parent function_signature already captured this
            }
            _ => {
                // Check for top-level or class-level function declarations
                // Dart grammar may represent them differently
                if kind == "declaration" || kind == "function_declaration" {
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
                            signature: self.extract_dart_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                    }
                }
            }
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_dart_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = match kind {
            "function_signature" | "method_signature" | "function_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        // Dart function invocations
        if kind == "selector" || kind == "argument_part" {
            // handled by parent
        }

        // Match function calls: identifier(...) or expr.identifier(...)
        if kind == "identifier" {
            if let Some(parent_node) = node.parent() {
                let parent_kind = parent_node.kind();
                if parent_kind == "assignable_expression"
                    || parent_kind == "primary"
                    || parent_kind == "function_expression_body"
                {
                    // Check if followed by argument_part
                    if let Some(next) = parent_node.next_sibling() {
                        if next.kind() == "selector" || next.kind() == "argument_part" {
                            let callee = self.node_text(node, source).to_string();
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
            }
        }

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_dart_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        let kind = node.kind();

        if kind == "import_or_export" || kind == "import_specification" || kind == "library_import"
        {
            let text = self.node_text(node, source).to_string();
            if text.starts_with("import") {
                // import 'package:foo/bar.dart' as baz;
                // import 'package:foo/bar.dart' show Baz;
                // import 'package:foo/bar.dart';
                if let Some(start) = text.find('\'').or_else(|| text.find('"')) {
                    let quote = text.as_bytes()[start] as char;
                    if let Some(end) = text[start + 1..].find(quote) {
                        let module = text[start + 1..start + 1 + end].to_string();
                        let local = module
                            .rsplit('/')
                            .next()
                            .unwrap_or(&module)
                            .trim_end_matches(".dart")
                            .to_string();

                        // Check for 'as' alias
                        let alias = if let Some(as_pos) = text.find(" as ") {
                            let rest = text[as_pos + 4..].trim().trim_end_matches(';');
                            Some(rest.to_string())
                        } else {
                            None
                        };

                        imports.push(ImportInfo {
                            local_name: alias.unwrap_or_else(|| local.clone()),
                            source_module: module,
                            original_name: None,
                        });
                    }
                }
            }
            return;
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_dart_imports(child, source, imports);
        }
    }

    fn extract_dart_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }
}
