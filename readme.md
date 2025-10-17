# mdkloc v2.0.0

A fast, multi-language lines-of-code analyzer written in Rust. mdkloc reports per-language code, comment, and blank line counts by directory and totals. It aims to align with common tools (like tokei) while remaining simple and fast.

## What's New in 2.0.0

- Major language expansion: Scala, YAML, JSON, XML (incl. SVG/XSL), HTML, TOML, CMake, Dockerfile, Makefile, INI, HCL/Terraform, ReStructuredText, Velocity, Mustache, Protobuf, plus classic languages: Algol, COBOL, Fortran, x86 Assembly, DCL (OpenVMS), and IPLAN (PSS/E).
- Special-filename detection: Dockerfile, Makefile, CMakeLists.txt.
- CLI enhancements: `--max-depth`, `--non-recursive`, and `--filespec` filtering; colored output.
- Improved tests and stability.

## Features

- **Multi-language support** (non-exhaustive):
  - Core: Rust, Go, C/C++, Java, C#, Python, JavaScript/TypeScript (JSX/TSX), PHP, Perl, Ruby, Shell, Pascal
  - Config/Markup: YAML, JSON, XML, HTML, TOML, INI, CMake, Makefile, Dockerfile, HCL/Terraform, ReStructuredText, Velocity, Mustache, Protobuf
  - Classic/Legacy: Algol, COBOL, Fortran, Assembly, DCL (OpenVMS), IPLAN (PSS/E)

- **Comprehensive Analysis**: Provides detailed statistics for each file and directory:
  - Code lines count
  - Comment lines count (including support for language-specific comment styles)
  - Blank lines count
  - Per-language and overall metrics

- **Performance**:
  - Real-time progress + performance metrics
  - Efficient line counting per language
  - Configurable entry limits and depth limits

- **Smart detection**:
  - Extension-based language detection + special filenames (Dockerfile/Makefile/CMakeLists.txt)
  - Multiple comment styles supported (line/block/doc, where applicable)
  - Unicode normalization for paths; case-insensitive matching

## Installation

To install the tool, you'll need Rust installed on your system. Then run:

Build from source:

```bash
git clone <repository-url>
cd mdkloc
cargo build --release
```

## Usage

Basic usage:

```bash
mdkloc [PATH]
```

### Command Line Options

- `[PATH]`: Directory to analyze (defaults to current directory)
- `-i, --ignore <PATH>`: Ignore directories (repeatable)
- `-v, --verbose`: Per-file stats while scanning
- `-m, --max-entries <N>`: Max entries to process (default: 1,000,000)
- `-d, --max-depth <N>`: Limit recursion depth (default: 100)
- `-n, --non-recursive`: Only analyze the top-level directory
- `-f, --filespec <GLOB>`: Only include files matching the glob in each directory

### Examples

Analyze current directory:
```bash
mdkloc
```

Analyze specific directory with ignored paths:
```bash
mdkloc /path/to/project --ignore node_modules --ignore target
```

Enable verbose output:
```bash
mdkloc --verbose
```

## Output Format

The tool provides three levels of output:

1. **Progress Updates** (during processing):
   ```
   Processed 150 files (75.0 files/sec) and 45000 lines (22500.0 lines/sec)...
   ```

2. **Detailed Analysis** (per directory):
   ```
   Directory                                 Language     Files      Code  Comments     Blank
   -------------------------------------------------------------------------------
   ./src                                    Rust            10      1500       300       200
   ./tests                                  Rust             5       800       150       100
   ```

3. **Summary Statistics**:
   ```
   Overall Summary:
   Total files processed: 15
   Total lines processed: 3050
   Code lines:     2300 (75.4%)
   Comment lines:  450 (14.8%)
   Blank lines:    300 (9.8%)
   ```

## Features by Language (selection)

| Language    | Line Comments | Block Comments | Doc Comments | Special Features |
|------------|---------------|----------------|--------------|------------------|
| Rust       | //           | /* */         | /// //!      | Attribute lines count as code |
| Python     | #            | ''' '''       | -            | Multi-line strings |
| JavaScript | //           | /* */ <!--    | -            | JSX/HTML-style comments |
| Ruby       | #            | =begin/=end   | -            | Shebang support |
| Pascal     | //           | { } (* *)     | -            | Multiple block styles |
| YAML/TOML  | #            | -             | -            | Hash comments only |
| JSON       | -            | -             | -            | All non-blank is code |
| XML/HTML   | -            | <!-- -->      | -            | Block comments only |
| CMake      | #            | -             | -            | Line comments |
| Makefile   | #            | -             | -            | Line comments |
| HCL        | // #         | /* */         | -            | Line+block comments |
| COBOL      | col-7 */     | -             | -            | Fixed/free comment forms |
| Fortran    | ! / col-1    | -             | -            | Fixed-form indicators |
| Assembly   | ; #          | -             | -            | Line comments |
| DCL        | ! $!         | -             | -            | Line comments |
| IPLAN      | !            | /* */         | -            | Line+block comments |

## Auto-Ignored Directories

The following directories are automatically ignored:
- `target`
- `node_modules`
- `build`
- `dist`
- `.git`
- `venv`
- `__pycache__`
- `bin`
- `obj`

## Performance Considerations

- Uses efficient file reading with UTF-8 validation
- Handles invalid UTF-8 sequences gracefully
- Implements parallel processing for large codebases
- Provides real-time progress updates
- Configurable limits to prevent resource exhaustion

## Contributing

Contributions are welcome! Areas for potential improvement:

- Additional language support
- Enhanced comment detection algorithms
- Performance optimizations
- Additional metrics and analysis features
- Test coverage expansion

## Testing

Run the test suite:

```bash
cargo test
```

Integration tests that exercise the compiled binary live under `tests/`. Run them directly with:

```bash
cargo test --test cli_smoke
```

Before opening a pull request, run the repository checklist:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo llvm-cov --workspace --summary-only
```

See `docs/2025.10.17 - Coverage Recovery Plan.md` for the latest coverage targets and follow-up actions.

The project includes comprehensive tests covering:
- Directory scanning
- Line counting for each supported language
- UTF-8 handling
- Path truncation
- Extension recognition

## License

Licensed under the terms in LICENSE.

---

Notes
- Some legacy/templating languages are handled with practical heuristics (e.g., Algol COMMENT...; COBOL column 7; Fortran fixed/free forms). If you have dialect-specific files, open an issue with examples and we can refine the counters.
- To compare with tokei, use the Code column in both tools and ensure you scan the same directory set and language filters.
