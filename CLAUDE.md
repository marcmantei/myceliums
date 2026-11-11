# Myceliums — Agent Instructions

## Before committing

Always run `cargo fmt --all` before creating a commit. Rustfmt failures block CI auto-merge.

## CI requirements

All PRs must pass these checks before merge:
- **Build**: `cargo build --workspace --locked`
- **Test**: `cargo test --workspace --locked` (Linux and macOS)
- **Clippy**: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- **Rustfmt**: `cargo fmt --all -- --check`
- **MSRV**: `cargo check --workspace --all-targets` on the `rust-version` from `Cargo.toml`
- **cargo-deny**: `cargo deny check` — policy in `deny.toml`. A new advisory with no fix gets a commented `ignore` entry there, never a disabled check.

Workflow hygiene: pin third-party actions to a full commit SHA with the version in a comment, and give each workflow read-only (`contents: read`) or empty (`{}`) default permissions plus job-scoped extras only where needed.

## Commit style

- Use conventional commits: `feat:`, `fix:`, `style:`, `refactor:`, `test:`, `docs:`, `chore:`
- Reference the issue number in the commit message, e.g. `feat: add X (#123)`

## PR workflow

- PRs with all CI checks green auto-merge via squash
- If rustfmt fails, fix it locally with `cargo fmt --all` and push. Nothing reformats automatically: `autofix.yml` is manual-only (`gh workflow run autofix.yml --ref <branch>` pushes a format commit to that branch). A push made with `GITHUB_TOKEN` does not re-trigger CI, so push an empty commit or re-run the checks afterwards.
