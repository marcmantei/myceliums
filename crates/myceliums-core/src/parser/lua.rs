//! Lua extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // ─── Lua ───────────────────────────────────────────────────────────

    pub(super) fn extract_lua_symbols(
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
                    let sym_kind = if name.contains(':') {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "local_function" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "function_definition_statement" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if name.contains(':') {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_lua_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "local_variable_declaration" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator"
                        || child.kind() == "identifier"
                        || child.kind() == "name"
                    {
                        let name = self.node_text(child, source).to_string();
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
                }
                return;
            }
            "assignment_statement" => {
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_list" || child.kind() == "variable" {
                        let name = self.node_text(child, source).to_string();
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
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_lua_symbols(child, source, symbols, parent);
        }
    }

    fn extract_lua_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('\n') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_lua_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        let fn_name = match kind {
            "function_declaration" | "local_function" | "function_definition_statement" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "function_call" {
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
            } else if let Some(method_node) = node.child_by_field_name("method") {
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
            } else if let Some(first_child) = node.child(0) {
                let callee = self.node_text(first_child, source).to_string();
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
            self.extract_lua_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_lua_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "function_call" {
            let func_text = if let Some(name_node) = node.child_by_field_name("name") {
                Some(self.node_text(name_node, source).to_string())
            } else {
                node.child(0)
                    .map(|first_child| self.node_text(first_child, source).to_string())
            };

            if let Some(func_name) = func_text {
                if func_name == "require" {
                    if let Some(args) = node.child_by_field_name("arguments") {
                        let arg_text = self.node_text(args, source);
                        let module = arg_text
                            .trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
                            .trim_matches('\'')
                            .trim_matches('"')
                            .to_string();
                        if !module.is_empty() {
                            let local_name =
                                module.rsplit('.').next().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: Some("require".to_string()),
                            });
                        }
                    } else {
                        let cursor = &mut node.walk();
                        for child in node.children(cursor) {
                            if child.kind() == "string" || child.kind() == "string_literal" {
                                let module = self
                                    .node_text(child, source)
                                    .trim_matches('\'')
                                    .trim_matches('"')
                                    .to_string();
                                if !module.is_empty() {
                                    let local_name =
                                        module.rsplit('.').next().unwrap_or(&module).to_string();
                                    imports.push(ImportInfo {
                                        local_name,
                                        source_module: module,
                                        original_name: Some("require".to_string()),
                                    });
                                }
                                break;
                            }
                        }
                    }
                    return;
                }
            }
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_lua_imports(child, source, imports);
        }
    }
}
