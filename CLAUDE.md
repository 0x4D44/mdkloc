# mdkloc - Development Guide

## Build Commands
```
cargo build                 # Build debug version
cargo build --release       # Build release version
cargo run -- [PATH]         # Run with optional path argument
cargo test                  # Run all tests
cargo test test_rust_line_counting  # Run specific test
cargo clippy                # Run linter
cargo fmt                   # Format code
```

## Code Style Guidelines
- **Formatting**: Use rustfmt (cargo fmt)
- **Linting**: Follow clippy recommendations
- **Naming**: Use snake_case for variables/functions, CamelCase for types/structs
- **Imports**: Group std imports first, then third-party crates, then local modules
- **Error Handling**: Use Result<T, E> with proper propagation (?)
- **Comments**: Add doc comments (///) for public functions/structs
- **Testing**: Write unit tests for each language parser function

## Project Structure
- `src/main.rs`: Single file application with language parsers
- Current focus: Multi-language source code line counter
- Supported languages: Rust, Python, Java, C/C++, JavaScript, etc.