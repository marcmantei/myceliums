# Myceliums — Agent Instructions

## Before committing

Always run `cargo fmt --all` before creating a commit. Rustfmt failures block CI auto-merge.

## CI requirements

All PRs must pass these checks before merge:
- **Build**: `cargo build --workspace`
- **Test**: `cargo test --workspace`
- **Clippy**: `cargo clippy --workspace -- -D warnings`
- **Rustfmt**: `cargo fmt --all -- --check`

## Commit style

- Use conventional commits: `feat:`, `fix:`, `style:`, `refactor:`, `test:`, `docs:`, `chore:`
- Reference the issue number in the commit message, e.g. `feat: add X (#123)`

## PR workflow

- PRs with all CI checks green auto-merge via squash
- If rustfmt fails, the autofix workflow will push a format commit — wait for re-run before investigating other failures
