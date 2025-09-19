# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` — CLI entry point and core logic for multi-language LOC/statistics.
- `Cargo.toml`/`Cargo.lock` — crate metadata and dependencies.
- `readme.md` — usage and overview; `LICENSE` — licensing.
- Optional tests live in `tests/` (integration) and `#[cfg(test)]` modules (unit).
- Model notes: `GEMINI.md`, `CLAUDE.md` are companion docs. This AGENTS.md applies repo-wide.

## Build, Test, and Development Commands
- `cargo build` — compile in debug mode.
- `cargo run -- <path> [flags]` — run locally (e.g., `cargo run -- . -v -f "*.rs"`).
- `cargo test` — run unit/integration tests.
- `cargo fmt` — format code with rustfmt.
- `cargo clippy -- -D warnings` — lint and treat warnings as errors.
- `cargo run -- --help` — view CLI options.

## Coding Style & Naming Conventions
- Rust 2021; rustfmt defaults (4-space indent). Run `cargo fmt` before commits.
- Naming: `snake_case` for functions/vars, `CamelCase` for types/traits, `SCREAMING_SNAKE_CASE` for consts.
- Errors: return `Result<T, E>`, prefer `?`; avoid `unwrap()`/`expect()` outside tests.
- Keep functions small and focused; prefer clear, non-allocating paths in hot loops.

## Testing Guidelines
- Unit tests colocated under `#[cfg(test)] mod tests { ... }` near the code.
- Integration tests in `tests/*.rs` (e.g., `tests/cli_counts.rs`).
- Use descriptive names and cover core parsing branches and edge cases (comments, blanks, encodings).
- Run `cargo test -- --nocapture` to view printed diagnostics.

## Commit & Pull Request Guidelines
- Commits: imperative, concise summaries (e.g., "Add support for TOML", "Fix PHP post-comment code detection"). One logical change per commit.
- PRs: clear description, rationale, reproduction steps, and sample before/after output. Link related issues. Ensure `cargo fmt`, `clippy`, and tests pass.

## Agent-Specific Instructions
- Scope: applies to the entire repository. Make minimal, targeted changes and preserve file names and public behavior unless necessary.
- Don’t add new dependencies without justification. Update `readme.md` when behavior or flags change.
- Validate locally: run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` before proposing changes.
- Avoid introducing secrets, network calls, or license headers.
