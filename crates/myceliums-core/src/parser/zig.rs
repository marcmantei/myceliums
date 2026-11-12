//! Zig extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // ─── Zig ───────────────────────────────────────────────────────────

    pub(super) fn extract_zig_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "FnProto" | "fn_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_zig_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "ContainerDecl" | "struct_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = {
                        let text = self.node_text(node, source);
                        if text.starts_with("enum") {
                            SymbolKind::Enum
                        } else {
                            SymbolKind::Class
                        }
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_zig_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    let cursor = &mut node.walk();
                    for child in node.children(cursor) {
                        self.extract_zig_symbols(child, source, symbols, Some(&name));
                    }
                    return;
                }
            }
            "VarDecl" | "variable_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let text = self.node_text(node, source);
                    let sym_kind = if text.trim_start().starts_with("const") {
                        let cursor = &mut node.walk();
                        let mut is_container = false;
                        for child in node.children(cursor) {
                            if matches!(child.kind(), "ContainerDecl" | "struct_declaration") {
                                is_container = true;
                                break;
                            }
                        }
                        if is_container {
                            SymbolKind::Class
                        } else {
                            SymbolKind::Constant
                        }
                    } else {
                        SymbolKind::Variable
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
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

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_zig_symbols(child, source, symbols, parent);
        }
    }

    fn extract_zig_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_zig_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "FnProto" | "fn_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "call_expression" | "FnCallExpr" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let callee = self.node_text(func_node, source).to_string();
                    let callee_short = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee_short,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                } else if let Some(first_child) = node.child(0) {
                    let callee = self.node_text(first_child, source).to_string();
                    let callee_short = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: callee_short,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "builtin_call_expr" | "BuiltinCallExpr" => {
                let text = self.node_text(node, source);
                if let Some(paren_pos) = text.find('(') {
                    let builtin_name = text[..paren_pos].trim().to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: builtin_name,
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
            self.extract_zig_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_zig_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "VarDecl" | "variable_declaration" => {
                let text = self.node_text(node, source);
                if text.contains("@import") {
                    let local_name = if let Some(name_node) = node.child_by_field_name("name") {
                        self.node_text(name_node, source).to_string()
                    } else {
                        String::new()
                    };

                    if let Some(start) = text.find("@import(") {
                        let after_import = &text[start + 8..];
                        let module = after_import
                            .trim_start_matches(['"', '\''])
                            .split(['"', '\''])
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !module.is_empty() && !local_name.is_empty() {
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: Some("@import".to_string()),
                            });
                        }
                    }
                    return;
                }
                if text.contains("@cImport") {
                    let local_name = if let Some(name_node) = node.child_by_field_name("name") {
                        self.node_text(name_node, source).to_string()
                    } else {
                        String::new()
                    };

                    let mut search = text;
                    while let Some(start) = search.find("@cInclude(") {
                        let after = &search[start + 10..];
                        let header = after
                            .trim_start_matches(['"', '\''])
                            .split(['"', '\''])
                            .next()
                            .unwrap_or("");
                        if !header.is_empty() {
                            imports.push(ImportInfo {
                                local_name: local_name.clone(),
                                source_module: header.to_string(),
                                original_name: Some("@cImport".to_string()),
                            });
                        }
                        search = after;
                    }
                    return;
                }
            }
            "builtin_call_expr" | "BuiltinCallExpr" => {
                let text = self.node_text(node, source);
                if text.starts_with("@import(") {
                    let module = text
                        .trim_start_matches("@import(")
                        .trim_end_matches(')')
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    if !module.is_empty() {
                        let local_name = module.rsplit('.').next().unwrap_or(&module).to_string();
                        imports.push(ImportInfo {
                            local_name,
                            source_module: module,
                            original_name: Some("@import".to_string()),
                        });
                    }
                    return;
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_zig_imports(child, source, imports);
        }
    }
}
