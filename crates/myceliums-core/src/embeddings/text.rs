//! The documents symbols are embedded from (#92, #157).
//!
//! A stored vector encodes whatever text it was computed from, so the text is
//! versioned: an index records the [`TextTemplate`] its vectors were built
//! with ([`super::IndexEmbeddingMeta::text_version`]), and the fingerprint
//! covers it, so two templates never share an index.
//!
//! - **Template 1** ([`TextTemplate::Flat`]) — `{kind} {name} {decorators}
//!   {signature} {return_type} {superclasses} {content_head}`, the content cut
//!   to a byte budget derived from the model's token limit. The only template
//!   before #157; kept so indexes built with it stay queryable and
//!   incremental runs on them stay consistent.
//! - **Template 2** ([`TextTemplate::Structured`]) — a header of the most
//!   telling lines first, then the body:
//!
//!   ```text
//!   python method src/requests/sessions.py::SessionRedirectMixin.resolve_redirects
//!   session redirect mixin resolve redirects
//!   def resolve_redirects(self, resp, req, stream=False, ...)
//!   Receives a Response. Returns a generator of Responses or Requests.
//!   hist = []  # keep track of history
//!   ...
//!   ```
//!
//!   language, kind and `file::qualified.name`; the qualified name split into
//!   words; decorators and the signature; the doc comment or docstring; then
//!   the body without the signature line and docstring it repeats, every line
//!   trimmed and blank lines dropped (indentation costs tokens and carries no
//!   meaning for retrieval). The window cuts the end of the text, never the
//!   header.

use myceliums_storage::{CodeSymbol, SymbolKind, SymbolMetadata};

/// A version of the embedding-document template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextTemplate {
    /// Template 1: the flat, byte-budgeted document of #92.
    Flat,
    /// Template 2: structured header first, normalised body (#157).
    Structured,
}

impl TextTemplate {
    /// Every template this build can produce.
    pub const ALL: [TextTemplate; 2] = [TextTemplate::Flat, TextTemplate::Structured];

    /// The template of a recorded version; `None` for one this build does not
    /// know (`0`: unrecorded, written before #92).
    pub fn from_version(version: u32) -> Option<Self> {
        match version {
            1 => Some(Self::Flat),
            2 => Some(Self::Structured),
            _ => None,
        }
    }

    /// The version recorded in an index.
    pub fn version(self) -> u32 {
        match self {
            Self::Flat => 1,
            Self::Structured => 2,
        }
    }

    /// Whether symbols embedded with this template can be chunked: template 1
    /// predates chunking and stays one vector per symbol.
    pub fn supports_chunking(self) -> bool {
        self == Self::Structured
    }
}

/// Most bytes of a doc comment kept in the header, so a long doc cannot push
/// the signature and body out of the window.
pub const DOC_BYTES: usize = 1_000;

/// Most bytes of a signature kept in the header.
pub const SIGNATURE_BYTES: usize = 512;

/// A symbol's embedding document under [`TextTemplate::Structured`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The first header line: language, kind, `file::qualified.name`. Every
    /// body chunk of the symbol starts with it.
    pub context: String,
    /// The whole header, `context` included, one item per line.
    pub header: String,
    /// The normalised body.
    pub body: String,
    /// Whether the body may be split into chunks: not for containers, whose
    /// members are symbols with vectors of their own.
    pub chunkable: bool,
}

impl Document {
    /// The full document text: header, then body.
    pub fn text(&self) -> String {
        if self.body.is_empty() {
            self.header.clone()
        } else {
            format!("{}\n{}", self.header, self.body)
        }
    }
}

/// Whether symbols of `kind` hold other symbols, which carry their own
/// vectors: their bodies are not chunked.
pub fn is_container(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum | SymbolKind::Module
    )
}

/// Build the structured (template 2) document of `symbol`.
pub fn structured(symbol: &CodeSymbol) -> Document {
    let meta: Option<SymbolMetadata> = symbol
        .metadata
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());
    let language = language_name(&symbol.file_path);
    let kind = symbol.kind.to_string().to_lowercase();
    let context = format!(
        "{language} {kind} {}::{}",
        symbol.file_path, symbol.qualified_name
    );

    let mut header = vec![context.clone()];
    let words = crate::tokenize::tokenize(&symbol.qualified_name).join(" ");
    // A one-word name split into words is just itself, lowercased.
    if !words.is_empty() && words != symbol.qualified_name.to_lowercase() {
        header.push(words);
    }
    let signature = collapse_whitespace(symbol.signature.trim());
    let decorators = meta
        .as_ref()
        .map(|m| m.decorators.join(" "))
        .unwrap_or_default();
    let declaration = match (decorators.is_empty(), signature.is_empty()) {
        (true, true) => String::new(),
        (true, false) => signature.clone(),
        (false, true) => decorators,
        (false, false) => format!("{decorators} {signature}"),
    };
    if !declaration.is_empty() {
        header.push(cut(&declaration, SIGNATURE_BYTES).to_string());
    }

    // The body without the signature it starts with.
    let mut body = symbol.content.as_str();
    let trimmed = body.trim_start();
    if !symbol.signature.trim().is_empty() && trimmed.starts_with(symbol.signature.trim()) {
        body = &trimmed[symbol.signature.trim().len()..];
    }
    let mut docs: Vec<String> = Vec::new();
    if let Some(doc) = meta.as_ref().and_then(|m| m.doc.as_deref()) {
        docs.push(doc.to_string());
    }
    if language == "python" {
        if let Some((docstring, rest)) = python_docstring(body) {
            docs.push(docstring);
            body = rest;
        }
    }
    let doc = collapse_whitespace(&docs.join(" "));
    if !doc.is_empty() {
        header.push(cut(&doc, DOC_BYTES).to_string());
    }

    Document {
        context,
        header: header.join("\n"),
        body: normalize_body(body),
        chunkable: !is_container(&symbol.kind),
    }
}

/// The language a file is written in, by extension (`python`, `go`, ...), or
/// `code` when the extension is unknown.
fn language_name(file_path: &str) -> &'static str {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    match crate::parser::SourceLanguage::from_extension(ext) {
        Some(lang) => static_language_name(lang),
        None => "code",
    }
}

fn static_language_name(lang: crate::parser::SourceLanguage) -> &'static str {
    use crate::parser::SourceLanguage as L;
    match lang {
        L::TypeScript | L::Tsx => "typescript",
        L::JavaScript => "javascript",
        L::Python => "python",
        L::Go => "go",
        L::Rust => "rust",
        L::Java => "java",
        L::CSharp => "csharp",
        L::C => "c",
        L::Cpp => "cpp",
        L::Ruby => "ruby",
        L::Kotlin => "kotlin",
        L::Swift => "swift",
        L::Php => "php",
        L::Lua => "lua",
        L::Zig => "zig",
        L::PowerShell => "powershell",
        L::Elixir => "elixir",
        L::Scala => "scala",
        L::ObjectiveC => "objective-c",
        L::Dart => "dart",
        L::Vue => "vue",
        L::Svelte => "svelte",
        L::Markdown | L::Mdx => "markdown",
        L::PlainText => "text",
        L::Jupyter => "notebook",
        #[cfg(feature = "pdf")]
        L::Pdf => "pdf",
        L::Email | L::Mbox => "email",
    }
}

/// A Python body's docstring and the body after it: after the `:` that ends
/// the signature, the first statement when it is a string literal.
fn python_docstring(body: &str) -> Option<(String, &str)> {
    let rest = body.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let prefix_len = rest
        .char_indices()
        .take_while(|(_, c)| matches!(c, 'r' | 'R' | 'u' | 'U' | 'b' | 'B'))
        .count()
        .min(2);
    let literal = &rest[prefix_len..];
    for quote in ["\"\"\"", "'''", "\"", "'"] {
        if let Some(inner) = literal.strip_prefix(quote) {
            let end = inner.find(quote)?;
            // A one-quote string ends on its line.
            if quote.len() == 1 && inner[..end].contains('\n') {
                return None;
            }
            return Some((inner[..end].to_string(), &inner[end + quote.len()..]));
        }
    }
    None
}

/// Trim every line, drop blank ones, collapse runs of whitespace.
pub fn normalize_body(body: &str) -> String {
    body.lines()
        .map(|line| collapse_whitespace(line.trim()))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace every run of whitespace (newlines included) with one space.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to at most `max` bytes on a character boundary.
pub fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(
        name: &str,
        qualified: &str,
        kind: SymbolKind,
        file: &str,
        signature: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> CodeSymbol {
        CodeSymbol {
            uid: "u".into(),
            name: name.into(),
            qualified_name: qualified.into(),
            kind,
            file_path: file.into(),
            start_line: 1,
            end_line: 9,
            signature: signature.into(),
            content: content.into(),
            repo_id: "r".into(),
            metadata: metadata.map(str::to_string),
        }
    }

    #[test]
    fn versions_round_trip_and_unknown_ones_are_refused() {
        for template in TextTemplate::ALL {
            assert_eq!(
                TextTemplate::from_version(template.version()),
                Some(template)
            );
        }
        assert_eq!(TextTemplate::from_version(0), None);
        assert_eq!(TextTemplate::from_version(99), None);
        assert!(!TextTemplate::Flat.supports_chunking());
        assert!(TextTemplate::Structured.supports_chunking());
    }

    #[test]
    fn python_method_puts_header_and_docstring_first() {
        let content = "def get_user(self, user_id):\n    \"\"\"Look up a user\n    by id.\"\"\"\n    return self.db.find(user_id)\n";
        let doc = structured(&symbol(
            "get_user",
            "UserService.get_user",
            SymbolKind::Method,
            "services/user_service.py",
            "def get_user(self, user_id)",
            content,
            Some(r#"{"decorators":["@cached"]}"#),
        ));
        assert_eq!(
            doc.text(),
            "python method services/user_service.py::UserService.get_user\n\
             user service get user\n\
             @cached def get_user(self, user_id)\n\
             Look up a user by id.\n\
             return self.db.find(user_id)"
        );
        assert_eq!(
            doc.context,
            "python method services/user_service.py::UserService.get_user"
        );
        assert!(doc.chunkable);
    }

    #[test]
    fn leading_doc_comments_come_from_the_metadata() {
        let doc = structured(&symbol(
            "Next",
            "Context.Next",
            SymbolKind::Method,
            "context.go",
            "func (c *Context) Next()",
            "func (c *Context) Next() {\n\tc.index++\n\tfor c.index < int8(len(c.handlers)) {\n\t\tc.handlers[c.index](c)\n\t}\n}",
            Some(r#"{"doc":"Next should be used only inside middleware."}"#),
        ));
        assert_eq!(
            doc.text(),
            "go method context.go::Context.Next\n\
             context next\n\
             func (c *Context) Next()\n\
             Next should be used only inside middleware.\n\
             {\n\
             c.index++\n\
             for c.index < int8(len(c.handlers)) {\n\
             c.handlers[c.index](c)\n\
             }\n\
             }"
        );
    }

    #[test]
    fn containers_are_not_chunkable() {
        let doc = structured(&symbol(
            "Session",
            "Session",
            SymbolKind::Class,
            "sessions.py",
            "class Session(SessionRedirectMixin)",
            "class Session(SessionRedirectMixin):\n    pass",
            None,
        ));
        assert!(!doc.chunkable);
        // A one-word name adds no word line.
        assert_eq!(
            doc.header,
            "python class sessions.py::Session\nclass Session(SessionRedirectMixin)"
        );
    }

    #[test]
    fn long_docs_and_signatures_are_cut_on_char_boundaries() {
        let long_doc = "é".repeat(DOC_BYTES);
        let meta = serde_json::json!({ "doc": long_doc }).to_string();
        let doc = structured(&symbol(
            "f",
            "f",
            SymbolKind::Function,
            "a.ts",
            "function f()",
            "function f() {}",
            Some(&meta),
        ));
        let doc_line = doc.header.lines().nth(2).unwrap();
        assert!(doc_line.len() <= DOC_BYTES);
        assert!(doc_line.chars().all(|c| c == 'é'));
    }

    #[test]
    fn docstrings_are_recognised_in_their_python_forms() {
        assert_eq!(
            python_docstring(":\n    r'''Raw.'''\n    pass"),
            Some(("Raw.".to_string(), "\n    pass"))
        );
        assert_eq!(
            python_docstring(":\n    \"One line.\"\n    x = 1"),
            Some(("One line.".to_string(), "\n    x = 1"))
        );
        assert_eq!(python_docstring(":\n    x = '''not a docstring'''"), None);
        assert_eq!(python_docstring(":\n    \"unterminated"), None);
    }

    #[test]
    fn unknown_extensions_read_as_code() {
        assert_eq!(language_name("Makefile"), "code");
        assert_eq!(language_name("src/app.tsx"), "typescript");
    }
}
