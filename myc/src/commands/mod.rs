//! Subcommands that live outside `main.rs` (which #101 will split up).
//!
//! The agent integration kit (#126): one-shot graph navigation for agents
//! and hooks ([`nav`]), the Claude Code grep-redirect hook ([`hook_grep`]),
//! the routing block for CLAUDE.md / AGENTS.md ([`routing`]) and the layered
//! Claude Code installer ([`setup_claude`]).

pub mod hook_grep;
pub mod nav;
pub mod routing;
pub mod setup_claude;

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands};
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Commands, clap::Error> {
        Cli::try_parse_from(args).map(|cli| cli.command)
    }

    #[test]
    fn navigation_commands_parse_with_defaults() {
        let Commands::Callers {
            symbol,
            depth,
            repo,
            json,
            budget,
        } = parse(&["myc", "callers", "parse_query"]).unwrap()
        else {
            panic!("callers");
        };
        assert_eq!(
            (symbol.as_str(), depth, repo, json, budget),
            ("parse_query", 1, None, false, super::nav::DEFAULT_BUDGET)
        );
        assert!(matches!(
            parse(&["myc", "callees", "x", "--depth", "3", "--json"]).unwrap(),
            Commands::Callees {
                depth: 3,
                json: true,
                ..
            }
        ));
        let Commands::Context { query, .. } = parse(&[
            "myc", "context", "how", "does", "auth", "work", "--budget", "500",
        ])
        .unwrap() else {
            panic!("context");
        };
        assert_eq!(query.join(" "), "how does auth work");
        assert!(parse(&["myc", "context"]).is_err(), "context needs text");
    }

    #[test]
    fn impact_target_and_diff_are_exclusive() {
        assert!(matches!(
            parse(&["myc", "impact", "src/a.rs", "--json"]).unwrap(),
            Commands::Impact {
                target: Some(_),
                json: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&["myc", "impact"]).unwrap(),
            Commands::Impact { target: None, .. }
        ));
        assert!(parse(&["myc", "impact", "x", "--diff", "d.patch"]).is_err());
    }

    #[test]
    fn setup_claude_defaults_to_project_scope_with_every_layer() {
        let Commands::SetupClaude {
            scope,
            no_routing,
            no_redirect_hook,
            no_agent,
            uninstall,
            ..
        } = parse(&["myc", "setup-claude"]).unwrap()
        else {
            panic!("setup-claude");
        };
        assert_eq!(scope, super::setup_claude::Scope::Project);
        assert!(!(no_routing || no_redirect_hook || no_agent || uninstall));
        assert!(matches!(
            parse(&["myc", "setup-claude", "--scope", "user", "--no-agent"]).unwrap(),
            Commands::SetupClaude {
                scope: super::setup_claude::Scope::User,
                no_agent: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&["myc", "hook-grep"]).unwrap(),
            Commands::HookGrep
        ));
    }
}
