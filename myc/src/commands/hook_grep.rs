//! `myc hook-grep`: a Claude Code PreToolUse hook that answers a grep for a
//! known symbol with the matching myceliums call (#126, layer 3).
//!
//! Claude Code runs the hook before a `Grep` tool call or a `Bash` call that
//! runs grep/rg, and passes the call as JSON on stdin. When the search pattern
//! looks like an identifier that the project's index defines, the hook prints
//! a PreToolUse JSON object whose `additionalContext` names the exact `myc`
//! command (e.g. "`myc callers parse_query` lists its 7 direct callers with
//! file:line"). It never blocks: no `permissionDecision` is set, so the tool
//! call goes through the normal permission flow, and every failure (malformed
//! input, no index, slow store) ends silently with exit code 0. Searches for
//! string literals, regexes, plain words and config files stay silent.
//!
//! Hook input and output follow the Claude Code hooks reference,
//! <https://code.claude.com/docs/en/hooks> (checked 2026-11-18):
//! * input: `hook_event_name`, `cwd`, `tool_name`, and `tool_input` with
//!   `pattern`/`path`/`glob`/`type` for Grep or `command` for Bash;
//! * output: `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
//!   "additionalContext": "..."}}` on stdout with exit 0. `additionalContext`
//!   is "added to Claude's context alongside the tool result"; the reference
//!   asks for factual phrasing rather than imperative instructions, and stdout
//!   must hold nothing but the JSON object.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use myceliums_storage::{RelationshipKind, RepoRegistry, Store, SymbolKind, SymbolLocation};

/// Default upper bound for the whole index lookup. The hook answers well
/// inside this on an indexed repository; if the store is slower (cold disk,
/// huge index, debug build) the hook gives up silently instead of delaying
/// the tool call. `MYC_HOOK_DEADLINE_MS` overrides it.
const LOOKUP_DEADLINE: Duration = Duration::from_millis(400);

/// Environment variable overriding [`LOOKUP_DEADLINE`], in milliseconds.
const DEADLINE_ENV: &str = "MYC_HOOK_DEADLINE_MS";

/// Environment variable that makes the hook say on stderr why it stayed
/// silent. Claude Code does not show a hook's stderr to the model on exit 0.
const DEBUG_ENV: &str = "MYC_HOOK_DEBUG";

/// At most this many identifier candidates are looked up per tool call.
const MAX_CANDIDATES: usize = 3;

/// Hook stdin larger than this is not a tool call we care about.
const MAX_INPUT_BYTES: u64 = 1 << 20;

/// An identifier-shaped search pattern worth looking up in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The name as the agent wrote it, possibly qualified (`Item.AsString`).
    pub query: String,
    /// The last segment, which is what the index stores as `name`.
    pub short: String,
}

/// Declaration keywords an agent puts in front of a name it greps for.
const DECL_KEYWORDS: &[&str] = &[
    "fn",
    "pub",
    "async",
    "def",
    "func",
    "function",
    "class",
    "struct",
    "interface",
    "trait",
    "enum",
    "type",
    "impl",
    "module",
    "mod",
    "export",
    "default",
    "static",
    "public",
    "private",
    "protected",
    "abstract",
    "override",
    "val",
    "var",
    "let",
    "const",
    "object",
    "protocol",
    "extension",
    "record",
    "void",
];

/// File types whose contents are configuration, prose or logs: a search
/// there is a literal search even when the pattern is identifier-shaped.
const NON_CODE_TYPES: &[&str] = &[
    "json",
    "yaml",
    "yml",
    "toml",
    "md",
    "markdown",
    "mdx",
    "txt",
    "log",
    "lock",
    "ini",
    "cfg",
    "conf",
    "env",
    "csv",
    "tsv",
    "xml",
    "properties",
    "sql",
];

fn is_non_code_glob(glob: &str) -> bool {
    let g = glob.trim().trim_matches(['\'', '"']).to_ascii_lowercase();
    NON_CODE_TYPES.iter().any(|ext| {
        g.ends_with(&format!(".{ext}"))
            || g.ends_with(&format!(".{ext}}}"))
            || g.contains(&format!("{{{ext},"))
            || g.contains(&format!(",{ext}}}"))
    }) || g.ends_with(".env")
        || g.contains("dockerfile")
}

fn is_non_code_type(t: &str) -> bool {
    NON_CODE_TYPES.contains(&t.trim().to_ascii_lowercase().as_str())
}

/// Removes one regex decoration layer that wraps an identifier.
fn strip_regex_decorations(pattern: &str) -> String {
    let mut s = pattern.trim().to_string();
    if let Some(rest) = s.strip_prefix('^') {
        s = rest.to_string();
    }
    if s.ends_with('$') && !s.ends_with("\\$") {
        s.pop();
    }
    for (from, to) in [
        ("\\b", ""),
        ("\\<", ""),
        ("\\>", ""),
        ("\\s+", " "),
        ("\\s*", " "),
        ("\\s", " "),
        (" +", " "),
        (" *", " "),
        ("\\.", "."),
        ("\\(", "("),
        ("\\)", ")"),
        ("\\*", "*"),
        ("\\:", ":"),
    ] {
        s = s.replace(from, to);
    }
    s
}

/// The identifier a single (non-alternation) pattern searches for, if any.
fn single_identifier(alternative: &str) -> Option<Candidate> {
    let mut s = strip_regex_decorations(alternative);

    // Leading declaration keywords (`fn x`, `pub async fn x`, `def x(`) and
    // the parenthesised group after one: a Go receiver
    // (`func (p *Pattern) MatchItem`) or a visibility (`pub(crate) fn x`).
    let mut structural = false;
    for _ in 0..2 {
        loop {
            let trimmed = s.trim_start();
            let word_end = trimmed
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(trimmed.len());
            let word = &trimmed[..word_end];
            let rest = &trimmed[word_end..];
            if !word.is_empty() && DECL_KEYWORDS.contains(&word) && rest.starts_with([' ', '(']) {
                structural = true;
                s = rest.to_string();
            } else {
                break;
            }
        }
        let trimmed = s.trim_start();
        if !(structural && trimmed.starts_with('(')) {
            break;
        }
        let close = trimmed.find(')')?;
        let group = &trimmed[1..close];
        if !group
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ' ' | '*' | '.'))
        {
            return None;
        }
        s = trimmed[close + 1..].to_string();
    }
    // A call or declaration parenthesis ends the name: `parse_query(`.
    if let Some(open) = s.find('(') {
        structural = true;
        s.truncate(open);
    }

    let word = s.trim();
    let word = word.strip_prefix('.').map_or(word, |w| {
        structural = true;
        w
    });
    if word.is_empty()
        || word.contains(char::is_whitespace)
        || !word
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | ':' | '#'))
    {
        return None;
    }
    let short = word
        .rsplit(['.', ':', '#'])
        .next()
        .filter(|seg| !seg.is_empty())?
        .to_string();
    if word.contains(['.', ':', '#']) {
        structural = true;
    }
    let first = short.chars().next()?;
    if short.chars().count() < 3 || !(first.is_alphabetic() || first == '_' || first == '$') {
        return None;
    }
    let has_lower = short.chars().any(char::is_lowercase);
    let has_upper = short.chars().any(char::is_uppercase);
    // snake_case, SCREAMING_SNAKE longer than 3 characters, camelCase or
    // PascalCase.
    let identifier_shaped =
        short.contains('_') && (has_lower || short.len() > 3) || has_lower && has_upper;
    // A plain lowercase word (`error`, `config`) is a text search unless the
    // pattern framed it as code (keyword, call parenthesis, qualifier).
    if !(identifier_shaped || structural && has_lower) {
        return None;
    }
    Some(Candidate {
        query: word.to_string(),
        short,
    })
}

/// Identifier candidates in a search pattern (regex or literal).
///
/// Top-level alternations (`foo|bar`, grep's `foo\|bar`) are split and each
/// branch is checked on its own. Anything with character classes, quantifiers
/// or spaces between words is a text search and yields nothing.
pub fn identifier_candidates(pattern: &str) -> Vec<Candidate> {
    if pattern.len() > 200 || pattern.contains('\n') {
        return vec![];
    }
    let mut out: Vec<Candidate> = Vec::new();
    for alt in pattern.replace("\\|", "|").split('|') {
        if let Some(c) = single_identifier(alt) {
            if !out.iter().any(|o| o.query == c.query) {
                out.push(c);
            }
        }
        if out.len() >= MAX_CANDIDATES {
            break;
        }
    }
    out
}

// ── Shell parsing for Bash tool calls ────────────────────────────────

/// Splits a shell command line into simple commands (word lists).
///
/// Handles single and double quotes, backslash escapes and the separators
/// `|`, `||`, `&&`, `;`, `&`, newlines and parentheses. Not a full shell
/// parser: substitutions and here-docs are treated as plain words, which at
/// worst makes the hook stay silent.
pub fn split_commands(line: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    let flush_word = |words: &mut Vec<String>, word: &mut String, in_word: &mut bool| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&n @ ('"' | '\\' | '$' | '`')) => {
                                word.push(n);
                                chars.next();
                            }
                            _ => word.push('\\'),
                        },
                        _ => word.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    if n != '\n' {
                        word.push(n);
                    }
                }
            }
            '|' | '&' | ';' | '\n' | '(' | ')' => {
                flush_word(&mut words, &mut word, &mut in_word);
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => flush_word(&mut words, &mut word, &mut in_word),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    flush_word(&mut words, &mut word, &mut in_word);
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

/// What a grep/rg invocation searches for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GrepCall {
    /// Patterns (`-e` values, or the first operand).
    pub patterns: Vec<String>,
    /// Whether a file filter restricts the search to config/prose files.
    pub non_code_filter: bool,
}

/// Parses one simple command; `None` unless it runs grep, egrep, fgrep, rg or
/// `git grep`.
pub fn parse_grep_command(words: &[String]) -> Option<GrepCall> {
    let mut i = 0;
    // Environment assignments and transparent wrappers.
    while i < words.len()
        && (words[i].contains('=') && !words[i].starts_with('-')
            || matches!(
                words[i].as_str(),
                "sudo" | "command" | "exec" | "nice" | "time"
            ))
    {
        i += 1;
    }
    let program = words.get(i)?;
    let base = Path::new(program)
        .file_name()?
        .to_string_lossy()
        .to_string();
    let rg = match base.as_str() {
        "rg" => true,
        "grep" | "egrep" | "fgrep" => false,
        "git" if words.get(i + 1).map(String::as_str) == Some("grep") => {
            i += 1;
            false
        }
        _ => return None,
    };
    i += 1;

    // Short options that take a value, per tool.
    let short_with_value: &[char] = if rg {
        &[
            'A', 'B', 'C', 'e', 'f', 'g', 'j', 'M', 'm', 'r', 't', 'T', 'E',
        ]
    } else {
        &['A', 'B', 'C', 'D', 'd', 'e', 'f', 'm']
    };
    const LONG_WITH_VALUE: &[&str] = &[
        "--regexp",
        "--file",
        "--glob",
        "--iglob",
        "--type",
        "--type-not",
        "--type-add",
        "--max-count",
        "--context",
        "--after-context",
        "--before-context",
        "--include",
        "--exclude",
        "--exclude-dir",
        "--replace",
        "--max-depth",
        "--max-columns",
        "--threads",
        "--encoding",
        "--sort",
        "--sortr",
        "--color",
        "--colors",
        "--devices",
        "--directories",
        "--binary-files",
        "--label",
        "--pre",
        "--path-separator",
    ];

    let mut call = GrepCall::default();
    let mut explicit = false;
    let mut operands = Vec::new();
    let mut options_done = false;
    while i < words.len() {
        let w = &words[i];
        i += 1;
        if options_done || !w.starts_with('-') || w == "-" {
            operands.push(w.clone());
            continue;
        }
        if w == "--" {
            options_done = true;
            continue;
        }
        if let Some(long) = w.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (format!("--{n}"), Some(v.to_string())),
                None => (w.clone(), None),
            };
            let value = if inline.is_some() || !LONG_WITH_VALUE.contains(&name.as_str()) {
                inline
            } else {
                let v = words.get(i).cloned();
                i += 1;
                v
            };
            if let Some(v) = value {
                match name.as_str() {
                    "--regexp" => {
                        explicit = true;
                        call.patterns.push(v);
                    }
                    "--file" => explicit = true,
                    "--glob" | "--iglob" | "--include" if is_non_code_glob(&v) => {
                        call.non_code_filter = true
                    }
                    "--type" if is_non_code_type(&v) => call.non_code_filter = true,
                    _ => {}
                }
            }
            continue;
        }
        // Short option cluster: `-rn`, `-e PAT`, `-ePAT`, `-A3`, `-g '*.json'`.
        let cluster: Vec<char> = w[1..].chars().collect();
        for (pos, opt) in cluster.iter().enumerate() {
            if !short_with_value.contains(opt) {
                continue;
            }
            let attached: String = cluster[pos + 1..].iter().collect();
            let value = if attached.is_empty() {
                let v = words.get(i).cloned();
                i += 1;
                v
            } else {
                Some(attached)
            };
            if let Some(v) = value {
                match opt {
                    'e' => {
                        explicit = true;
                        call.patterns.push(v);
                    }
                    'f' => explicit = true,
                    'g' if rg && is_non_code_glob(&v) => call.non_code_filter = true,
                    't' if rg && is_non_code_type(&v) => call.non_code_filter = true,
                    _ => {}
                }
            }
            break;
        }
    }
    if !explicit {
        if let Some(first) = operands.first() {
            call.patterns.push(first.clone());
        }
        // Searching named config files (`grep foo package.json`).
        if operands.len() > 1 && operands[1..].iter().all(|p| is_non_code_glob(p)) {
            call.non_code_filter = true;
        }
    }
    Some(call)
}

/// The tool call a hook invocation is about.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SearchCall {
    /// Directory the session runs in (`cwd` of the hook input).
    pub cwd: Option<PathBuf>,
    /// Identifier candidates across all patterns, in order, deduplicated.
    pub candidates: Vec<Candidate>,
}

/// Extracts identifier candidates from PreToolUse hook input. Returns an
/// empty candidate list for anything that is not a code search.
pub fn search_call_from_input(input: &serde_json::Value) -> SearchCall {
    let mut call = SearchCall {
        cwd: input["cwd"].as_str().map(PathBuf::from),
        ..SearchCall::default()
    };
    if let Some(event) = input["hook_event_name"].as_str() {
        if event != "PreToolUse" {
            return call;
        }
    }
    let tool_input = &input["tool_input"];
    let patterns: Vec<String> = match input["tool_name"].as_str() {
        Some("Grep") => {
            let glob = tool_input["glob"].as_str().unwrap_or("");
            let file_type = tool_input["type"].as_str().unwrap_or("");
            let path = tool_input["path"].as_str().unwrap_or("");
            if is_non_code_glob(glob) || is_non_code_type(file_type) || is_non_code_glob(path) {
                return call;
            }
            tool_input["pattern"]
                .as_str()
                .map(str::to_string)
                .into_iter()
                .collect()
        }
        Some("Bash") => {
            let command = tool_input["command"].as_str().unwrap_or("");
            if command.len() > 4096 {
                return call;
            }
            split_commands(command)
                .iter()
                .filter_map(|words| parse_grep_command(words))
                .filter(|g| !g.non_code_filter)
                .flat_map(|g| g.patterns)
                .collect()
        }
        _ => vec![],
    };
    for pattern in patterns {
        for c in identifier_candidates(&pattern) {
            if call.candidates.len() < MAX_CANDIDATES && !call.candidates.contains(&c) {
                call.candidates.push(c);
            }
        }
    }
    call
}

// ── Decision ─────────────────────────────────────────────────────────

/// What the index knows about one candidate.
#[derive(Debug, Clone)]
pub struct Known {
    /// The candidate as searched.
    pub candidate: Candidate,
    /// Matching definitions (functions, methods and types only).
    pub definitions: Vec<SymbolLocation>,
    /// Direct callers of those definitions, when counted.
    pub callers: Option<usize>,
}

fn is_callable(kind: &SymbolKind) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
}

fn is_type(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum | SymbolKind::TypeAlias
    )
}

/// Keeps the definitions a qualified candidate (`Item.AsString`) names.
fn matching_definitions(candidate: &Candidate, found: Vec<SymbolLocation>) -> Vec<SymbolLocation> {
    let defs = found
        .into_iter()
        .filter(|d| is_callable(&d.kind) || is_type(&d.kind));
    if candidate.query == candidate.short {
        return defs.collect();
    }
    let norm = |s: &str| s.replace("::", ".").replace(['#', '/'], ".");
    let wanted = norm(&candidate.query);
    let dotted = format!(".{wanted}");
    defs.filter(|d| {
        let qn = norm(&d.qualified_name);
        qn == wanted || qn.ends_with(&dotted)
    })
    .collect()
}

/// Wraps a name in backticks for the message, quoting it for the shell when
/// it holds characters a shell would interpret.
fn shell_arg(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
    {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "'\\''"))
    }
}

/// The `additionalContext` sentence for one known symbol, phrased as facts.
pub fn hint_for(known: &Known) -> Option<String> {
    let defs = &known.definitions;
    let first = defs.first()?;
    let name = shell_arg(&known.candidate.query);
    if defs.len() > 1 {
        let locations: Vec<String> = defs
            .iter()
            .take(3)
            .map(|d| format!("{}:{}", d.file_path, d.start_line))
            .collect();
        let more = if defs.len() > 3 {
            format!(", +{} more", defs.len() - 3)
        } else {
            String::new()
        };
        return Some(format!(
            "The myceliums index has {} definitions named `{}` ({}{}). `myc callers {}` lists them with file:line; a qualified name such as `{}` selects one.",
            defs.len(),
            known.candidate.query,
            locations.join(", "),
            more,
            name,
            first.qualified_name
        ));
    }
    let at = format!("{}:{}", first.file_path, first.start_line);
    if is_callable(&first.kind) {
        let callers = match known.callers {
            Some(0) => "its direct callers (the index records none)".to_string(),
            Some(1) => "its 1 direct caller".to_string(),
            Some(n) => format!("its {n} direct callers"),
            None => "its direct callers".to_string(),
        };
        Some(format!(
            "`{}` is a {} defined at {at} in the myceliums index. `myc callers {name}` lists {callers} with file:line, `myc callees {name}` what it calls, `myc impact {name}` the files that depend on it.",
            known.candidate.query, first.kind
        ))
    } else {
        Some(format!(
            "`{}` is a {} defined at {at} in the myceliums index. `myc impact {name}` lists the symbols and files that depend on it.",
            known.candidate.query, first.kind
        ))
    }
}

/// The PreToolUse output object for the given hints, or `None` to stay silent.
pub fn hook_output(hints: &[String]) -> Option<serde_json::Value> {
    if hints.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": hints.join(" "),
        }
    }))
}

/// Looks the candidates up in the index covering `cwd`.
async fn lookup(
    data_dir: &Path,
    cwd: &Path,
    candidates: &[Candidate],
) -> anyhow::Result<Vec<Known>> {
    let registry = RepoRegistry::load(&data_dir.join("repos.json"))?;
    let Some(info) = registry.find_containing(cwd) else {
        return Ok(vec![]);
    };
    RepoRegistry::validate_repo_id(&info.id)?;
    let store = Store::open(&RepoRegistry::repo_db_path(data_dir, &info.id), &info.id).await?;
    let mut known = Vec::new();
    for candidate in candidates {
        let found = store.find_symbols_named(&candidate.short, 25).await?;
        let definitions = matching_definitions(candidate, found);
        if definitions.is_empty() {
            continue;
        }
        let callers = if definitions.len() == 1 && is_callable(&definitions[0].kind) {
            let uids = vec![definitions[0].uid.clone()];
            Some(store.count_incoming(&uids, RelationshipKind::Calls).await?)
        } else {
            None
        };
        known.push(Known {
            candidate: candidate.clone(),
            definitions,
            callers,
        });
    }
    Ok(known)
}

/// Computes the hook's stdout for one stdin payload. `Err` means silent and
/// says why.
pub async fn respond(
    data_dir: &Path,
    stdin: &str,
    fallback_cwd: Option<PathBuf>,
    deadline: Duration,
) -> Result<String, String> {
    let input: serde_json::Value =
        serde_json::from_str(stdin).map_err(|e| format!("input is not JSON: {e}"))?;
    let call = search_call_from_input(&input);
    if call.candidates.is_empty() {
        return Err("no identifier-shaped search pattern".into());
    }
    let cwd = call
        .cwd
        .or(fallback_cwd)
        .ok_or("no working directory in the input")?;
    let known = tokio::time::timeout(deadline, lookup(data_dir, &cwd, &call.candidates))
        .await
        .map_err(|_| format!("index lookup took longer than {} ms", deadline.as_millis()))?
        .map_err(|e| format!("index lookup failed: {e:#}"))?;
    let hints: Vec<String> = known.iter().filter_map(hint_for).take(2).collect();
    hook_output(&hints).map(|v| v.to_string()).ok_or_else(|| {
        format!(
            "no indexed function, method or type named {:?} under {}",
            call.candidates,
            cwd.display()
        )
    })
}

/// `myc hook-grep`: reads PreToolUse JSON on stdin; prints a hint or nothing.
/// Always exits 0.
pub async fn cmd_hook_grep() -> anyhow::Result<()> {
    let debug = std::env::var_os(DEBUG_ENV).is_some_and(|v| !v.is_empty() && v != "0");
    let mut input = String::new();
    if std::io::stdin()
        .take(MAX_INPUT_BYTES)
        .read_to_string(&mut input)
        .is_err()
    {
        return Ok(());
    }
    let fallback_cwd = std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let deadline = std::env::var(DEADLINE_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(LOOKUP_DEADLINE, Duration::from_millis);
    match respond(&crate::data_dir(), &input, fallback_cwd, deadline).await {
        Ok(out) => println!("{out}"),
        Err(reason) if debug => eprintln!("myc hook-grep: silent: {reason}"),
        Err(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(pattern: &str) -> Vec<String> {
        identifier_candidates(pattern)
            .into_iter()
            .map(|c| c.query)
            .collect()
    }

    #[test]
    fn identifier_patterns_yield_the_name() {
        for (pattern, want) in [
            ("parse_query", "parse_query"),
            ("parseQuery", "parseQuery"),
            ("HttpClient", "HttpClient"),
            ("\\bparse_query\\b", "parse_query"),
            ("parse_query\\(", "parse_query"),
            ("parse_query(", "parse_query"),
            ("fn parse_query", "parse_query"),
            ("pub async fn parse_query", "parse_query"),
            ("def solve_dependencies\\(", "solve_dependencies"),
            ("func \\(p \\*Pattern\\) MatchItem", "MatchItem"),
            ("function\\s+parseAccept", "parseAccept"),
            ("class HTTPException", "HTTPException"),
            ("\\.AsString\\(", "AsString"),
            ("Item.AsString", "Item.AsString"),
            ("Item::as_string", "Item::as_string"),
            ("^export function getCookie", "getCookie"),
            ("MAX_RETRIES", "MAX_RETRIES"),
            ("pub(crate) fn parse_query", "parse_query"),
            // A plain word becomes a candidate when framed as code.
            ("def parse\\(", "parse"),
            ("\\.render\\(", "render"),
        ] {
            assert_eq!(names(pattern), [want], "pattern {pattern:?}");
        }
    }

    #[test]
    fn literal_and_regex_searches_yield_nothing() {
        for pattern in [
            "error",
            "TODO",
            "FIXME",
            "connection refused",
            "\"timeout\": 30",
            "[a-z]+_handler",
            "parse.*query",
            "foo\\d+",
            "https://example.com",
            "x",
            "id",
            "",
            "user name",
            "{{ value }}",
            "^\\s*$",
            "log::info!",
        ] {
            assert!(
                names(pattern).is_empty(),
                "pattern {pattern:?} gave {:?}",
                names(pattern)
            );
        }
    }

    #[test]
    fn alternations_are_split() {
        assert_eq!(
            names("parse_query|run_search"),
            ["parse_query", "run_search"]
        );
        assert_eq!(names("parse_query\\|plain"), ["parse_query"]);
        assert_eq!(names("a_b|c_d|e_f|g_h|i_j").len(), MAX_CANDIDATES);
    }

    #[test]
    fn shell_splitting_handles_quotes_and_separators() {
        let cmds = split_commands(
            r#"cd src && grep -rn "fn parse_query" . | head -5; rg 'a b' -g "*.rs""#,
        );
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[1], ["grep", "-rn", "fn parse_query", "."]);
        assert_eq!(cmds[3], ["rg", "a b", "-g", "*.rs"]);
        assert_eq!(
            split_commands(r"grep foo\ bar x"),
            [vec!["grep", "foo bar", "x"]]
        );
    }

    fn grep(cmd: &str) -> Option<GrepCall> {
        split_commands(cmd)
            .first()
            .and_then(|w| parse_grep_command(w))
    }

    #[test]
    fn grep_and_rg_patterns_are_found_past_options() {
        assert_eq!(
            grep("grep -rn parse_query src").unwrap().patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("grep -A 3 -B2 parse_query .").unwrap().patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("grep --include=*.py -e run_search -e other .")
                .unwrap()
                .patterns,
            ["run_search", "other"]
        );
        assert_eq!(
            grep("rg -t rust -g '*.rs' parse_query").unwrap().patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("rg --type rust parse_query").unwrap().patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("git grep -n parse_query").unwrap().patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("LC_ALL=C /usr/bin/grep -w parse_query x")
                .unwrap()
                .patterns,
            ["parse_query"]
        );
        assert_eq!(
            grep("grep -f patterns.txt x").unwrap().patterns,
            Vec::<String>::new()
        );
        assert_eq!(grep("grep -- -dash x").unwrap().patterns, ["-dash"]);
        assert!(grep("cat foo").is_none());
        assert!(grep("git log --grep parse_query").is_none());
    }

    #[test]
    fn config_file_searches_are_flagged() {
        assert!(grep("rg -g '*.json' parse_query").unwrap().non_code_filter);
        assert!(grep("rg -t yaml parse_query").unwrap().non_code_filter);
        assert!(
            grep("grep --include=*.toml -r parse_query .")
                .unwrap()
                .non_code_filter
        );
        assert!(
            grep("grep parse_query package.json")
                .unwrap()
                .non_code_filter
        );
        assert!(
            !grep("grep parse_query src/main.rs")
                .unwrap()
                .non_code_filter
        );
    }

    fn input(tool: &str, tool_input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "session_id": "s",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/repo",
            "permission_mode": "default",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": tool_input,
            "tool_use_id": "toolu_1",
        })
    }

    #[test]
    fn hook_input_for_grep_and_bash() {
        let call = search_call_from_input(&input(
            "Grep",
            serde_json::json!({"pattern": "parse_query\\(", "path": "src"}),
        ));
        assert_eq!(call.cwd.as_deref(), Some(Path::new("/repo")));
        assert_eq!(call.candidates[0].query, "parse_query");

        let config = search_call_from_input(&input(
            "Grep",
            serde_json::json!({"pattern": "parse_query", "glob": "*.yaml"}),
        ));
        assert!(config.candidates.is_empty());
        let typed = search_call_from_input(&input(
            "Grep",
            serde_json::json!({"pattern": "parse_query", "type": "json"}),
        ));
        assert!(typed.candidates.is_empty());

        let bash = search_call_from_input(&input(
            "Bash",
            serde_json::json!({"command": "grep -rn 'def run_search' . | head", "description": "find"}),
        ));
        assert_eq!(bash.candidates[0].query, "run_search");
        let other = search_call_from_input(&input(
            "Bash",
            serde_json::json!({"command": "cargo test parse_query"}),
        ));
        assert!(other.candidates.is_empty());
        let read = search_call_from_input(&input(
            "Read",
            serde_json::json!({"file_path": "parse_query"}),
        ));
        assert!(read.candidates.is_empty());
        let mut post = input("Grep", serde_json::json!({"pattern": "parse_query"}));
        post["hook_event_name"] = "PostToolUse".into();
        assert!(search_call_from_input(&post).candidates.is_empty());
    }

    fn loc(
        uid: &str,
        name: &str,
        qn: &str,
        kind: SymbolKind,
        file: &str,
        line: u32,
    ) -> SymbolLocation {
        SymbolLocation {
            uid: uid.into(),
            name: name.into(),
            qualified_name: qn.into(),
            kind,
            file_path: file.into(),
            start_line: line,
        }
    }

    fn cand(q: &str) -> Candidate {
        identifier_candidates(q).remove(0)
    }

    #[test]
    fn hints_name_the_exact_command() {
        let known = Known {
            candidate: cand("parse_query"),
            definitions: vec![loc(
                "u",
                "parse_query",
                "q::parse_query",
                SymbolKind::Function,
                "src/q.rs",
                42,
            )],
            callers: Some(7),
        };
        let hint = hint_for(&known).unwrap();
        assert!(
            hint.contains("`myc callers parse_query` lists its 7 direct callers with file:line"),
            "{hint}"
        );
        assert!(hint.contains("src/q.rs:42"), "{hint}");

        let class = Known {
            candidate: cand("HttpClient"),
            definitions: vec![loc(
                "c",
                "HttpClient",
                "net.HttpClient",
                SymbolKind::Class,
                "src/net.ts",
                3,
            )],
            callers: None,
        };
        assert!(hint_for(&class)
            .unwrap()
            .contains("`myc impact HttpClient`"));

        let ambiguous = Known {
            candidate: cand("parse_it"),
            definitions: vec![
                loc(
                    "a",
                    "parse_it",
                    "a::parse_it",
                    SymbolKind::Function,
                    "src/a.rs",
                    1,
                ),
                loc(
                    "b",
                    "parse_it",
                    "b::parse_it",
                    SymbolKind::Function,
                    "src/b.rs",
                    2,
                ),
            ],
            callers: None,
        };
        let hint = hint_for(&ambiguous).unwrap();
        assert!(
            hint.contains("2 definitions") && hint.contains("src/a.rs:1, src/b.rs:2"),
            "{hint}"
        );
        assert!(hint.contains("`a::parse_it`"), "{hint}");
    }

    #[test]
    fn qualified_candidates_filter_definitions_and_drop_variables() {
        let found = vec![
            loc(
                "1",
                "AsString",
                "fzf.Item.AsString",
                SymbolKind::Method,
                "item.go",
                1,
            ),
            loc(
                "2",
                "AsString",
                "fzf.Other.AsString",
                SymbolKind::Method,
                "other.go",
                1,
            ),
            loc("3", "AsString", "AsString", SymbolKind::Variable, "x.go", 1),
        ];
        let kept = matching_definitions(&cand("Item.AsString"), found.clone());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].uid, "1");
        assert_eq!(matching_definitions(&cand("AsString"), found).len(), 2);
    }

    #[test]
    fn output_is_the_documented_pretooluse_shape_without_a_decision() {
        assert!(hook_output(&[]).is_none());
        let v = hook_output(&["a".into(), "b".into()]).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "a b");
        assert!(
            v["hookSpecificOutput"].get("permissionDecision").is_none(),
            "must never decide"
        );
        assert!(v.get("decision").is_none() && v.get("continue").is_none());
    }

    #[tokio::test]
    async fn respond_is_silent_on_malformed_or_irrelevant_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let silent = |stdin: String| {
            let path = path.clone();
            async move { respond(&path, &stdin, None, LOOKUP_DEADLINE).await }
        };
        for stdin in ["", "not json", "[]", "{}", "{\"tool_name\": 5}"] {
            assert!(silent(stdin.to_string()).await.is_err(), "{stdin:?}");
        }
        // A real identifier, but no registry and no index: silent.
        let call = input("Grep", serde_json::json!({"pattern": "parse_query"})).to_string();
        let reason = silent(call.clone()).await.unwrap_err();
        assert!(reason.contains("no indexed function"), "{reason}");
        // Registry present but cwd outside every indexed repo: silent.
        std::fs::write(dir.path().join("repos.json"), "{\"repos\":{}}").unwrap();
        assert!(silent(call.clone()).await.is_err());
        // A corrupt registry is not an error either, just a reason.
        std::fs::write(dir.path().join("repos.json"), "{broken").unwrap();
        let reason = silent(call).await.unwrap_err();
        assert!(reason.contains("lookup failed"), "{reason}");
    }
}
