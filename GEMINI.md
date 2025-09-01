# Project Overview

This project is a high-performance, multi-language source code analyzer written in Rust. It provides detailed statistics about code, comment, and blank line distribution across a codebase.

**Main Technologies:**

*   **Language:** Rust
*   **Libraries:**
    *   `clap`: For command-line argument parsing.
    *   `unicode-normalization`: For normalizing path strings.

**Architecture:**

The tool is a command-line application that recursively scans a directory, identifies files of supported languages based on their extensions, and analyzes each file to count lines of code, comments, and blank lines. It supports parallel processing and provides real-time progress updates.

# Building and Running

**Build:**

To build the project, use the following command:

```bash
cargo build --release
```

**Run:**

To run the application, use the following command:

```bash
./target/release/mdkloc [PATH] [OPTIONS]
```

**Command-line Options:**

*   `[PATH]`: The directory to analyze (defaults to the current directory).
*   `-i, --ignore <PATHS>`: Directories to ignore (can be specified multiple times).
*   `-v, --verbose`: Enable verbose output with per-file statistics.
*   `-m, --max-entries <NUMBER>`: Maximum number of entries to process (default: 1000000).
*   `-d, --max-depth <NUMBER>`: Maximum directory depth to scan (default: 100).

**Test:**

To run the test suite, use the following command:

```bash
cargo test
```

# Development Conventions

*   **Coding Style:** The code follows standard Rust conventions, with a focus on performance and safety.
*   **Error Handling:** The application uses `io::Result` for error handling and provides informative error messages.
*   **Testing:** The project includes a comprehensive test suite using `#[cfg(test)]` and the `tempfile` crate for creating temporary files and directories for testing purposes.
*   **Modularity:** The code is organized into functions with specific responsibilities, such as parsing command-line arguments, scanning directories, and counting lines of code for different languages.
*   **Clean Compilation:** All code must compile cleanly with no warnings.
