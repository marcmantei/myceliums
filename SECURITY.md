# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.2.x   | Yes       |
| < 0.2   | No        |

## Reporting a vulnerability

If you discover a security vulnerability, please report it responsibly.

**Do not open a public issue.** Instead, use one of:

1. **GitHub Security Advisories** — [Report a vulnerability](https://github.com/marcmantei/myceliums/security/advisories/new) (preferred)
2. **Email** — Send details to **security@myceliums.ai**

Please include:

- Description of the vulnerability
- Steps to reproduce
- Affected version(s)
- Potential impact

## What to expect

- **Acknowledgement** within 48 hours
- **Status update** within 7 days
- **Fix or mitigation** as soon as practical, depending on severity

## Scope

Myceliums is a local-first CLI tool and MCP server. It does not phone home, collect telemetry, or transmit data to external services. The primary attack surface is:

- **Parsing untrusted code** — tree-sitter grammars process arbitrary source files
- **MCP stdio transport** — the MCP server communicates over stdin/stdout with a local AI agent
- **`myc serve`** — the HTTP visualization server binds to localhost

## Dependencies

We recommend running `cargo audit` periodically to check for known vulnerabilities in dependencies.
