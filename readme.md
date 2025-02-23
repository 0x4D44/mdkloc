# Source Code Analysis Tool

A fast and versatile command-line tool for analyzing source code across multiple programming languages. It computes detailed statistics on code lines, comment lines, and blank lines, providing a clear picture of the structure and documentation quality of your projects.

---

## Overview

The **Source Code Analysis Tool** is designed to help developers understand the composition of their codebases. Whether you’re looking to enforce coding standards, improve documentation, or simply gain insights into your project's structure, this tool offers:

- **Multi-language support:** Analyze files written in Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript, PHP, Perl, Ruby, Shell, and Pascal.
- **Detailed statistics:** Get counts of code lines, comment lines, and blank lines for each file and directory.
- **Real-time performance metrics:** Track files and lines processed with progress updates.
- **Customizable scanning:** Ignore specified directories, control verbosity, and limit the number of entries scanned.

---

## Features

- **Language Detection:** Automatically identifies the programming language based on file extension (case-insensitive).
- **Line Counting Strategies:** Implements specialized parsers for different languages to correctly identify code, comments, and blank lines.
- **Performance Metrics:** Displays real-time updates on files and lines processed, including throughput statistics.
- **Directory Scanning:** Recursively scans directories with built-in support to ignore common build and dependency folders (e.g., `node_modules`, `target`, `.git`, etc.).
- **Verbose Mode:** Optionally print detailed file-level analysis during the scan.
- **Unicode Handling:** Normalizes file paths using Unicode NFKC and safely reads file contents with invalid UTF‑8 sequences.

---

## Installation

### Prerequisites

- **Rust Toolchain:** Ensure you have the [Rust toolchain](https://www.rust-lang.org/tools/install) installed on your machine.

### Building from Source

Clone the repository and build the project using Cargo:

```bash
git clone https://github.com/yourusername/source-code-analysis-tool.git
cd source-code-analysis-tool
cargo build --release
