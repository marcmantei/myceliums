//! Elixir extraction.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // ── Elixir ──────────────────────────────────────────────────────────

    fn extract_elixir_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Elixir, signature is up to "do" keyword or first line
        if let Some(pos) = text.find(" do") {
            text[..pos + 3].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_elixir_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();

        if kind == "call" {
            // In Elixir tree-sitter, most definitions are `call` nodes.
            // The first child (target) tells us what kind of definition it is.
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "defmodule" => {
                        // defmodule MyModule do ... end
                        if let Some(arg) = node.child(1) {
                            let name = self.node_text(arg, source).to_string();
                            let name = name.trim().to_string();
                            symbols.push(ParsedSymbol {
                                name: name.clone(),
                                kind: SymbolKind::Module,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_elixir_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                            // Recurse into the module body with module name as parent
                            let cursor = &mut node.walk();
                            for child in node.children(cursor) {
                                self.extract_elixir_symbols(child, source, symbols, Some(&name));
                            }
                            return;
                        }
                    }
                    "def" | "defp" | "defmacro" | "defmacrop" => {
                        // def function_name(args) do ... end
                        if let Some(arg) = node.child(1) {
                            let arg_text = self.node_text(arg, source).to_string();
                            let fn_name = arg_text
                                .split('(')
                                .next()
                                .unwrap_or(&arg_text)
                                .trim()
                                .to_string();
                            symbols.push(ParsedSymbol {
                                name: fn_name,
                                kind: SymbolKind::Function,
                                start_line: node.start_position().row as u32 + 1,
                                end_line: node.end_position().row as u32 + 1,
                                signature: self.extract_elixir_signature(node, source),
                                content: self.node_text(node, source).to_string(),
                                parent_name: parent.map(|s| s.to_string()),
                                metadata: None,
                            });
                        }
                        return; // Don't recurse into function bodies for symbols
                    }
                    "defstruct" => {
                        // defstruct [:field1, :field2]
                        symbols.push(ParsedSymbol {
                            name: parent.unwrap_or("anonymous").to_string(),
                            kind: SymbolKind::Class,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            signature: self.extract_elixir_signature(node, source),
                            content: self.node_text(node, source).to_string(),
                            parent_name: parent.map(|s| s.to_string()),
                            metadata: None,
                        });
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_elixir_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function scope
        let fn_name = if kind == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "def" | "defp" | "defmacro" | "defmacrop" => node.child(1).map(|arg| {
                        let arg_text = self.node_text(arg, source).to_string();
                        arg_text
                            .split('(')
                            .next()
                            .unwrap_or(&arg_text)
                            .trim()
                            .to_string()
                    }),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source).to_string();
                // Skip definition keywords and import keywords
                match target_text.as_str() {
                    "defmodule" | "def" | "defp" | "defmacro" | "defmacrop" | "defstruct"
                    | "use" | "import" | "alias" | "require" => {}
                    _ => {
                        // This is a regular function call
                        let callee = target_text
                            .rsplit('.')
                            .next()
                            .unwrap_or(&target_text)
                            .to_string();
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

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_elixir_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "call" {
            if let Some(target) = node.child(0) {
                let target_text = self.node_text(target, source);
                match target_text {
                    "use" | "import" | "alias" | "require" => {
                        // use Phoenix.Controller
                        // import Ecto.Query
                        // alias MyApp.Accounts.User
                        // require Logger
                        if let Some(arg) = node.child(1) {
                            let module = self.node_text(arg, source).to_string();
                            let module = module.trim().to_string();
                            let local_name =
                                module.rsplit('.').next().unwrap_or(&module).to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: module,
                                original_name: None,
                            });
                        }
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_elixir_imports(child, source, imports);
        }
    }
}
