# mdkloc Context for Gemini

## Project Overview
`mdkloc` is a fast, multi-language lines-of-code (LOC) analyzer written in Rust. It counts code, comments, and blank lines across many programming languages (Rust, Python, C/C++, Java, Go, etc.) and markup formats. It aims to be a performant and simple alternative to tools like `tokei`.

## Architecture & Structure
- **Type:** Rust CLI application.
- **Entry Point:** `src/main.rs` (Monolithic structure containing logic and language parsers).
- **Tests:**
  - Unit tests: Located within `src/main.rs` (and `src/tests_included.rs` for test modules).
  - Integration tests: Located in `tests/` (e.g., `tests/cli_smoke.rs`), exercising the compiled binary.
- **Documentation:** `readme.md` (User guide), `docs/` (Project history and plans), `AGENTS.md` (Agent guidelines).

## Development Workflow

### Build & Run
*   **Build Debug:** `cargo build`
*   **Build Release:** `cargo build --release`
*   **Run:** `cargo run -- [PATH] [FLAGS]`
    *   Example: `cargo run -- . --verbose --non-recursive`

### Testing & Verification
*   **Run All Tests:** `cargo test`
*   **Run Specific Test:** `cargo test --test <test_name>` (e.g., `cargo test --test cli_smoke`)
*   **Format Code:** `cargo fmt`
*   **Lint:** `cargo clippy -- -D warnings`
*   **Coverage:** `cargo llvm-cov --workspace --summary-only` (requires `cargo-llvm-cov`)

## Conventions & Guidelines

### Coding Style
*   **Edition:** Rust 2021.
*   **Formatting:** Strictly adhere to `rustfmt`.
*   **Naming:** `snake_case` for functions/vars, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
*   **Error Handling:** Use `Result<T, E>` with propagation (`?`). Avoid `unwrap()`/`expect()` in production code; reserved for tests.

### Agent Instructions (from AGENTS.md)
*   **Minimal Changes:** Make targeted edits; do not rename files or change public interfaces unnecessarily.
*   **Dependencies:** Avoid adding new dependencies unless absolutely required.
*   **Verification:** ALWAYS run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` before considering a task complete.
*   **Safety:** Never introduce secrets, network calls, or license headers.

## Key Features
*   **Role Breakdown:** Distinguishes between "Mainline" (production) and "Test" code.
*   **Performance Metrics:** Tracks processing speed (files/lines per sec).
*   **Language Support:** Extensive list including legacy languages (COBOL, Fortran) and modern config files (TOML, YAML, HCL).
