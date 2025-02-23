# mdkloc

A high-performance, multi-language source code analyzer written in Rust that provides detailed statistics about code, comment, and blank line distribution across your codebase.

## Features

- **Multi-Language Support**: Analyzes source code in multiple programming languages, including:
  - Systems: Rust, Go, C/C++
  - JVM: Java
  - .NET: C#
  - Web: JavaScript, TypeScript, JSX, TSX, PHP
  - Scripting: Python, Perl, Ruby, Shell
  - Others: Pascal

- **Comprehensive Analysis**: Provides detailed statistics for each file and directory:
  - Code lines count
  - Comment lines count (including support for language-specific comment styles)
  - Blank lines count
  - Per-language and overall metrics

- **Performance Features**:
  - Parallel processing capabilities
  - Real-time progress tracking
  - Performance metrics reporting
  - Configurable entry limits for large directories

- **Smart Detection**:
  - Automatic language detection based on file extensions
  - Support for multiple comment styles (line, block, documentation)
  - Unicode normalization for path handling
  - Case-insensitive file extension matching

## Installation

To install the tool, you'll need Rust installed on your system. Then run:

```bash
cargo install source-code-analyzer  # Replace with actual crate name
```

Or build from source:

```bash
git clone <repository-url>
cd source-code-analyzer
cargo build --release
```

## Usage

Basic usage:

```bash
source-code-analyzer [PATH]
```

### Command Line Options

- `[PATH]`: Directory to analyze (defaults to current directory)
- `-i, --ignore <PATHS>`: Directories to ignore (can be specified multiple times)
- `-v, --verbose`: Enable verbose output with per-file statistics
- `-m, --max-entries <NUMBER>`: Maximum number of entries to process (default: 1000000)

### Examples

Analyze current directory:
```bash
source-code-analyzer
```

Analyze specific directory with ignored paths:
```bash
source-code-analyzer /path/to/project --ignore node_modules --ignore target
```

Enable verbose output:
```bash
source-code-analyzer --verbose
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

## Features by Language

| Language    | Line Comments | Block Comments | Doc Comments | Special Features |
|------------|---------------|----------------|--------------|------------------|
| Rust       | //           | /* */         | /// //!      | Attribute support |
| Python     | #            | ''' '''       | -            | Multi-line strings |
| JavaScript | //           | /* */ <!--    | -            | JSX comments |
| Ruby       | #            | =begin/=end   | -            | Shebang support |
| Pascal     | //           | { } (* *)     | -            | Multiple block styles |

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

The project includes comprehensive tests covering:
- Directory scanning
- Line counting for each supported language
- UTF-8 handling
- Path truncation
- Extension recognition

## License

[Add your license information here]