//! `myc setup-claude`: installs the Claude Code integration layers (#126).
//!
//! | Layer | Project scope (default) | User scope |
//! |---|---|---|
//! | MCP server | `.mcp.json` | `claude mcp add -s user` (fallback `~/.claude.json`) |
//! | Hooks: SessionStart index refresh, PostToolUse result formatter, PreToolUse grep redirect | `.claude/settings.json` | `~/.claude/settings.json` |
//! | Routing block | `CLAUDE.md` | `~/.claude/CLAUDE.md` |
//! | `code-explorer` subagent | `.claude/agents/code-explorer.md` | `~/.claude/agents/code-explorer.md` |
//!
//! Re-running is idempotent and makes the files match the chosen layers: an
//! opt-out flag removes a layer installed earlier. `--uninstall` removes
//! exactly the myceliums parts at that scope — the `myceliums` MCP entry, hook
//! handlers that run `myc session|format-hook|hook-grep`, the marked routing
//! block and an agent file that still carries the myceliums marker line — and
//! leaves everything else in those files alone.
//!
//! File formats follow the Claude Code docs (checked 2026-11-18):
//! hooks <https://code.claude.com/docs/en/hooks>, project MCP servers
//! <https://code.claude.com/docs/en/mcp>, subagents
//! <https://code.claude.com/docs/en/sub-agents>.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use serde_json::{json, Map, Value};

use super::routing::{self, FileChange, Flavor};
use crate::setup::editors::write_user_file;

/// Where the integration is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    /// The project's `.mcp.json`, `.claude/` and `CLAUDE.md`, to commit for the team.
    Project,
    /// `~/.claude.json` and `~/.claude/`, for every project of this user.
    User,
}

/// Which optional layers to install. The MCP server and the SessionStart /
/// PostToolUse hooks are always installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layers {
    /// Routing block in CLAUDE.md.
    pub routing: bool,
    /// PreToolUse hook that answers symbol greps with the matching myc call.
    pub redirect_hook: bool,
    /// `code-explorer` subagent.
    pub agent: bool,
    /// How Claude Code loads the MCP tools (layer 2).
    pub tool_loading: ToolLoading,
}

impl Layers {
    /// Every layer, the core tools loaded up front.
    pub const ALL: Layers = Layers {
        routing: true,
        redirect_hook: true,
        agent: true,
        tool_loading: ToolLoading::UpFront,
    };
}

/// How Claude Code loads the myceliums MCP tools (layer 2, #97): the
/// server entry's `alwaysLoad` (code.claude.com/docs/en/mcp, "Exempt a
/// server from deferral").
///
/// Only a personal entry (user scope) carries the key. A project's
/// `.mcp.json` is shared: the installer's `mcp.toolsets` says nothing about
/// a teammate's (or an older `myc` with 55 tools), so at project scope the
/// entry is always [`ToolLoading::PerTool`] and the core tools load up front
/// through their own `_meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolLoading {
    /// `"alwaysLoad": true`. The server exposes the core toolset only, so
    /// every tool it lists loads up front, and Claude Code waits for them
    /// (at most 5 s) before the first prompt.
    UpFront,
    /// No `alwaysLoad` (project scope, or `mcp.toolsets` adds groups): the
    /// core tools still load up front through their own
    /// `_meta["anthropic/alwaysLoad"]`; other groups stay behind tool search
    /// instead of all loading into every request.
    PerTool,
    /// `"alwaysLoad": false` (`--no-always-load`): every myceliums tool stays
    /// behind tool search, the core tools' `_meta` included (Claude Code
    /// 2.1.287 or later).
    Deferred,
}

impl ToolLoading {
    /// The loading that fits this machine's `mcp.toolsets` (what `myc mcp`
    /// serves without `--toolsets`), or [`ToolLoading::Deferred`] when the
    /// user opted out. An unreadable config or invalid toolset name counts
    /// as "not the core toolset only", so nothing extra loads up front.
    pub fn for_config(opt_out: bool) -> ToolLoading {
        if opt_out {
            return ToolLoading::Deferred;
        }
        let names = myceliums_core::GlobalConfig::load(&crate::repo::data_dir())
            .map(|c| c.mcp.toolsets)
            .unwrap_or_else(|_| vec!["?".to_string()]);
        match myceliums_mcp::Toolsets::parse_names(names.iter().map(String::as_str)) {
            Ok(toolsets) if toolsets == myceliums_mcp::Toolsets::core() => ToolLoading::UpFront,
            _ => ToolLoading::PerTool,
        }
    }

    /// The server entry's `alwaysLoad` value; `None` leaves it out.
    fn always_load(self) -> Option<bool> {
        match self {
            ToolLoading::UpFront => Some(true),
            ToolLoading::PerTool => None,
            ToolLoading::Deferred => Some(false),
        }
    }

    /// The loading written at `scope`: project files never carry the key.
    fn at(self, scope: Scope) -> ToolLoading {
        match scope {
            Scope::Project => ToolLoading::PerTool,
            Scope::User => self,
        }
    }

    /// For the setup report.
    fn describe(self) -> &'static str {
        match self {
            ToolLoading::UpFront => "alwaysLoad: core tools loaded up front",
            ToolLoading::PerTool => {
                "core tools up front through their _meta, other groups deferred"
            }
            ToolLoading::Deferred => "alwaysLoad false: all tools deferred (--no-always-load)",
        }
    }
}

/// Files the integration touches at one scope.
#[derive(Debug, Clone)]
pub struct Targets {
    /// Scope these targets belong to.
    pub scope: Scope,
    /// MCP server config file (`.mcp.json` or `~/.claude.json`).
    pub mcp_config: PathBuf,
    /// Claude Code settings file holding the hooks.
    pub settings: PathBuf,
    /// Memory file that receives the routing block.
    pub memory: PathBuf,
    /// Subagent definition.
    pub agent: PathBuf,
    /// Directory that may be removed when uninstalling leaves it empty
    /// (`<project>/.claude`); `None` at user scope.
    pub claude_dir: Option<PathBuf>,
}

impl Targets {
    /// Targets inside the project rooted at `root`.
    pub fn project(root: &Path) -> Self {
        let claude = root.join(".claude");
        Self {
            scope: Scope::Project,
            mcp_config: root.join(".mcp.json"),
            settings: claude.join("settings.json"),
            memory: root.join("CLAUDE.md"),
            agent: claude.join("agents").join(AGENT_FILE),
            claude_dir: Some(claude),
        }
    }

    /// Targets in the user's home directory.
    pub fn user(home: &Path) -> Self {
        let claude = home.join(".claude");
        Self {
            scope: Scope::User,
            mcp_config: home.join(".claude.json"),
            settings: claude.join("settings.json"),
            memory: claude.join("CLAUDE.md"),
            agent: claude.join("agents").join(AGENT_FILE),
            claude_dir: None,
        }
    }

    /// Whether files that become empty on uninstall are deleted. Only project
    /// files are: at user scope an empty file is left as it is.
    fn delete_empty(&self) -> bool {
        self.scope == Scope::Project
    }
}

/// Subagent file name.
pub const AGENT_FILE: &str = "code-explorer.md";
/// Version of the subagent template; bump it when the text changes.
pub const AGENT_VERSION: u32 = 2;
/// Marker line in the subagent's frontmatter. A file without it is not ours.
const AGENT_MARKER: &str = "# myceliums code-explorer";

/// The `code-explorer` subagent definition.
///
/// `tools` lists the whole myceliums MCP server (`mcp__myceliums__*`), Read
/// to confirm results, and ToolSearch in case the session defers MCP tool
/// schemas. Bash is left out: the subagent format can grant or withhold a
/// tool but not restrict Bash to `myc` commands.
pub fn agent_definition() -> String {
    format!(
        "---\n\
name: code-explorer\n\
{AGENT_MARKER} v{AGENT_VERSION}, written by `myc setup-claude`. Delete this line to keep your edits: `myc setup-claude` then neither overwrites nor removes the file.\n\
description: Answers structural questions about this codebase from the myceliums code graph - who calls a function, what it calls, what a change would break, where something is implemented, how a flow gets from A to B. Use proactively for these instead of grep; returns symbols with file:line.\n\
tools: mcp__myceliums__*, Read, ToolSearch\n\
---\n\
\n\
You answer questions about the structure of this repository with the myceliums code graph, then confirm with Read.\n\
\n\
0. Without `repo_id` the myceliums tools use the index covering the working directory; when none covers it they say so, and `list_repos` lists the indexed repositories to pass as `repo_id`.\n\
1. Locate: `search_code` for a name or for a description of what the code does; `explain_symbol` shows a symbol's source, callers and callees.\n\
2. Follow edges: `find_callers` / `find_callees` (`depth` for transitive chains), `impact_of_change` for what a change to a symbol, a file or the uncommitted diff would break, `trace_flow` for how A reaches B, `find_dead_code` for functions nothing calls.\n\
3. Ambiguous names: the error lists `file:qualified_name` candidates; pick one by file or qualified name, and say which one you used.\n\
4. Read only the lines needed to confirm a result. The graph reflects the last `myc analyze`; when it disagrees with the code, trust the code and say so.\n\
\n\
Answer with the symbols as `name - path:line`, one per line, then at most two sentences. Report only what the graph and the code show.\n"
    )
}

// ── Hook handlers ────────────────────────────────────────────────────

/// `myc` subcommands whose hook handlers belong to this integration.
const OUR_SUBCOMMANDS: &[&str] = &["session", "format-hook", "hook-grep"];

/// First two shell words of `command`, honouring single and double quotes.
fn first_two_words(command: &str) -> (String, String) {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                    if words.len() == 2 {
                        break;
                    }
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word && words.len() < 2 {
        words.push(word);
    }
    let mut it = words.into_iter();
    (it.next().unwrap_or_default(), it.next().unwrap_or_default())
}

/// Whether a hook `command` runs one of our `myc` subcommands.
pub fn is_myceliums_hook_command(command: &str) -> bool {
    let (program, sub) = first_two_words(command);
    let base = Path::new(&program)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let sub = sub.trim_end_matches(';');
    (base == "myc" || base == "myc.exe") && OUR_SUBCOMMANDS.contains(&sub)
}

/// Quotes `path` for a POSIX shell when it holds characters the shell would
/// interpret.
pub fn shell_quote(path: &str) -> String {
    if !path.is_empty()
        && path.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | ':' | '@' | '~')
        })
    {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn handler(command: String, extra: &[(&str, Value)]) -> Value {
    let mut h = Map::new();
    h.insert("type".into(), json!("command"));
    for (k, v) in extra {
        h.insert((*k).into(), v.clone());
    }
    h.insert("command".into(), json!(command));
    Value::Object(h)
}

/// Claude Code's timeout for the SessionStart hook. The hook itself returns
/// in well under a second (the re-index runs detached).
const SESSION_HOOK_TIMEOUT_SECS: u64 = 15;

/// Hook groups to install, per event, for the `myc` invocation `myc`.
pub fn hook_groups(myc: &str, layers: Layers) -> Vec<(&'static str, Value)> {
    let myc = shell_quote(myc);
    let mut groups = vec![
        (
            // stdout is one JSON object whose `additionalContext` reaches the
            // model (#152); `; exit 0` keeps the session safe from a crash or
            // an older `myc`. Claude Code waits for this hook, so `myc` only
            // checks the index and starts a re-index in the background; the
            // `timeout` stops anything that hangs anyway. `--timeout 300` is
            // ignored by current `myc` and bounds an older one.
            "SessionStart",
            json!({
                "matcher": "",
                "hooks": [handler(
                    format!("{myc} session . --yes --timeout 300 2>/dev/null; exit 0"),
                    &[("timeout", json!(SESSION_HOOK_TIMEOUT_SECS))],
                )],
            }),
        ),
        (
            // `.*` makes the matcher a regex; a bare `mcp__myceliums__` would be
            // compared as an exact tool name and never match.
            "PostToolUse",
            json!({
                "matcher": "mcp__myceliums__.*",
                "hooks": [handler(format!("{myc} format-hook 2>/dev/null"), &[])],
            }),
        ),
    ];
    if layers.redirect_hook {
        // `|| true`: a PreToolUse hook that exits 2 blocks the tool call. An
        // older `myc` without `hook-grep` (a teammate's install, another
        // binary first on PATH) would exit 2 on the unknown subcommand and
        // deny every grep; this way the hook can only ever exit 0.
        let grep = format!("{myc} hook-grep 2>/dev/null || true");
        let timeout = ("timeout", json!(5));
        groups.push((
            "PreToolUse",
            json!({"matcher": "Grep", "hooks": [handler(grep.clone(), std::slice::from_ref(&timeout))]}),
        ));
        // `if` (permission-rule syntax) keeps the hook off Bash calls that
        // do not run grep/rg; Claude Code checks every subcommand of a
        // pipeline or `&&` chain against it.
        let bash_handlers: Vec<Value> = ["Bash(grep *)", "Bash(rg *)", "Bash(git grep *)"]
            .iter()
            .map(|rule| handler(grep.clone(), &[("if", json!(rule)), timeout.clone()]))
            .collect();
        groups.push((
            "PreToolUse",
            json!({"matcher": "Bash", "hooks": bash_handlers}),
        ));
    }
    groups
}

/// Removes our handlers from every event in `hooks`; groups left without
/// handlers and events left without groups are dropped. Returns how many
/// handlers were removed.
pub fn remove_our_hooks(hooks: &mut Map<String, Value>) -> usize {
    let mut removed = 0;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = handlers.len();
                handlers.retain(|h| !h["command"].as_str().is_some_and(is_myceliums_hook_command));
                removed += before - handlers.len();
            }
        }
        groups.retain(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|h| !h.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    removed
}

// ── JSON files ───────────────────────────────────────────────────────

/// Reads a JSON object file; a missing file is an empty object. A file that
/// exists but is not a JSON object is an error: we never overwrite what we
/// cannot parse.
fn read_json_object(path: &Path) -> Result<(Map<String, Value>, bool)> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok((Map::new(), true)),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok((map, true)),
            Ok(_) => bail!(
                "{} is not a JSON object; fix it by hand, then re-run",
                path.display()
            ),
            Err(e) => bail!(
                "{} is not valid JSON ({e}); fix it by hand, then re-run",
                path.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Map::new(), false)),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn write_json_object(path: &Path, map: Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map))? + "\n";
    write_user_file(path, &text)
}

/// Applies `edit` to the JSON object in `path` and writes it back if it
/// changed. With `delete_if_empty`, a file left as `{}` is deleted.
fn edit_json(
    path: &Path,
    delete_if_empty: bool,
    edit: impl FnOnce(&mut Map<String, Value>),
) -> Result<FileChange> {
    let (mut map, existed) = read_json_object(path)?;
    let before = map.clone();
    edit(&mut map);
    if existed && map == before {
        return Ok(FileChange::Unchanged);
    }
    if !existed && map.is_empty() {
        return Ok(FileChange::Absent);
    }
    if delete_if_empty && map.is_empty() && !is_symlink(path) {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to delete {}", path.display()))?;
        return Ok(FileChange::Deleted);
    }
    write_json_object(path, map)?;
    Ok(if existed {
        FileChange::Updated
    } else {
        FileChange::Created
    })
}

/// The `mcpServers.myceliums` entry.
pub fn mcp_entry(myc: &str, loading: ToolLoading) -> Value {
    let mut entry = json!({"type": "stdio", "command": myc, "args": ["mcp"]});
    if let Some(always) = loading.always_load() {
        entry["alwaysLoad"] = json!(always);
    }
    entry
}

fn set_mcp_server(map: &mut Map<String, Value>, entry: Value) {
    let servers = map.entry("mcpServers").or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    if let Some(servers) = servers.as_object_mut() {
        servers.insert("myceliums".into(), entry);
    }
}

fn remove_mcp_server(map: &mut Map<String, Value>) {
    if let Some(servers) = map.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove("myceliums");
        if servers.is_empty() {
            map.remove("mcpServers");
        }
    }
}

/// Whether `path` is a symbolic link. Files that are links are emptied, not
/// deleted, so the link and its target stay as the user set them up.
pub(crate) fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

// ── Agent file ───────────────────────────────────────────────────────

fn agent_is_ours(path: &Path) -> Result<Option<bool>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            text.lines()
                .any(|l| l.trim_start().starts_with(AGENT_MARKER)),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

/// Outcome of installing the subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChange {
    /// Written or refreshed (or already current).
    Written(FileChange),
    /// A file without our marker exists; left untouched.
    KeptUserFile,
}

fn install_agent(path: &Path) -> Result<AgentChange> {
    let content = agent_definition();
    match agent_is_ours(path)? {
        Some(false) => return Ok(AgentChange::KeptUserFile),
        Some(true) if std::fs::read_to_string(path)? == content => {
            return Ok(AgentChange::Written(FileChange::Unchanged))
        }
        _ => {}
    }
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    write_user_file(path, &content)?;
    Ok(AgentChange::Written(if existed {
        FileChange::Updated
    } else {
        FileChange::Created
    }))
}

fn remove_agent(path: &Path) -> Result<FileChange> {
    match agent_is_ours(path)? {
        Some(true) => {
            std::fs::remove_file(path)
                .with_context(|| format!("Failed to delete {}", path.display()))?;
            if let Some(dir) = path.parent() {
                // Only removes the directory when it is empty.
                let _ = std::fs::remove_dir(dir);
            }
            Ok(FileChange::Deleted)
        }
        _ => Ok(FileChange::Absent),
    }
}

// ── Install / uninstall ──────────────────────────────────────────────

/// What `install` / `uninstall` did, one line per layer, for the report.
#[derive(Debug, Default)]
pub struct Report {
    /// `(layer, outcome)` pairs in the order they ran.
    pub lines: Vec<(String, String)>,
}

impl Report {
    fn add(&mut self, layer: &str, outcome: impl Into<String>) {
        self.lines.push((layer.to_string(), outcome.into()));
    }
}

fn describe(change: FileChange, path: &Path) -> String {
    let p = path.display();
    match change {
        FileChange::Created => format!("created {p}"),
        FileChange::Updated => format!("updated {p}"),
        FileChange::Unchanged => format!("up to date in {p}"),
        FileChange::Removed => format!("removed from {p}"),
        FileChange::Deleted => format!("removed {p}"),
        FileChange::Absent => format!("not present in {p}"),
    }
}

/// How the MCP server is registered at user scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRegistrar {
    /// `claude mcp add -s user`, falling back to editing `~/.claude.json`.
    ClaudeCli,
    /// Edit the config file directly (project scope, tests).
    File,
}

/// Installs the integration into `targets`, invoking `myc` as `myc_cmd`.
pub fn install(
    targets: &Targets,
    myc_cmd: &str,
    layers: Layers,
    registrar: McpRegistrar,
) -> Result<Report> {
    let mut report = Report::default();

    // 1. MCP server.
    let tool_loading = layers.tool_loading.at(targets.scope);
    let entry = mcp_entry(myc_cmd, tool_loading);
    let loading = tool_loading.describe();
    let via_cli = registrar == McpRegistrar::ClaudeCli && claude_cli_add(&entry);
    if via_cli {
        report.add(
            "MCP server",
            format!("registered with `claude mcp add-json -s user` ({loading})"),
        );
    } else {
        let change = edit_json(&targets.mcp_config, false, |m| set_mcp_server(m, entry))?;
        report.add(
            "MCP server",
            format!("{} ({loading})", describe(change, &targets.mcp_config)),
        );
    }

    // 2. Hooks: drop ours, then add the wanted set.
    let groups = hook_groups(myc_cmd, layers);
    let change = edit_json(&targets.settings, false, |settings| {
        let hooks = settings.entry("hooks").or_insert_with(|| json!({}));
        if !hooks.is_object() {
            *hooks = json!({});
        }
        let hooks = hooks.as_object_mut().expect("hooks is an object");
        remove_our_hooks(hooks);
        for (event, group) in groups {
            let entry = hooks.entry(event).or_insert_with(|| json!([]));
            if !entry.is_array() {
                *entry = json!([]);
            }
            entry.as_array_mut().expect("array").push(group);
        }
    })?;
    let hook_names = if layers.redirect_hook {
        "SessionStart + PostToolUse + PreToolUse grep redirect"
    } else {
        "SessionStart + PostToolUse"
    };
    report.add(
        "Hooks",
        format!("{hook_names}: {}", describe(change, &targets.settings)),
    );

    // 3. Routing block.
    if layers.routing {
        let block = routing::routing_block(Flavor::Claude {
            subagent: layers.agent,
        });
        let change = routing::install_block(&targets.memory, &block)?;
        report.add("Routing", describe(change, &targets.memory));
    } else {
        let change = routing::uninstall_block(&targets.memory, targets.delete_empty())?;
        report.add(
            "Routing",
            format!(
                "skipped (--no-routing); {}",
                describe(change, &targets.memory)
            ),
        );
    }

    // 4. Subagent.
    if layers.agent {
        match install_agent(&targets.agent)? {
            AgentChange::Written(change) => {
                report.add("Subagent", describe(change, &targets.agent))
            }
            AgentChange::KeptUserFile => report.add(
                "Subagent",
                format!(
                    "kept {} (no myceliums marker: your own file)",
                    targets.agent.display()
                ),
            ),
        }
    } else {
        let change = remove_agent(&targets.agent)?;
        report.add(
            "Subagent",
            format!("skipped (--no-agent); {}", describe(change, &targets.agent)),
        );
    }
    Ok(report)
}

/// Removes everything `install` may have written at `targets`.
pub fn uninstall(targets: &Targets, registrar: McpRegistrar) -> Result<Report> {
    let mut report = Report::default();
    let delete_empty = targets.delete_empty();

    if registrar == McpRegistrar::ClaudeCli && claude_cli_remove() {
        report.add("MCP server", "removed with `claude mcp remove -s user`");
    }
    let change = edit_json(&targets.mcp_config, delete_empty, remove_mcp_server)?;
    report.add("MCP server", describe(change, &targets.mcp_config));

    let mut removed = 0;
    let change = edit_json(&targets.settings, delete_empty, |settings| {
        if let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
            removed = remove_our_hooks(hooks);
            if hooks.is_empty() {
                settings.remove("hooks");
            }
        }
    })?;
    report.add(
        "Hooks",
        format!(
            "{removed} handler(s); {}",
            describe(change, &targets.settings)
        ),
    );

    let change = routing::uninstall_block(&targets.memory, delete_empty)?;
    report.add("Routing", describe(change, &targets.memory));

    let change = remove_agent(&targets.agent)?;
    report.add("Subagent", describe(change, &targets.agent));

    if let Some(dir) = &targets.claude_dir {
        // Only removes `.claude` when nothing else is in it.
        let _ = std::fs::remove_dir(dir);
    }
    Ok(report)
}

/// `claude mcp add-json -s user myceliums <entry>`: unlike `claude mcp add`,
/// it takes the whole entry, `alwaysLoad` included.
fn claude_cli_add(entry: &Value) -> bool {
    let run = |args: &[&str]| {
        std::process::Command::new("claude")
            .args(args)
            .output()
            .map(|o| {
                (
                    o.status.success(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                )
            })
    };
    let entry = entry.to_string();
    let add = ["mcp", "add-json", "-s", "user", "myceliums", entry.as_str()];
    match run(&add) {
        Ok((true, _)) => true,
        Ok((false, stderr)) if stderr.contains("already exists") => {
            // best-effort: drop the stale entry; the re-add decides success.
            let _ = run(&["mcp", "remove", "-s", "user", "myceliums"]);
            matches!(run(&add), Ok((true, _)))
        }
        _ => false,
    }
}

fn claude_cli_remove() -> bool {
    std::process::Command::new("claude")
        .args(["mcp", "remove", "-s", "user", "myceliums"])
        .output()
        .is_ok_and(|o| o.status.success())
}

// ── CLI entry point ──────────────────────────────────────────────────

/// The `myc` executable named on `PATH`, if any.
fn myc_on_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "myc.exe" } else { "myc" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// The top level of the git checkout containing `dir`, if any.
pub(crate) fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    myceliums_core::git_read::git_read(
        dir,
        &["rev-parse", "--show-toplevel"],
        std::time::Duration::from_secs(5),
    )
    .ok()
    .map(|out| PathBuf::from(out.trim()))
    .filter(|p| !p.as_os_str().is_empty())
}

/// The project root: `--project`, else the git top level of the working
/// directory.
fn project_root(explicit: Option<&Path>) -> Result<PathBuf> {
    let root = match explicit {
        Some(p) => std::fs::canonicalize(p)
            .with_context(|| format!("No such directory: {}", p.display()))?,
        None => {
            let cwd = std::env::current_dir().context("Cannot read the working directory")?;
            let Some(top) = git_toplevel(&cwd) else {
                bail!(
                    "Not inside a git repository (or git is not installed). Run this in your project, pass --project <dir>, or use --scope user."
                );
            };
            top
        }
    };
    if dirs::home_dir().is_some_and(|h| std::fs::canonicalize(h).is_ok_and(|h| h == root)) {
        bail!(
            "{} is your home directory; use --scope user for a user-wide install",
            root.display()
        );
    }
    Ok(root)
}

fn user_scope_has_our_hooks(home: &Path) -> bool {
    let Ok((settings, _)) = read_json_object(&Targets::user(home).settings) else {
        return false;
    };
    let mut hooks = settings
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    remove_our_hooks(&mut hooks) > 0
}

/// `myc setup-claude [--scope project|user] [--uninstall] [--no-*]`.
pub fn cmd_setup_claude(
    scope: Scope,
    project: Option<&Path>,
    layers: Layers,
    uninstall: bool,
) -> Result<()> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let exe = std::env::current_exe().context("Could not determine myc binary path")?;
    let exe = exe.to_string_lossy().to_string();
    let (targets, registrar, myc_cmd) = match scope {
        Scope::Project => {
            let root = project_root(project)?;
            // A committed file must not name one machine's binary path.
            let cmd = if myc_on_path().is_some() {
                "myc".to_string()
            } else {
                exe.clone()
            };
            (Targets::project(&root), McpRegistrar::File, cmd)
        }
        Scope::User => (Targets::user(&home), McpRegistrar::ClaudeCli, exe.clone()),
    };

    let scope_name = match scope {
        Scope::Project => "project",
        Scope::User => "user",
    };
    if uninstall {
        println!("Removing the myceliums Claude Code integration ({scope_name} scope)...\n");
        let report = self::uninstall(&targets, registrar)?;
        print_report(&report);
        println!("\n  Done.");
        if scope == Scope::Project && user_scope_has_our_hooks(&home) {
            println!(
                "  Note: myceliums is still installed at user scope (~/.claude/settings.json);\n        `myc setup-claude --scope user --uninstall` removes it too."
            );
        }
        return Ok(());
    }

    println!("Setting up the myceliums Claude Code integration ({scope_name} scope)...\n");
    let report = install(&targets, &myc_cmd, layers, registrar)?;
    print_report(&report);
    println!();
    if scope == Scope::Project {
        if myc_cmd != "myc" {
            println!(
                "  Note: `myc` is not on PATH, so the project files name {exe}.\n        Put myc on PATH and re-run before committing them for a team."
            );
        }
        println!("  Commit .mcp.json, .claude/ and CLAUDE.md to share the integration.");
        println!("  Claude Code asks once per user to approve the project's MCP server.");
        if user_scope_has_our_hooks(&home) {
            println!(
                "  Note: myceliums is also installed at user scope (~/.claude/settings.json);\n        `myc setup-claude --scope user --uninstall` avoids running both."
            );
        }
    }
    match scope {
        Scope::User if layers.tool_loading == ToolLoading::UpFront => println!(
            "  The core tools load up front (alwaysLoad); re-run after adding toolsets with\n        `myc configure --set mcp.toolsets=...`, so the added groups stay deferred."
        ),
        Scope::Project => println!(
            "  .mcp.json carries no alwaysLoad (it is shared); the core tools still load up front\n        through their own _meta. For yourself only, a local entry replaces the project one:\n        claude mcp add-json -s local myceliums '{}'",
            mcp_entry(
                "myc",
                if layers.tool_loading == ToolLoading::Deferred {
                    ToolLoading::Deferred
                } else {
                    ToolLoading::UpFront
                }
            )
        ),
        _ => {}
    }
    println!(
        "  Opt out of a layer with --no-routing, --no-redirect-hook, --no-agent or --no-always-load."
    );
    println!("  To remove: myc setup-claude --scope {scope_name} --uninstall");
    Ok(())
}

fn print_report(report: &Report) {
    for (layer, outcome) in &report.lines {
        println!("  \u{2713} {layer:<11} {outcome}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn hook_commands_are_recognised_exactly() {
        for ours in [
            "myc hook-grep",
            "/usr/local/bin/myc session . --yes --timeout 300 2>/dev/null; exit 0",
            "/opt/homebrew/bin/myc format-hook 2>/dev/null",
            "'/Users/a b/bin/myc' hook-grep",
            "myc hook-grep 2>/dev/null || true",
        ] {
            assert!(is_myceliums_hook_command(ours), "{ours}");
        }
        for other in [
            "mycelium session",
            "myc analyze .",
            "echo myc hook-grep",
            "/bin/rmyc hook-grep",
            "prettier --write",
            "",
        ] {
            assert!(!is_myceliums_hook_command(other), "{other}");
        }
    }

    #[test]
    fn shell_quote_only_when_needed() {
        assert_eq!(shell_quote("/usr/local/bin/myc"), "/usr/local/bin/myc");
        assert_eq!(shell_quote("/Users/a b/myc"), "'/Users/a b/myc'");
        assert_eq!(shell_quote("/x/it's/myc"), "'/x/it'\\''s/myc'");
    }

    #[test]
    fn hook_groups_use_regex_matcher_for_mcp_and_never_block() {
        let groups = hook_groups("myc", Layers::ALL);
        let post = groups.iter().find(|(e, _)| *e == "PostToolUse").unwrap();
        assert_eq!(post.1["matcher"], "mcp__myceliums__.*");
        let pre: Vec<&Value> = groups
            .iter()
            .filter(|(e, _)| *e == "PreToolUse")
            .map(|(_, g)| g)
            .collect();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["matcher"], "Grep");
        assert_eq!(pre[1]["matcher"], "Bash");
        let ifs: Vec<&str> = pre[1]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["if"].as_str().unwrap())
            .collect();
        assert_eq!(ifs, ["Bash(grep *)", "Bash(rg *)", "Bash(git grep *)"]);
        for g in &pre {
            for h in g["hooks"].as_array().unwrap() {
                assert_eq!(h["command"], "myc hook-grep 2>/dev/null || true");
                assert_eq!(h["timeout"], 5);
                assert!(
                    h.get("onFailure").is_none(),
                    "a failing hook must not block"
                );
            }
        }
        let without = hook_groups(
            "myc",
            Layers {
                redirect_hook: false,
                ..Layers::ALL
            },
        );
        assert!(without.iter().all(|(e, _)| *e != "PreToolUse"));

        let start = groups.iter().find(|(e, _)| *e == "SessionStart").unwrap();
        let h = &start.1["hooks"][0];
        assert_eq!(
            h["command"],
            "myc session . --yes --timeout 300 2>/dev/null; exit 0"
        );
        assert_eq!(h["timeout"], 15);
        assert!(is_myceliums_hook_command(h["command"].as_str().unwrap()));
    }

    /// Exit code 2 from a PreToolUse hook blocks the tool call. Whatever the
    /// `myc` on the machine does (old version without `hook-grep` exiting 2
    /// on the unknown subcommand, a crash, no binary at all), the installed
    /// command must exit 0.
    #[cfg(unix)]
    #[test]
    fn installed_grep_hook_command_never_exits_non_zero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut fakes = Vec::new();
        for (name, body) in [
            (
                "old-myc",
                "echo 'error: unrecognized subcommand' >&2; exit 2",
            ),
            ("crashing-myc", "kill -SEGV $$"),
            ("noisy-myc", "echo '{\"decision\": \"block\"}' >&2; exit 1"),
        ] {
            let path = dir.path().join(name).join("myc");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            fakes.push(path.to_string_lossy().into_owned());
        }
        fakes.push(
            dir.path()
                .join("missing")
                .join("myc")
                .to_string_lossy()
                .into_owned(),
        );

        for myc in &fakes {
            for (event, group) in hook_groups(myc, Layers::ALL) {
                // PreToolUse exit 2 blocks the tool call; SessionStart must
                // never put a hook error into the session (#152).
                if event != "PreToolUse" && event != "SessionStart" {
                    continue;
                }
                for h in group["hooks"].as_array().unwrap() {
                    let command = h["command"].as_str().unwrap();
                    let status = std::process::Command::new("sh")
                        .args(["-c", command])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .unwrap();
                    assert_eq!(status.code(), Some(0), "{command}");
                }
            }
        }
    }

    fn foreign_settings() -> Value {
        json!({
            "permissions": {"allow": ["Bash(ls)"]},
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard.sh"}]}],
                "PostToolUse": [{
                    "matcher": "Edit",
                    "hooks": [
                        {"type": "command", "command": "prettier --write"},
                        {"type": "command", "command": "/old/bin/myc format-hook 2>/dev/null"}
                    ]
                }],
                "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "/old/bin/myc session . --yes"}]}]
            }
        })
    }

    #[test]
    fn project_install_is_idempotent_and_uninstall_restores_foreign_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let t = Targets::project(root);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            &t.settings,
            serde_json::to_string_pretty(&foreign_settings()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            &t.mcp_config,
            r#"{"mcpServers": {"other": {"command": "x"}}}"#,
        )
        .unwrap();
        std::fs::write(&t.memory, "# Rules\n\nBe nice.\n").unwrap();

        install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        let snapshot = |t: &Targets| {
            (
                std::fs::read_to_string(&t.mcp_config).unwrap(),
                std::fs::read_to_string(&t.settings).unwrap(),
                std::fs::read_to_string(&t.memory).unwrap(),
                std::fs::read_to_string(&t.agent).unwrap(),
            )
        };
        let first = snapshot(&t);
        install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        assert_eq!(first, snapshot(&t), "second install must change nothing");

        let mcp = read(&t.mcp_config);
        // A shared project file never carries alwaysLoad.
        assert_eq!(
            mcp["mcpServers"]["myceliums"],
            mcp_entry("myc", ToolLoading::PerTool)
        );
        assert!(mcp["mcpServers"]["myceliums"].get("alwaysLoad").is_none());
        assert_eq!(mcp["mcpServers"]["other"]["command"], "x");

        let settings = read(&t.settings);
        assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre[0]["hooks"][0]["command"], "guard.sh");
        assert_eq!(pre.len(), 3, "guard + Grep + Bash groups: {pre:?}");
        // The stale handler from an old install was replaced, not duplicated.
        let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(post[0]["hooks"][0]["command"], "prettier --write");
        assert_eq!(post[1]["matcher"], "mcp__myceliums__.*");
        assert_eq!(
            settings["hooks"]["SessionStart"].as_array().unwrap().len(),
            1
        );

        assert!(first
            .2
            .starts_with("# Rules\n\nBe nice.\n\n<!-- BEGIN myceliums -->"));
        assert!(first.2.contains("code-explorer"));
        assert_eq!(first.3, agent_definition());

        uninstall(&t, McpRegistrar::File).unwrap();
        let mcp = read(&t.mcp_config);
        assert!(mcp["mcpServers"].get("myceliums").is_none());
        assert_eq!(mcp["mcpServers"]["other"]["command"], "x");
        let settings = read(&t.settings);
        let mut expected = foreign_settings();
        // Our old handlers go too: they were myceliums handlers.
        expected["hooks"]["PostToolUse"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .pop();
        expected["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("SessionStart");
        assert_eq!(settings, expected);
        assert_eq!(
            std::fs::read_to_string(&t.memory).unwrap(),
            "# Rules\n\nBe nice.\n"
        );
        assert!(!t.agent.exists());
        assert!(!root.join(".claude").join("agents").exists());
    }

    #[test]
    fn project_uninstall_deletes_files_it_alone_created() {
        let dir = tempfile::tempdir().unwrap();
        let t = Targets::project(dir.path());
        install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        for p in [&t.mcp_config, &t.settings, &t.memory, &t.agent] {
            assert!(p.exists(), "{}", p.display());
        }
        uninstall(&t, McpRegistrar::File).unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "nothing may be left behind"
        );
        // Uninstalling again is a no-op, not an error.
        uninstall(&t, McpRegistrar::File).unwrap();
    }

    #[test]
    fn opt_outs_skip_and_remove_layers() {
        let dir = tempfile::tempdir().unwrap();
        let t = Targets::project(dir.path());
        install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        let none = Layers {
            routing: false,
            redirect_hook: false,
            agent: false,
            tool_loading: ToolLoading::Deferred,
        };
        install(&t, "myc", none, McpRegistrar::File).unwrap();
        assert!(
            !t.memory.exists(),
            "routing block removed, empty CLAUDE.md deleted"
        );
        assert!(!t.agent.exists());
        let settings = read(&t.settings);
        assert!(settings["hooks"].get("PreToolUse").is_none());
        assert!(settings["hooks"].get("SessionStart").is_some());
        assert!(read(&t.mcp_config)["mcpServers"].get("myceliums").is_some());
    }

    #[test]
    fn routing_without_subagent_does_not_mention_it() {
        let dir = tempfile::tempdir().unwrap();
        let t = Targets::project(dir.path());
        install(
            &t,
            "myc",
            Layers {
                agent: false,
                ..Layers::ALL
            },
            McpRegistrar::File,
        )
        .unwrap();
        assert!(!std::fs::read_to_string(&t.memory)
            .unwrap()
            .contains("code-explorer"));
    }

    #[test]
    fn a_users_own_agent_file_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let t = Targets::project(dir.path());
        std::fs::create_dir_all(t.agent.parent().unwrap()).unwrap();
        std::fs::write(&t.agent, "---\nname: code-explorer\n---\nmine\n").unwrap();
        let report = install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        assert!(report.lines.iter().any(|(_, o)| o.contains("kept")));
        uninstall(&t, McpRegistrar::File).unwrap();
        assert_eq!(
            std::fs::read_to_string(&t.agent).unwrap(),
            "---\nname: code-explorer\n---\nmine\n"
        );
    }

    #[test]
    fn unparseable_settings_are_refused_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let t = Targets::project(dir.path());
        std::fs::create_dir_all(t.settings.parent().unwrap()).unwrap();
        std::fs::write(&t.settings, "{ not json").unwrap();
        let err = install(&t, "myc", Layers::ALL, McpRegistrar::File).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "{err}");
        assert_eq!(std::fs::read_to_string(&t.settings).unwrap(), "{ not json");
    }

    #[test]
    fn user_scope_writes_under_home_and_keeps_empty_files() {
        let home = tempfile::tempdir().unwrap();
        let t = Targets::user(home.path());
        std::fs::write(&t.mcp_config, r#"{"numStartups": 3}"#).unwrap();
        install(&t, "/opt/myc/bin/myc", Layers::ALL, McpRegistrar::File).unwrap();
        let claude_json = read(&home.path().join(".claude.json"));
        assert_eq!(claude_json["numStartups"], 3);
        assert_eq!(
            claude_json["mcpServers"]["myceliums"]["command"],
            "/opt/myc/bin/myc"
        );
        let settings = read(&home.path().join(".claude/settings.json"));
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "/opt/myc/bin/myc hook-grep 2>/dev/null || true"
        );
        assert!(home.path().join(".claude/CLAUDE.md").exists());
        assert!(home.path().join(".claude/agents/code-explorer.md").exists());

        uninstall(&t, McpRegistrar::File).unwrap();
        assert_eq!(
            read(&home.path().join(".claude.json")),
            json!({"numStartups": 3})
        );
        // User files stay (emptied), the agent file goes.
        assert_eq!(read(&home.path().join(".claude/settings.json")), json!({}));
        assert_eq!(
            std::fs::read_to_string(home.path().join(".claude/CLAUDE.md")).unwrap(),
            ""
        );
        assert!(!home.path().join(".claude/agents/code-explorer.md").exists());
        assert!(home.path().join(".claude").exists());
    }

    #[test]
    fn agent_definition_has_the_documented_frontmatter() {
        let def = agent_definition();
        let front = def
            .strip_prefix("---\n")
            .unwrap()
            .split("\n---\n")
            .next()
            .unwrap();
        let keys: Vec<&str> = front
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, ["name", "description", "tools"]);
        assert!(front.contains("tools: mcp__myceliums__*, Read, ToolSearch"));
        assert!(front.contains(AGENT_MARKER));
        assert!(def.contains("Use proactively"));
    }

    #[test]
    fn the_mcp_entry_says_how_the_tools_load() {
        let up = mcp_entry("myc", ToolLoading::UpFront);
        assert_eq!(up["alwaysLoad"], true);
        assert_eq!(
            up["args"],
            json!(["mcp"]),
            "no pinned toolset: old myc keeps working"
        );
        assert!(mcp_entry("myc", ToolLoading::PerTool)
            .get("alwaysLoad")
            .is_none());
        assert_eq!(mcp_entry("myc", ToolLoading::Deferred)["alwaysLoad"], false);

        let deferred = Layers {
            tool_loading: ToolLoading::Deferred,
            ..Layers::ALL
        };
        let entry = |t: &Targets| read(&t.mcp_config)["mcpServers"]["myceliums"].clone();

        // A personal (user-scope) entry carries the key, rewritten in place.
        let home = tempfile::tempdir().unwrap();
        let user = Targets::user(home.path());
        install(&user, "myc", deferred, McpRegistrar::File).unwrap();
        assert_eq!(entry(&user)["alwaysLoad"], false);
        install(&user, "myc", Layers::ALL, McpRegistrar::File).unwrap();
        assert_eq!(entry(&user)["alwaysLoad"], true);

        // The shared project file never does, whatever the installer chose.
        let dir = tempfile::tempdir().unwrap();
        let project = Targets::project(dir.path());
        for layers in [Layers::ALL, deferred] {
            install(&project, "myc", layers, McpRegistrar::File).unwrap();
            assert!(entry(&project).get("alwaysLoad").is_none());
        }
    }
}
