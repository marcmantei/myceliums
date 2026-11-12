//! JVM-family extraction: Java, Kotlin and Scala.

use super::{CallReference, ImportInfo, ParsedSymbol, SourceParser};
use myceliums_storage::SymbolKind;
use tree_sitter::Node;

impl SourceParser {
    // --- Java ---

    pub(super) fn extract_java_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class body with class name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
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
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into interface body with interface name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
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
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into enum body with enum name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "record_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into record body with record name as parent
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_java_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "annotation_type_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_java_signature(node, source),
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
                        signature: self.extract_java_signature(node, source),
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
                        signature: self.extract_java_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "field_declaration" => {
                // field_declaration has a "declarator" field containing variable_declarator(s)
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
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
                return; // Don't recurse further
            }
            "constant_declaration" => {
                // static final fields sometimes parsed as constant_declaration
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "variable_declarator" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = self.node_text(name_node, source).to_string();
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

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_symbols(child, source, symbols, parent);
        }
    }

    fn extract_java_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_java_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "method_declaration" | "constructor_declaration" => node
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        match kind {
            "method_invocation" => {
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
                }
            }
            "object_creation_expression" => {
                // new ClassName(args) — extract the type name as callee
                if let Some(type_node) = node.child_by_field_name("type") {
                    let type_name = self.node_text(type_node, source).to_string();
                    // Strip generic parameters (e.g., `ArrayList<String>` -> `ArrayList`)
                    let clean_name = type_name
                        .split('<')
                        .next()
                        .unwrap_or(&type_name)
                        .trim()
                        .to_string();
                    if let Some(caller) = scope {
                        calls.push(CallReference {
                            caller_name: caller.to_string(),
                            callee_name: clean_name,
                            line: node.start_position().row as u32 + 1,
                            chain: None,
                            file: None,
                        });
                    }
                }
            }
            "explicit_constructor_invocation" => {
                // super(...) or this(...) calls
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    let child_text = self.node_text(child, source);
                    if child_text == "super" || child_text == "this" {
                        if let Some(caller) = scope {
                            calls.push(CallReference {
                                caller_name: caller.to_string(),
                                callee_name: child_text.to_string(),
                                line: node.start_position().row as u32 + 1,
                                chain: None,
                                file: None,
                            });
                        }
                        break;
                    }
                }
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_java_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "import_declaration" => {
                // Get the full import text to detect wildcards and static imports
                let full_text = self.node_text(node, source).to_string();
                let is_static = full_text.contains("static ");

                // Find the scoped_identifier or identifier child
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        let import_path = self.node_text(child, source).to_string();

                        // Check if this is a wildcard import (has asterisk sibling)
                        let is_wildcard = full_text.contains(".*");

                        if is_wildcard {
                            let module_path = import_path.clone();
                            imports.push(ImportInfo {
                                local_name: "*".to_string(),
                                source_module: format!("{}.*", module_path),
                                original_name: None,
                            });
                        } else {
                            // Regular import: `import com.example.MyClass;`
                            let local_name = import_path
                                .rsplit('.')
                                .next()
                                .unwrap_or(&import_path)
                                .to_string();
                            imports.push(ImportInfo {
                                local_name,
                                source_module: import_path,
                                original_name: if is_static {
                                    Some("static".to_string())
                                } else {
                                    None
                                },
                            });
                        }
                        break;
                    }
                }
                return;
            }
            "package_declaration" => {
                // Track the package as module info
                let cursor = &mut node.walk();
                for child in node.children(cursor) {
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        let package_name = self.node_text(child, source).to_string();
                        imports.push(ImportInfo {
                            local_name: package_name.clone(),
                            source_module: package_name,
                            original_name: Some("package".to_string()),
                        });
                        break;
                    }
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_java_imports(child, source, imports);
        }
    }

    // --- Kotlin ---

    /// Check if a class_declaration is actually an interface
    fn kotlin_is_interface(&self, node: Node) -> bool {
        let mut cursor = node.walk();
        let result = node.children(&mut cursor).any(|c| c.kind() == "interface");
        result
    }

    pub(super) fn extract_kotlin_symbols(
        &self,
        node: Node,
        source: &[u8],
        symbols: &mut Vec<ParsedSymbol>,
        parent: Option<&str>,
    ) {
        let kind = node.kind();
        match kind {
            "class_declaration" => {
                // In tree-sitter-kotlin, both classes and interfaces use class_declaration.
                // Interfaces have an "interface" keyword child instead of "class".
                let is_interface = self.kotlin_is_interface(node);

                // The name is a direct `identifier` child (not field-based)
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    let name = self.node_text(name_node, source).to_string();
                    let sym_kind = if is_interface {
                        SymbolKind::Interface
                    } else {
                        SymbolKind::Class
                    };
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: sym_kind,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class_body with class/interface name as parent
                    if let Some(body) = self.find_child_by_kind(node, "class_body") {
                        self.extract_kotlin_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "object_declaration" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Class,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    // Recurse into class_body with object name as parent
                    if let Some(body) = self.find_child_by_kind(node, "class_body") {
                        self.extract_kotlin_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "companion_object" => {
                // companion object { ... } — use the parent class name or "Companion"
                let companion_name = self
                    .find_child_by_kind(node, "identifier")
                    .map(|n| self.node_text(n, source).to_string())
                    .unwrap_or_else(|| "Companion".to_string());
                let effective_parent = parent.unwrap_or(&companion_name);
                // Recurse into class_body with parent's name so methods appear under the class
                if let Some(body) = self.find_child_by_kind(node, "class_body") {
                    self.extract_kotlin_symbols(body, source, symbols, Some(effective_parent));
                    return;
                }
            }
            "function_declaration" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
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
                        signature: self.extract_kotlin_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "property_declaration" => {
                // val/var declarations — the name is in a variable_declaration child
                if let Some(var_decl) = self.find_child_by_kind(node, "variable_declaration") {
                    if let Some(name_node) = self.find_child_by_kind(var_decl, "identifier") {
                        let name = self.node_text(name_node, source).to_string();
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
                return; // Don't recurse further
            }
            "enum_entry" => {
                if let Some(name_node) = self.find_child_by_kind(node, "identifier") {
                    symbols.push(ParsedSymbol {
                        name: self.node_text(name_node, source).to_string(),
                        kind: SymbolKind::Enum,
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
            self.extract_kotlin_symbols(child, source, symbols, parent);
        }
    }

    fn extract_kotlin_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_kotlin_calls(
        &self,
        node: Node,
        source: &[u8],
        calls: &mut Vec<CallReference>,
        current_fn: Option<&str>,
    ) {
        let kind = node.kind();

        // Track current function/method scope
        let fn_name = match kind {
            "function_declaration" => self
                .find_child_by_kind(node, "identifier")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(first_child) = node.child(0) {
                let callee = match first_child.kind() {
                    "simple_identifier" | "identifier" => {
                        self.node_text(first_child, source).to_string()
                    }
                    "navigation_expression" => {
                        // obj.method() — extract the last simple_identifier
                        let nav_cursor = &mut first_child.walk();
                        let mut last_name = self.node_text(first_child, source).to_string();
                        for nav_child in first_child.children(nav_cursor) {
                            if nav_child.kind() == "simple_identifier" {
                                last_name = self.node_text(nav_child, source).to_string();
                            }
                        }
                        last_name
                    }
                    _ => self.node_text(first_child, source).to_string(),
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
            self.extract_kotlin_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_kotlin_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        match node.kind() {
            "import" => {
                // import kotlinx.coroutines.flow.Flow
                // import java.util.UUID as JavaUUID
                let cursor = &mut node.walk();
                let mut import_path = String::new();
                let mut alias: Option<String> = None;

                for child in node.children(cursor) {
                    if child.kind() == "qualified_identifier" {
                        import_path = self.node_text(child, source).to_string();
                    }
                    // `as` keyword followed by an identifier for aliased imports
                    if child.kind() == "identifier" {
                        // This is the alias after `as`
                        alias = Some(self.node_text(child, source).to_string());
                    }
                }

                if !import_path.is_empty() {
                    let is_wildcard =
                        import_path.ends_with(".*") || self.node_text(node, source).contains(".*");
                    if is_wildcard {
                        imports.push(ImportInfo {
                            local_name: "*".to_string(),
                            source_module: import_path,
                            original_name: None,
                        });
                    } else {
                        let local_name = if let Some(ref a) = alias {
                            a.clone()
                        } else {
                            import_path
                                .rsplit('.')
                                .next()
                                .unwrap_or(&import_path)
                                .to_string()
                        };
                        imports.push(ImportInfo {
                            local_name,
                            source_module: import_path,
                            original_name: alias,
                        });
                    }
                }
                return;
            }
            "package_header" => {
                // package com.example.mypackage
                if let Some(qi) = self.find_child_by_kind(node, "qualified_identifier") {
                    let package_name = self.node_text(qi, source).to_string();
                    imports.push(ImportInfo {
                        local_name: package_name.clone(),
                        source_module: package_name,
                        original_name: Some("package".to_string()),
                    });
                }
                return;
            }
            _ => {}
        }

        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_kotlin_imports(child, source, imports);
        }
    }

    // ── Scala ───────────────────────────────────────────────────────────

    fn extract_scala_signature(&self, node: Node, source: &[u8]) -> String {
        let text = self.node_text(node, source);
        // For Scala, signature is up to the opening brace or `=`
        if let Some(pos) = text.find('{') {
            text[..pos].trim().to_string()
        } else if let Some(pos) = text.find('=') {
            text[..pos + 1].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").trim().to_string()
        }
    }

    pub(super) fn extract_scala_symbols(
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
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "object_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Module,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "trait_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name: name.clone(),
                        kind: SymbolKind::Interface,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                    if let Some(body) = node.child_by_field_name("body") {
                        self.extract_scala_symbols(body, source, symbols, Some(&name));
                        return;
                    }
                }
            }
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.node_text(name_node, source).to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: self.extract_scala_signature(node, source),
                        content: self.node_text(node, source).to_string(),
                        parent_name: parent.map(|s| s.to_string()),
                        metadata: None,
                    });
                }
            }
            "val_definition" | "var_definition" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    let name = self.node_text(pattern, source).to_string();
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
            _ => {}
        }

        // Default: recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_symbols(child, source, symbols, parent);
        }
    }

    pub(super) fn extract_scala_calls(
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
                .child_by_field_name("name")
                .map(|n| self.node_text(n, source).to_string()),
            _ => None,
        };

        let scope = fn_name.as_deref().or(current_fn);

        if kind == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let callee_text = self.node_text(func_node, source).to_string();
                let callee = callee_text
                    .rsplit('.')
                    .next()
                    .unwrap_or(&callee_text)
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

        // Recurse into children
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_calls(child, source, calls, scope);
        }
    }

    pub(super) fn extract_scala_imports(
        &self,
        node: Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        if node.kind() == "import_declaration" {
            let text = self.node_text(node, source).to_string();
            let path = text.trim_start_matches("import").trim();

            if path.contains('{') {
                // Grouped imports: import foo.{Bar, Baz}
                if let Some(brace_start) = path.find('{') {
                    let prefix = path[..brace_start].trim_end_matches('.').to_string();
                    let inner = &path[brace_start + 1..];
                    let inner = inner.trim_end_matches('}');
                    for item in inner.split(',') {
                        let item = item.trim();
                        if item.is_empty() {
                            continue;
                        }
                        // Handle rename: Bar => RenamedBar
                        if item.contains("=>") {
                            let parts: Vec<&str> = item.split("=>").collect();
                            if parts.len() == 2 {
                                let original = parts[0].trim().to_string();
                                let local = parts[1].trim().to_string();
                                imports.push(ImportInfo {
                                    local_name: local,
                                    source_module: format!("{}.{}", prefix, original),
                                    original_name: Some(original),
                                });
                            }
                        } else {
                            let full_path = format!("{}.{}", prefix, item);
                            imports.push(ImportInfo {
                                local_name: item.to_string(),
                                source_module: full_path,
                                original_name: None,
                            });
                        }
                    }
                }
            } else {
                // Simple import
                let local_name = path.rsplit('.').next().unwrap_or(path).to_string();
                imports.push(ImportInfo {
                    local_name,
                    source_module: path.to_string(),
                    original_name: None,
                });
            }
            return;
        }

        // Recurse
        let cursor = &mut node.walk();
        for child in node.children(cursor) {
            self.extract_scala_imports(child, source, imports);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{SourceLanguage, SourceParser};
    use myceliums_storage::SymbolKind;

    // --- Java tests ---

    #[test]
    fn test_parse_java_class() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class UserService {
    public String getUser(String id) {
        return null;
    }
    public void deleteUser(String id) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let class_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "UserService")
            .unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        let method_sym = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(method_sym.kind, SymbolKind::Method);
        assert_eq!(method_sym.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_java_constructor() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class Server {
    private int port;

    public Server(int port) {
        this.port = port;
    }

    public void start() {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Server"));
        assert!(names.contains(&"Server")); // constructor
        assert!(names.contains(&"start"));
        assert!(names.contains(&"port"));

        let constructor = result
            .symbols
            .iter()
            .find(|s| s.name == "Server" && s.kind == SymbolKind::Method)
            .unwrap();
        assert_eq!(constructor.parent_name.as_deref(), Some("Server"));
    }

    #[test]
    fn test_parse_java_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public interface Repository {
    Object findById(String id);
    void save(Object entity);
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let iface = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);

        let method = result
            .symbols
            .iter()
            .find(|s| s.name == "findById")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.parent_name.as_deref(), Some("Repository"));
    }

    #[test]
    fn test_parse_java_enum() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
enum Status {
    ACTIVE,
    INACTIVE,
    ERROR
}
"#,
            )
            .unwrap();
        assert_eq!(
            result.symbols.iter().filter(|s| s.name == "Status").count(),
            1
        );
        let status = result.symbols.iter().find(|s| s.name == "Status").unwrap();
        assert_eq!(status.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_java_record() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
record Point(int x, int y) {
    public double distance() {
        return Math.sqrt(x * x + y * y);
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"distance"));

        let point = result.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_java_field() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class Config {
    private String host;
    private int port;
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"host"));
        assert!(names.contains(&"port"));

        let host = result.symbols.iter().find(|s| s.name == "host").unwrap();
        assert_eq!(host.kind, SymbolKind::Variable);
        assert_eq!(host.parent_name.as_deref(), Some("Config"));
    }

    #[test]
    fn test_call_extraction_java() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
public class App {
    public void main() {
        String result = processData(input);
        System.out.println(result);
        List<String> items = new ArrayList<>();
    }
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
        assert!(result.calls.iter().any(|c| c.callee_name == "ArrayList"));
    }

    #[test]
    fn test_import_extraction_java() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
package com.example.app;

import java.util.List;
import java.util.Map;
import java.io.*;
"#,
            )
            .unwrap();
        assert!(result.imports.iter().any(|i| i.local_name == "List"));
        assert!(result.imports.iter().any(|i| i.local_name == "Map"));
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "*" && i.source_module.contains("java.io")));
        // Package declaration tracked
        assert!(result
            .imports
            .iter()
            .any(|i| i.source_module == "com.example.app"
                && i.original_name.as_deref() == Some("package")));
    }

    #[test]
    fn test_import_extraction_java_static() {
        let mut parser = SourceParser::new(SourceLanguage::Java).unwrap();
        let result = parser
            .parse(
                r#"
import static java.lang.Math.PI;
"#,
            )
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].local_name, "PI");
        assert_eq!(result.imports[0].original_name.as_deref(), Some("static"));
    }

    // --- Kotlin tests ---

    #[test]
    fn test_parse_kotlin_data_class() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse("data class User(val id: String, val name: String)")
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "User");
        assert_eq!(result.symbols[0].kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_kotlin_class_with_methods() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    fun getUser(id: String): User? {
        return null
    }
    fun deleteUser(id: String) {
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"deleteUser"));

        let get_user = result.symbols.iter().find(|s| s.name == "getUser").unwrap();
        assert_eq!(get_user.kind, SymbolKind::Method);
        assert_eq!(get_user.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_kotlin_object() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
object AppConfig {
    val version = "1.0.0"
    fun getEnvironment(): String {
        return "development"
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"AppConfig"));
        assert!(names.contains(&"getEnvironment"));

        let config = result
            .symbols
            .iter()
            .find(|s| s.name == "AppConfig")
            .unwrap();
        assert_eq!(config.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_kotlin_interface() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
interface Repository<T> {
    fun findById(id: String): T?
    fun save(entity: T): T
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Repository"));
        assert!(names.contains(&"findById"));
        assert!(names.contains(&"save"));

        let repo = result
            .symbols
            .iter()
            .find(|s| s.name == "Repository")
            .unwrap();
        assert_eq!(repo.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_parse_kotlin_companion_object() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
class UserService {
    companion object {
        fun create(): UserService {
            return UserService()
        }
    }
}
"#,
            )
            .unwrap();
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"create"));

        let create = result.symbols.iter().find(|s| s.name == "create").unwrap();
        assert_eq!(create.kind, SymbolKind::Method);
        assert_eq!(create.parent_name.as_deref(), Some("UserService"));
    }

    #[test]
    fn test_parse_kotlin_top_level_function() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
fun main() {
    println("Hello")
}
"#,
            )
            .unwrap();
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "main");
        assert_eq!(result.symbols[0].kind, SymbolKind::Function);
        assert_eq!(result.symbols[0].parent_name, None);
    }

    #[test]
    fn test_call_extraction_kotlin() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
fun main() {
    val result = processData(input)
    println(result)
    config.getEnvironment()
}
"#,
            )
            .unwrap();
        assert!(result.calls.iter().any(|c| c.callee_name == "processData"));
        assert!(result.calls.iter().any(|c| c.callee_name == "println"));
    }

    #[test]
    fn test_import_extraction_kotlin() {
        let mut parser = SourceParser::new(SourceLanguage::Kotlin).unwrap();
        let result = parser
            .parse(
                r#"
package com.example.myapp

import kotlinx.coroutines.flow.Flow
import java.util.UUID as JavaUUID
"#,
            )
            .unwrap();
        assert!(result
            .imports
            .iter()
            .any(|i| i.local_name == "com.example.myapp"
                && i.original_name.as_deref() == Some("package")));
        assert!(result.imports.iter().any(|i| i.local_name == "Flow"));
    }

    #[test]
    fn test_parse_kotlin_extension() {
        let lang = SourceLanguage::from_extension("kt");
        assert_eq!(lang, Some(SourceLanguage::Kotlin));
        let lang = SourceLanguage::from_extension("kts");
        assert_eq!(lang, Some(SourceLanguage::Kotlin));
    }
}
