# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.3.x   | Yes       |
| < 0.3   | No        |

## Reporting a vulnerability

If you discover a security vulnerability, please report it responsibly.

**Do not open a public issue.** Instead, report it privately through
**[GitHub Security Advisories](https://github.com/marcmantei/myceliums/security/advisories/new)**.

Please include:

- Description of the vulnerability
- Steps to reproduce
- Affected version(s)
- Potential impact

## What to expect

Myceliums is maintained by a single person on a best-effort basis, so this is a
statement of intent rather than a service-level agreement: reports are usually
acknowledged within a few days, and fixes land as soon as practical, prioritised
by severity. If a report goes unanswered for two weeks, feel free to ping the
advisory thread.

## Scope

Myceliums is a local-first CLI tool and MCP server. It does not phone home, collect telemetry, or transmit data to external services. The primary attack surface is:

- **Parsing untrusted code** — tree-sitter grammars process arbitrary source files
- **MCP stdio transport** — the MCP server communicates over stdin/stdout with a local AI agent
- **`myc serve`** — the HTTP visualization server binds to localhost

## Dependencies

We recommend running `cargo audit` periodically to check for known vulnerabilities in dependencies.
