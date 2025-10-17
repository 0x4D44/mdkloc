//! Source Code Analysis Tool
//!
//! This tool performs comprehensive analysis of source code across multiple programming languages,
//! providing detailed statistics about code, comment, and blank line distribution.
//!
//! Supported languages: Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript,
//! PHP, Perl, Ruby, Shell, Pascal, Scala, YAML, XML, JSON, HTML, TOML,
//! Makefile, Dockerfile, INI, HCL, CMake, PowerShell, Batch, TCL,
//! ReStructuredText, Velocity, Mustache, Protobuf, SVG, XSL,
//! Algol, COBOL, Fortran, Assembly, DCL, IPLAN.

use clap::{ArgAction, Parser};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use colored::*;
use glob::Pattern;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// Fixed width for the directory column.
const DIR_WIDTH: usize = 40;
const LANG_WIDTH: usize = 16;

// Performance metrics structure
struct PerformanceMetrics {
    files_processed: Arc<AtomicU64>,
    lines_processed: Arc<AtomicU64>,
    start_time: Instant,
    last_update: Instant,
    writer: Box<dyn Write + Send>,
    progress_enabled: bool,
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Source code analyser for multiple programming languages",
    long_about = "Supported languages: Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript, PHP, Perl, Ruby, Shell, Pascal, Scala, YAML, XML, JSON, HTML, TOML, Makefile, Dockerfile, INI, HCL, CMake, PowerShell, Batch, TCL, ReStructuredText, Velocity, Mustache, Protobuf, SVG, XSL, Algol, COBOL, Fortran, Assembly, DCL, IPLAN.",
    color = clap::ColorChoice::Always
)]
struct Args {
    #[arg(default_value = ".")]
    path: String,

    #[arg(short, long, action = ArgAction::Append)]
    ignore: Vec<String>,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long, default_value = "1000000")]
    max_entries: usize,

    #[arg(short = 'd', long, default_value = "100")]
    max_depth: usize,

    #[arg(short = 'n', long)]
    non_recursive: bool,

    #[arg(short = 'f', long)]
    filespec: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
struct LanguageStats {
    code_lines: u64,
    comment_lines: u64,
    blank_lines: u64,
    overlap_lines: u64,
}

#[derive(Debug, Default)]
struct DirectoryStats {
    language_stats: HashMap<String, (u64, LanguageStats)>, // (file_count, stats) per language
}

fn normalize_stats(mut stats: LanguageStats, total_lines: u64) -> LanguageStats {
    if total_lines == 0 {
        return stats;
    }
    let sum = stats.code_lines + stats.comment_lines + stats.blank_lines;
    if sum > total_lines {
        let mut overlap = sum - total_lines;
        if stats.blank_lines > 0 {
            let blank_reduce = stats.blank_lines.min(overlap);
            stats.blank_lines -= blank_reduce;
            overlap -= blank_reduce;
        }
        stats.overlap_lines = overlap;
    } else if sum < total_lines && sum > 0 {
        stats.blank_lines += total_lines - sum;
        stats.overlap_lines = 0;
    } else {
        stats.overlap_lines = 0;
    }
    stats
}

impl PerformanceMetrics {
    fn new() -> Self {
        PerformanceMetrics::with_writer(Box::new(io::stdout()), true)
    }

    fn with_writer(writer: Box<dyn Write + Send>, progress_enabled: bool) -> Self {
        PerformanceMetrics {
            files_processed: Arc::new(AtomicU64::new(0)),
            lines_processed: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            last_update: Instant::now(),
            writer,
            progress_enabled,
        }
    }

    fn update(&mut self, new_lines: u64) {
        self.files_processed.fetch_add(1, Ordering::Relaxed);
        self.lines_processed.fetch_add(new_lines, Ordering::Relaxed);

        // Update progress every second
        let now = Instant::now();
        if now.duration_since(self.last_update) >= Duration::from_secs(1) {
            self.print_progress();
            self.last_update = now;
        }
    }

    fn print_progress(&mut self) {
        if !self.progress_enabled {
            return;
        }

        let elapsed = self.start_time.elapsed().as_secs_f64();
        let files = self.files_processed.load(Ordering::Relaxed);
        let lines = self.lines_processed.load(Ordering::Relaxed);

        let writer = &mut self.writer;
        let _ = write!(
            writer,
            "\rProcessed {} files ({:.1} files/sec) and {} lines ({:.1} lines/sec)...",
            files,
            files as f64 / elapsed,
            lines,
            lines as f64 / elapsed
        );
        let _ = writer.flush();
    }

    fn print_final_stats(&mut self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let files = self.files_processed.load(Ordering::Relaxed);
        let lines = self.lines_processed.load(Ordering::Relaxed);

        let writer = &mut self.writer;
        let _ = writeln!(writer, "\n\n{}", "Performance Summary:".blue().bold());
        let _ = writeln!(
            writer,
            "Total time: {} seconds",
            format!("{:.2}", elapsed).bright_yellow()
        );
        let _ = writeln!(
            writer,
            "Files processed: {} ({})",
            files.to_string().bright_yellow(),
            format!("{:.1} files/sec", safe_rate(files, elapsed)).bright_yellow()
        );
        let _ = writeln!(
            writer,
            "Lines processed: {} ({})",
            lines.to_string().bright_yellow(),
            format!("{:.1} lines/sec", safe_rate(lines, elapsed)).bright_yellow()
        );
    }
}

/// Reads a file’s entire content as lines, converting invalid UTF‑8 sequences using replacement characters.
struct LossyLineReader {
    reader: BufReader<fs::File>,
    buffer: Vec<u8>,
}

impl LossyLineReader {
    fn new(file: fs::File) -> Self {
        Self {
            reader: BufReader::new(file),
            buffer: Vec::with_capacity(8 * 1024),
        }
    }
}

impl Iterator for LossyLineReader {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        self.buffer.clear();
        match self.reader.read_until(b'\n', &mut self.buffer) {
            Ok(0) => None,
            Ok(_) => {
                let text = String::from_utf8_lossy(&self.buffer);
                let line = text.trim_end_matches(['\n', '\r']).to_string();
                Some(Ok(line))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

/// Returns an iterator over the lines of a file, replacing invalid UTF-8 bytes with the replacement character.
fn read_file_lines_lossy(file_path: &Path) -> io::Result<LossyLineReader> {
    let file = fs::File::open(file_path)?;
    Ok(LossyLineReader::new(file))
}

/// Identify the language based on filename and/or extension (case-insensitive).
/// Returns a static string to avoid allocations; callers can `.to_string()` when needed.
fn get_language_from_extension(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_lowercase();
    // Special filenames without extensions
    if lower.starts_with("dockerfile") {
        return Some("Dockerfile");
    }
    if lower == "makefile" || lower == "gnumakefile" || lower == "bsdmakefile" {
        return Some("Makefile");
    }
    if lower == "cmakelists.txt" {
        return Some("CMake");
    }
    // Common shell dotfiles
    match lower.as_str() {
        ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | ".zprofile" | ".zshenv"
        | ".kshrc" | ".cshrc" => {
            return Some("Shell");
        }
        _ => {}
    }

    // Extract extension if present
    let (_stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e.to_lowercase()),
        _ => return None,
    };
    let ext = ext.as_str();
    match ext {
        // Core set
        "rs" => Some("Rust"),
        "go" => Some("Go"),
        "py" => Some("Python"),
        "java" => Some("Java"),
        "cpp" | "c" | "h" | "hpp" => Some("C/C++"),
        "cs" => Some("C#"),
        "js" => Some("JavaScript"),
        "ts" => Some("TypeScript"),
        "jsx" => Some("JSX"),
        "tsx" => Some("TSX"),
        "php" => Some("PHP"),
        "pl" | "pm" | "t" => Some("Perl"),
        "rb" => Some("Ruby"),
        "sh" => Some("Shell"),
        "pas" => Some("Pascal"),
        // Newly supported
        "scala" | "sbt" => Some("Scala"),
        "yaml" | "yml" => Some("YAML"),
        "json" => Some("JSON"),
        // XML family (SVG/XSL handled separately)
        "xml" | "xsd" => Some("XML"),
        "html" | "htm" | "xhtml" => Some("HTML"),
        "toml" => Some("TOML"),
        // Makefile variants
        "mk" | "mak" => Some("Makefile"),
        // INI-like
        "ini" | "cfg" | "conf" | "properties" | "prop" => Some("INI"),
        // HCL / Terraform
        "hcl" | "tf" | "tfvars" => Some("HCL"),
        // CMake modules
        "cmake" => Some("CMake"),
        // PowerShell
        "ps1" | "psm1" | "psd1" => Some("PowerShell"),
        // Batch / CMD
        "bat" | "cmd" => Some("Batch"),
        // TCL
        "tcl" => Some("TCL"),
        // ReStructuredText
        "rst" | "rest" => Some("ReStructuredText"),
        // Velocity templates
        "vm" | "vtl" => Some("Velocity"),
        // Mustache templates
        "mustache" => Some("Mustache"),
        // Protobuf
        "proto" => Some("Protobuf"),
        // SVG / XSL
        "svg" => Some("SVG"),
        "xsl" | "xslt" => Some("XSL"),
        // Algol
        "alg" | "algol" | "a60" | "a68" => Some("Algol"),
        // COBOL and copybooks
        "cob" | "cbl" | "cobol" | "cpy" => Some("COBOL"),
        // Fortran (fixed/free forms)
        "f" | "for" | "f77" | "f90" | "f95" | "f03" | "f08" | "f18" => Some("Fortran"),
        // Assembly (x86 et al.)
        "asm" | "s" | "S" => Some("Assembly"),
        // DCL (OpenVMS command procedures)
        "com" => Some("DCL"),
        // IPLAN (PSS/E)
        "ipl" => Some("IPLAN"),
        _ => None,
    }
}

fn is_ignored_dir(path: &Path) -> bool {
    let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ignored = [
        "target",
        "node_modules",
        "build",
        "dist",
        ".git",
        "venv",
        "__pycache__",
        "bin",
        "obj",
    ];
    ignored.contains(&dir_name)
}

/// Helper function that truncates the given string to a maximum number of characters by keeping the last characters.
/// If truncation occurs, the returned string is prefixed with "..." so that its total length equals max_len.
fn truncate_start(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        // More efficient implementation without multiple reverses and unnecessary allocations
        // Skip front chars to keep only the last (max_len - 3) chars, then prepend "..."
        let skip_count = char_count - (max_len - 3);
        let truncated: String = s.chars().skip(skip_count).collect();
        format!("...{}", truncated)
    }
}

fn safe_rate(value: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= f64::EPSILON {
        0.0
    } else {
        value as f64 / elapsed_secs
    }
}

fn safe_percentage(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 / denominator as f64) * 100.0
    }
}

/// Delegate counting to the appropriate parser based on file extension.
fn count_lines_with_stats(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Inspect filename for special cases (Dockerfile*, Makefile variants)
    let file_name_lower = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    if file_name_lower.starts_with("dockerfile") {
        return count_dockerfile_lines(file_path);
    }
    if file_name_lower == "makefile"
        || file_name_lower == "gnumakefile"
        || file_name_lower == "bsdmakefile"
    {
        return count_makefile_lines(file_path);
    }
    if file_name_lower == "cmakelists.txt" {
        return count_cmake_lines(file_path);
    }
    // Get extension in lowercase for case-insensitive matching.
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "rs" => count_rust_lines(file_path),
        "go" => count_c_style_lines(file_path),
        "py" => count_python_lines(file_path),
        "java" | "c" | "cpp" | "h" | "hpp" | "cs" => count_c_style_lines(file_path),
        "js" | "ts" | "jsx" | "tsx" => count_javascript_lines(file_path),
        "php" => count_php_lines(file_path),
        "pl" | "pm" | "t" => count_perl_lines(file_path),
        "rb" => count_ruby_lines(file_path),
        "sh" => count_shell_lines(file_path),
        "pas" => count_pascal_lines(file_path),
        // Newly supported languages
        "scala" | "sbt" => count_c_style_lines(file_path),
        "yaml" | "yml" => count_yaml_lines(file_path),
        "json" => count_json_lines(file_path),
        "xml" | "xsd" => count_xml_like_lines(file_path),
        "html" | "htm" | "xhtml" => count_xml_like_lines(file_path),
        "toml" => count_toml_lines(file_path),
        "mk" | "mak" => count_makefile_lines(file_path),
        "ini" | "cfg" | "conf" | "properties" | "prop" => count_ini_lines(file_path),
        "hcl" | "tf" | "tfvars" => count_hcl_lines(file_path),
        "cmake" => count_cmake_lines(file_path),
        "ps1" | "psm1" | "psd1" => count_powershell_lines(file_path),
        "bat" | "cmd" => count_batch_lines(file_path),
        "tcl" => count_tcl_lines(file_path),
        "rst" | "rest" => count_rst_lines(file_path),
        "vm" | "vtl" => count_velocity_lines(file_path),
        "mustache" => count_mustache_lines(file_path),
        "proto" => count_c_style_lines(file_path),
        "svg" => count_xml_like_lines(file_path),
        "xsl" | "xslt" => count_xml_like_lines(file_path),
        // New classic languages
        "alg" | "algol" | "a60" | "a68" => count_algol_lines(file_path),
        "cob" | "cbl" | "cobol" | "cpy" => count_cobol_lines(file_path),
        "f" | "for" | "f77" | "f90" | "f95" | "f03" | "f08" | "f18" => {
            count_fortran_lines(file_path)
        }
        "asm" | "s" => count_asm_lines(file_path),
        "com" => count_dcl_lines(file_path),
        "ipl" => count_iplan_lines(file_path),
        _ => count_generic_lines(file_path),
    }
}

fn count_generic_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if line.trim().is_empty() {
            stats.blank_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_rust_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block_comment {
            stats.comment_lines += 1;
            if trimmed.contains("*/") {
                in_block_comment = false;
                if let Some(after_comment) = trimmed.split("*/").nth(1) {
                    if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("#[") {
            stats.code_lines += 1;
            continue;
        }
        if trimmed.contains("/*") {
            stats.comment_lines += 1;
            if let Some(before_comment) = trimmed.split("/*").next() {
                if !before_comment.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            if !trimmed.contains("*/") {
                in_block_comment = true;
            } else if let Some(after_comment) = trimmed.split("*/").nth(1) {
                if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with("///") || trimmed.starts_with("//!") || trimmed.starts_with("//") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_python_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_multiline_string = false;
    let mut multiline_quote_char = '"';
    let mut prev_line_continued = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_multiline_string {
            stats.comment_lines += 1;
            let quote = multiline_quote_char.to_string().repeat(3);
            if trimmed.contains(&quote) {
                in_multiline_string = false;
                if let Some(code) = trimmed.split(&quote).nth(1) {
                    if !code.trim().is_empty() && !code.trim_start().starts_with("#") {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("#") {
            stats.comment_lines += 1;
            continue;
        }
        if (trimmed.starts_with("'''") || trimmed.starts_with("\"\"\"")) && !prev_line_continued {
            let quote = &trimmed[..3];
            if trimmed.len() >= 6 && trimmed[3..].contains(quote) {
                stats.comment_lines += 1;
                if let Some(code) = trimmed.split(quote).nth(2) {
                    if !code.trim().is_empty() && !code.trim_start().starts_with("#") {
                        stats.code_lines += 1;
                    }
                }
            } else {
                in_multiline_string = true;
                multiline_quote_char = quote.chars().next().unwrap();
                stats.comment_lines += 1;
            }
            continue;
        }
        prev_line_continued = trimmed.ends_with('\\');
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_c_style_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let mut s = line.as_str();
        let trimmed_line = s.trim();
        if trimmed_line.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        loop {
            if in_block_comment {
                if let Some(end) = s.find("*/") {
                    stats.comment_lines += 1;
                    s = &s[end + 2..];
                    in_block_comment = false;
                    if s.trim().is_empty() {
                        break;
                    } else {
                        continue;
                    }
                } else {
                    stats.comment_lines += 1;
                    break;
                }
            } else {
                let p_line = s.find("//");
                let p_block = s.find("/*");
                match (p_line, p_block) {
                    (None, None) => {
                        if !s.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        break;
                    }
                    (Some(pl), None) => {
                        let before = &s[..pl];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1; // rest of line is comment
                        break;
                    }
                    (None, Some(pb)) => {
                        let before = &s[..pb];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        s = &s[pb + 2..];
                        if let Some(end) = s.find("*/") {
                            s = &s[end + 2..];
                            if s.trim().is_empty() {
                                break;
                            } else {
                                continue;
                            }
                        } else {
                            in_block_comment = true;
                            break;
                        }
                    }
                    (Some(pl), Some(pb)) => {
                        if pl < pb {
                            let before = &s[..pl];
                            if !before.trim().is_empty() {
                                stats.code_lines += 1;
                            }
                            stats.comment_lines += 1;
                            break; // rest is comment
                        } else {
                            let before = &s[..pb];
                            if !before.trim().is_empty() {
                                stats.code_lines += 1;
                            }
                            stats.comment_lines += 1;
                            s = &s[pb + 2..];
                            if let Some(end) = s.find("*/") {
                                s = &s[end + 2..];
                                if s.trim().is_empty() {
                                    break;
                                } else {
                                    continue;
                                }
                            } else {
                                in_block_comment = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok((stats, total_lines))
}

fn count_javascript_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut in_jsx_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block_comment {
            stats.comment_lines += 1;
            if trimmed.contains("*/") {
                in_block_comment = false;
                if let Some(after_comment) = trimmed.split("*/").nth(1) {
                    if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if in_jsx_comment {
            stats.comment_lines += 1;
            if trimmed.contains("-->") {
                in_jsx_comment = false;
                if let Some(after_comment) = trimmed.split("-->").nth(1) {
                    if !after_comment.trim().is_empty() {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            stats.comment_lines += 1;
            if let Some(before_comment) = trimmed.split("/*").next() {
                if !before_comment.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            if !trimmed.contains("*/") {
                in_block_comment = true;
            } else if let Some(after_comment) = trimmed.split("*/").nth(1) {
                if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with("<!--") {
            stats.comment_lines += 1;
            if let Some(before_comment) = trimmed.split("<!--").next() {
                if !before_comment.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            if !trimmed.contains("-->") {
                in_jsx_comment = true;
            } else if let Some(after_comment) = trimmed.split("-->").nth(1) {
                if !after_comment.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with("//") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_php_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block_comment {
            stats.comment_lines += 1;
            if trimmed.contains("*/") {
                in_block_comment = false;
                if let Some(code) = trimmed.split("*/").nth(1) {
                    let code_trimmed = code.trim_start();
                    if !code_trimmed.is_empty()
                        && !code_trimmed.starts_with("//")
                        && !code_trimmed.starts_with('#')
                    {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if let Some(pos) = trimmed.find("/*") {
            // code before block
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            // same-line close?
            if let Some(end) = trimmed[pos..].find("*/") {
                let after = &trimmed[(pos + end + 2)..];
                let after_trim = after.trim_start();
                if !after_trim.is_empty()
                    && !after_trim.starts_with("//")
                    && !after_trim.starts_with('#')
                {
                    stats.code_lines += 1;
                }
            } else {
                in_block_comment = true;
            }
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with("#") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_perl_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_pod_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with("=pod") || trimmed.starts_with("=head") {
            in_pod_comment = true;
            stats.comment_lines += 1;
            continue;
        }
        if trimmed.starts_with("=cut") {
            in_pod_comment = false;
            stats.comment_lines += 1;
            continue;
        }
        if in_pod_comment {
            stats.comment_lines += 1;
            continue;
        }
        if trimmed.starts_with('#') && !trimmed.starts_with("#!") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

/// Ruby: supports line comments (with a special case for shebang) and block comments delimited by "=begin" and "=end".
fn count_ruby_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut line_number = 0;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        line_number += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block_comment {
            stats.comment_lines += 1;
            if trimmed == "=end" {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("=begin") {
            in_block_comment = true;
            stats.comment_lines += 1;
            continue;
        }
        if trimmed.starts_with("#") {
            if line_number == 1 && trimmed.starts_with("#!") {
                stats.code_lines += 1;
            } else {
                stats.comment_lines += 1;
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

/// Shell: supports line comments (with a special case for shebang).
fn count_shell_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut line_number = 0;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        line_number += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with("#") {
            if line_number == 1 && trimmed.starts_with("#!") {
                stats.code_lines += 1;
            } else {
                stats.comment_lines += 1;
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

/// Pascal: supports line comments ("//") and block comments delimited by "{" and "}" or "(*" and "*)".
/// Improved to support nested block comments by tracking nesting level.
fn count_pascal_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;

    // Track both comment type and nesting level
    let mut brace_comment_level = 0; // For { } comments
    let mut parenthesis_comment_level = 0; // For (* *) comments

    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }

        // If in any block comment
        if brace_comment_level > 0 || parenthesis_comment_level > 0 {
            stats.comment_lines += 1;

            // Count nested braces
            if brace_comment_level > 0 {
                brace_comment_level += trimmed.matches("{").count() as i32;
                brace_comment_level -= trimmed.matches("}").count() as i32;

                // If we've closed all brace comments, check for code after the closing brace
                if brace_comment_level == 0 {
                    if let Some(after) = trimmed.split("}").last() {
                        if !after.trim().is_empty() && !after.trim().starts_with("//") {
                            stats.code_lines += 1;
                        }
                    }
                }
            }

            // Count nested parenthesis comments
            if parenthesis_comment_level > 0 {
                parenthesis_comment_level += trimmed.matches("(*").count() as i32;
                parenthesis_comment_level -= trimmed.matches("*)").count() as i32;

                // If we've closed all parenthesis comments, check for code after
                if parenthesis_comment_level == 0 {
                    if let Some(after) = trimmed.split("*)").last() {
                        if !after.trim().is_empty() && !after.trim().starts_with("//") {
                            stats.code_lines += 1;
                        }
                    }
                }
            }

            continue;
        }

        // Line comments
        if trimmed.starts_with("//") {
            stats.comment_lines += 1;
            continue;
        }

        // Start of brace comment
        if trimmed.contains("{") {
            stats.comment_lines += 1;

            // Check for code before the comment
            if let Some(before) = trimmed.split('{').next() {
                if !before.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }

            brace_comment_level += 1;
            brace_comment_level -= trimmed.matches("}").count() as i32;

            // If comment ends on same line
            if brace_comment_level == 0 {
                if let Some(after) = trimmed.split("}").last() {
                    if !after.trim().is_empty() && !after.trim().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }

            continue;
        }

        // Start of parenthesis comment
        if trimmed.contains("(*") {
            stats.comment_lines += 1;

            // Check for code before the comment
            if let Some(before) = trimmed.split("(*").next() {
                if !before.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }

            parenthesis_comment_level += 1;
            parenthesis_comment_level -= trimmed.matches("*)").count() as i32;

            // If comment ends on same line
            if parenthesis_comment_level == 0 {
                if let Some(after) = trimmed.split("*)").last() {
                    if !after.trim().is_empty() && !after.trim().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }

            continue;
        }

        // Regular code line
        stats.code_lines += 1;
    }

    Ok((stats, total_lines))
}

// TOML: supports line comments with '#'.
// (removed duplicate count_toml_lines)

/// Count lines for languages with hash-prefixed line comments only (e.g., YAML, TOML).
fn count_hash_comment_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
        } else if trimmed.starts_with('#') {
            stats.comment_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_yaml_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    count_hash_comment_lines(file_path)
}

fn count_toml_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    count_hash_comment_lines(file_path)
}

fn count_makefile_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Make treats leading '#' as comment. We don’t parse recipe semantics; keep it simple.
    count_hash_comment_lines(file_path)
}

fn count_dockerfile_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Dockerfile uses '#' for comments; everything else is code or blank.
    count_hash_comment_lines(file_path)
}

fn count_ini_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
        } else if trimmed.starts_with(';') || trimmed.starts_with('#') {
            stats.comment_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_hcl_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_block = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let mut s = line.as_str();
        let trimmed_line = s.trim();
        if trimmed_line.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        loop {
            if in_block {
                if let Some(end) = s.find("*/") {
                    stats.comment_lines += 1;
                    s = &s[end + 2..];
                    in_block = false;
                    if s.trim().is_empty() {
                        break;
                    } else {
                        continue;
                    }
                } else {
                    stats.comment_lines += 1;
                    break;
                }
            } else {
                let p_line1 = s.find("//");
                let p_line2 = s.find('#');
                let p_block = s.find("/*");
                let mut next: Option<(&str, usize)> = None;
                if let Some(i) = p_line1 {
                    next = Some(("//", i));
                }
                if let Some(i) = p_line2 {
                    next = match next {
                        Some((k, j)) if j <= i => Some((k, j)),
                        _ => Some(("#", i)),
                    };
                }
                if let Some(i) = p_block {
                    next = match next {
                        Some((k, j)) if j <= i => Some((k, j)),
                        _ => Some(("/*", i)),
                    };
                }
                match next {
                    None => {
                        if !s.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        break;
                    }
                    Some(("//", i)) => {
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        break;
                    }
                    Some(("#", i)) => {
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        break;
                    }
                    Some(("/*", i)) => {
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        s = &s[i + 2..];
                        if let Some(end) = s.find("*/") {
                            s = &s[end + 2..];
                            if s.trim().is_empty() {
                                break;
                            } else {
                                continue;
                            }
                        } else {
                            in_block = true;
                            break;
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    Ok((stats, total_lines))
}

fn count_rst_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Keep simple and in line with tokei: non-blank lines are code; no comments.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if line.trim().is_empty() {
            stats.blank_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_velocity_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Velocity: '##' line comments, '#* ... *#' block comments. Count code before/after markers.
    let mut stats = LanguageStats::default();
    let mut in_block = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block {
            stats.comment_lines += 1;
            if let Some(pos) = trimmed.find("*#") {
                in_block = false;
                let after = &trimmed[(pos + 2)..];
                if !after.trim().is_empty() && !after.trim_start().starts_with("##") {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with("##") {
            stats.comment_lines += 1;
            continue;
        }
        if let Some(pos) = trimmed.find("#*") {
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            if !trimmed[pos..].contains("*#") {
                in_block = true;
            } else if let Some(end) = trimmed[pos..].find("*#") {
                let after = &trimmed[(pos + end + 2)..];
                if !after.trim().is_empty() && !after.trim_start().starts_with("##") {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_mustache_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Mustache: comments start with '{{!' and end at the next '}}' (may cross lines).
    let mut stats = LanguageStats::default();
    let mut in_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_comment {
            stats.comment_lines += 1;
            if let Some(pos) = trimmed.find("}}") {
                // close
                in_comment = false;
                let after = &trimmed[(pos + 2)..];
                if !after.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if let Some(pos) = trimmed.find("{{!") {
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            if !trimmed[pos..].contains("}}") {
                in_comment = true;
            } else if let Some(end) = trimmed[pos..].find("}}") {
                let after = &trimmed[(pos + end + 2)..];
                if !after.trim().is_empty() {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

// --- New classic languages ---

fn count_algol_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Approximate support for ALGOL 60/68 comment styles:
    // - Lines beginning with 'COMMENT' (case-insensitive) treated as comment (until ';' on the same line).
    // - Single-line forms like 'co ... co' and '# ... #' are treated as full-line comments if they start the line.
    let mut stats = LanguageStats::default();
    let mut in_comment_until_semicolon = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        let lower = trimmed.to_lowercase();
        if in_comment_until_semicolon {
            stats.comment_lines += 1;
            if lower.contains(';') {
                in_comment_until_semicolon = false;
            }
            continue;
        }
        if lower.starts_with("comment") {
            stats.comment_lines += 1;
            if !lower.contains(';') {
                in_comment_until_semicolon = true;
            }
            continue;
        }
        if lower.starts_with("co ") && lower.ends_with(" co") {
            stats.comment_lines += 1;
            continue;
        }
        if lower.starts_with('#') {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_cobol_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // COBOL: fixed format comment indicator in column 7 ('*' or '/'),
    // and free-format comment starting with '*>'. We treat lines accordingly.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if line.trim().is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("*>") {
            stats.comment_lines += 1;
            continue;
        }
        // Column 7 indicator (index 6, 0-based) in the original line
        let col7 = line.chars().nth(6);
        if matches!(col7, Some('*') | Some('/')) {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_fortran_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Fortran: fixed-form comment if first column is C/c/*/D/d; '!' creates inline comment in free-form.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if line.trim().is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        let first = line.chars().next().unwrap_or(' ');
        let trimmed = line.trim_start();
        if matches!(first, 'C' | 'c' | '*' | 'D' | 'd') {
            stats.comment_lines += 1;
            continue;
        }
        if let Some(pos) = trimmed.find('!') {
            // code before '!' counts as code; rest as comment
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_asm_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Assembly (NASM/MASM ';' comments, GAS '#' comments). Full-line only.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.starts_with("//") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_dcl_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // OpenVMS DCL: comments start with '!' or '$!' on a line. Commands typically start with '$'.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    let mut is_dcl: Option<bool> = None;

    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if is_dcl.is_none() {
            let trimmed_start = line.trim_start();
            if !trimmed_start.is_empty() {
                is_dcl = Some(trimmed_start.starts_with('$') || trimmed_start.starts_with('!'));
            }
        }
        if matches!(is_dcl, Some(false)) {
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with("$!") || trimmed.starts_with('!') {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }

    if matches!(is_dcl, Some(false)) {
        Ok((LanguageStats::default(), total_lines))
    } else {
        Ok((stats, total_lines))
    }
}

fn count_iplan_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // PSS/E IPLAN: supports C-style block comments /* ... */ and '!' full-line comments.
    let mut stats = LanguageStats::default();
    let mut in_block = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if in_block {
            stats.comment_lines += 1;
            if let Some(pos) = trimmed.find("*/") {
                in_block = false;
                let after = &trimmed[(pos + 2)..];
                if !after.trim().is_empty() && !after.trim_start().starts_with('!') {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with('!') {
            stats.comment_lines += 1;
            continue;
        }
        if let Some(pos) = trimmed.find("/*") {
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            if !trimmed[pos..].contains("*/") {
                in_block = true;
            } else if let Some(end) = trimmed[pos..].find("*/") {
                let after = &trimmed[(pos + end + 2)..];
                if !after.trim().is_empty() && !after.trim_start().starts_with('!') {
                    stats.code_lines += 1;
                }
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

fn count_cmake_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // CMake uses '#' for line comments; no block comment syntax.
    count_hash_comment_lines(file_path)
}

fn count_powershell_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // PowerShell supports '#' line comments and <# ... #> block comments.
    let mut stats = LanguageStats::default();
    let mut in_block = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let mut s = line.as_str();
        let trimmed_line = s.trim();
        if trimmed_line.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        loop {
            if in_block {
                if let Some(end) = s.find("#>") {
                    stats.comment_lines += 1;
                    s = &s[end + 2..];
                    in_block = false;
                    if s.trim().is_empty() {
                        break;
                    } else {
                        continue;
                    }
                } else {
                    stats.comment_lines += 1;
                    break;
                }
            } else {
                let p_line = s.find('#');
                let p_block = s.find("<#");
                match (p_line, p_block) {
                    (None, None) => {
                        if !s.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        break;
                    }
                    (Some(pl), None) => {
                        let before = &s[..pl];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        break;
                    }
                    (None, Some(pb)) => {
                        let before = &s[..pb];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        s = &s[pb + 2..];
                        if let Some(end) = s.find("#>") {
                            s = &s[end + 2..];
                            if s.trim().is_empty() {
                                break;
                            } else {
                                continue;
                            }
                        } else {
                            in_block = true;
                            break;
                        }
                    }
                    (Some(pl), Some(pb)) => {
                        if pl < pb {
                            let before = &s[..pl];
                            if !before.trim().is_empty() {
                                stats.code_lines += 1;
                            }
                            stats.comment_lines += 1;
                            break;
                        } else {
                            let before = &s[..pb];
                            if !before.trim().is_empty() {
                                stats.code_lines += 1;
                            }
                            stats.comment_lines += 1;
                            s = &s[pb + 2..];
                            if let Some(end) = s.find("#>") {
                                s = &s[end + 2..];
                                if s.trim().is_empty() {
                                    break;
                                } else {
                                    continue;
                                }
                            } else {
                                in_block = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok((stats, total_lines))
}

fn count_batch_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Batch files treat lines starting with REM (case-insensitive) or :: as comments.
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        let upper = trimmed.to_uppercase();
        if upper.starts_with("REM ") || upper == "REM" || trimmed.starts_with("::") {
            stats.comment_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_tcl_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // TCL: '#' starts a comment; shebang on first line counts as code like shell.
    let mut stats = LanguageStats::default();
    let mut line_no = 0u64;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        line_no += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with('#') {
            if line_no == 1 && trimmed.starts_with("#!") {
                stats.code_lines += 1;
            } else {
                stats.comment_lines += 1;
            }
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}

/// JSON has no comments per spec; count non-blank as code.
fn count_json_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        if line.trim().is_empty() {
            stats.blank_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

/// Shared XML/HTML style comment handling for <!-- ... -->. Everything else non-blank is code.
fn count_xml_like_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let mut stats = LanguageStats::default();
    let mut in_comment = false;
    let mut total_lines = 0;
    for line_result in read_file_lines_lossy(file_path)? {
        let line = line_result?;
        total_lines += 1;
        let mut s = line.as_str();
        let trimmed_line = s.trim();
        if trimmed_line.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        loop {
            if in_comment {
                if let Some(end) = s.find("-->") {
                    stats.comment_lines += 1;
                    s = &s[end + 3..];
                    in_comment = false;
                    if s.trim().is_empty() {
                        break;
                    } else {
                        continue;
                    }
                } else {
                    stats.comment_lines += 1;
                    break;
                }
            } else if let Some(pos) = s.find("<!--") {
                let before = &s[..pos];
                if !before.trim().is_empty() {
                    stats.code_lines += 1;
                }
                stats.comment_lines += 1;
                s = &s[pos + 4..];
                if let Some(end) = s.find("-->") {
                    s = &s[end + 3..];
                    if s.trim().is_empty() {
                        break;
                    } else {
                        continue;
                    }
                } else {
                    in_comment = true;
                    break;
                }
            } else {
                if !s.trim().is_empty() {
                    stats.code_lines += 1;
                }
                break;
            }
        }
    }
    Ok((stats, total_lines))
}

/// Recursively scan directories and collect statistics.
/// Added error tracking and directory depth limiting to prevent stack overflow.
fn should_process_file(filespec: Option<&Pattern>, root_path: &Path, file_path: &Path) -> bool {
    filespec
        .map(|pattern| filespec_matches(pattern, root_path, file_path))
        .unwrap_or(true)
}

fn filespec_matches(pattern: &Pattern, root_path: &Path, file_path: &Path) -> bool {
    if file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| pattern.matches(name))
        .unwrap_or(false)
    {
        return true;
    }

    let relative = match file_path.strip_prefix(root_path) {
        Ok(rel) => rel,
        Err(_) => return false,
    };

    let rel_str = match relative.to_str() {
        Some(s) => s.replace('\\', "/"),
        None => return false,
    };

    pattern.matches(&rel_str)
}

#[allow(clippy::too_many_arguments)]
fn process_file(
    file_path: &Path,
    args: &Args,
    root_path: &Path,
    metrics: &mut PerformanceMetrics,
    stats: &mut HashMap<PathBuf, DirectoryStats>,
    entries_count: &mut usize,
    error_count: &mut usize,
    filespec: Option<&Pattern>,
) -> io::Result<()> {
    if !should_process_file(filespec, root_path, file_path) {
        return Ok(());
    }

    *entries_count += 1;
    if *entries_count > args.max_entries {
        return Err(io::Error::other("Too many entries in directory tree"));
    }

    let Some(language) = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(get_language_from_extension)
    else {
        return Ok(());
    };

    match count_lines_with_stats(file_path) {
        Ok((raw_stats, total_lines)) => {
            let file_stats = normalize_stats(raw_stats, total_lines);
            metrics.update(total_lines);
            let total_line_kinds =
                file_stats.code_lines + file_stats.comment_lines + file_stats.blank_lines;
            if total_line_kinds > 0 || total_lines == 0 {
                let dir_path = file_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let dir_stats = stats.entry(dir_path).or_default();
                let (count, lang_stats) = dir_stats
                    .language_stats
                    .entry(language.to_string())
                    .or_insert((0, LanguageStats::default()));
                *count += 1;
                lang_stats.code_lines += file_stats.code_lines;
                lang_stats.comment_lines += file_stats.comment_lines;
                lang_stats.blank_lines += file_stats.blank_lines;
                lang_stats.overlap_lines += file_stats.overlap_lines;

                if args.verbose {
                    println!("File: {}", file_path.display());
                    println!("  Code lines: {}", file_stats.code_lines);
                    println!("  Comment lines: {}", file_stats.comment_lines);
                    println!("  Blank lines: {}", file_stats.blank_lines);
                    println!("  Mixed code/comment lines: {}", file_stats.overlap_lines);
                    println!();
                }
            }
        }
        Err(err) => {
            eprintln!("Error counting lines in {}: {}", file_path.display(), err);
            *error_count += 1;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn scan_directory_impl(
    path: &Path,
    args: &Args,
    root_path: &Path,
    metrics: &mut PerformanceMetrics,
    current_depth: usize,
    entries_count: &mut usize,
    error_count: &mut usize,
    filespec: Option<&Pattern>,
) -> io::Result<HashMap<PathBuf, DirectoryStats>> {
    if current_depth > args.max_depth {
        eprintln!(
            "Warning: Maximum directory depth ({}) reached at {}",
            args.max_depth,
            path.display()
        );
        *error_count += 1;
        return Ok(HashMap::new());
    }

    if args.non_recursive && current_depth > 0 {
        return Ok(HashMap::new());
    }

    let mut stats: HashMap<PathBuf, DirectoryStats> =
        HashMap::with_capacity(if path.is_dir() { 128 } else { 1 });

    if is_ignored_dir(path) || args.ignore.iter().any(|d| path.ends_with(Path::new(d))) {
        return Ok(stats);
    }

    let metadata = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!("Error reading metadata for {}: {}", path.display(), err);
            *error_count += 1;
            return Ok(stats);
        }
    };

    if metadata.is_file() {
        process_file(
            path,
            args,
            root_path,
            metrics,
            &mut stats,
            entries_count,
            error_count,
            filespec,
        )?;
        return Ok(stats);
    }

    if !metadata.is_dir() {
        return Ok(stats);
    }

    let read_dir = match fs::read_dir(path) {
        Ok(iter) => iter,
        Err(err) => {
            eprintln!("Error reading directory {}: {}", path.display(), err);
            *error_count += 1;
            return Ok(stats);
        }
    };

    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("Error reading entry in {}: {}", path.display(), err);
                *error_count += 1;
                continue;
            }
        };

        let entry_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(err) => {
                eprintln!("Error reading type for {}: {}", entry_path.display(), err);
                *error_count += 1;
                continue;
            }
        };

        if file_type.is_dir() && !file_type.is_symlink() {
            if args.non_recursive {
                continue;
            }
            match scan_directory_impl(
                &entry_path,
                args,
                root_path,
                metrics,
                current_depth + 1,
                entries_count,
                error_count,
                filespec,
            ) {
                Ok(sub_stats) => {
                    for (dir, stat) in sub_stats {
                        if let Some(existing) = stats.get_mut(&dir) {
                            for (lang, (count, lang_stats)) in stat.language_stats {
                                let (existing_count, existing_stats) = existing
                                    .language_stats
                                    .entry(lang)
                                    .or_insert((0, LanguageStats::default()));
                                *existing_count += count;
                                existing_stats.code_lines += lang_stats.code_lines;
                                existing_stats.comment_lines += lang_stats.comment_lines;
                                existing_stats.blank_lines += lang_stats.blank_lines;
                                existing_stats.overlap_lines += lang_stats.overlap_lines;
                            }
                        } else {
                            stats.insert(dir, stat);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Error scanning directory {}: {}", entry_path.display(), err);
                    *error_count += 1;
                }
            }
        } else if file_type.is_file() && !file_type.is_symlink() {
            process_file(
                &entry_path,
                args,
                root_path,
                metrics,
                &mut stats,
                entries_count,
                error_count,
                filespec,
            )?;
        }
    }

    Ok(stats)
}

fn scan_directory(
    path: &Path,
    args: &Args,
    _current_dir: &Path,
    metrics: &mut PerformanceMetrics,
    current_depth: usize,
    entries_count: &mut usize,
    error_count: &mut usize,
) -> io::Result<HashMap<PathBuf, DirectoryStats>> {
    let filespec_pattern = match args.filespec.as_deref() {
        Some(spec) => Some(Pattern::new(spec).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid filespec pattern '{}': {}", spec, err),
            )
        })?),
        None => None,
    };

    let root_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    scan_directory_impl(
        &root_path,
        args,
        &root_path,
        metrics,
        current_depth,
        entries_count,
        error_count,
        filespec_pattern.as_ref(),
    )
}

/// Helper function to print stats for a language
fn format_language_stats_line(
    prefix: &str,
    lang: &str,
    file_count: u64,
    stats: &LanguageStats,
) -> String {
    format!(
        "{:<40} {:<width$} {:>8} {:>10} {:>10} {:>10} {:>10}",
        prefix,
        lang,
        file_count,
        stats.code_lines,
        stats.comment_lines,
        stats.overlap_lines,
        stats.blank_lines,
        width = LANG_WIDTH
    )
}

fn print_language_stats(prefix: &str, lang: &str, file_count: u64, stats: &LanguageStats) {
    // Keep rows uncolored to ensure ANSI-safe alignment; headers are colored separately.
    println!(
        "{}",
        format_language_stats_line(prefix, lang, file_count, stats)
    );
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let mut metrics = PerformanceMetrics::new();
    run_cli_with_metrics(args, &mut metrics)
}

fn run_cli_with_metrics(args: Args, metrics: &mut PerformanceMetrics) -> io::Result<()> {
    println!(
        "{} {}",
        env!("CARGO_PKG_NAME").bright_cyan().bold(),
        format!("v{}", env!("CARGO_PKG_VERSION")).bright_yellow()
    );

    let path = Path::new(&args.path);
    let current_dir = env::current_dir()?;
    let mut error_count = 0;

    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Path does not exist: {}", path.display()),
        ));
    }

    println!("Starting source code analysis...");
    // Start with depth 0 and track errors
    let mut entries_count: usize = 0;
    let stats = scan_directory(
        path,
        &args,
        &current_dir,
        metrics,
        0,
        &mut entries_count,
        &mut error_count,
    )?;
    metrics.print_final_stats();
    let files_processed = metrics.files_processed.load(Ordering::Relaxed);
    let lines_processed = metrics.lines_processed.load(Ordering::Relaxed);

    // Print detailed analysis with fixed-width directory field.
    let mut total_by_language: HashMap<String, (u64, LanguageStats)> = HashMap::new();
    let mut sorted_stats: Vec<_> = stats.iter().collect();
    sorted_stats.sort_by(|(a, _), (b, _)| a.to_string_lossy().cmp(&b.to_string_lossy()));

    println!("\n\nDetailed source code analysis:");
    println!("{}", "-".repeat(112));
    println!(
        "{:<40} {:<width$} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "Directory",
        "Language",
        "Files",
        "Code",
        "Comments",
        "Mixed",
        "Blank",
        width = LANG_WIDTH
    );
    println!("{}", "-".repeat(112));

    for (path, dir_stats) in &sorted_stats {
        // Use a reference to avoid unnecessary string cloning
        let raw_display = match path.strip_prefix(&current_dir) {
            Ok(p) if p.as_os_str().is_empty() => ".",
            Ok(p) => p.to_str().unwrap_or(path.to_str().unwrap_or("")),
            Err(_) => path.to_str().unwrap_or(""),
        };

        // Truncate the directory name from the start if it is too long.
        let display_path = truncate_start(raw_display, DIR_WIDTH);

        let mut languages: Vec<_> = dir_stats.language_stats.iter().collect();
        languages.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (lang, (file_count, lang_stats)) in &languages {
            print_language_stats(&display_path, lang, *file_count, lang_stats);

            let (total_count, total_stats) = total_by_language
                .entry(lang.to_string())
                .or_insert((0, LanguageStats::default()));
            *total_count += file_count;
            total_stats.code_lines += lang_stats.code_lines;
            total_stats.comment_lines += lang_stats.comment_lines;
            total_stats.blank_lines += lang_stats.blank_lines;
            total_stats.overlap_lines += lang_stats.overlap_lines;
        }
    }

    println!("{:-<112}", "");
    println!("Totals by language:");

    let mut sorted_totals: Vec<_> = total_by_language.iter().collect();
    sorted_totals.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (lang, (file_count, stats)) in sorted_totals {
        print_language_stats("", lang, *file_count, stats);
    }

    let mut grand_total = LanguageStats::default();

    for (_, (_files, stats)) in total_by_language.iter() {
        grand_total.code_lines += stats.code_lines;
        grand_total.comment_lines += stats.comment_lines;
        grand_total.blank_lines += stats.blank_lines;
        grand_total.overlap_lines += stats.overlap_lines;
    }

    if files_processed > 0 || lines_processed > 0 {
        println!("\n{}", "Overall Summary:".blue().bold());
        println!(
            "Total files processed: {}",
            files_processed.to_string().bright_yellow()
        );
        println!(
            "Total lines processed: {}",
            lines_processed.to_string().bright_yellow()
        );
        println!(
            "Code lines:     {} ({})",
            grand_total.code_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.code_lines, lines_processed)
            )
            .bright_yellow()
        );
        println!(
            "Comment lines:  {} ({})",
            grand_total.comment_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.comment_lines, lines_processed)
            )
            .bright_yellow()
        );
        println!(
            "Mixed lines:    {} ({})",
            grand_total.overlap_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.overlap_lines, lines_processed)
            )
            .bright_yellow()
        );
        println!(
            "Blank lines:    {} ({})",
            grand_total.blank_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.blank_lines, lines_processed)
            )
            .bright_yellow()
        );

        if error_count > 0 {
            println!(
                "\n{}: {}",
                "Warning".red().bold(),
                error_count.to_string().bright_yellow()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use colored::control;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    fn test_args() -> Args {
        Args {
            path: String::from("."),
            ignore: Vec::new(),
            verbose: false,
            max_entries: 1000000,
            max_depth: 100,
            non_recursive: false,
            filespec: None,
        }
    }

    fn test_metrics() -> PerformanceMetrics {
        PerformanceMetrics::with_writer(Box::new(io::sink()), false)
    }

    fn create_test_file(dir: &Path, name: &str, content: &str) -> io::Result<()> {
        let path = dir.join(name);
        let mut file = File::create(path)?;
        write!(file, "{}", content)?;
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_special_cases() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Dockerfile.prod",
            "FROM alpine\n# comment\n",
        )?;
        create_test_file(temp_dir.path(), "Makefile", "all:\n\t@echo \\\"done\\\"\n")?;
        create_test_file(
            temp_dir.path(),
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.25)\n# note\n",
        )?;
        create_test_file(temp_dir.path(), "unknown.xyz", "plain text line\n")?;

        let (docker_stats, docker_total) =
            count_lines_with_stats(&temp_dir.path().join("Dockerfile.prod"))?;
        assert_eq!(docker_total, 2);
        assert!(docker_stats.comment_lines >= 1);

        let (make_stats, _) = count_lines_with_stats(&temp_dir.path().join("Makefile"))?;
        assert!(make_stats.code_lines >= 1);

        let (cmake_stats, _) = count_lines_with_stats(&temp_dir.path().join("CMakeLists.txt"))?;
        assert!(cmake_stats.comment_lines >= 1);

        let (unknown_stats, _) = count_lines_with_stats(&temp_dir.path().join("unknown.xyz"))?;
        assert!(unknown_stats.code_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_proto_and_svg() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "model.proto",
            "syntax = \"proto3\";\n// comment\nmessage Foo {\n  string name = 1;\n}\n",
        )?;
        let (proto_stats, _) = count_lines_with_stats(&temp_dir.path().join("model.proto"))?;
        assert!(
            proto_stats.comment_lines >= 1 && proto_stats.code_lines >= 3,
            "proto stats: {:?}",
            proto_stats
        );

        create_test_file(
            temp_dir.path(),
            "diagram.SVG",
            "<svg><!-- note --><g/></svg>\n",
        )?;
        let (svg_stats, _) = count_lines_with_stats(&temp_dir.path().join("diagram.SVG"))?;
        assert!(
            svg_stats.comment_lines >= 1 && svg_stats.code_lines >= 1,
            "svg stats: {:?}",
            svg_stats
        );
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_powershell() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "script.PS1",
            "Write-Host 'start'\n<# block comment #>\nWrite-Host 'done'\n",
        )?;
        let (stats, _) = count_lines_with_stats(&temp_dir.path().join("script.PS1"))?;
        assert!(
            stats.code_lines >= 2 && stats.comment_lines >= 1,
            "powershell stats: {:?}",
            stats
        );
        Ok(())
    }

    #[test]
    fn test_process_file_missing_source_increments_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let missing = temp_dir.path().join("ghost.rs");
        let mut metrics = test_metrics();
        let mut stats = std::collections::HashMap::new();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;

        process_file(
            &missing,
            &test_args(),
            temp_dir.path(),
            &mut metrics,
            &mut stats,
            &mut entries_count,
            &mut error_count,
            None,
        )?;

        assert!(stats.is_empty());
        assert_eq!(error_count, 1);
        assert_eq!(entries_count, 1);
        Ok(())
    }

    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
            Self { buffer }
        }

        fn into_string(buffer: Arc<Mutex<Vec<u8>>>) -> String {
            let data = buffer.lock().expect("lock poisoned").clone();
            String::from_utf8_lossy(&data).into_owned()
        }
    }

    #[test]
    fn test_performance_metrics_custom_writer() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter::new(buffer.clone());
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), true);
        metrics.last_update = metrics.start_time - Duration::from_secs(2);
        metrics.update(10);
        metrics.print_final_stats();
        let output = CaptureWriter::into_string(buffer);
        assert!(output.contains("Processed"));
        assert!(output.contains("Performance Summary"));
    }

    #[test]
    fn test_performance_metrics_progress() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter::new(buffer.clone());
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), true);
        metrics.last_update = metrics.start_time - Duration::from_secs(2);
        metrics.update(5);
        let output = CaptureWriter::into_string(buffer.clone());
        assert!(
            output.contains("Processed 1 files"),
            "progress output missing expected prefix: {output}"
        );
        metrics.print_progress();
        let output = CaptureWriter::into_string(buffer);
        assert!(
            output.contains("files/sec"),
            "progress output missing rate info: {output}"
        );
    }

    #[test]
    fn test_performance_metrics_disabled_progress_skips_output() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter::new(buffer.clone());
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), false);
        metrics.last_update = metrics.start_time - Duration::from_secs(2);
        metrics.update(3);
        let output = CaptureWriter::into_string(buffer);
        assert!(
            output.is_empty(),
            "expected no output when progress disabled, got: {output}"
        );
    }

    #[test]
    fn test_performance_metrics_update_throttle_without_output() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter::new(buffer.clone());
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), true);
        metrics.update(1);
        let output = CaptureWriter::into_string(buffer);
        assert!(
            output.is_empty(),
            "throttle should suppress early output, got: {output}"
        );
    }

    #[test]
    fn test_run_cli_with_metrics_outputs_summary() -> io::Result<()> {
        control::set_override(false);
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "main.rs", "fn main() {}\n// comment\n")?;
        let args = Args::parse_from([
            "mdkloc",
            temp_dir
                .path()
                .to_str()
                .expect("temp dir path should be valid UTF-8"),
            "--non-recursive",
        ]);
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter::new(buffer.clone());
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), false);
        run_cli_with_metrics(args, &mut metrics)?;
        let output = CaptureWriter::into_string(buffer);
        assert!(
            output.contains("files/sec"),
            "expected rates to be reported in output: {output}"
        );
        Ok(())
    }

    #[test]
    fn test_run_cli_with_metrics_missing_path() {
        control::set_override(false);
        let missing = TempDir::new()
            .expect("create temp dir")
            .path()
            .join("subdir")
            .join("missing");
        let args = Args::parse_from([
            "mdkloc",
            missing.to_str().expect("path should be valid UTF-8"),
        ]);
        let mut metrics = test_metrics();
        let result = run_cli_with_metrics(args, &mut metrics);
        assert!(result.is_err());
        if let Err(err) = result {
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut guard = self.buffer.lock().expect("lock poisoned");
            guard.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_safe_rate_handles_zero_elapsed() {
        assert_eq!(safe_rate(100, 0.0), 0.0);
    }

    #[test]
    fn test_safe_rate_precision() {
        let rate = safe_rate(4850468, 10.0);
        assert!((rate - 485046.8).abs() < 1e-6);
    }

    #[test]
    fn test_safe_percentage_handles_zero_denominator() {
        assert_eq!(safe_percentage(42, 0), 0.0);
    }

    #[test]
    fn test_safe_percentage_precision() {
        let pct = safe_percentage(375, 1000);
        assert!((pct - 37.5).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_stats_eliminates_overlap() {
        let stats = LanguageStats {
            code_lines: 2,
            comment_lines: 2,
            blank_lines: 0,
            overlap_lines: 0,
        };
        let normalized = normalize_stats(stats, 3);
        assert_eq!(
            normalized.code_lines + normalized.comment_lines + normalized.blank_lines
                - normalized.overlap_lines,
            3
        );
        assert_eq!(normalized.comment_lines, stats.comment_lines);
        assert_eq!(normalized.overlap_lines, 1);
    }

    #[test]
    fn test_normalize_stats_does_not_inflate_when_zero_sum() {
        let stats = LanguageStats {
            code_lines: 0,
            comment_lines: 0,
            blank_lines: 0,
            overlap_lines: 0,
        };
        let normalized = normalize_stats(stats, 5);
        assert_eq!(normalized.code_lines, 0);
        assert_eq!(normalized.comment_lines, 0);
        assert_eq!(normalized.blank_lines, 0);
        assert_eq!(normalized.overlap_lines, 0);
    }

    #[test]
    fn test_directory_scanning() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let args = test_args();
        let mut metrics = test_metrics();
        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir(&sub_dir)?;
        create_test_file(
            temp_dir.path(),
            "main.rs",
            "fn main() {\n// Comment\nprintln!(\"Hello\");\n}\n",
        )?;
        create_test_file(
            &sub_dir,
            "lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n/* Block comment */\na + b\n}\n",
        )?;
        create_test_file(temp_dir.path(), "readme.md", "# Test Project")?;
        let mut error_count = 0;
        let mut entries_count = 0usize;
        let stats = scan_directory(
            temp_dir.path(),
            &args,
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        let root_canon = fs::canonicalize(temp_dir.path())?;
        let main_stats = stats
            .get(&root_canon)
            .or_else(|| stats.get(temp_dir.path()))
            .unwrap();
        let main_rust_stats = main_stats.language_stats.get("Rust").unwrap();
        assert_eq!(main_rust_stats.0, 1);
        assert_eq!(main_rust_stats.1.code_lines, 3);
        assert_eq!(main_rust_stats.1.comment_lines, 1);
        let sub_canon = fs::canonicalize(&sub_dir)?;
        let sub_stats = stats
            .get(&sub_canon)
            .or_else(|| stats.get(&sub_dir))
            .unwrap();
        let sub_rust_stats = sub_stats.language_stats.get("Rust").unwrap();
        assert_eq!(sub_rust_stats.0, 1);
        assert_eq!(sub_rust_stats.1.code_lines, 3);
        assert_eq!(sub_rust_stats.1.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_scan_directory_respects_ignore_list() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let target_dir = root.join("target");
        fs::create_dir(&target_dir)?;
        create_test_file(&target_dir, "skip.rs", "fn skipped() {}\n")?;
        create_test_file(root, "main.rs", "fn main() {}\n")?;

        let mut args = test_args();
        args.ignore = vec!["target".to_string()];

        let mut metrics = test_metrics();

        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        assert_eq!(error_count, 0);

        let target_canon = fs::canonicalize(&target_dir)?;
        assert!(
            !stats.contains_key(&target_canon),
            "ignored directory should not appear in stats"
        );

        let root_canon = fs::canonicalize(root)?;
        let root_stats = stats
            .get(&root_canon)
            .expect("root stats should exist after scanning");
        let rust_entry = root_stats
            .language_stats
            .get("Rust")
            .expect("Rust stats should be present");
        assert_eq!(rust_entry.0, 1);
        assert_eq!(rust_entry.1.code_lines, 1);
        Ok(())
    }

    #[test]
    fn test_scan_directory_missing_path_records_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let missing = temp_dir.path().join("does_not_exist");
        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;

        let stats = scan_directory(
            &missing,
            &args,
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        assert!(stats.is_empty());
        assert_eq!(
            error_count, 1,
            "missing path should increment error counter"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_warns_on_max_depth() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let level1 = root.join("level1");
        let level2 = level1.join("level2");
        fs::create_dir(&level1)?;
        fs::create_dir(&level2)?;
        create_test_file(root, "root_file.rs", "fn root_file() {}\n")?;
        create_test_file(&level1, "child.rs", "fn child() {}\n")?;
        create_test_file(&level2, "nested.rs", "fn nested() {}\n")?;

        let mut args = test_args();
        args.max_depth = 0;

        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        let root_key = fs::canonicalize(root)?;
        let level1_key = fs::canonicalize(&level1)?;
        assert!(
            stats.contains_key(&root_key),
            "root stats should still exist"
        );
        assert!(
            !stats.contains_key(&level1_key),
            "children beyond max_depth should be skipped"
        );
        assert_eq!(
            error_count, 1,
            "exceeding max_depth should log a warning/error"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_auto_ignores_special_dirs() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let git_dir = root.join(".git");
        let node_modules = root.join("node_modules");
        fs::create_dir(&git_dir)?;
        fs::create_dir(&node_modules)?;
        create_test_file(root, "main.rs", "fn main() {}\n")?;
        create_test_file(&git_dir, "ignored.rs", "fn ignored() {}\n")?;
        create_test_file(&node_modules, "ignored.js", "console.log('ignored');\n")?;

        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        let root_key = fs::canonicalize(root)?;
        assert!(
            stats.contains_key(&root_key),
            "root stats should exist when scanning root"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&git_dir)?),
            ".git directory should be auto-ignored"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&node_modules)?),
            "node_modules directory should be auto-ignored"
        );
        assert_eq!(error_count, 0);
        Ok(())
    }

    #[test]
    fn test_rust_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "test.rs", "fn main() {\n// Line comment\n/* Block comment */\n/// Doc comment\n//! Module comment\nprintln!(\"Hello\");\n}\n")?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("test.rs").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_rust_block_comment_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "trail.rs",
            "fn main() {\nlet value = 1; /* comment */ println!(\"{}\", value);\n}\n",
        )?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("trail.rs").as_path())?;
        assert_eq!(stats.code_lines, 4);
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_rust_block_comment_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mix.rs",
            "fn noisy() {\nlet value = 1; /* block */ // trailing comment\n}\n",
        )?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("mix.rs").as_path())?;
        assert_eq!(stats.code_lines, 3, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_rust_multiline_block_close_followed_by_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "multi.rs",
            "fn tricky() {\n/* start\nstill comment */ // trailing\nlet x = 1;\n}\n",
        )?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("multi.rs").as_path())?;
        assert!(stats.code_lines >= 3, "stats: {:?}", stats);
        assert!(stats.comment_lines >= 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_rust_attribute_and_multiline_block_resume() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "attr.rs",
            r#"#[cfg(test)]
fn decorated() {
    let value = /* start block
    still comment
*/ 1; // trailing inline
    let inline = 2; /* inline block */ println!("{}", inline);
}
"#,
        )?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("attr.rs").as_path())?;
        assert_eq!(stats.code_lines, 7, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 4, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "test.py",
            "def main():\n# Line comment\n'''Block\ncomment'''\nprint('Hello')\n\n",
        )?;
        let (stats, _total_lines) = count_python_lines(temp_dir.path().join("test.py").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 3);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_python_triple_double_quote() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "test_ddq.py",
            "def main():\n\"\"\"Block\ncomment\"\"\"\nprint('Hello')\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("test_ddq.py").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_triple_quote_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_doc.py",
            "def inline():\n\"\"\"doc\"\"\" print('after') # trailing\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("inline_doc.py").as_path())?;
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_triple_quote_same_line_only_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_comment.py",
            "def note():\n\"\"\"doc\"\"\" # trailing comment\npass\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("inline_comment.py").as_path())?;
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_triple_quotes_and_continuation() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.py",
            "def doc():\n\"\"\"Doc\"\"\" # inline\nvalue = \"hello\" \\\n# comment on continuation\n'''Inline''' print('done')\n",
        )?;
        let (stats, _total_lines) = count_python_lines(temp_dir.path().join("mixed.py").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_python_triple_quote_after_continuation() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "continuation.py",
            "def tricky():\nvalue = \"line\" \\\n\"\"\"not doc\"\"\"\nprint('done')\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("continuation.py").as_path())?;
        assert!(
            stats.comment_lines == 0,
            "continuation should prevent docstring counting as comment: {:?}",
            stats
        );
        assert!(stats.code_lines >= 3, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_powershell_nested_block_transitions() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "complex.ps1",
            "<# start\nstill comment #> Write-Host 'post'\nWrite-Host 'mid' <# open #> more <# again\nmulti #> done\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("complex.ps1").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 3);
        Ok(())
    }

    #[test]
    fn test_powershell_mixed_block_and_line_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.ps1",
            "Write-Host 'start'\n<# header #> Write-Host 'after'\nWrite-Host 'open' <# comment\nstill comment\n#> Write-Host 'tail' # annotate\nWrite-Host 'line mix' # trailing <# unreachable #>\nWrite-Host 'closing' <# comment #> # trailing\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("mixed.ps1").as_path())?;
        assert_eq!(stats.code_lines, 6, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 8, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_powershell_line_comment_before_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "order.ps1",
            "Write-Host 'alpha' # inline comment <# block #> Write-Host 'beta'\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("order.ps1").as_path())?;
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_inline_comment_after_docstring() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc.py",
            "\"\"\"heading\"\"\" # title\nprint('body')  # trailing\n",
        )?;
        let (stats, _total_lines) = count_python_lines(temp_dir.path().join("doc.py").as_path())?;
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_javascript_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "test.js", "function main() {\n// Line comment\n/* Block comment */\n/* Multi-line\ncomment */\n<!-- JSX comment -->\nconsole.log('Hello');\n}\n")?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("test.js").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 5);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_javascript_jsx_comment_transition() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "jsx.js",
            "const markup = '<div>';\n<!-- jsx\ncomment --> <span>done</span>\nlet value = 1; /* block */ console.log(value);\n/* open\ncomment */\nconsole.log('after');\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("jsx.js").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 4);
        Ok(())
    }

    #[test]
    fn test_javascript_block_comment_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mix.js",
            "const a = 1; /* inline */ const b = 2;\n/* multi\ncomment */ const c = 3;\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("mix.js").as_path())?;
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_javascript_block_close_followed_by_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "close_line.js",
            "function demo() {\n  const value = 1; /* block */ // trailing\n  return value;\n}\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("close_line.js").as_path())?;
        assert!(stats.code_lines >= 4, "stats: {:?}", stats);
        assert!(stats.comment_lines <= 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_javascript_jsx_and_block_single_line_variants() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "jsx_mix.js",
            "const view = () => {\n    return <div />;\n};\n<!-- jsx start\nstill comment --> const resumed = true;\n<!-- inline --> const inline = true;\n/* block start\nstill block */ const next = 1;\n/* inline block */ const tail = 2;\nconst trailing = 3; // inline comment\n// header\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("jsx_mix.js").as_path())?;
        assert_eq!(stats.code_lines, 8, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 7, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_perl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "test.pl", "#!/usr/bin/perl\n# Line comment\n=pod\nDocumentation block\n=cut\nprint \"Hello\";\n\n")?;
        let (stats, _total_lines) = count_perl_lines(temp_dir.path().join("test.pl").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 4);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_ruby_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "test.rb", "#!/usr/bin/env ruby\n# This is a comment\nputs 'Hello, world!'\n=begin\nThis is a block comment\n=end\nputs 'Goodbye'\n")?;
        let (stats, _total_lines) = count_ruby_lines(temp_dir.path().join("test.rb").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        Ok(())
    }

    #[test]
    fn test_shell_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "test.sh",
            "#!/bin/bash\n# This is a comment\necho \"Hello, world!\"\n",
        )?;
        let (stats, _total_lines) = count_shell_lines(temp_dir.path().join("test.sh").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_pascal_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "test.pas", "program Test;\n// This is a line comment\n{ This is a block comment }\nwriteln('Hello, world!');\n(* Another block comment\nspanning multiple lines *)\nwriteln('Goodbye');\n")?;
        let (stats, _total_lines) = count_pascal_lines(temp_dir.path().join("test.pas").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        Ok(())
    }

    #[test]
    fn test_pascal_mixed_comment_styles_single_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.pas",
            "{ block } writeln('a');\n(* another *) writeln('b'); // trailing\n",
        )?;
        let (stats, _total_lines) =
            count_pascal_lines(temp_dir.path().join("mixed.pas").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_pascal_nested_block_comment_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "nested.pas",
            "{ comment } writeln('done');\n(* block *) writeln('after');\n",
        )?;
        let (stats, _total_lines) =
            count_pascal_lines(temp_dir.path().join("nested.pas").as_path())?;
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_pascal_nested_block_exit_counts() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blocks.pas",
            "program Blocks;\n{ outer\n{ inner }\nstill } writeln('after brace');\n(* level\n(* inner *)\n*) writeln('after paren');\n(* open only\nstill comment\n*) // trailing comment\nwriteln('done');\nend.\n",
        )?;
        let (stats, _total_lines) =
            count_pascal_lines(temp_dir.path().join("blocks.pas").as_path())?;
        assert_eq!(stats.code_lines, 5, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 9, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    // --- New Tests ---

    #[test]
    fn test_case_insensitive_extension() {
        // Test that uppercase or mixed-case extensions are correctly recognized.
        assert_eq!(get_language_from_extension("TEST.RS"), Some("Rust"));
        assert_eq!(
            get_language_from_extension("example.Js"),
            Some("JavaScript")
        );
        assert_eq!(get_language_from_extension("module.Py"), Some("Python"));
        assert_eq!(get_language_from_extension("FOO.TS"), Some("TypeScript"));
    }

    #[test]
    fn test_get_language_from_extension_multipart_and_unknown() {
        assert_eq!(
            get_language_from_extension("component.d.ts"),
            Some("TypeScript")
        );
        assert_eq!(get_language_from_extension("layout.view.jsx"), Some("JSX"));
        assert_eq!(get_language_from_extension("CONFIG.CFG"), Some("INI"));
        assert_eq!(get_language_from_extension("archive.tar.gz"), None);
    }

    #[test]
    fn test_dotfile_language_detection() {
        assert_eq!(get_language_from_extension(".bashrc"), Some("Shell"));
        assert_eq!(get_language_from_extension(".zprofile"), Some("Shell"));
        assert_eq!(
            get_language_from_extension("Dockerfile.prod"),
            Some("Dockerfile")
        );
        assert_eq!(get_language_from_extension("CMakeLists.txt"), Some("CMake"));
    }

    #[test]
    fn test_args_parsing_flags() {
        let args = Args::parse_from([
            "mdkloc",
            "--non-recursive",
            "--ignore",
            "target",
            "--filespec",
            "*.rs",
            "--max-entries",
            "42",
            "--max-depth",
            "3",
            "--verbose",
            ".",
        ]);
        assert!(args.non_recursive);
        assert!(args.verbose);
        assert_eq!(args.ignore, vec!["target".to_string()]);
        assert_eq!(args.filespec.as_deref(), Some("*.rs"));
        assert_eq!(args.max_entries, 42);
        assert_eq!(args.max_depth, 3);
    }

    #[test]
    fn test_invalid_utf8_handling() -> io::Result<()> {
        // Create a file with invalid UTF-8 bytes.
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("invalid.txt");
        // Write valid UTF-8 text, then an invalid byte (0xFF), then more valid text.
        fs::write(&file_path, b"hello\n\xFFworld\n")?;
        // read_file_lines_lossy should not error and should replace the invalid byte.
        let lines: Vec<String> =
            read_file_lines_lossy(&file_path)?.collect::<Result<Vec<_>, io::Error>>()?;
        // Expect two lines: "hello" and "�world"
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello");
        // The invalid byte is replaced with the Unicode replacement character.
        assert!(lines[1].contains("�world"));
        Ok(())
    }

    #[test]
    fn test_generic_line_counting() -> io::Result<()> {
        // Create a file with an unknown extension containing blank and code lines.
        let temp_dir = TempDir::new()?;
        // Mix of code lines and blank lines
        let content = "first line\n\nsecond line\n   \nthird line\n";
        create_test_file(temp_dir.path(), "file.xyz", content)?;

        let (stats, _total_lines) =
            count_generic_lines(temp_dir.path().join("file.xyz").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.blank_lines, 2);
        // Generic counting does not track comment lines
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_truncate_start() {
        // When the string is short, it remains unchanged.
        assert_eq!(truncate_start("short", DIR_WIDTH), "short");
        // When too long, it should be truncated from the start.
        let long_str = "winmerge-master\\Externals\\boost\\boost\\config\\compiler";
        let truncated = truncate_start(long_str, DIR_WIDTH);
        assert_eq!(truncated.chars().count(), DIR_WIDTH);
        assert!(truncated.starts_with("..."));
        // The truncated version should contain the important ending portion.
        let expected_ending: String = long_str
            .chars()
            .rev()
            .take(DIR_WIDTH - 3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert!(truncated.ends_with(&expected_ending));
    }

    #[test]
    fn test_yaml_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "test.yaml",
            "# comment\nkey: value\n\nlist:\n  - item # inline text after value (treated as code)\n",
        )?;
        let (stats, _total_lines) = count_yaml_lines(temp_dir.path().join("test.yaml").as_path())?;
        assert_eq!(stats.code_lines, 3); // key, list:, item
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_toml_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Cargo.toml",
            "# comment\n[package]\nname = 'demo'\n\n[dependencies]\n",
        )?;
        let (stats, _total_lines) = count_toml_lines(temp_dir.path().join("Cargo.toml").as_path())?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_json_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "data.json",
            "{\n  \"k\": 1,\n  \"arr\": [1,2]\n}\n\n",
        )?;
        let (stats, _total_lines) = count_json_lines(temp_dir.path().join("data.json").as_path())?;
        assert_eq!(stats.code_lines, 4);
        assert_eq!(stats.comment_lines, 0);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_xml_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "data.xml",
            "<root>\n<!-- c1 -->\n<!--\n block\n-->\n<child/>\n</root>\n",
        )?;
        let (stats, _total_lines) =
            count_xml_like_lines(temp_dir.path().join("data.xml").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_html_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "index.html",
            "<html>\n<body>\n<!-- banner -->\n<div>hi</div>\n<!--\n multi\n-->\n</body>\n</html>\n",
        )?;
        let (stats, _total_lines) =
            count_xml_like_lines(temp_dir.path().join("index.html").as_path())?;
        assert!(stats.code_lines >= 5); // <html>, <body>, <div>, </body>, </html>
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_makefile_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Makefile",
            "# comment\n\nall:\n\t@echo hello # inline\n",
        )?;
        let (stats, _total_lines) =
            count_makefile_lines(temp_dir.path().join("Makefile").as_path())?;
        assert_eq!(stats.code_lines, 2); // all:, recipe line
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_makefile_extension_mapping() {
        assert_eq!(get_language_from_extension("rules.mk"), Some("Makefile"));
        assert_eq!(get_language_from_extension("GNUmakefile"), Some("Makefile"));
    }

    #[test]
    fn test_dockerfile_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Dockerfile",
            "# comment\nFROM alpine\nRUN echo hi\n",
        )?;
        let (stats, _total_lines) =
            count_dockerfile_lines(temp_dir.path().join("Dockerfile").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_ini_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "config.ini",
            "; top comment\n# another\n[core]\nname = demo\n\n",
        )?;
        let (stats, _total_lines) = count_ini_lines(temp_dir.path().join("config.ini").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_ini_mixed_comment_styles() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "settings.ini",
            "name=value\n; comment\nvalue = other # trailing\n\n",
        )?;
        let (stats, _total_lines) =
            count_ini_lines(temp_dir.path().join("settings.ini").as_path())?;
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "main.tf",
            "# comment\n// also comment\nresource \"x\" \"y\" {\n  a = 1 /* inline */\n}\n/*\nblock\n*/\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(temp_dir.path().join("main.tf").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 4);
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline.tf",
            "resource \"x\" \"y\" { /* block */ name = \"demo\" }\nvalue = 1 /* comment */\n/* open\n comment */ value = 2\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(temp_dir.path().join("inline.tf").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 3);
        Ok(())
    }

    #[test]
    fn test_hcl_block_close_followed_by_hash_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "trailing.tf",
            "resource \"x\" \"y\" {\n  value = 1 /* block */ # trailing\n}\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(temp_dir.path().join("trailing.tf").as_path())?;
        assert!(stats.code_lines >= 2, "stats: {:?}", stats);
        assert!(stats.comment_lines >= 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_hash_comment_precedes_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "hash_first.tf",
            "resource \"x\" \"y\" {\n  value = 1 # hash before block /* still comment */\n  another = 2\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("hash_first.tf").as_path())?;
        assert!(stats.code_lines >= 3, "stats: {:?}", stats);
        assert!(stats.comment_lines >= 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_cmake_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "CMakeLists.txt",
            "# top\ncmake_minimum_required(VERSION 3.25)\nproject(demo)\n# end\n",
        )?;
        let (stats, _total_lines) =
            count_cmake_lines(temp_dir.path().join("CMakeLists.txt").as_path())?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_powershell_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "script.ps1",
            "# line\nWrite-Host 'hi'\n<# block\ncomment #> Write-Host 'after'\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("script.ps1").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_powershell_block_comment_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.ps1",
            "Write-Host 1 <# inline #> # trailing\n<# block\ncontinues\n#>\nWrite-Host 2\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("mixed.ps1").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_batch_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "run.bat",
            "REM header\n:: also comment\n@echo on\nset X=1\n",
        )?;
        let (stats, _total_lines) = count_batch_lines(temp_dir.path().join("run.bat").as_path())?;
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 2);
        Ok(())
    }

    #[test]
    fn test_tcl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "prog.tcl",
            "#! /usr/bin/env tclsh\n# comment\nputs \"hello\"\n",
        )?;
        let (stats, _total_lines) = count_tcl_lines(temp_dir.path().join("prog.tcl").as_path())?;
        assert_eq!(stats.code_lines, 2); // shebang + puts
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_rst_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc.rst",
            "Title\n=====\n\n.. comment\n\nParagraph text.\n",
        )?;
        let (stats, _total_lines) = count_rst_lines(temp_dir.path().join("doc.rst").as_path())?;
        assert_eq!(stats.blank_lines, 2);
        assert_eq!(stats.comment_lines, 0);
        assert_eq!(stats.code_lines, 4);
        Ok(())
    }

    #[test]
    fn test_velocity_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template.vm",
            "## line comment\nHello #* block *# World\n#* multi\nline *#\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template.vm").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_mustache_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view.mustache",
            "{{! top }}\nHello {{name}}\n{{! multi\n line }}\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("view.mustache").as_path())?;
        assert!(stats.code_lines >= 1);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_proto_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "msg.proto",
            "// comment\n/* block */\nsyntax = \"proto3\";\n",
        )?;
        let (stats, _total_lines) =
            count_c_style_lines(temp_dir.path().join("msg.proto").as_path())?;
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_cstyle_inline_block_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "x.c",
            "int a; /* comment */ int b;\n/* start\n */ int c;\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(temp_dir.path().join("x.c").as_path())?;
        // Expect 3 code lines: "int a;", "int b;" on first line, and "int c;" on third
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_cstyle_multiple_pairs_one_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "y.c",
            "int a; /* c1 */ mid /* c2 */ end;\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(temp_dir.path().join("y.c").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_cstyle_mixed_line_and_block_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.c",
            "int a = 0; // comment /* ignored */\nint b = 0; /* block */ // trailing\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(temp_dir.path().join("mixed.c").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_cstyle_block_comment_trailing_code_multi_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block.c",
            "int value = 0; /* start\ncontinues */ value += 1;\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(temp_dir.path().join("block.c").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_cstyle_block_close_followed_by_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "after_close.c",
            "int main() {\n  int value = 0; /* block */ // trailing\n  return value;\n}\n",
        )?;
        let (stats, _total_lines) =
            count_c_style_lines(temp_dir.path().join("after_close.c").as_path())?;
        assert!(stats.code_lines >= 4, "stats: {:?}", stats);
        assert!(stats.comment_lines >= 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_cstyle_block_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "combo.c",
            "int main() {\n/* comment opens\ncontinues */ // trailing\nreturn 0;\n}\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(temp_dir.path().join("combo.c").as_path())?;
        assert!(stats.comment_lines >= 2, "stats: {:?}", stats);
        assert!(stats.code_lines >= 3, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_php_inline_block_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "x.php",
            "<?php\n$y = 1; /* c */ $z = 2;\n?>\n",
        )?;
        let (stats, _total_lines) = count_php_lines(temp_dir.path().join("x.php").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_php_block_comment_followed_by_hash_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "y.php",
            "<?php\n$foo = 1; /* block */ # trailing\n?>\n",
        )?;
        let (stats, _total_lines) = count_php_lines(temp_dir.path().join("y.php").as_path())?;
        assert!(stats.comment_lines >= 1); // block + hash comment
        assert!(stats.code_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_php_block_comment_trailing_code_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline.php",
            "<?php\n$value = 1; /* start\nstill comment */ $value++;\n?>\n",
        )?;
        let (stats, _total_lines) = count_php_lines(temp_dir.path().join("inline.php").as_path())?;
        assert!(stats.code_lines >= 2);
        assert!(stats.comment_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_php_block_and_hash_comment_suppression() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "complex.php",
            "<?php\n$val = 1; /* comment */ $other = 2; # trailing\n/* opening\nstill comment\n*/ # suppressed\necho 'done'; /* inline */ echo 'more';\n$final = true; /* keep */ // rest after comment\n# shell style comment\n?>\n",
        )?;
        let (stats, _total_lines) = count_php_lines(temp_dir.path().join("complex.php").as_path())?;
        assert_eq!(stats.code_lines, 7, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 7, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_svg_xsl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "pic.svg", "<svg><!-- c --><g/></svg>\n")?;
        create_test_file(
            temp_dir.path(),
            "sheet.xsl",
            "<xsl:stylesheet><!-- c --></xsl:stylesheet>\n",
        )?;
        let (svg_stats, _) = count_xml_like_lines(temp_dir.path().join("pic.svg").as_path())?;
        let (xsl_stats, _) = count_xml_like_lines(temp_dir.path().join("sheet.xsl").as_path())?;
        assert!(svg_stats.code_lines >= 1 && svg_stats.comment_lines >= 1);
        assert!(xsl_stats.code_lines >= 1 && xsl_stats.comment_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_xml_multiple_pairs_one_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "z.xml", "<a><!--c1--><b/><!--c2--></a>\n")?;
        let (stats, _total) = count_xml_like_lines(temp_dir.path().join("z.xml").as_path())?;
        assert!(stats.code_lines >= 1);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_max_depth_children_not_grandchildren() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let child = root.join("child");
        let grand = child.join("grand");
        fs::create_dir(&child)?;
        fs::create_dir(&grand)?;
        create_test_file(root, "a.rs", "fn main(){}\n")?;
        create_test_file(&child, "b.rs", "fn main(){}\n")?;
        create_test_file(&grand, "c.rs", "fn main(){}\n")?;

        let args = Args {
            max_depth: 1,
            ..test_args()
        };
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        // Count Rust files aggregated across all dirs in stats
        let mut rust_files = 0u64;
        for dir in stats.values() {
            if let Some((n, _)) = dir.language_stats.get("Rust") {
                rust_files += *n;
            }
        }
        assert_eq!(rust_files, 2); // root and child only
        assert!(
            error_count >= 1,
            "expected depth limit to increment error count, got {error_count}"
        );
        Ok(())
    }

    #[test]
    fn test_filespec_filters_rs_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "a.rs", "fn main(){}\n")?;
        create_test_file(root, "b.py", "print('x')\n")?;
        let args = Args {
            filespec: Some("*.rs".to_string()),
            ..test_args()
        };
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        // Assert only Rust present
        for dir in stats.values() {
            for (lang, (n, _)) in &dir.language_stats {
                assert_eq!(lang.as_str(), "Rust");
                assert_eq!(*n, 1);
            }
        }
        Ok(())
    }

    #[test]
    fn test_filespec_matches_nested_relative_path() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let nested = root.join("src").join("utils");
        fs::create_dir_all(&nested)?;
        let file_path = nested.join("lib.rs");
        create_test_file(&nested, "lib.rs", "pub fn helper() {}\n")?;

        let include = Pattern::new("src/**/*.rs").expect("glob compiles");
        assert!(
            filespec_matches(&include, root, &file_path),
            "src/**/*.rs should match nested file path"
        );

        let exclude = Pattern::new("tests/**/*.rs").expect("glob compiles");
        assert!(
            !filespec_matches(&exclude, root, &file_path),
            "tests/**/*.rs should not match source file"
        );
        Ok(())
    }

    #[test]
    fn test_should_process_file_respects_filespec() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir)?;
        create_test_file(&src_dir, "main.rs", "fn main() {}\n")?;
        let file_path = src_dir.join("main.rs");

        let include = Pattern::new("src/*.rs").expect("glob compiles");
        assert!(
            should_process_file(Some(&include), root, &file_path),
            "matching filespec should allow processing"
        );

        let exclude = Pattern::new("tests/*.rs").expect("glob compiles");
        assert!(
            !should_process_file(Some(&exclude), root, &file_path),
            "non-matching filespec should deny processing"
        );

        assert!(
            should_process_file(None, root, &file_path),
            "missing filespec should allow processing by default"
        );
        Ok(())
    }

    #[test]
    fn test_filespec_recurses_into_nested_dirs() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let nested = root.join("nested").join("deep");
        fs::create_dir_all(&nested)?;
        create_test_file(root, "skip.py", "print('skip')\n")?;
        create_test_file(&nested, "find.rs", "fn nested() {}\n")?;
        create_test_file(&nested, "ignore.py", "print('ignore')\n")?;

        let args = Args {
            filespec: Some("*.rs".to_string()),
            ..test_args()
        };
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        let nested_canon = fs::canonicalize(&nested)?;
        let has_nested_stats = stats.contains_key(&nested_canon) || stats.contains_key(&nested);
        assert!(has_nested_stats, "expected nested directory stats");

        let rust_files: u64 = stats
            .values()
            .flat_map(|dir| dir.language_stats.get("Rust").map(|(n, _)| *n))
            .sum();
        assert_eq!(rust_files, 1);
        Ok(())
    }

    #[test]
    fn test_invalid_filespec_returns_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "a.rs", "fn main(){}\n")?;
        let args = Args {
            filespec: Some("[".to_string()),
            ..test_args()
        };
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let err = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )
        .expect_err("expected invalid filespec to return an error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[test]
    fn test_skip_zero_stat_dcl_in_aggregation() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "not_dcl.com", "echo hi\n")?;
        create_test_file(root, "a.rs", "fn main(){}\n")?;
        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        let mut has_dcl = false;
        for dir in stats.values() {
            if dir.language_stats.contains_key("DCL") {
                has_dcl = true;
                break;
            }
        }
        assert!(!has_dcl);
        Ok(())
    }

    #[test]
    fn test_empty_file_counts_towards_totals() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "empty.rs", "")?;
        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        let root_canon = fs::canonicalize(root)?;
        let dir_stats = stats
            .get(&root_canon)
            .or_else(|| stats.get(root))
            .expect("expected root directory stats for empty file");
        let (file_count, lang_stats) = dir_stats
            .language_stats
            .get("Rust")
            .expect("expected Rust entry for empty file");
        assert_eq!(*file_count, 1);
        assert_eq!(lang_stats.code_lines, 0);
        assert_eq!(lang_stats.comment_lines, 0);
        assert_eq!(lang_stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_mixed_code_and_comment_counts_once() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.rs",
            "fn main() { println!(\"hi\"); } // greet\n/* block */\n",
        )?;
        let (raw_stats, total_lines) =
            count_lines_with_stats(temp_dir.path().join("mixed.rs").as_path())?;
        let stats = normalize_stats(raw_stats, total_lines);
        assert_eq!(total_lines, 2);
        assert_eq!(
            stats.code_lines + stats.comment_lines + stats.blank_lines,
            total_lines
        );
        assert!(stats.code_lines >= 1);
        Ok(())
    }

    #[test]
    fn test_scan_directory_sums_match_metrics() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(
            root,
            "mixed.rs",
            "fn main() { println!(\"hi\"); } // greet\n/* block */\n",
        )?;
        create_test_file(root, "script.py", "print('hi')  # greet\n\n")?;
        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        assert_eq!(error_count, 0);
        let mut aggregated = LanguageStats::default();
        for dir_stats in stats.values() {
            for (_, lang_stats) in dir_stats.language_stats.values() {
                aggregated.code_lines += lang_stats.code_lines;
                aggregated.comment_lines += lang_stats.comment_lines;
                aggregated.blank_lines += lang_stats.blank_lines;
                aggregated.overlap_lines += lang_stats.overlap_lines;
            }
        }
        let sum = aggregated.code_lines + aggregated.comment_lines + aggregated.blank_lines
            - aggregated.overlap_lines;
        let lines_processed = metrics.lines_processed.load(Ordering::Relaxed);
        assert_eq!(sum, lines_processed);
        Ok(())
    }

    #[test]
    fn test_algol_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "demo.alg",
            "begin\nCOMMENT this is a comment;\nend\n",
        )?;
        let (stats, _total) = count_algol_lines(temp_dir.path().join("demo.alg").as_path())?;
        assert_eq!(stats.code_lines, 2); // begin/end
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_algol_comment_variants() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "variants.alg",
            "COMMENT block without semicolon\nstill comment;\nco inline co\n# hash comment\nbegin\nend\n",
        )?;
        let (stats, _total) = count_algol_lines(temp_dir.path().join("variants.alg").as_path())?;
        assert!(stats.comment_lines >= 2);
        assert!(stats.code_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_algol_comment_with_semicolon_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline.alg",
            "COMMENT single line;\nbegin\n  real x;\nend\n",
        )?;
        let (stats, _total) = count_algol_lines(temp_dir.path().join("inline.alg").as_path())?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert!(stats.code_lines >= 3, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_cobol_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "prog.cob",
            "       IDENTIFICATION DIVISION.\n      * comment in col 7\n       PROGRAM-ID. DEMO.\n       *> free comment\n",
        )?;
        let (stats, _total) = count_cobol_lines(temp_dir.path().join("prog.cob").as_path())?;
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert!(stats.code_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_fortran_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "m.f90",
            "! comment\nprogram x\nprint *, 'hi'\nend\n",
        )?;
        let (stats, _total) = count_fortran_lines(temp_dir.path().join("m.f90").as_path())?;
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.code_lines, 3);
        Ok(())
    }

    #[test]
    fn test_asm_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "x.asm", "; c\n# also c\nmov eax, eax\n")?;
        let (stats, _total) = count_asm_lines(temp_dir.path().join("x.asm").as_path())?;
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_dcl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "proc.com",
            "$! comment\n$ write sys$output \"hi\"\n",
        )?;
        let (stats, _total) = count_dcl_lines(temp_dir.path().join("proc.com").as_path())?;
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_dcl_non_dcl_com_file_sniff() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "not_dcl.com", "echo hi\n")?;
        let (stats, _total) = count_dcl_lines(temp_dir.path().join("not_dcl.com").as_path())?;
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_dotfile_shell_detection() {
        assert_eq!(get_language_from_extension(".bashrc"), Some("Shell"));
        assert_eq!(get_language_from_extension(".zshrc"), Some("Shell"));
    }

    #[test]
    fn test_row_formatting_is_ansi_safe() {
        let line = format_language_stats_line(
            "./dir",
            "Rust",
            12,
            &LanguageStats {
                code_lines: 34,
                comment_lines: 5,
                blank_lines: 6,
                overlap_lines: 2,
            },
        );
        // No ANSI escape
        assert!(!line.contains('\u{1b}'));
        // Check widths (basic sanity)
        // prefix (<=40 left), space, lang (<=16), space, 8, space, 10, space, 10, space, 10, space, 10
        // Total minimum length should be >= 40+1+16+1+8+1+10+1+10+1+10+1+10 = 110
        assert!(line.len() >= 110);
    }

    #[test]
    fn test_max_entries_enforced() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let args = Args {
            max_entries: 1,
            ..test_args()
        };
        let mut metrics = test_metrics();
        // Create two files
        create_test_file(temp_dir.path(), "a.rs", "fn main(){}\n")?;
        create_test_file(temp_dir.path(), "b.rs", "fn main(){}\n")?;
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let res = scan_directory(
            temp_dir.path(),
            &args,
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        );
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn test_iplan_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "calc.ipl", "/* c */\n! c\nSET X = 1\n")?;
        let (stats, _total) = count_iplan_lines(temp_dir.path().join("calc.ipl").as_path())?;
        assert!(stats.comment_lines >= 2);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_iplan_block_followed_by_bang_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mix.ipl",
            "SET X = 1 /* inline */ ! trailing\n/* block\ncontinues */ ! next\nVALUE\n",
        )?;
        let (stats, _total) = count_iplan_lines(temp_dir.path().join("mix.ipl").as_path())?;
        assert!(stats.code_lines >= 1);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_iplan_block_close_skips_bang_followup() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "comment.ipl",
            "SET J = 1\n/* start\n! nested comment\n*/ ! still comment\nVALUE /* inline */ ! comment\nVALUE ! inline comment\n! trailing only\nVALUE2\n",
        )?;
        let (stats, _total_lines) =
            count_iplan_lines(temp_dir.path().join("comment.ipl").as_path())?;
        assert_eq!(stats.code_lines, 4, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 5, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_scala_is_c_style() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Main.scala",
            "object Main {\n// comment\n/* block */\nval x = 1\n}\n",
        )?;
        let (stats, _total_lines) = count_c_style_lines(&temp_dir.path().join("Main.scala"))?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        Ok(())
    }

    // Additional hardening tests

    #[test]
    fn test_cobol_short_line_and_leading_spaces() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        // Short line (<7 chars) should not be treated as comment
        create_test_file(temp_dir.path(), "short.cob", "*\n")?;
        let (stats1, _) = count_cobol_lines(temp_dir.path().join("short.cob").as_path())?;
        assert_eq!(stats1.code_lines, 1);
        // Leading spaces then '*' in column 1 is code (not fixed-form comment)
        create_test_file(temp_dir.path(), "lead.cob", "   * TEXT\n")?;
        let (stats2, _) = count_cobol_lines(temp_dir.path().join("lead.cob").as_path())?;
        assert_eq!(stats2.code_lines, 1);
        Ok(())
    }

    #[test]
    fn test_fortran_fixed_vs_free_form() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        // Fixed-form comment indicator in col 1
        create_test_file(temp_dir.path(), "f1.f", "C comment\n")?;
        let (s1, _) = count_fortran_lines(temp_dir.path().join("f1.f").as_path())?;
        assert_eq!(s1.comment_lines, 1);
        // Leading space then 'C' is code (free form)
        create_test_file(temp_dir.path(), "f2.f", " C not comment\n")?;
        let (s2, _) = count_fortran_lines(temp_dir.path().join("f2.f").as_path())?;
        assert_eq!(s2.code_lines, 1);
        // Inline '!' split
        create_test_file(temp_dir.path(), "f3.f90", "print *, 'x' ! trailing\n")?;
        let (s3, _) = count_fortran_lines(temp_dir.path().join("f3.f90").as_path())?;
        assert_eq!(s3.code_lines, 1);
        assert_eq!(s3.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_hcl_multiple_pairs_inline() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "x.tf", "a=1 /*c*/ b=2 /*d*/ c=3\n")?;
        let (stats, _) = count_hcl_lines(temp_dir.path().join("x.tf").as_path())?;
        assert!(stats.code_lines >= 3);
        assert!(stats.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_powershell_inline_and_multiblock() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "ps.ps1",
            "Write-Host 'a' <# c #> 'b' <# d #> 'c'\n",
        )?;
        let (s1, _) = count_powershell_lines(temp_dir.path().join("ps.ps1").as_path())?;
        assert!(s1.code_lines >= 3);
        assert!(s1.comment_lines >= 2);
        create_test_file(
            temp_dir.path(),
            "ps2.ps1",
            "Write-Host 'x'\n<#\nblock\n#> Write-Host 'y'\n",
        )?;
        let (s2, _) = count_powershell_lines(temp_dir.path().join("ps2.ps1").as_path())?;
        assert!(s2.code_lines >= 2);
        assert!(s2.comment_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_pascal_mixed_nested_blocks() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "p.pas",
            "{c1} (*c2*) code\n(* multi\nline *) code2\n",
        )?;
        let (stats, _) = count_pascal_lines(temp_dir.path().join("p.pas").as_path())?;
        assert!(stats.comment_lines >= 2);
        assert!(stats.code_lines >= 2);
        Ok(())
    }

    #[test]
    fn test_perl_pod_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "p.pl",
            "print 'x';\n=pod\nthis is pod\n=cut\nprint 'y';\n",
        )?;
        let (stats, _) = count_perl_lines(temp_dir.path().join("p.pl").as_path())?;
        assert!(stats.comment_lines >= 2);
        assert_eq!(stats.code_lines, 2);
        Ok(())
    }

    #[test]
    fn test_inline_hash_is_code_for_hash_langs() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "a.yaml", "key: 1 # inline\n")?;
        let (yml, _) = count_yaml_lines(temp_dir.path().join("a.yaml").as_path())?;
        assert_eq!(yml.code_lines, 1);
        create_test_file(temp_dir.path(), "a.toml", "name='x' # inline\n")?;
        let (toml, _) = count_toml_lines(temp_dir.path().join("a.toml").as_path())?;
        assert_eq!(toml.code_lines, 1);
        create_test_file(temp_dir.path(), "a.ini", "name=value ; inline\n")?;
        let (ini, _) = count_ini_lines(temp_dir.path().join("a.ini").as_path())?;
        assert_eq!(ini.code_lines, 1);
        create_test_file(temp_dir.path(), "CMakeLists.txt", "set(X 1) # inline\n")?;
        let (cmake, _) = count_cmake_lines(temp_dir.path().join("CMakeLists.txt").as_path())?;
        assert_eq!(cmake.code_lines, 1);
        create_test_file(temp_dir.path(), "Makefile", "VAR=1 # inline\n")?;
        let (mk, _) = count_makefile_lines(temp_dir.path().join("Makefile").as_path())?;
        assert_eq!(mk.code_lines, 1);
        Ok(())
    }

    #[test]
    fn test_hash_comment_mixed_lines() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.hash",
            "# header\nvalue: 1\n\n  # indented\nnext: 2 # trailing\n",
        )?;
        let (stats, total) =
            count_hash_comment_lines(temp_dir.path().join("mixed.hash").as_path())?;
        assert_eq!(total, 5);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hash_comment_trailing_and_blank_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "trailing.yaml",
            "title: demo # inline\n\n# comment only\nvalue: 42\n",
        )?;
        let (stats, total) =
            count_hash_comment_lines(temp_dir.path().join("trailing.yaml").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hash_comment_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "comments.hash", "# comment\n# another\n")?;
        let (stats, total) =
            count_hash_comment_lines(temp_dir.path().join("comments.hash").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_hash_comment_blank_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "blank.hash", "\n\n")?;
        let (stats, total) =
            count_hash_comment_lines(temp_dir.path().join("blank.hash").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 0);
        assert_eq!(stats.blank_lines, 2);
        Ok(())
    }

    #[test]
    fn test_toml_blank_and_comment_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "sample.toml",
            "# header comment\n\nname = \"demo\" # trailing\n",
        )?;
        let (stats, total) =
            count_toml_lines(temp_dir.path().join("sample.toml").as_path())?;
        assert_eq!(total, 3);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_toml_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "comment.toml",
            "# header\n# detail\n",
        )?;
        let (stats, total) =
            count_toml_lines(temp_dir.path().join("comment.toml").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_yaml_blank_and_comment_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "sample.yaml",
            "\n# comment line\nkey: value\n",
        )?;
        let (stats, total) =
            count_yaml_lines(temp_dir.path().join("sample.yaml").as_path())?;
        assert_eq!(total, 3);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_yaml_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "comment.yaml",
            "# only comment\n# another\n",
        )?;
        let (stats, total) =
            count_yaml_lines(temp_dir.path().join("comment.yaml").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_makefile_comment_and_blank_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Makefile",
            "# comment\n\nall:\n\t@echo done\n",
        )?;
        let (stats, total) =
            count_makefile_lines(temp_dir.path().join("Makefile").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_makefile_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "Makefile", "# comment\n# another\n")?;
        let (stats, total) = count_makefile_lines(temp_dir.path().join("Makefile").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_dockerfile_comment_and_blank_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Dockerfile",
            "FROM alpine\n# comment\n\nRUN echo hi\n",
        )?;
        let (stats, total) =
            count_dockerfile_lines(temp_dir.path().join("Dockerfile").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_dockerfile_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Dockerfile",
            "# comment\n# another\n",
        )?;
        let (stats, total) =
            count_dockerfile_lines(temp_dir.path().join("Dockerfile").as_path())?;
        assert_eq!(total, 2);
        assert_eq!(stats.code_lines, 0);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_non_recursive_root_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let child = root.join("child");
        fs::create_dir(&child)?;
        create_test_file(root, "a.rs", "fn main(){}\n")?;
        create_test_file(&child, "b.rs", "fn main(){}\n")?;
        let args = Args {
            non_recursive: true,
            ..test_args()
        };
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;
        // Ensure only one Rust file counted
        let mut rust_files = 0u64;
        for dir in stats.values() {
            if let Some((n, _)) = dir.language_stats.get("Rust") {
                rust_files += *n;
            }
        }
        assert_eq!(rust_files, 1);
        Ok(())
    }

    #[test]
    fn test_scan_directory_missing_root_metadata_increments_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let missing = temp_dir.path().join("does_not_exist");
        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;

        let stats = scan_directory(
            &missing,
            &args,
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        assert!(stats.is_empty(), "expected no stats for missing path");
        assert_eq!(
            error_count, 1,
            "missing path should increment error counter, got {error_count}"
        );
        Ok(())
    }
}
