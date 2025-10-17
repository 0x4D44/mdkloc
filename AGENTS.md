# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` hosts the CLI entry point and primary logic for counting lines of code across languages.
- `Cargo.toml` and `Cargo.lock` capture crate metadata, dependencies, and reproducible builds; edit `Cargo.toml` only when a dependency or feature genuinely changes.
- Place unit tests beside the code they exercise under `#[cfg(test)]`. Integration scenarios live in `tests/`.
- `readme.md` documents usage; update it whenever flags, output formats, or notable workflows change.

## Build, Test, and Development Commands
- `cargo build` compiles in debug mode; use before submitting changes to surface compiler warnings early.
- `cargo run -- <path> [flags]` executes the CLI locally, for example `cargo run -- . -v -f "*.rs"`.
- `cargo test` runs all unit and integration suites; append `-- --nocapture` when you need diagnostic output.
- `cargo fmt` applies repository formatting defaults; run prior to opening a pull request.
- `cargo clippy -- -D warnings` enforces lint cleanliness by treating warnings as failures.

## Coding Style & Naming Conventions
- Target Rust 2021 idioms with 4-space indentation and rustfmt defaults; avoid manual alignment that rustfmt would reflow.
- Prefer `snake_case` for functions and variables, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- Use `Result<T, E>` and the `?` operator for error propagation; reserve `unwrap()`/`expect()` for tests.
- Keep functions compact and avoid unnecessary allocations in tight loops.

## Testing Guidelines
- Cover edge cases like blank lines, comments, mixed encodings, and multi-language directories.
- Name tests descriptively (e.g., `counts_ignore_comments`) and place integration fixtures under `tests/`.
- Validate behavioural changes with `cargo test` and, where relevant, targeted `cargo run` examples.

## Commit & Pull Request Guidelines
- Write imperative, single-purpose commit messages (e.g., "Add support for TOML reports").
- Ensure every PR includes a short rationale, reproduction steps, and sample before/after CLI output.
- Confirm `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` all pass before requesting review.

## Agent-Specific Notes
- Make minimal, targeted edits; do not rename files or adjust public interfaces without necessity.
- Avoid adding dependencies unless they unlock required functionality; discuss trade-offs in PR notes.
- Never introduce secrets, network calls, or license headers. If unexpected external changes appear, pause and seek guidance.
