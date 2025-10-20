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
use std::ffi::OsString;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use colored::*;
use glob::Pattern;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::sync::OnceLock;

// Fixed width for the directory column.
const DIR_WIDTH: usize = 40;
const LANG_WIDTH: usize = 16;

const METADATA_FAIL_TAG: &str = "__mdkloc_metadata_fail__";
const READ_DIR_FAIL_TAG: &str = "__mdkloc_read_dir_fail__";
const ENTRY_ITER_FAIL_TAG: &str = "__mdkloc_entry_iter_fail__";
const FILE_TYPE_FAIL_TAG: &str = "__mdkloc_file_type_fail__.rs";
const FAULT_ENV_VAR: &str = "MDKLOC_ENABLE_FAULTS";

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

fn merge_directory_stats(
    target: &mut HashMap<PathBuf, DirectoryStats>,
    dir: PathBuf,
    stat: DirectoryStats,
) {
    if let Some(existing) = target.get_mut(&dir) {
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
        target.insert(dir, stat);
    }
}

fn find_powershell_line_comment(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'#' {
            let is_block_start = idx > 0 && bytes[idx - 1] == b'<';
            let is_block_end = idx + 1 < bytes.len() && bytes[idx + 1] == b'>';
            if !is_block_start && !is_block_end {
                return Some(idx);
            }
        }
    }
    None
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
    reader: BufReader<Box<dyn Read + Send>>,
    buffer: Vec<u8>,
}

impl LossyLineReader {
    fn new(file: fs::File) -> Self {
        Self::from_reader(Box::new(file))
    }

    fn from_reader(reader: Box<dyn Read + Send>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: Vec::with_capacity(8 * 1024),
        }
    }

    #[cfg(test)]
    fn with_reader<R: Read + Send + 'static>(reader: R) -> Self {
        Self::from_reader(Box::new(reader))
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

fn format_directory_display(path: &Path, current_dir: &Path) -> String {
    let raw = match path.strip_prefix(current_dir) {
        Ok(p) if p.as_os_str().is_empty() => ".".to_string(),
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    };
    truncate_start(&raw, DIR_WIDTH)
}

fn failure_injection_enabled() -> bool {
    cfg!(test) || std::env::var_os(FAULT_ENV_VAR).is_some()
}

fn should_simulate_path_failure(path: &Path, needle: &str) -> bool {
    failure_injection_enabled()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == needle)
            .unwrap_or(false)
}

fn should_simulate_entry_failure(entry: &fs::DirEntry, needle: &str) -> bool {
    failure_injection_enabled()
        && entry
            .file_name()
            .to_str()
            .map(|name| name == needle)
            .unwrap_or(false)
}

fn fetch_metadata(path: &Path) -> io::Result<fs::Metadata> {
    if should_simulate_path_failure(path, METADATA_FAIL_TAG) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "simulated metadata read failure",
        ));
    }
    fs::metadata(path)
}

struct ReadDirStream {
    inner: fs::ReadDir,
    #[cfg(test)]
    injected_error: Option<io::Error>,
}

impl ReadDirStream {
    #[cfg(test)]
    fn new(inner: fs::ReadDir, inject_entry_error: bool) -> Self {
        let injected_error = inject_entry_error.then(|| {
            io::Error::new(
                io::ErrorKind::Other,
                "simulated directory entry iteration failure",
            )
        });
        ReadDirStream {
            inner,
            injected_error,
        }
    }

    #[cfg(not(test))]
    fn new(inner: fs::ReadDir, _inject_entry_error: bool) -> Self {
        ReadDirStream { inner }
    }
}

impl Iterator for ReadDirStream {
    type Item = io::Result<fs::DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        #[cfg(test)]
        if let Some(err) = self.injected_error.take() {
            return Some(Err(err));
        }

        self.inner.next()
    }
}

fn read_dir_stream(path: &Path) -> io::Result<ReadDirStream> {
    if should_simulate_path_failure(path, READ_DIR_FAIL_TAG) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "simulated read_dir failure",
        ));
    }
    let iter = fs::read_dir(path)?;
    Ok(ReadDirStream::new(
        iter,
        should_simulate_path_failure(path, ENTRY_ITER_FAIL_TAG),
    ))
}

fn entry_file_type(entry: &fs::DirEntry) -> io::Result<fs::FileType> {
    if should_simulate_entry_failure(entry, FILE_TYPE_FAIL_TAG) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "simulated file_type failure",
        ));
    }
    entry.file_type()
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
        let line_pos = trimmed.find("//");
        let block_pos = trimmed.find("/*");
        let jsx_pos = trimmed.find("<!--");

        if let Some(pl) = line_pos {
            if block_pos.map_or(true, |pb| pl < pb) && jsx_pos.map_or(true, |pj| pl < pj) {
                let before = &trimmed[..pl];
                if !before.trim().is_empty() {
                    stats.code_lines += 1;
                }
                stats.comment_lines += 1;
                continue;
            }
        }

        if let Some(pos) = block_pos {
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            let after = &trimmed[(pos + 2)..];
            if let Some(end) = after.find("*/") {
                let trailing = &after[(end + 2)..];
                if !trailing.trim().is_empty()
                    && !trailing.trim_start().starts_with("//")
                    && !trailing.trim_start().starts_with("<#")
                {
                    stats.code_lines += 1;
                }
            } else {
                in_block_comment = true;
            }
            continue;
        }
        if let Some(pos) = jsx_pos {
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }
            stats.comment_lines += 1;
            let after = &trimmed[(pos + 4)..];
            if let Some(end) = after.find("-->") {
                let trailing = &after[(end + 3)..];
                if !trailing.trim().is_empty() {
                    stats.code_lines += 1;
                }
            } else {
                in_jsx_comment = true;
            }
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
                    let after_trimmed = s.trim_start();
                    if after_trimmed.is_empty() {
                        break;
                    } else if after_trimmed.starts_with("##")
                        || after_trimmed.starts_with("//")
                        || after_trimmed.starts_with('#')
                    {
                        stats.comment_lines += 1;
                        break;
                    } else {
                        s = after_trimmed;
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
                if let Some((token, i)) = next {
                    if token == "//" {
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        break;
                    } else if token == "#" {
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        break;
                    } else {
                        debug_assert_eq!(token, "/*");
                        let before = &s[..i];
                        if !before.trim().is_empty() {
                            stats.code_lines += 1;
                        }
                        stats.comment_lines += 1;
                        s = &s[i + 2..];
                        if let Some(end) = s.find("*/") {
                            s = &s[end + 2..];
                            let after_trimmed = s.trim_start();
                            if after_trimmed.is_empty() {
                                break;
                            } else if after_trimmed.starts_with("##")
                                || after_trimmed.starts_with("//")
                                || after_trimmed.starts_with('#')
                            {
                                stats.comment_lines += 1;
                                break;
                            } else {
                                s = after_trimmed;
                                continue;
                            }
                        } else {
                            in_block = true;
                            break;
                        }
                    }
                } else {
                    if !s.trim().is_empty() {
                        stats.code_lines += 1;
                    }
                    break;
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

fn apply_velocity_tail(fragment: &str, stats: &mut LanguageStats) {
    if fragment.is_empty() {
        return;
    }
    if fragment.starts_with("##") {
        stats.comment_lines += 1;
    } else {
        stats.code_lines += 1;
    }
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
                let after_trimmed = after.trim_start();
                apply_velocity_tail(after_trimmed, &mut stats);
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
                let after_trimmed = after.trim_start();
                apply_velocity_tail(after_trimmed, &mut stats);
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
                let p_line = find_powershell_line_comment(s);
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

    let metadata = match fetch_metadata(path) {
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

    let read_dir = match read_dir_stream(path) {
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
        let file_type = match entry_file_type(&entry) {
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
                        merge_directory_stats(&mut stats, dir, stat);
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

fn build_analysis_report(
    current_dir: &Path,
    stats: &HashMap<PathBuf, DirectoryStats>,
    files_processed: u64,
    lines_processed: u64,
    error_count: usize,
) -> String {
    let mut output = String::new();
    let mut sorted_stats: Vec<_> = stats.iter().collect();
    sorted_stats.sort_by(|(a, _), (b, _)| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let mut total_by_language: HashMap<String, (u64, LanguageStats)> = HashMap::new();

    let _ = writeln!(output, "\n\nDetailed source code analysis:");
    let _ = writeln!(output, "{}", "-".repeat(112));
    let _ = writeln!(
        output,
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
    let _ = writeln!(output, "{}", "-".repeat(112));

    for (path, dir_stats) in sorted_stats {
        let display_path = format_directory_display(path, current_dir);
        let mut languages: Vec<_> = dir_stats.language_stats.iter().collect();
        languages.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (lang, (file_count, lang_stats)) in languages {
            let line = format_language_stats_line(&display_path, lang, *file_count, lang_stats);
            let _ = writeln!(output, "{}", line);
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

    let _ = writeln!(output, "{:-<112}", "");
    let _ = writeln!(output, "Totals by language:");

    let mut sorted_totals: Vec<_> = total_by_language.iter().collect();
    sorted_totals.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (lang, (file_count, stats)) in sorted_totals {
        let line = format_language_stats_line("", lang, *file_count, stats);
        let _ = writeln!(output, "{}", line);
    }

    let mut grand_total = LanguageStats::default();
    for (_, (_files, stats)) in total_by_language.iter() {
        grand_total.code_lines += stats.code_lines;
        grand_total.comment_lines += stats.comment_lines;
        grand_total.blank_lines += stats.blank_lines;
        grand_total.overlap_lines += stats.overlap_lines;
    }

    if files_processed > 0 || lines_processed > 0 {
        let _ = writeln!(output, "\n{}", "Overall Summary:".blue().bold());
        let _ = writeln!(
            output,
            "Total files processed: {}",
            files_processed.to_string().bright_yellow()
        );
        let _ = writeln!(
            output,
            "Total lines processed: {}",
            lines_processed.to_string().bright_yellow()
        );
        let _ = writeln!(
            output,
            "Code lines:     {} ({})",
            grand_total.code_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.code_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Comment lines:  {} ({})",
            grand_total.comment_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.comment_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Mixed lines:    {} ({})",
            grand_total.overlap_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.overlap_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Blank lines:    {} ({})",
            grand_total.blank_lines.to_string().bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.blank_lines, lines_processed)
            )
            .bright_yellow()
        );

        if error_count > 0 {
            let _ = writeln!(
                output,
                "\n{}: {}",
                "Warning".red().bold(),
                error_count.to_string().bright_yellow()
            );
        }
    }

    output
}

fn main() -> io::Result<()> {
    run_with_args(current_args())
}

#[cfg(test)]
fn current_args() -> Vec<OsString> {
    take_override_args().unwrap_or_else(|| env::args_os().collect())
}

#[cfg(not(test))]
fn current_args() -> Vec<OsString> {
    env::args_os().collect()
}

fn run_with_args<I, T>(args: I) -> io::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = Args::parse_from(args);
    let mut metrics = PerformanceMetrics::new();
    run_cli_with_metrics(args, &mut metrics)
}

#[cfg(test)]
static TEST_ARGS_OVERRIDE: OnceLock<std::sync::Mutex<Option<Vec<OsString>>>> = OnceLock::new();

#[cfg(test)]
fn take_override_args() -> Option<Vec<OsString>> {
    TEST_ARGS_OVERRIDE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

#[cfg(test)]
fn set_override_args(args: Vec<OsString>) {
    let mutex = TEST_ARGS_OVERRIDE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = mutex.lock() {
        *guard = Some(args);
    }
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
    let report = build_analysis_report(
        &current_dir,
        &stats,
        files_processed,
        lines_processed,
        error_count,
    );
    print!("{}", report);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use colored::control;
    use std::env;
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
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

    struct CurrentDirGuard {
        original: PathBuf,
    }

    impl CurrentDirGuard {
        fn change_to(path: &Path) -> io::Result<Self> {
            let original = env::current_dir()?;
            env::set_current_dir(path)?;
            Ok(Self { original })
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.original);
        }
    }

    #[test]
    fn test_performance_metrics_new_defaults() {
        let mut metrics = PerformanceMetrics::new();
        assert!(
            metrics.progress_enabled,
            "expected progress enabled by default"
        );
        assert_eq!(
            metrics.files_processed.load(Ordering::Relaxed),
            0,
            "files counter should start at zero"
        );
        assert_eq!(
            metrics.lines_processed.load(Ordering::Relaxed),
            0,
            "lines counter should start at zero"
        );
        metrics.update(7);
        assert_eq!(
            metrics.files_processed.load(Ordering::Relaxed),
            1,
            "file counter should increment after update"
        );
        assert_eq!(
            metrics.lines_processed.load(Ordering::Relaxed),
            7,
            "line counter should accumulate after update"
        );
    }

    #[test]
    fn test_lossy_line_reader_surfaces_errors() {
        struct FailAfterFirstRead {
            state: u8,
        }

        impl Read for FailAfterFirstRead {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.state {
                    0 => {
                        let data = b"ok\n";
                        let len = data.len().min(buf.len());
                        buf[..len].copy_from_slice(&data[..len]);
                        self.state = 1;
                        Ok(len)
                    }
                    1 => {
                        self.state = 2;
                        Err(io::Error::new(io::ErrorKind::Other, "simulated failure"))
                    }
                    _ => Ok(0),
                }
            }
        }

        let mut reader = LossyLineReader::with_reader(FailAfterFirstRead { state: 0 });
        let first_line = reader
            .next()
            .expect("expected first item")
            .expect("first read should succeed");
        assert_eq!(first_line, "ok");
        let second = reader.next().expect("expected error result");
        assert!(
            second.is_err(),
            "lossy reader should surface the simulated failure"
        );
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
    fn test_count_lines_with_stats_hcl_ini_combo() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.tf",
            "resource \"x\" \"y\" {\n  attr = 1 /* block */ attr2 = 2 # trailing hash\n}\n",
        )?;
        create_test_file(
            temp_dir.path(),
            "mixed.ini",
            "# heading\n[core]\nkey=value\nvalue2 = 2 # inline note\n; footer\n",
        )?;

        let (hcl_stats, _total_lines) = count_lines_with_stats(&temp_dir.path().join("mixed.tf"))?;
        assert!(
            hcl_stats.code_lines >= 4,
            "expect code before block, after block, and braces: {hcl_stats:?}"
        );
        assert!(
            hcl_stats.comment_lines >= 2,
            "expect both block and hash comments counted: {hcl_stats:?}"
        );

        let (ini_stats, _total_lines) = count_lines_with_stats(&temp_dir.path().join("mixed.ini"))?;
        assert_eq!(
            ini_stats.comment_lines, 2,
            "expect leading # and trailing ; lines as comments: {ini_stats:?}"
        );
        assert_eq!(
            ini_stats.code_lines, 3,
            "expect [core], key=value, and inline hash line as code: {ini_stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_uppercase_ini() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "CONFIG.INI",
            "# heading\n[Core]\nvalue=1\n",
        )?;
        let (stats, _total_lines) = count_lines_with_stats(&temp_dir.path().join("CONFIG.INI"))?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_uppercase_cfg() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "SETTINGS.CFG", "# heading\noption=value\n")?;
        let (stats, _total_lines) = count_lines_with_stats(&temp_dir.path().join("SETTINGS.CFG"))?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_uppercase_tfvars() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "variables.TFVARS", "# note\nvalue = 1\n")?;
        let (stats, _total_lines) =
            count_lines_with_stats(&temp_dir.path().join("variables.TFVARS"))?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_uppercase_conf() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "SAMPLE.CONF",
            "# heading\n[section]\nvalue=1\n",
        )?;
        let (stats, _total_lines) = count_lines_with_stats(&temp_dir.path().join("SAMPLE.CONF"))?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "terraform.tfvars.json",
            "{\n  \"value\": 1,\n  \"flag\": true\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("terraform.tfvars.json"))?;
        assert_eq!(total_lines, 4);
        assert_eq!(stats.code_lines, 4, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 0, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_upper_tfvars_json_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "variables.TFVARS.JSON",
            "{\n  \"enabled\": true\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("variables.TFVARS.JSON"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(stats.code_lines, 3, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 0, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_case() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "variables.TfVars.json",
            "{\n  \"value\": 1,\n  \"enabled\": true\n}\n",
        )?;
        let (stats, _total_lines) =
            count_lines_with_stats(&temp_dir.path().join("variables.TfVars.json"))?;
        assert_eq!(stats.comment_lines, 0, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 4, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_backup_extension() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "variables.tfvars.json.bak",
            "{\n  \"value\": 1\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("variables.tfvars.json.bak"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(
            stats.code_lines, 3,
            "generic handler should count non-blank lines as code: {stats:?}"
        );
        assert_eq!(stats.comment_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_backup_mixed_case() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "Terraform.TfVars.JSON.BAK",
            "{\n  \"enabled\": true\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("Terraform.TfVars.JSON.BAK"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(
            stats.code_lines, 3,
            "mixed-case backup should still count lines as generic: {stats:?}"
        );
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_extra_suffix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "variables.tfvars.Json.bak.old",
            "{\n  \"value\": 1\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("variables.tfvars.Json.bak.old"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(stats.code_lines, 3, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_backup_suffix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "config.TfVars.JSON.backup",
            "{\n  \"enabled\": false\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("config.TfVars.JSON.backup"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(
            stats.code_lines, 3,
            "backup suffix should fall back to generic counting: {stats:?}"
        );
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_tmp_backup_chain() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "vars.tfvars.json.tmp.backup",
            "{\n  \"value\": 2\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("vars.tfvars.json.tmp.backup"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(
            stats.code_lines, 3,
            "tmp backup chain should count as generic"
        );
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_tfvars_json_tilde_backup() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "vars.tfvars.json~",
            "{\n  \"value\": 3\n}\n",
        )?;
        let (stats, total_lines) =
            count_lines_with_stats(&temp_dir.path().join("vars.tfvars.json~"))?;
        assert_eq!(total_lines, 3);
        assert_eq!(stats.code_lines, 3, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 0);
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
    fn test_count_lines_with_stats_algol_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "sample.alg",
            "begin\nCOMMENT demo;\nco middle co\n# inline\nend\n",
        )?;
        let (stats, total) = count_lines_with_stats(&temp_dir.path().join("sample.alg"))?;
        assert_eq!(total, 5);
        assert_eq!(stats.code_lines, 2, "algol code stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 3, "algol comment stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_cobol_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "example.cob",
            "       IDENTIFICATION DIVISION.\n000000 WORKING-STORAGE SECTION.\n      * COMMENT LINE\n      / ANOTHER COMMENT\nPROCEDURE DIVISION.\n",
        )?;
        let (stats, total) = count_lines_with_stats(&temp_dir.path().join("example.cob"))?;
        assert_eq!(total, 5);
        assert_eq!(stats.code_lines, 3, "cobol code stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 2, "cobol comment stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_fortran_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "program.f90",
            "      PROGRAM HELLO\nC FIXED COMMENT\n      PRINT *, 'HI' ! inline\n      END\n",
        )?;
        let (stats, total) = count_lines_with_stats(&temp_dir.path().join("program.f90"))?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 3, "fortran code stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 2, "fortran comment stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_velocity_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template.vm",
            "#foreach($i in [1])\n#* block comment *#\n## inline note\n$foo\n",
        )?;
        let (stats, total) = count_lines_with_stats(&temp_dir.path().join("template.vm"))?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 2, "velocity code stats: {:?}", stats);
        assert_eq!(
            stats.comment_lines, 2,
            "velocity comment stats: {:?}",
            stats
        );
        Ok(())
    }

    #[test]
    fn test_count_lines_with_stats_mustache_dispatch() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view.mustache",
            "Hello {{! single-line }} World\n{{! multi\nline\n}}\n{{name}}\n",
        )?;
        let (stats, total) = count_lines_with_stats(&temp_dir.path().join("view.mustache"))?;
        assert_eq!(total, 5);
        assert_eq!(stats.code_lines, 3, "mustache code stats: {:?}", stats);
        assert_eq!(
            stats.comment_lines, 4,
            "mustache comment stats: {:?}",
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

    #[test]
    fn test_process_file_verbose_prints_stats() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "verbose.rs",
            "fn main() {}\n// comment line\n",
        )?;

        let mut args = test_args();
        args.verbose = true;
        let mut metrics = test_metrics();
        let mut stats = std::collections::HashMap::new();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;

        process_file(
            &temp_dir.path().join("verbose.rs"),
            &args,
            temp_dir.path(),
            &mut metrics,
            &mut stats,
            &mut entries_count,
            &mut error_count,
            None,
        )?;

        let dir_stats = stats
            .get(temp_dir.path())
            .expect("verbose scan should record directory stats");
        let (file_count, lang_stats) = dir_stats
            .language_stats
            .get("Rust")
            .expect("expected Rust stats for verbose file");
        assert_eq!(*file_count, 1);
        assert!(lang_stats.code_lines >= 1);
        assert_eq!(error_count, 0);
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
    fn test_run_cli_with_metrics_emits_progress_output() -> io::Result<()> {
        control::set_override(false);
        let temp_dir = TempDir::new()?;
        for idx in 0..5 {
            create_test_file(
                temp_dir.path(),
                &format!("file{idx}.rs"),
                "fn main() {}\n// comment\n",
            )?;
        }
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
        let mut metrics = PerformanceMetrics::with_writer(Box::new(writer), true);
        metrics.last_update = metrics.start_time - Duration::from_secs(2);
        run_cli_with_metrics(args, &mut metrics)?;
        let progress_output = CaptureWriter::into_string(buffer);
        assert!(
            progress_output.contains("Processed"),
            "expected progress output, got: {progress_output}"
        );
        assert!(
            progress_output.contains("files/sec"),
            "expected progress rate information, got: {progress_output}"
        );
        Ok(())
    }

    #[test]
    fn test_run_cli_with_metrics_zero_files() -> io::Result<()> {
        control::set_override(false);
        let temp_dir = TempDir::new()?;
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
            output.contains("Performance Summary"),
            "zero-file run should still display performance summary: {output}"
        );
        assert!(
            output.contains("Files processed"),
            "zero-file run should report file count: {output}"
        );
        assert!(
            output.contains("Lines processed"),
            "zero-file run should report line count: {output}"
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

    #[test]
    fn test_run_with_args_executes_cli() -> io::Result<()> {
        control::set_override(false);
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "sample.rs", "fn main() {}\n// comment\n")?;
        let args = vec![
            OsString::from("mdkloc"),
            temp_dir.path().as_os_str().to_os_string(),
            OsString::from("--non-recursive"),
        ];
        run_with_args(args)?;
        Ok(())
    }

    #[test]
    fn test_main_uses_override_args() -> io::Result<()> {
        control::set_override(false);
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "main.rs", "fn main() {}\n// comment\n")?;
        let args = vec![
            OsString::from("mdkloc"),
            temp_dir.path().as_os_str().to_os_string(),
            OsString::from("--non-recursive"),
        ];
        set_override_args(args);
        let result = super::main();
        assert!(
            result.is_ok(),
            "main should run successfully with override args: {result:?}"
        );
        Ok(())
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
    fn test_normalize_stats_reduces_blank_lines_before_overlap() {
        let stats = LanguageStats {
            code_lines: 2,
            comment_lines: 1,
            blank_lines: 3,
            overlap_lines: 0,
        };
        let normalized = normalize_stats(stats, 4);
        assert_eq!(
            normalized.code_lines + normalized.comment_lines + normalized.blank_lines
                - normalized.overlap_lines,
            4
        );
        assert_eq!(
            normalized.blank_lines, 1,
            "expected blank lines to shrink before overlap is recorded"
        );
        assert_eq!(
            normalized.overlap_lines, 0,
            "blank line reduction should consume the overlap delta"
        );
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
    fn test_normalize_stats_backfills_blank_lines_when_underflow() {
        let stats = LanguageStats {
            code_lines: 2,
            comment_lines: 1,
            blank_lines: 0,
            overlap_lines: 0,
        };
        let normalized = normalize_stats(stats, 6);
        assert_eq!(
            normalized.code_lines + normalized.comment_lines + normalized.blank_lines
                - normalized.overlap_lines,
            6
        );
        assert_eq!(
            normalized.blank_lines, 3,
            "expected blank lines to expand to match the total when sum < total_lines"
        );
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
    fn test_scan_directory_metadata_error_increments_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let sentinel = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        assert!(
            stats.is_empty(),
            "metadata failure should skip directory stats entirely"
        );
        assert!(
            error_count >= 1,
            "metadata failure should increment error count, got {error_count}"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_metadata_failure_keeps_sibling() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let good_dir = root.join("good");
        fs::create_dir(&good_dir)?;
        create_test_file(&good_dir, "main.rs", "fn main() {}\n")?;

        let sentinel = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(errors >= 1, "metadata failure should increment errors");
        let good_key = fs::canonicalize(&good_dir)?;
        assert!(
            stats.contains_key(&good_key),
            "sibling directory should remain in stats after metadata failure"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&sentinel)?),
            "metadata failure directory should be skipped in stats"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_read_dir_error_increments_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let sentinel = root.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        assert!(
            !stats.contains_key(&fs::canonicalize(&sentinel)?),
            "read_dir failure should prevent stats for the directory"
        );
        assert!(
            error_count >= 1,
            "read_dir failure should increment error count, got {error_count}"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_entry_iteration_error_is_counted() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let sentinel = root.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&sentinel)?;
        create_test_file(&sentinel, "ok.rs", "fn main() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        let sentinel_canon = fs::canonicalize(&sentinel)?;
        let dir_stats = stats
            .get(&sentinel_canon)
            .or_else(|| stats.get(&sentinel))
            .expect("directory stats should exist after iteration error");
        assert!(
            dir_stats.language_stats.contains_key("Rust"),
            "expected Rust stats even after iteration error"
        );
        assert!(
            error_count >= 1,
            "iteration error should increment error count, got {error_count}"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_alternating_success_failure_deeper() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // Healthy root file to ensure overall stats persist.
        create_test_file(root, "root.rs", "fn root() {}\n")?;

        // Level 1 alternating: success directory with nested failure and healthy leaves.
        let level1_ok = root.join("level1_ok");
        fs::create_dir(&level1_ok)?;
        create_test_file(&level1_ok, "ok.rs", "fn ok_level1() {}\n")?;

        let entry_fail = level1_ok.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail)?;
        create_test_file(&entry_fail, "entry.rs", "fn entry() {}\n")?;

        let nested_ok = entry_fail.join("nested_ok");
        fs::create_dir(&nested_ok)?;
        create_test_file(&nested_ok, "nested_ok.rs", "fn nested_ok() {}\n")?;

        // Inject metadata failure in nested leaf.
        let nested_meta = nested_ok.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&nested_meta)?;

        // Alternate with read_dir failure deeper.
        let nested_read_fail = entry_fail.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&nested_read_fail)?;

        // Separate branch: metadata failure at root level.
        let meta_fail_root = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&meta_fail_root)?;

        // Separate branch: read_dir failure at root level.
        let read_fail_root = root.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_fail_root)?;

        // Separate branch: file_type failure at root level.
        let _file_type_fail_root = root.join(super::FILE_TYPE_FAIL_TAG);
        create_test_file(&root, super::FILE_TYPE_FAIL_TAG, "fn file_fail() {}\n")?;

        // Healthy sibling to ensure stats persist.
        let healthy = root.join("healthy");
        fs::create_dir(&healthy)?;
        create_test_file(&healthy, "healthy.rs", "fn healthy() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 4,
            "expected alternating metadata/read_dir/entry/file_type failures to increment errors: {errors}"
        );

        let healthy_key = fs::canonicalize(&healthy)?;
        assert!(
            stats.contains_key(&healthy_key),
            "healthy directory should remain in stats after alternating failures"
        );

        let entry_key = fs::canonicalize(&entry_fail)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail))
            .expect("entry iteration failure directory should retain stats");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry iteration failure directory should keep Rust stats: {entry_stats:?}"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&meta_fail_root)?),
            "metadata failure directory should be excluded from stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&read_fail_root)?),
            "read_dir failure directory should be excluded from stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&nested_meta)?),
            "nested metadata failure directory should be excluded from stats"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_file_type_error_skips_entry() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, super::FILE_TYPE_FAIL_TAG, "fn main() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
        )?;

        assert!(
            stats.is_empty(),
            "file type failure should prevent stats accumulation"
        );
        assert!(
            error_count >= 1,
            "file type failure should increment error count, got {error_count}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_directory_impl_skips_special_file() -> io::Result<()> {
        use std::os::unix::net::UnixListener;

        let temp_dir = TempDir::new()?;
        let socket_path = temp_dir.path().join("listener.sock");
        let _listener = UnixListener::bind(&socket_path)?;

        let args = test_args();
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory_impl(
            &socket_path,
            &args,
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
            None,
        )?;

        assert!(
            stats.is_empty(),
            "special file should not contribute stats: {stats:?}"
        );
        assert_eq!(
            error_count, 0,
            "special file should be skipped without error increment"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_nested_failure_permutations() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        let good_level1 = root.join("good_l1");
        fs::create_dir(&good_level1)?;
        create_test_file(&good_level1, "main.rs", "fn main() {}\n")?;

        let fail_level1 = root.join("fail_l1");
        fs::create_dir(&fail_level1)?;

        let metadata_fail = fail_level1.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&metadata_fail)?;

        let read_dir_fail = fail_level1.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_dir_fail)?;

        let entry_level2 = fail_level1.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_level2)?;
        create_test_file(&entry_level2, "keep.rs", "fn keep() {}\n")?;

        let entry_nested = entry_level2.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_nested)?;
        create_test_file(&entry_nested, "nested.rs", "fn nested() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 4,
            "expected metadata, read_dir, and nested entry failures to increment errors: {errors}"
        );

        let good_key = fs::canonicalize(&good_level1)?;
        assert!(
            stats.contains_key(&good_key),
            "good_l1 stats should remain despite sibling failures"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&metadata_fail)?),
            "metadata failure directory should be absent from stats"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&read_dir_fail)?),
            "read_dir failure directory should be absent from stats"
        );

        let entry_key = fs::canonicalize(&entry_level2)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_level2))
            .expect("ENTRY_ITER_FAIL_TAG directory should retain stats");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry iteration directory should keep Rust stats despite simulated failure: {entry_stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_mixed_failure_tree() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        create_test_file(root, "root.rs", "fn root() {}\n")?;

        let meta_fail = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&meta_fail)?;

        let parent = root.join("parent");
        fs::create_dir(&parent)?;

        let read_fail = parent.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_fail)?;

        let entry_dir = parent.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_dir)?;
        create_test_file(&entry_dir, "ok.rs", "fn ok() {}\n")?;

        let nested_meta = entry_dir.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&nested_meta)?;

        create_test_file(&entry_dir, super::FILE_TYPE_FAIL_TAG, "fn bad() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 4,
            "expected combined failures (metadata/read_dir/entry/file_type) to increment errors: {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        if let Some(entry) = stats.remove(&root_key) {
            stats.insert(root.to_path_buf(), entry);
        }
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should exist");
        let root_has_rust = root_stats.language_stats.contains_key("Rust");
        assert!(
            root_has_rust,
            "root stats should retain Rust counts even with failures: {root_stats:?}"
        );

        let meta_seen = stats.contains_key(&fs::canonicalize(&meta_fail)?);
        assert!(
            !meta_seen,
            "metadata failure directory should be excluded from stats"
        );
        let read_seen = stats.contains_key(&fs::canonicalize(&read_fail)?);
        assert!(
            !read_seen,
            "read_dir failure directory should be excluded from stats"
        );
        let nested_seen = stats.contains_key(&fs::canonicalize(&nested_meta)?);
        assert!(
            !nested_seen,
            "nested metadata failure directory should be excluded from stats"
        );

        let entry_key = fs::canonicalize(&entry_dir)?;
        if !stats.contains_key(&entry_key) {
            if let Some(entry) = stats.remove(&entry_dir) {
                stats.insert(entry_key.clone(), entry);
            }
        }
        if let Some(entry) = stats.remove(&entry_key) {
            stats.insert(entry_dir.clone(), entry);
        }
        let entry_stats = stats
            .remove(&entry_key)
            .or_else(|| stats.remove(&entry_dir))
            .expect("entry iteration directory stats should be present");
        let entry_has_rust = entry_stats.language_stats.contains_key("Rust");
        assert!(
            entry_has_rust,
            "entry iteration directory should keep Rust stats despite failures: {entry_stats:?}"
        );
        stats.insert(entry_dir.clone(), entry_stats);
        let lookup = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_dir))
            .expect("fallback should locate entry iteration stats after reinsertion");
        assert!(
            lookup.language_stats.contains_key("Rust"),
            "fallback lookup should still expose Rust stats: {lookup:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_duplicate_canonical_merge() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let shared = root.join("shared");
        fs::create_dir(&shared)?;
        create_test_file(&shared, "one.rs", "fn one() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut merged: HashMap<PathBuf, DirectoryStats> = HashMap::new();

        for _ in 0..2 {
            let sub_stats = scan_directory_impl(
                &shared,
                &test_args(),
                root,
                &mut metrics,
                1,
                &mut entries,
                &mut errors,
                None,
            )?;
            for (dir, stat) in sub_stats {
                merge_directory_stats(&mut merged, dir, stat);
            }
        }

        assert_eq!(errors, 0, "duplicate merge should not introduce errors");
        assert!(
            entries >= 2,
            "expected entries counter to reflect duplicate scans: {entries}"
        );

        let shared_key = fs::canonicalize(&shared)?;
        let shared_stats = merged
            .get(&shared_key)
            .or_else(|| merged.get(&shared))
            .expect("shared directory stats should exist after merging duplicates");
        let rust_entry = shared_stats
            .language_stats
            .get("Rust")
            .expect("Rust stats should be present after merge");
        assert_eq!(
            rust_entry.0, 2,
            "expected file count to accumulate across duplicate merges: {rust_entry:?}"
        );
        assert_eq!(
            rust_entry.1.code_lines, 2,
            "code lines should accumulate across duplicate merges: {rust_entry:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_alternating_failures() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // top-level good artifact
        create_test_file(root, "main.rs", "fn main() {}\n")?;

        // first-level directory that will fail read_dir
        let fail_dir = root.join("fail_dir");
        fs::create_dir(&fail_dir)?;
        let read_dir_fail = fail_dir.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_dir_fail)?;

        // sibling directory that succeeds but contains a nested entry iteration failure
        let ok_dir = root.join("ok_dir");
        fs::create_dir(&ok_dir)?;
        create_test_file(&ok_dir, "ok.rs", "fn ok() {}\n")?;
        let entry_fail = ok_dir.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail)?;
        create_test_file(&entry_fail, "entry.rs", "fn entry() {}\n")?;

        // nested metadata failure beneath the entry iteration failure
        let nested_meta = entry_fail.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&nested_meta)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 3,
            "expected read_dir, entry iteration, and nested metadata failures to increment errors: {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        if let Some(entry) = stats.remove(&root_key) {
            stats.insert(root.to_path_buf(), entry);
        }
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should exist after alternating failures");
        let root_has_rust = root_stats.language_stats.contains_key("Rust");
        assert!(
            root_has_rust,
            "root stats should retain Rust counts despite alternating failures: {root_stats:?}"
        );

        let read_excluded = stats.contains_key(&fs::canonicalize(&read_dir_fail)?);
        assert!(
            !read_excluded,
            "read_dir failure directory should be excluded from stats"
        );
        let nested_excluded = stats.contains_key(&fs::canonicalize(&nested_meta)?);
        assert!(
            !nested_excluded,
            "nested metadata failure directory should be excluded from stats"
        );

        let entry_key = fs::canonicalize(&entry_fail)?;
        if !stats.contains_key(&entry_key) {
            if let Some(entry) = stats.remove(&entry_fail) {
                stats.insert(entry_key.clone(), entry);
            }
        }
        if let Some(entry) = stats.remove(&entry_key) {
            stats.insert(entry_fail.clone(), entry);
        }
        let entry_stats = stats
            .remove(&entry_key)
            .or_else(|| stats.remove(&entry_fail))
            .expect("entry iteration directory stats should be present after mixed failures");
        let entry_has_rust = entry_stats.language_stats.contains_key("Rust");
        assert!(
            entry_has_rust,
            "entry iteration directory should keep Rust stats despite failures: {entry_stats:?}"
        );
        stats.insert(entry_fail.clone(), entry_stats);
        let entry_lookup = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail))
            .expect("fallback should locate entry iteration stats after reinsertion");
        assert!(
            entry_lookup.language_stats.contains_key("Rust"),
            "fallback lookup should retain Rust stats: {entry_lookup:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_canonical_fallback_alias_key() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        let alias_dir = root.join("alias_dir");
        fs::create_dir(&alias_dir)?;
        create_test_file(&alias_dir, "lib.rs", "fn lib() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert_eq!(errors, 0, "expected scan without errors");
        assert!(
            entries >= 1,
            "expected at least one entry processed for alias directory: {entries}"
        );

        let canonical = fs::canonicalize(&alias_dir)?;
        let alias_stats = stats
            .remove(&canonical)
            .expect("expected canonical entry for alias directory");
        stats.insert(alias_dir.clone(), alias_stats);

        let fallback_stats = stats
            .remove(&canonical)
            .or_else(|| stats.remove(&alias_dir))
            .expect("fallback should retrieve stats when canonical key is missing");
        assert!(
            fallback_stats.language_stats.contains_key("Rust"),
            "alias directory stats should retain Rust counts after fallback: {fallback_stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_deeper_alternating_failures() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        create_test_file(root, "root.rs", "fn root() {}\n")?;

        // Healthy sibling to ensure good stats persist.
        let healthy = root.join("healthy");
        fs::create_dir(&healthy)?;
        create_test_file(&healthy, "ok.rs", "fn ok() {}\n")?;

        // First alternating branch: success dir containing entry failure that alternates deeper.
        let level1_ok = root.join("level1_ok");
        fs::create_dir(&level1_ok)?;
        create_test_file(&level1_ok, "ok.rs", "fn ok_level1() {}\n")?;

        let entry_fail_level1 = level1_ok.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail_level1)?;
        create_test_file(&entry_fail_level1, "entry_l1.rs", "fn entry_l1() {}\n")?;

        // Nested healthy dir under the entry failure to keep stats.
        let nested_ok = entry_fail_level1.join("nested_ok");
        fs::create_dir(&nested_ok)?;
        create_test_file(&nested_ok, "nested_ok.rs", "fn nested_ok() {}\n")?;

        // Alternate with a deeper entry failure that contains both metadata and file type sentinels.
        let deep_entry_fail = nested_ok.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&deep_entry_fail)?;
        create_test_file(&deep_entry_fail, "deep_entry.rs", "fn deep_entry() {}\n")?;
        create_test_file(
            &deep_entry_fail,
            super::FILE_TYPE_FAIL_TAG,
            "fn should_fail() {}\n",
        )?;
        let deep_meta_fail = deep_entry_fail.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&deep_meta_fail)?;

        // Inject a read_dir failure alongside the healthy directory to continue alternating.
        let nested_read_fail = entry_fail_level1.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&nested_read_fail)?;

        // Second alternating branch: immediate read_dir failure at level 1.
        let level1_read_fail = root.join("level1_read_fail");
        fs::create_dir(&level1_read_fail)?;
        let level1_read_sentinel = level1_read_fail.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&level1_read_sentinel)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 5,
            "expected alternating read_dir, entry, metadata, and file_type failures to increment errors: {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        if let Some(entry) = stats.remove(&root_key) {
            stats.insert(root.to_path_buf(), entry);
        }
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should exist after deeper alternating failures");
        let root_has_rust = root_stats.language_stats.contains_key("Rust");
        assert!(
            root_has_rust,
            "root stats should retain Rust counts despite deeper alternating failures: {root_stats:?}"
        );

        let healthy_key = fs::canonicalize(&healthy)?;
        let healthy_stats = stats
            .get(&healthy_key)
            .or_else(|| stats.get(&healthy))
            .expect("healthy sibling should retain stats");
        let healthy_has_rust = healthy_stats.language_stats.contains_key("Rust");
        assert!(
            healthy_has_rust,
            "healthy sibling should maintain Rust stats: {healthy_stats:?}"
        );

        // Ensure failure sentinels are excluded.
        let level1_read_excluded = stats.contains_key(&fs::canonicalize(&level1_read_sentinel)?);
        assert!(
            !level1_read_excluded,
            "level1 read_dir failure should not appear in stats"
        );
        let deep_meta_excluded = stats.contains_key(&fs::canonicalize(&deep_meta_fail)?);
        assert!(
            !deep_meta_excluded,
            "deep metadata failure should not appear in stats"
        );
        let nested_read_excluded = stats.contains_key(&fs::canonicalize(&nested_read_fail)?);
        assert!(
            !nested_read_excluded,
            "nested read_dir failure should not appear in stats"
        );

        // Exercise fallback between canonical and relative keys for the deepest entry failure.
        let deep_entry_key = fs::canonicalize(&deep_entry_fail)?;
        let deep_entry_stats = stats
            .remove(&deep_entry_key)
            .or_else(|| stats.remove(&deep_entry_fail))
            .expect("deep entry failure stats should be present for fallback testing");
        stats.insert(deep_entry_fail.clone(), deep_entry_stats);
        let deep_entry_lookup = stats
            .get(&deep_entry_key)
            .or_else(|| stats.get(&deep_entry_fail))
            .expect("deep entry fallback should succeed");
        let deep_entry_has_rust = deep_entry_lookup.language_stats.contains_key("Rust");
        assert!(
            deep_entry_has_rust,
            "deep entry failure branch should retain Rust stats despite surrounding failures: {deep_entry_lookup:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_non_recursive_skips_nested() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let nested = root.join("nested");
        fs::create_dir(&nested)?;
        create_test_file(&nested, "nested.rs", "fn nested() {}\n")?;

        let mut args = test_args();
        args.non_recursive = true;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory_impl(
            &nested,
            &args,
            root,
            &mut metrics,
            1,
            &mut entries,
            &mut errors,
            None,
        )?;

        assert!(
            stats.is_empty(),
            "non-recursive scan should skip nested directories: {stats:?}"
        );
        assert_eq!(
            entries, 0,
            "non-recursive scan should not count entries for nested directories"
        );
        assert_eq!(errors, 0, "non-recursive skip should not add errors");
        Ok(())
    }

    #[test]
    fn test_scan_directory_deeper_alternating_with_filters() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        create_test_file(root, "root.rs", "fn root() {}\n")?;

        let healthy = root.join("healthy_branch");
        fs::create_dir(&healthy)?;
        create_test_file(&healthy, "healthy.rs", "fn healthy() {}\n")?;

        let ignored = root.join("ignored_branch");
        fs::create_dir(&ignored)?;
        create_test_file(&ignored, "ignored.rs", "fn ignored() {}\n")?;

        let alternating = root.join("alternating_branch");
        fs::create_dir(&alternating)?;
        create_test_file(&alternating, "alt.rs", "fn alt() {}\n")?;

        let alternating_entry = alternating.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&alternating_entry)?;
        create_test_file(&alternating_entry, "entry.rs", "fn entry_alt() {}\n")?;

        let alternating_read = alternating.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&alternating_read)?;

        let alternating_meta = alternating_entry.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&alternating_meta)?;

        let mut args = test_args();
        args.ignore = vec!["ignored_branch".to_string()];
        args.max_depth = 3;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 2,
            "expected read_dir and metadata failures to increment errors: {errors}"
        );

        let ignored_key = fs::canonicalize(&ignored)?;
        assert!(
            !stats.contains_key(&ignored_key),
            "ignored branch should not appear in stats"
        );

        let healthy_key = fs::canonicalize(&healthy)?;
        let healthy_stats = stats
            .get(&healthy_key)
            .or_else(|| stats.get(&healthy))
            .expect("healthy branch should retain stats");
        assert!(
            healthy_stats.language_stats.contains_key("Rust"),
            "healthy branch should maintain Rust stats: {healthy_stats:?}"
        );

        let alternating_key = fs::canonicalize(&alternating)?;
        if let Some(entry) = stats.remove(&alternating_key) {
            stats.insert(alternating.clone(), entry);
        }
        let alternating_stats = stats
            .get(&alternating_key)
            .or_else(|| stats.get(&alternating))
            .expect("alternating branch should retain stats despite failures");
        assert!(
            alternating_stats.language_stats.contains_key("Rust"),
            "alternating branch should keep Rust stats: {alternating_stats:?}"
        );

        let entry_key = fs::canonicalize(&alternating_entry)?;
        if let Some(entry) = stats.remove(&entry_key) {
            stats.insert(alternating_entry.clone(), entry);
        }
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&alternating_entry))
            .expect("entry failure branch should retain stats after fallback");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry failure branch should keep Rust stats despite filters: {entry_stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_nested_metadata_error_keeps_siblings() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let good_dir = root.join("good");
        fs::create_dir(&good_dir)?;
        create_test_file(&good_dir, "main.rs", "fn main() {}\n")?;

        let sentinel = good_dir.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(errors >= 1, "metadata failure should produce an error");

        let good_key = fs::canonicalize(&good_dir)?;
        let good_stats = stats
            .get(&good_key)
            .or_else(|| stats.get(&good_dir))
            .expect("good directory stats should still be recorded");
        assert!(
            good_stats.language_stats.contains_key("Rust"),
            "expected Rust stats for good directory after metadata failure"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_nested_read_dir_error_keeps_parent() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let good_dir = root.join("good");
        fs::create_dir(&good_dir)?;
        create_test_file(&good_dir, "main.rs", "fn main() {}\n")?;

        let sentinel = good_dir.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(errors >= 1, "read_dir failure should produce an error");
        assert!(
            !stats.contains_key(&fs::canonicalize(&sentinel)?),
            "sentinel directory should not appear in stats after read_dir failure"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_multiple_failure_siblings() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let good_dir = root.join("good");
        fs::create_dir(&good_dir)?;
        create_test_file(&good_dir, "ok.rs", "fn ok() {}\n")?;

        let metadata_fail = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&metadata_fail)?;

        let read_dir_fail = root.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_dir_fail)?;

        let entry_fail = root.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail)?;
        create_test_file(&entry_fail, "entry.rs", "fn entry() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 3,
            "expected metadata, read_dir, and entry iteration failures to increment errors: {errors}"
        );

        let good_key = fs::canonicalize(&good_dir)?;
        assert!(
            stats.contains_key(&good_key),
            "good directory stats should remain even with sibling failures"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&metadata_fail)?),
            "metadata failure directory should be absent from stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&read_dir_fail)?),
            "read_dir failure directory should be absent from stats"
        );

        let entry_key = fs::canonicalize(&entry_fail)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail))
            .expect("entry iteration directory should retain stats");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry iteration directory should keep Rust stats despite simulated failure: {entry_stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_records_recursive_error() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        let overflow = root.join("overflow");
        fs::create_dir(&overflow)?;
        create_test_file(&overflow, "first.rs", "fn first() {}\n")?;
        create_test_file(&overflow, "second.rs", "fn second() {}\n")?;

        let mut args = test_args();
        args.max_entries = 1;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert_eq!(
            errors, 1,
            "expected a single error when subdirectory scan overflows max_entries: {errors}"
        );
        assert_eq!(
            entries, 2,
            "expected entries counter to reflect the second file triggering overflow: {entries}"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&overflow)?),
            "overflow directory should not contribute stats after recursive scan error: {stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_handles_file_root() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("single.rs");
        create_test_file(temp_dir.path(), "single.rs", "fn main() {}\n// comment\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            &file_path,
            &test_args(),
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert_eq!(errors, 0, "file root scan should not record errors");
        assert_eq!(entries, 1, "expected the single file to be processed");

        let parent_dir = file_path.parent().expect("file should have parent");
        let guard = CurrentDirGuard::change_to(parent_dir)?;
        let parent_canonical = fs::canonicalize(Path::new("."))?;
        let dir_stats = stats
            .get(Path::new("."))
            .or_else(|| stats.get(&parent_canonical))
            .expect("directory stats should capture file root processing");
        drop(guard);
        let (lang, (file_total, lang_stats)) = dir_stats
            .language_stats
            .iter()
            .next()
            .expect("language stats should contain Rust entry");
        assert_eq!(lang.as_str(), "Rust");
        assert_eq!(*file_total, 1, "expected exactly one Rust file recorded");
        assert_eq!(
            lang_stats.code_lines, 1,
            "expected code line from main function; stats: {lang_stats:?}"
        );
        assert_eq!(
            lang_stats.comment_lines, 1,
            "expected single comment line captured; stats: {lang_stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_extended_failure_tree() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        create_test_file(root, "main.rs", "fn main() {}\n")?;

        let branch_a = root.join("branch_a");
        fs::create_dir(&branch_a)?;
        create_test_file(&branch_a, "a.rs", "fn a() {}\n")?;

        let entry_fail_level1 = branch_a.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail_level1)?;
        create_test_file(&entry_fail_level1, "level1_ok.rs", "fn l1() {}\n")?;

        let entry_fail_level2 = entry_fail_level1.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail_level2)?;
        create_test_file(&entry_fail_level2, "level2_ok.rs", "fn l2() {}\n")?;
        create_test_file(
            &entry_fail_level2,
            super::FILE_TYPE_FAIL_TAG,
            "fn impossible() {}\n",
        )?;

        let nested_meta_fail = entry_fail_level2.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&nested_meta_fail)?;

        let read_fail_branch = root.join("read_fail_branch");
        fs::create_dir(&read_fail_branch)?;
        let read_fail = read_fail_branch.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_fail)?;

        let top_metadata_fail = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&top_metadata_fail)?;

        let branch_b = root.join("branch_b");
        fs::create_dir(&branch_b)?;
        create_test_file(&branch_b, "b.rs", "fn b() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 6,
            "expected cumulative metadata/read_dir/file_type/entry errors to increment counter: {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        if let Some(entry) = stats.remove(&root_key) {
            stats.insert(root.to_path_buf(), entry);
        }
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should exist after extended failure tree");
        assert!(
            root_stats.language_stats.contains_key("Rust"),
            "root stats should retain Rust counts after extended failure tree: {root_stats:?}"
        );

        let branch_a_key = fs::canonicalize(&branch_a)?;
        if let Some(entry) = stats.remove(&branch_a_key) {
            stats.insert(branch_a.clone(), entry);
        }
        let branch_a_stats = stats
            .get(&branch_a_key)
            .or_else(|| stats.get(&branch_a))
            .expect("branch_a stats should exist despite nested failures");
        assert!(
            branch_a_stats.language_stats.contains_key("Rust"),
            "branch_a should retain Rust stats: {branch_a_stats:?}"
        );

        let entry_level1_key = fs::canonicalize(&entry_fail_level1)?;
        if let Some(entry) = stats.remove(&entry_level1_key) {
            stats.insert(entry_fail_level1.clone(), entry);
        }
        let entry_level1_stats = stats
            .get(&entry_level1_key)
            .or_else(|| stats.get(&entry_fail_level1))
            .expect("entry_fail_level1 stats should be preserved");
        assert!(
            entry_level1_stats.language_stats.contains_key("Rust"),
            "entry_fail_level1 should retain Rust stats despite injected failures: {entry_level1_stats:?}"
        );

        let branch_b_key = fs::canonicalize(&branch_b)?;
        assert!(
            stats
                .get(&branch_b_key)
                .or_else(|| stats.get(&branch_b))
                .is_some(),
            "branch_b should contribute stats alongside failure branches"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&top_metadata_fail)?),
            "top-level metadata failure should be excluded from stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&read_fail)?),
            "read_dir failure directory should be excluded from stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&nested_meta_fail)?),
            "nested metadata failure should be excluded from stats"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_multiple_entry_failure_branches() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        create_test_file(root, "root.rs", "fn root() {}\n")?;

        for branch_name in ["branch_a", "branch_b"] {
            let branch = root.join(branch_name);
            fs::create_dir(&branch)?;
            create_test_file(&branch, "ok.rs", "fn ok() {}\n")?;

            let entry_fail = branch.join(super::ENTRY_ITER_FAIL_TAG);
            fs::create_dir(&entry_fail)?;
            create_test_file(&entry_fail, "inner.rs", "fn inner() {}\n")?;
            create_test_file(
                &entry_fail,
                super::FILE_TYPE_FAIL_TAG,
                "fn should_error() {}\n",
            )?;

            let nested_meta = entry_fail.join(super::METADATA_FAIL_TAG);
            fs::create_dir(&nested_meta)?;

            let nested_read_dir = entry_fail.join(super::READ_DIR_FAIL_TAG);
            fs::create_dir(&nested_read_dir)?;
        }

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 8,
            "expected cumulative failures across sibling entry branches: {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        if let Some(entry) = stats.remove(&root_key) {
            stats.insert(root.to_path_buf(), entry);
        }
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should survive multi-branch failures");
        assert!(
            root_stats.language_stats.contains_key("Rust"),
            "root stats should retain Rust counts after failures: {root_stats:?}"
        );

        for branch_name in ["branch_a", "branch_b"] {
            let branch = root.join(branch_name);
            let entry_dir = branch.join(super::ENTRY_ITER_FAIL_TAG);
            let entry_canonical = fs::canonicalize(&entry_dir).ok();
            if let Some(canon) = entry_canonical.as_ref() {
                if let Some(entry) = stats.remove(canon) {
                    stats.insert(entry_dir.clone(), entry);
                }

                // Regenerate canonical stats so both key forms coexist before exercising fallback.
                let mut regen_metrics = test_metrics();
                let mut regen_entries = 0usize;
                let mut regen_errors = 0usize;
                let mut regen_stats = scan_directory(
                    root,
                    &test_args(),
                    root,
                    &mut regen_metrics,
                    0,
                    &mut regen_entries,
                    &mut regen_errors,
                )?;
                let dup_entry = regen_stats
                    .remove(canon)
                    .or_else(|| regen_stats.remove(&entry_dir))
                    .expect("regenerated stats should contain canonical entry");
                stats.insert(canon.clone(), dup_entry);

                if let Some(entry) = stats.remove(canon) {
                    stats.insert(entry_dir.clone(), entry);
                }
            }
            let entry_stats = entry_canonical
                .and_then(|p| stats.get(&p))
                .or_else(|| stats.get(&entry_dir))
                .expect("entry iteration directory should keep stats despite sibling failures");
            assert!(
                entry_stats.language_stats.contains_key("Rust"),
                "entry iteration stats should retain Rust counts: {entry_stats:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn test_scan_directory_relative_root_fallback_stats() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let guard = CurrentDirGuard::change_to(temp_dir.path())?;
        create_test_file(Path::new("."), "main.rs", "fn main() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let mut stats = scan_directory(
            Path::new("."),
            &test_args(),
            temp_dir.path(),
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert_eq!(errors, 0, "relative scan should not record errors");
        assert_eq!(entries, 1, "expected single file processed");

        let canonical = fs::canonicalize(".")?;
        let entry = stats
            .remove(&canonical)
            .or_else(|| stats.remove(Path::new(".")))
            .expect("expected canonical or relative '.' key to exist initially");
        stats.insert(PathBuf::from("."), entry);

        let dot_stats = stats
            .get(&canonical)
            .or_else(|| stats.get(Path::new(".")))
            .expect("fallback should locate relative '.' entry");
        assert!(
            dot_stats.language_stats.contains_key("Rust"),
            "expected Rust stats in relative '.' entry: {dot_stats:?}"
        );

        drop(guard);
        Ok(())
    }

    #[test]
    fn test_scan_directory_nested_entry_and_file_type_failures() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let healthy = root.join("healthy");
        fs::create_dir(&healthy)?;
        create_test_file(&healthy, "ok.rs", "fn ok() {}\n")?;

        let entry_fail = healthy.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail)?;
        create_test_file(&entry_fail, "good.rs", "fn good() {}\n")?;
        create_test_file(&entry_fail, super::FILE_TYPE_FAIL_TAG, "fn bad() {}\n")?;

        let read_dir_fail = entry_fail.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_dir_fail)?;
        create_test_file(&read_dir_fail, "nested.rs", "fn nested() {}\n")?;

        let metadata_fail = entry_fail.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&metadata_fail)?;
        create_test_file(&metadata_fail, "ignored.rs", "fn ignored() {}\n")?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 4,
            "expected entry iteration, file type, read_dir, and metadata failures to increment errors: {errors}"
        );

        let healthy_key = fs::canonicalize(&healthy)?;
        let healthy_stats = stats
            .get(&healthy_key)
            .or_else(|| stats.get(&healthy))
            .expect("healthy directory stats should be recorded");
        assert!(
            healthy_stats.language_stats.contains_key("Rust"),
            "healthy directory should retain Rust stats after failures: {healthy_stats:?}"
        );

        let entry_key = fs::canonicalize(&entry_fail)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail))
            .expect("entry failure directory stats should exist after simulated error");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry failure directory should retain Rust stats after error injection: {entry_stats:?}"
        );

        assert!(
            !stats.contains_key(&fs::canonicalize(&read_dir_fail)?),
            "read_dir failure directory should be skipped in stats"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&metadata_fail)?),
            "metadata failure directory should be skipped in stats"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_respects_non_recursive_flag() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let subdir = root.join("sub");
        fs::create_dir(&subdir)?;
        create_test_file(root, "root.rs", "fn root() {}\n")?;
        create_test_file(&subdir, "child.rs", "fn child() {}\n")?;

        let mut args = test_args();
        args.non_recursive = true;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            stats.contains_key(&fs::canonicalize(root)?),
            "root stats should exist when non_recursive is true"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&subdir)?),
            "subdirectories should be skipped when non_recursive is true"
        );
        assert_eq!(errors, 0, "non-recursive scan should not produce errors");
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
    fn test_scan_directory_max_depth_with_failures() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let level1 = root.join("level1");
        let level2 = level1.join("level2");
        fs::create_dir(&level1)?;
        fs::create_dir(&level2)?;
        create_test_file(root, "root.rs", "fn root() {}\n")?;
        create_test_file(&level1, "child.rs", "fn child() {}\n")?;
        create_test_file(&level2, "grandchild.rs", "fn grandchild() {}\n")?;

        let sentinel = level2.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&sentinel)?;

        let mut args = test_args();
        args.max_depth = 1;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &args,
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        let root_key = fs::canonicalize(root)?;
        let level1_key = fs::canonicalize(&level1)?;
        let level2_key = fs::canonicalize(&level2)?;

        assert!(
            stats.contains_key(&root_key),
            "root stats should exist when max_depth restricts traversal"
        );
        assert!(
            stats.contains_key(&level1_key),
            "level1 should be included when max_depth allows depth 1"
        );
        assert!(
            !stats.contains_key(&level2_key),
            "level2 should be skipped when max_depth is 1"
        );
        assert!(
            errors >= 1,
            "skipping deeper levels should increment error_count via warning path, got {errors}"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_deep_alternating_failures() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // success branch
        let keep_dir = root.join("keep");
        fs::create_dir(&keep_dir)?;
        create_test_file(&keep_dir, "keep.rs", "fn keep() {}\n")?;

        // failure branch with alternating success
        let fail_root = root.join("fail_root");
        fs::create_dir(&fail_root)?;
        let entry_fail = fail_root.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail)?;
        create_test_file(&entry_fail, "entry.rs", "fn entry() {}\n")?;

        let success_under_fail = entry_fail.join("success");
        fs::create_dir(&success_under_fail)?;
        create_test_file(&success_under_fail, "ok.rs", "fn ok() {}\n")?;

        let metadata_fail = success_under_fail.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&metadata_fail)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 2,
            "expected alternating failure tree to increment errors multiple times: {errors}"
        );

        let keep_key = fs::canonicalize(&keep_dir)?;
        assert!(
            stats.contains_key(&keep_key),
            "keep directory should remain in stats despite sibling failures"
        );

        let entry_key = fs::canonicalize(&entry_fail)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail))
            .expect("entry failure directory should retain stats in alternating layout");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry failure directory should keep Rust stats after alternating failures: {entry_stats:?}"
        );

        Ok(())
    }

    #[test]
    fn test_scan_directory_failure_counter_accumulates() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "root.rs", "fn root() {}\n")?;

        let meta_fail = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&meta_fail)?;

        let read_fail = root.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_fail)?;

        let entry_dir = root.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_dir)?;
        create_test_file(&entry_dir, "keep.rs", "fn keep() {}\n")?;

        let nested_file_type = entry_dir.join("nested");
        fs::create_dir(&nested_file_type)?;
        create_test_file(
            &nested_file_type,
            super::FILE_TYPE_FAIL_TAG,
            "fn fail() {}\n",
        )?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 4,
            "expected multiple failure types to accumulate error count, got {errors}"
        );

        let entry_key = fs::canonicalize(&entry_dir)?;
        assert!(
            stats.contains_key(&entry_key),
            "entry iteration directory should remain in stats despite additional failures"
        );
        Ok(())
    }

    #[test]
    fn test_scan_directory_failure_counter_exceeds_four() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "root.rs", "fn root() {}\n")?;

        let meta_fail = root.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&meta_fail)?;

        let read_fail = root.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&read_fail)?;

        let entry_fail_level1 = root.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail_level1)?;
        create_test_file(&entry_fail_level1, "keep.rs", "fn keep() {}\n")?;

        let nested_meta = entry_fail_level1.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&nested_meta)?;

        let nested_read = entry_fail_level1.join(super::READ_DIR_FAIL_TAG);
        fs::create_dir(&nested_read)?;

        let entry_fail_level2 = entry_fail_level1.join(super::ENTRY_ITER_FAIL_TAG);
        fs::create_dir(&entry_fail_level2)?;
        create_test_file(&entry_fail_level2, "inner.rs", "fn inner() {}\n")?;

        create_test_file(
            &entry_fail_level2,
            super::FILE_TYPE_FAIL_TAG,
            "fn violation() {}\n",
        )?;

        let deep_meta = entry_fail_level2.join(super::METADATA_FAIL_TAG);
        fs::create_dir(&deep_meta)?;

        let mut metrics = test_metrics();
        let mut entries = 0usize;
        let mut errors = 0usize;
        let stats = scan_directory(
            root,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries,
            &mut errors,
        )?;

        assert!(
            errors >= 7,
            "expected failures to push error count beyond four, got {errors}"
        );

        let root_key = fs::canonicalize(root)?;
        let root_stats = stats
            .get(&root_key)
            .or_else(|| stats.get(root))
            .expect("root stats should exist after failure aggregation");
        assert!(
            root_stats.language_stats.contains_key("Rust"),
            "root stats should retain Rust code despite failures: {root_stats:?}"
        );

        let entry_key = fs::canonicalize(&entry_fail_level1)?;
        let entry_stats = stats
            .get(&entry_key)
            .or_else(|| stats.get(&entry_fail_level1))
            .expect("entry failure directory should retain stats");
        assert!(
            entry_stats.language_stats.contains_key("Rust"),
            "entry failure directory should retain Rust stats after multiple failures: {entry_stats:?}"
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
    fn test_scan_directory_ignore_list_retains_siblings() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let ignore_dir = root.join("ignore_me");
        let keep_dir = root.join("keep_me");
        fs::create_dir(&ignore_dir)?;
        fs::create_dir(&keep_dir)?;
        create_test_file(&ignore_dir, "ignored.rs", "fn ignored() {}\n")?;
        create_test_file(&keep_dir, "keep.rs", "fn keep() {}\n")?;

        let mut args = test_args();
        args.ignore = vec!["ignore_me".to_string()];

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

        let keep_key = fs::canonicalize(&keep_dir)?;
        assert!(
            stats.contains_key(&keep_key),
            "keep_me directory should remain in stats when ignore list excludes it"
        );
        assert!(
            !stats.contains_key(&fs::canonicalize(&ignore_dir)?),
            "ignore_me directory should be omitted from stats"
        );
        assert_eq!(
            error_count, 0,
            "ignoring directories should not raise errors"
        );
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
    fn test_rust_counts_blank_lines() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank.rs",
            "fn main() {\n\n    println!(\"hi\");\n}\n",
        )?;
        let (stats, _total_lines) = count_rust_lines(temp_dir.path().join("blank.rs").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank lines to be counted: {stats:?}"
        );
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
    fn test_rust_block_comment_followed_by_line_comment_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "close_line.rs",
            "fn annotate() {\nlet value = 1; /* block */ // trailing comment\n}\n",
        )?;
        let (stats, _total_lines) =
            count_rust_lines(temp_dir.path().join("close_line.rs").as_path())?;
        assert_eq!(
            stats.code_lines, 3,
            "code lines should not double-count after block close: {stats:?}"
        );
        assert_eq!(
            stats.comment_lines, 1,
            "trailing line comment on same line is suppressed after block close: {stats:?}"
        );
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
    fn test_rust_multiline_block_closes_with_trailing_code_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_close.rs",
            "fn value() {\n/* start\n  middle */ let x = 1;\n}\n",
        )?;
        let (stats, _total_lines) =
            count_rust_lines(temp_dir.path().join("inline_close.rs").as_path())?;
        assert!(
            stats.code_lines >= 2,
            "expected trailing code after block close counted as code: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected multiline block comment counted appropriately: {stats:?}"
        );
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
    fn test_python_multiline_comment_closes_with_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_code.py",
            "\"\"\"doc start\nbody\nend\"\"\" value = 42\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("doc_with_code.py").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected multiline docstring counted as comments: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 1,
            "code following docstring close should be counted once: {stats:?}"
        );
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
    fn test_python_docstring_trailing_comment_suppresses_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_comment.py",
            "\"\"\"doc\"\"\" # trailing comment only\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("doc_comment.py").as_path())?;
        assert_eq!(
            stats.code_lines, 0,
            "code should not be counted when trailing segment is comment: {stats:?}"
        );
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_python_docstring_closes_with_code_and_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_code_comment.py",
            "\"\"\"doc\\nbody\\nend\"\"\" value = 42 # note\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("doc_with_code_comment.py").as_path())?;
        assert!(
            stats.comment_lines >= 1,
            "expected docstring lines counted as comments: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected trailing code to be counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_python_docstring_with_blank_line_after_close() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_blank.py",
            "\"\"\"doc\"\"\"\n\nprint('done')\n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("doc_with_blank.py").as_path())?;
        assert!(
            stats.comment_lines >= 1,
            "expected docstring to count as comment: {stats:?}"
        );
        assert!(
            stats.blank_lines >= 1,
            "expected blank line after docstring: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected code following blank line: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_python_docstring_with_only_whitespace_after_close() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_whitespace.py",
            "\"\"\"doc\"\"\"\n    \n",
        )?;
        let (stats, _total_lines) =
            count_python_lines(temp_dir.path().join("doc_with_whitespace.py").as_path())?;
        assert_eq!(
            stats.comment_lines, 1,
            "docstring should count as comment: {stats:?}"
        );
        assert_eq!(
            stats.blank_lines, 1,
            "whitespace line should count as blank: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 0,
            "no code should be counted after whitespace: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_python_docstring_whitespace_then_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_whitespace_code.py",
            "\"\"\"doc\"\"\"\n    \nprint('done')\n",
        )?;
        let (stats, _total_lines) = count_python_lines(
            temp_dir
                .path()
                .join("doc_with_whitespace_code.py")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 1,
            "docstring line should count as comment: {stats:?}"
        );
        assert_eq!(
            stats.blank_lines, 1,
            "whitespace-only separator should count as blank: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "code following whitespace should be counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_python_docstring_whitespace_then_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "doc_with_whitespace_comment.py",
            "\"\"\"doc\"\"\"\n    \n# trailing comment\n",
        )?;
        let (stats, _total_lines) = count_python_lines(
            temp_dir
                .path()
                .join("doc_with_whitespace_comment.py")
                .as_path(),
        )?;
        assert!(
            stats.comment_lines >= 2,
            "expected docstring and trailing hash as comments: {stats:?}"
        );
        assert_eq!(stats.blank_lines, 1);
        assert_eq!(stats.code_lines, 0);
        Ok(())
    }

    #[test]
    fn test_yaml_hash_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "sample.yaml",
            "# leading comment\nkey: value\n\n# trailing\n",
        )?;
        let (stats, total) = count_yaml_lines(temp_dir.path().join("sample.yaml").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.comment_lines, 2);
        assert_eq!(stats.code_lines, 1);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_yaml_inline_hash_after_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "inline.yaml", "key: value # comment\n")?;
        let (stats, total) = count_yaml_lines(temp_dir.path().join("inline.yaml").as_path())?;
        assert_eq!(total, 1);
        assert_eq!(stats.code_lines, 1);
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_toml_hash_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "config.toml",
            "title = \"test\"\n# note\n\nvalue = 1\n",
        )?;
        let (stats, total) = count_toml_lines(temp_dir.path().join("config.toml").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.comment_lines, 1);
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_toml_inline_hash_after_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "inline.toml", "value = 1 # note\n")?;
        let (stats, total) = count_toml_lines(temp_dir.path().join("inline.toml").as_path())?;
        assert_eq!(total, 1);
        assert_eq!(stats.code_lines, 1);
        assert_eq!(stats.comment_lines, 0);
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_spanning_lines() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "multi.hcl",
            "resource \"x\" \"y\" {\n  /* start\n     still comment */ value = 1\n}\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(temp_dir.path().join("multi.hcl").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected resource lines counted as code: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_unterminated_block_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "unterminated.hcl",
            "variable \"x\" {\n  value = 1 /* start\n     still comment\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("unterminated.hcl").as_path())?;
        assert!(
            stats.comment_lines >= 1,
            "unterminated block should count comment lines: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "initial code should be counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_ini_hash_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "config.ini",
            "[section]\n# note\n; another\nkey=value\n",
        )?;
        let (stats, total) = count_ini_lines(temp_dir.path().join("config.ini").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.comment_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
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
    fn test_powershell_block_comment_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_comment.ps1",
            "<# note #> Write-Host 'done'\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("inline_comment.ps1").as_path())?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_powershell_block_comment_spanning_lines() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "multiline_comment.ps1",
            "<# start\nstill comment\n#>\nWrite-Host 'after'\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("multiline_comment.ps1").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected multiline comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected code after block to be counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_powershell_block_and_line_interleaved() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "interleaved.ps1",
            "Write-Host 'mix'<#block#>Write-Host 'after'<#open\ncontinued\n#># trailing\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("interleaved.ps1").as_path())?;
        assert!(
            stats.comment_lines >= 3,
            "expected multiple comment segments: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected inline code portions counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_build_analysis_report_includes_totals() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let mut stats_map = HashMap::new();
        let mut dir_stats = DirectoryStats::default();
        dir_stats.language_stats.insert(
            "Rust".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 3,
                    comment_lines: 1,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );
        dir_stats.language_stats.insert(
            "Python".to_string(),
            (
                2,
                LanguageStats {
                    code_lines: 4,
                    comment_lines: 2,
                    blank_lines: 1,
                    overlap_lines: 0,
                },
            ),
        );
        stats_map.insert(temp_dir.path().to_path_buf(), dir_stats);

        let report = build_analysis_report(temp_dir.path(), &stats_map, 3, 11, 1);
        assert!(
            report.contains("Totals by language:"),
            "report should include totals header: {report}"
        );
        assert!(
            report.contains("Rust") && report.contains("Python"),
            "report should include language rows: {report}"
        );
        assert!(
            report.contains("Overall Summary:"),
            "report should include overall summary block: {report}"
        );
        assert!(
            report.contains("Warning"),
            "report should note warnings when error count > 0: {report}"
        );
        Ok(())
    }

    #[test]
    fn test_build_analysis_report_handles_zero_totals() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let stats_map: HashMap<PathBuf, DirectoryStats> = HashMap::new();
        let report = build_analysis_report(temp_dir.path(), &stats_map, 0, 0, 0);
        assert!(
            report.contains("Detailed source code analysis"),
            "report should always include table header: {report}"
        );
        assert!(
            report.contains("Totals by language:"),
            "report should include totals header even when empty: {report}"
        );
        assert!(
            !report.contains("Overall Summary"),
            "zero files/lines should skip overall summary: {report}"
        );
        assert!(
            !report.contains("Warning"),
            "zero errors should not emit warning section: {report}"
        );
        Ok(())
    }

    #[test]
    fn test_build_analysis_report_multiple_directories() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let current = temp_dir.path();

        let mut stats_map = HashMap::new();
        let src_dir = current.join("src");
        let docs_dir = current.join("docs");
        let outside_dir = PathBuf::from("C:\\outside");

        let mut src_stats = DirectoryStats::default();
        src_stats.language_stats.insert(
            "Rust".to_string(),
            (
                2,
                LanguageStats {
                    code_lines: 10,
                    comment_lines: 2,
                    blank_lines: 1,
                    overlap_lines: 0,
                },
            ),
        );
        src_stats.language_stats.insert(
            "Python".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 5,
                    comment_lines: 0,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );

        let mut docs_stats = DirectoryStats::default();
        docs_stats
            .language_stats
            .insert("Markdown".to_string(), (1, LanguageStats::default()));

        let mut outside_stats = DirectoryStats::default();
        outside_stats.language_stats.insert(
            "Shell".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 0,
                    comment_lines: 1,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );

        stats_map.insert(src_dir.clone(), src_stats);
        stats_map.insert(docs_dir.clone(), docs_stats);
        stats_map.insert(outside_dir.clone(), outside_stats);

        let report = build_analysis_report(current, &stats_map, 4, 13, 0);

        assert!(
            report.contains("src"),
            "expected relative directory to appear in report: {report}"
        );
        assert!(
            report.contains("docs"),
            "expected second relative directory in report: {report}"
        );
        assert!(
            report.contains("C:\\outside"),
            "absolute path should remain in report: {report}"
        );
        assert!(
            report.contains("Totals by language:"),
            "report should include totals header: {report}"
        );
        assert!(
            report.contains("Overall Summary"),
            "non-zero files should include overall summary: {report}"
        );
        assert!(
            !report.contains("Warning"),
            "zero error count should suppress warning section: {report}"
        );

        Ok(())
    }

    #[test]
    fn test_build_analysis_report_long_path_truncation() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let base = temp_dir.path();
        let long_dir =
            base.join("a_very_long_directory_name_that_exceeds_the_width_limit_for_display");

        fs::create_dir_all(&long_dir)?;

        let mut stats_map = HashMap::new();
        let mut dir_stats = DirectoryStats::default();
        dir_stats.language_stats.insert(
            "Rust".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 3,
                    comment_lines: 0,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );
        stats_map.insert(long_dir.clone(), dir_stats);

        let display = super::format_directory_display(&long_dir, base);
        assert!(
            display.starts_with("..."),
            "long directory display should be truncated with ellipsis: {display}"
        );
        assert!(
            display.chars().count() <= DIR_WIDTH,
            "truncated display should not exceed DIR_WIDTH: {display}"
        );

        let report = build_analysis_report(base, &stats_map, 1, 3, 0);
        assert!(
            report.contains(&display),
            "report should contain truncated directory display: {report}"
        );
        assert!(
            report.contains("Rust"),
            "report should include language totals for truncated directory: {report}"
        );
        Ok(())
    }

    #[test]
    fn test_build_analysis_report_language_ordering() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let mut stats_map = HashMap::new();

        let mut dir_stats = DirectoryStats::default();
        dir_stats.language_stats.insert(
            "Zig".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 4,
                    comment_lines: 0,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );
        dir_stats.language_stats.insert(
            "Ada".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 4,
                    comment_lines: 0,
                    blank_lines: 0,
                    overlap_lines: 0,
                },
            ),
        );
        stats_map.insert(temp_dir.path().to_path_buf(), dir_stats);

        let report = build_analysis_report(temp_dir.path(), &stats_map, 2, 8, 0);
        let ada_idx = report.find("Ada");
        let zig_idx = report.find("Zig");
        assert!(
            ada_idx.is_some() && zig_idx.is_some() && ada_idx < zig_idx,
            "languages with equal totals should appear alphabetically: {report}"
        );
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
        assert_eq!(stats.code_lines, 3, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 3, "stats: {:?}", stats);
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
        assert_eq!(stats.comment_lines, 8, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_javascript_prefers_line_comment_over_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "line_vs_block.js",
            "const value = 1; // comment /* not a block */\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("line_vs_block.js").as_path())?;
        assert_eq!(
            stats.code_lines, 1,
            "expected code before // counted: {stats:?}"
        );
        assert_eq!(
            stats.comment_lines, 1,
            "expected line comment counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_javascript_jsx_comment_with_prefix_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "jsx_prefix.js",
            "const header = '<div>'; <!-- comment --> const footer = '</div>';\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("jsx_prefix.js").as_path())?;
        assert!(
            stats.code_lines >= 2,
            "expected code before and after JSX comment: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 1,
            "expected JSX comment counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_javascript_blank_line_and_jsx_tail() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_mix.js",
            "// header comment\n\nconst view = () => <div>ok</div>; /* inline */\n<!-- jsx block\ncontinues --> <span>tail</span>\n",
        )?;
        let (stats, _total_lines) =
            count_javascript_lines(temp_dir.path().join("blank_mix.js").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank line counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected block and JSX comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before/after comments counted: {stats:?}"
        );
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

    #[test]
    fn test_pascal_blank_lines_and_comment_tails() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_tails.pas",
            "program Blank;\n\nbegin\nvalue := 1; { brace } tail;\nvalue := 2; (* paren *) tail2;\nend.\n",
        )?;
        let (stats, _total_lines) =
            count_pascal_lines(temp_dir.path().join("blank_tails.pas").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank line counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected brace and paren comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 4,
            "expected code before/after comments counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_language_blank_line_tracking() -> io::Result<()> {
        let cases: Vec<(
            &str,
            &str,
            fn(&Path) -> io::Result<(LanguageStats, u64)>,
            &str,
        )> = vec![
            (
                "blank.php",
                "<?php\n\n$foo = 1; /* block */\n/* open\ncontinues */\n?>\n",
                count_php_lines,
                "PHP",
            ),
            (
                "blank.rb",
                "#!/usr/bin/env ruby\n\n=begin\nblock\n=end\nputs 'done'\n",
                count_ruby_lines,
                "Ruby",
            ),
            (
                "blank.sh",
                "#!/bin/sh\n\n# comment\nprintf 'hi'\n",
                count_shell_lines,
                "Shell",
            ),
            (
                "blank.asm",
                "; leading comment\n\nmov ax, bx\n",
                count_asm_lines,
                "Assembly",
            ),
            (
                "blank.com",
                "$ SET DEFAULT\n\n! comment line\n$ EXIT\n",
                count_dcl_lines,
                "DCL",
            ),
            (
                "blank.bat",
                "@echo off\n\nREM comment\n:: alternate\n",
                count_batch_lines,
                "Batch",
            ),
            (
                "blank.tcl",
                "#!/usr/bin/env tclsh\n\n# comment\nputs {hi}\n",
                count_tcl_lines,
                "TCL",
            ),
            (
                "blank.xml",
                "<root>\n<!-- comment -->\n\n<child />\n</root>\n",
                count_xml_like_lines,
                "XML",
            ),
        ];

        for (file_name, contents, counter, label) in cases {
            let temp_dir = TempDir::new()?;
            create_test_file(temp_dir.path(), file_name, contents)?;
            let (stats, _total_lines) = counter(&temp_dir.path().join(file_name))?;
            assert!(
                stats.blank_lines >= 1,
                "{label} should count at least one blank line, stats: {stats:?}"
            );
            assert!(
                stats.code_lines + stats.comment_lines > 0,
                "{label} should classify non-blank content, stats: {stats:?}"
            );
        }
        Ok(())
    }

    // --- New Tests ---

    #[test]
    fn test_merge_directory_stats_accumulates() {
        let mut target = HashMap::new();
        let dir = PathBuf::from("some/dir");

        let mut first = DirectoryStats::default();
        first.language_stats.insert(
            "Rust".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 10,
                    comment_lines: 2,
                    blank_lines: 1,
                    overlap_lines: 0,
                },
            ),
        );
        merge_directory_stats(&mut target, dir.clone(), first);

        let mut second = DirectoryStats::default();
        second.language_stats.insert(
            "Rust".to_string(),
            (
                2,
                LanguageStats {
                    code_lines: 7,
                    comment_lines: 3,
                    blank_lines: 0,
                    overlap_lines: 1,
                },
            ),
        );
        second.language_stats.insert(
            "Python".to_string(),
            (
                1,
                LanguageStats {
                    code_lines: 5,
                    comment_lines: 1,
                    blank_lines: 2,
                    overlap_lines: 0,
                },
            ),
        );
        merge_directory_stats(&mut target, dir.clone(), second);

        let entry = target
            .get(&dir)
            .expect("merged directory stats should be present");
        let (rust_count, rust_stats) = entry
            .language_stats
            .get("Rust")
            .expect("rust stats should exist after merge");
        assert_eq!(*rust_count, 3);
        assert_eq!(rust_stats.code_lines, 17);
        assert_eq!(rust_stats.comment_lines, 5);
        assert_eq!(rust_stats.blank_lines, 1);
        assert_eq!(rust_stats.overlap_lines, 1);

        let (py_count, py_stats) = entry
            .language_stats
            .get("Python")
            .expect("python stats should be inserted");
        assert_eq!(*py_count, 1);
        assert_eq!(py_stats.code_lines, 5);
    }

    #[test]
    fn test_scan_directory_impl_handles_file_root() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        create_test_file(root, "single.rs", "fn main() {}\n// comment\n")?;

        let file_path = root.join("single.rs");
        let mut metrics = test_metrics();
        let mut entries_count = 0usize;
        let mut error_count = 0usize;
        let stats = scan_directory_impl(
            &file_path,
            &test_args(),
            root,
            &mut metrics,
            0,
            &mut entries_count,
            &mut error_count,
            None,
        )?;

        assert_eq!(error_count, 0);
        assert_eq!(entries_count, 1);
        let canonical_root = fs::canonicalize(root)?;
        let dir_stats = stats
            .get(root)
            .or_else(|| stats.get(&canonical_root))
            .expect("directory stats should be recorded");
        assert!(dir_stats.language_stats.contains_key("Rust"));
        Ok(())
    }

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
        assert_eq!(get_language_from_extension("README"), None);
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
    fn test_format_directory_display_variants() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let base = fs::canonicalize(temp_dir.path())?;
        let nested = base.join("nested");
        fs::create_dir_all(&nested)?;

        let display_root = format_directory_display(&base, &base);
        assert_eq!(display_root, ".");

        let display_nested = format_directory_display(&nested, &base);
        assert_eq!(display_nested, "nested");

        let external_dir = TempDir::new()?;
        let external = fs::canonicalize(external_dir.path())?;
        let display_external = format_directory_display(&external, &base);
        let tail = external
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        assert!(
            display_external.ends_with(tail),
            "display should include tail segment: {display_external}"
        );
        assert!(
            display_external.chars().count() <= DIR_WIDTH,
            "display should honor width limit: {display_external}"
        );

        Ok(())
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
    fn test_hcl_block_comment_resumes_code_before_hash() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "combo.tf",
            "resource \"x\" \"y\" {\n  primary = 1 /* block */ secondary = 2 # trailing hash\n}\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(temp_dir.path().join("combo.tf").as_path())?;
        assert!(
            stats.code_lines >= 4,
            "expected code before block, after block, and braces: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected block and hash comments counted: {stats:?}"
        );
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_multiline_then_hash_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_then_hash_line.tf",
            "resource \"x\" \"y\" {\n  /* block\n     still comment */\n  # trailing hash\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("block_then_hash_line.tf").as_path())?;
        assert!(
            stats.comment_lines >= 3,
            "expected block lines and hash comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected resource header and brace counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_before_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_then_line.tf",
            "resource \"x\" \"y\" {\n  attr = 1 /* block */ // trailing line comment\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("block_then_line.tf").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block and line comments: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before block and closing brace: {stats:?}"
        );
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_line_comment_with_code_after() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_line_code.tf",
            "resource \"x\" \"y\" {\n  attr = 1 /* block */ value = 2 // trailing line comment\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("block_line_code.tf").as_path())?;
        assert!(
            stats.code_lines >= 4,
            "expected code before block, after block, and braces: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected both block and line comments counted: {stats:?}"
        );
        assert_eq!(stats.blank_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_blank_lines_are_counted() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_lines.tf",
            "resource \"x\" \"y\" {\n\n  value = 1\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("blank_lines.tf").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank separator to count as blank: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "resource header and assignment should count as code: {stats:?}"
        );
        assert_eq!(stats.comment_lines, 0, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_inline_code_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_block_line.tf",
            "value = 1 /* block */ value2 // trailing line comment\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("inline_block_line.tf").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block and line comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before and after block counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_inline_code_then_hash_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_block_hash.tf",
            "value = 1 /* block */ value2 # trailing hash comment\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("inline_block_hash.tf").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block and hash comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before and after block counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_block_comment_inline_code_then_doc_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_block_doc.tf",
            "value = 1 /* block */ value2 ## trailing doc\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("inline_block_doc.tf").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block and doc comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before and after block counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_multiline_block_close_trailing_code_and_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_close_code_line.tf",
            "resource \"x\" \"y\" {\n  attr = 1 /* block\n     still comment */ value = 2 // trailing line comment\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("block_close_code_line.tf").as_path())?;
        assert!(
            stats.code_lines >= 4,
            "expected resource, assignments, and closing brace counted as code: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 3,
            "expected block open, block close, and trailing line comment counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_multiline_block_close_trailing_comment_variants() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_close_comment_variants.tf",
            "/* doc block\n   continues */ ## doc comment\n/* another block\n   runs */ // trailing line comment\n/* hash block\n   persists */ # trailing hash\nresource \"x\" \"y\" {}\n",
        )?;
        let (stats, _total_lines) = count_hcl_lines(
            temp_dir
                .path()
                .join("block_close_comment_variants.tf")
                .as_path(),
        )?;
        assert!(
            stats.comment_lines >= 9,
            "expected each block open/close and trailing comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected resource declaration counted as code: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_hcl_line_comment_precedes_hash_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "line_then_hash.tf",
            "resource \"x\" \"y\" {\n  attr = 1 // primary # trailing\n}\n",
        )?;
        let (stats, _total_lines) =
            count_hcl_lines(temp_dir.path().join("line_then_hash.tf").as_path())?;
        assert!(
            stats.comment_lines >= 1,
            "expected line comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "resource header and assignment should count as code: {stats:?}"
        );
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
    fn test_velocity_line_counting_blank_and_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_blank.vm",
            "Hello\n\n#* block start\nstill comment\n*# tail code\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template_blank.vm").as_path())?;
        assert_eq!(
            stats.blank_lines, 1,
            "expected single blank separator: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 3,
            "expected multiline block comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected initial line and tail code after block: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_line_counting_block_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_trailing.vm",
            "Hello #* block *# ## trailing\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template_trailing.vm").as_path())?;
        assert!(
            stats.code_lines >= 1,
            "expected leading code counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected block and trailing line comment: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_line_counting_multiline_block_then_line_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_line.vm",
            "Hello\n#* block start\nstill comment\n*# ## trailing\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template_block_line.vm").as_path())?;
        assert!(
            stats.comment_lines >= 3,
            "expected block lines and trailing line comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected top-level code line counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_multiline_block_closes_without_trailing() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_only.vm",
            "Hello\n#* block start\nstill comment\n*#   \nValue\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template_block_only.vm").as_path())?;
        assert!(
            stats.comment_lines >= 3,
            "expected block comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected outer code lines counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_block_only_line_with_whitespace() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_whitespace.vm",
            "#* comment-only block *#   \nNext\n",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_whitespace.vm")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected single block comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected next line of code counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_inline_block_without_trailing() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_inline.vm",
            "#* inline block *#\nValue\n",
        )?;
        let (stats, _total_lines) =
            count_velocity_lines(temp_dir.path().join("template_block_inline.vm").as_path())?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected single block comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected following code line counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_inline_block_with_whitespace_tail_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_inline_ws_tail.vm",
            "Hello #* inline block *#   ",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_inline_ws_tail.vm")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected block comment counted once: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 1,
            "expected only leading code counted when trailing tail is whitespace: {stats:?}"
        );
        assert_eq!(
            stats.blank_lines, 0,
            "expected no blank lines in single-line inline block: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_inline_block_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_inline_tail.vm",
            "Hello #* inline block *#Tail\n",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_inline_tail.vm")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected inline block counted exactly once: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 2,
            "expected both leading and trailing code counted when tail has code: {stats:?}"
        );
        assert_eq!(stats.blank_lines, 0, "unexpected blank lines: {stats:?}");
        Ok(())
    }

    #[test]
    fn test_velocity_code_before_block_with_whitespace_tail() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_code_whitespace.vm",
            "Hello #* comment *#   \nNext\n",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_code_whitespace.vm")
                .as_path(),
        )?;
        assert!(
            stats.code_lines >= 2,
            "expected leading and trailing code counted: {stats:?}"
        );
        assert_eq!(
            stats.comment_lines, 1,
            "expected block comment counted once: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_multiline_block_closes_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_close_code.vm",
            "#* block start\nstill comment\n*# Tail\n",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_close_code.vm")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 3,
            "expected block lines counted as comments: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 1,
            "expected trailing code after block counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_velocity_multiline_block_closes_with_trailing_comment() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "template_block_close_comment.vm",
            "#* block start\nstill comment\n*#   ## trailing\n",
        )?;
        let (stats, _total_lines) = count_velocity_lines(
            temp_dir
                .path()
                .join("template_block_close_comment.vm")
                .as_path(),
        )?;
        assert_eq!(
            stats.comment_lines, 4,
            "expected block lines plus trailing line comment counted: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 0,
            "expected no code counted when trailing comment consumes line: {stats:?}"
        );
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
    fn test_mustache_line_counting_blank_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view_blank.mustache",
            "Hello {{name}}\n\n{{! trailing }}\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("view_blank.mustache").as_path())?;
        assert_eq!(
            stats.blank_lines, 1,
            "expected blank line counted: {stats:?}"
        );
        assert_eq!(
            stats.comment_lines, 1,
            "expected comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected code line counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_mustache_line_counting_comment_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view_trailing.mustache",
            "{{! comment }} tail\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("view_trailing.mustache").as_path())?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected comment counted: {stats:?}"
        );
        assert_eq!(
            stats.code_lines, 1,
            "expected trailing code counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_mustache_comment_only_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "comment_only.mustache",
            "{{! comment only }}\nHello\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("comment_only.mustache").as_path())?;
        assert_eq!(
            stats.comment_lines, 1,
            "expected lone comment line counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected subsequent code counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_mustache_inline_comment_with_surrounding_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view_inline.mustache",
            "prefix {{! inline note }} suffix\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("view_inline.mustache").as_path())?;
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert!(
            stats.code_lines >= 2,
            "expected code counted on both sides of inline comment: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_mustache_multiline_comment_with_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view_block.mustache",
            "{{! start\ncontinues\n}} tail\n",
        )?;
        let (stats, _total_lines) =
            count_mustache_lines(temp_dir.path().join("view_block.mustache").as_path())?;
        assert!(
            stats.comment_lines >= 3,
            "expected each line of the block comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected trailing code after block close counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_mustache_multiline_comment_without_trailing_code() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "view_block_no_tail.mustache",
            "{{! start\ncontinues\n}}\nHello\n",
        )?;
        let (stats, _total_lines) = count_mustache_lines(
            temp_dir
                .path()
                .join("view_block_no_tail.mustache")
                .as_path(),
        )?;
        assert!(
            stats.comment_lines >= 3,
            "expected block comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 1,
            "expected code after block on next line counted: {stats:?}"
        );
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
    fn test_cstyle_line_comment_counts_code_prefix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "line_comment.c",
            "int value = 42; // trailing comment\n",
        )?;
        let (stats, _total_lines) =
            count_c_style_lines(temp_dir.path().join("line_comment.c").as_path())?;
        assert_eq!(
            stats.code_lines, 1,
            "expected code before // counted: {stats:?}"
        );
        assert_eq!(
            stats.comment_lines, 1,
            "expected trailing // counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_cstyle_block_then_line_unterminated() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "block_unterminated.c",
            "int start = 0; /* begin // still comment\n*/ int done = 1;\n",
        )?;
        let (stats, _total_lines) =
            count_c_style_lines(temp_dir.path().join("block_unterminated.c").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected multi-line block comment recorded: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before and after block recorded: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_cstyle_blank_line_and_unterminated_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_block.c",
            "int a = 0;\n\n/* block starts\nstill comment\n*/ int b = 1;\n",
        )?;
        let (stats, _total_lines) =
            count_c_style_lines(temp_dir.path().join("blank_block.c").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank line counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected multi-line block comment counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before/after block counted: {stats:?}"
        );
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
    fn test_filespec_matches_outside_root_returns_false() -> io::Result<()> {
        let root = TempDir::new()?;
        let external = TempDir::new()?;
        let orphan_dir = external.path().join("orphan");
        fs::create_dir_all(&orphan_dir)?;
        create_test_file(&orphan_dir, "main.rs", "fn orphan() {}\n")?;

        let pattern = Pattern::new("src/**/*.rs").expect("glob compiles");
        let file_path = orphan_dir.join("main.rs");

        assert!(
            !filespec_matches(&pattern, root.path(), &file_path),
            "files outside the root should not match by relative pattern"
        );

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_filespec_matches_invalid_utf_path() -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;

        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        let pattern = Pattern::new("*.txt").expect("glob compiles");
        let bytes = [0xFFu8, b'n', b'o', b't', b'e', b'.', b't', b'x', b't'];
        let os_name = std::ffi::OsStr::from_bytes(&bytes);
        let file_path = root.join(os_name);
        File::create(&file_path)?;

        assert!(
            !filespec_matches(&pattern, root, &file_path),
            "invalid UTF path should not match pattern"
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
    fn test_powershell_blank_line_and_multiline_block() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_block.ps1",
            "Write-Host \"start\"\n\n<# open\nstill comment\n#>\nWrite-Host \"after\"\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("blank_block.ps1").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank line counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 2,
            "expected block comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before/after block counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_powershell_block_and_line_comment_without_close() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "line_block.ps1",
            "Write-Host 1 <# start block # trailing\n#> Write-Host 2\n",
        )?;
        let (stats, _total_lines) =
            count_powershell_lines(temp_dir.path().join("line_block.ps1").as_path())?;
        assert!(
            stats.comment_lines >= 2,
            "expected block and line comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected code before and after comments counted: {stats:?}"
        );
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
    fn test_algol_blank_and_hash_comment_mix() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.alg",
            "begin\n\n# hash comment\nco inline co\nCOMMENT block\nstill comment;\nend\n",
        )?;
        let (stats, _total) = count_algol_lines(temp_dir.path().join("mixed.alg").as_path())?;
        assert_eq!(
            stats.blank_lines, 1,
            "expected single blank line: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 4,
            "expected hash/co/block comment lines counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected begin/end counted as code: {stats:?}"
        );
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
    fn test_cobol_blank_and_free_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.cob",
            "       IDENTIFICATION DIVISION.\n\n      * Column seven star\n      *> free comment\n       PROGRAM-ID. SAMPLE.\n",
        )?;
        let (stats, _total) = count_cobol_lines(temp_dir.path().join("mixed.cob").as_path())?;
        assert_eq!(stats.blank_lines, 1, "expected blank separator: {stats:?}");
        assert!(
            stats.comment_lines >= 2,
            "expected fixed and free-form comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 2,
            "expected identification and program id lines counted: {stats:?}"
        );
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
    fn test_fortran_blank_and_legacy_comments() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "mixed.f90",
            "C legacy comment\n      PROGRAM TEST\n\n      ! full line\n      INTEGER :: X ! inline comment\n      X = 3\n      END PROGRAM TEST\n",
        )?;
        let (stats, _total) = count_fortran_lines(temp_dir.path().join("mixed.f90").as_path())?;
        assert_eq!(
            stats.blank_lines, 1,
            "expected single blank line: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 3,
            "expected legacy, full-line, and inline comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 3,
            "expected program, declaration, and assignment counted: {stats:?}"
        );
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
    fn test_iplan_blank_line_and_block_tail() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "blank_block.ipl",
            "SET BASE = 1\n\n/* start\ncontinues */ VALUE\n/* reopen\nstill comment\n*/ VALUE2\n! trailing\n",
        )?;
        let (stats, _total_lines) =
            count_iplan_lines(temp_dir.path().join("blank_block.ipl").as_path())?;
        assert!(
            stats.blank_lines >= 1,
            "expected blank line counted: {stats:?}"
        );
        assert!(
            stats.comment_lines >= 3,
            "expected multi-line block comments counted: {stats:?}"
        );
        assert!(
            stats.code_lines >= 3,
            "expected code after block comments counted: {stats:?}"
        );
        Ok(())
    }

    #[test]
    fn test_iplan_block_trailing_code_on_same_line() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(
            temp_dir.path(),
            "inline_tail.ipl",
            "/* header */ VALUE1\nVALUE2 /* close */ VALUE3\n",
        )?;
        let (stats, _total_lines) =
            count_iplan_lines(temp_dir.path().join("inline_tail.ipl").as_path())?;
        assert_eq!(
            stats.comment_lines, 2,
            "expected two comment lines: {stats:?}"
        );
        assert!(
            stats.code_lines >= 3,
            "expected code detected before and after comments: {stats:?}"
        );
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
        let (stats, total) = count_toml_lines(temp_dir.path().join("sample.toml").as_path())?;
        assert_eq!(total, 3);
        assert_eq!(stats.code_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_toml_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "comment.toml", "# header\n# detail\n")?;
        let (stats, total) = count_toml_lines(temp_dir.path().join("comment.toml").as_path())?;
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
        let (stats, total) = count_yaml_lines(temp_dir.path().join("sample.yaml").as_path())?;
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
        let (stats, total) = count_yaml_lines(temp_dir.path().join("comment.yaml").as_path())?;
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
        let (stats, total) = count_makefile_lines(temp_dir.path().join("Makefile").as_path())?;
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
        let (stats, total) = count_dockerfile_lines(temp_dir.path().join("Dockerfile").as_path())?;
        assert_eq!(total, 4);
        assert_eq!(stats.code_lines, 2, "stats: {:?}", stats);
        assert_eq!(stats.comment_lines, 1, "stats: {:?}", stats);
        assert_eq!(stats.blank_lines, 1, "stats: {:?}", stats);
        Ok(())
    }

    #[test]
    fn test_dockerfile_comment_only() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(temp_dir.path(), "Dockerfile", "# comment\n# another\n")?;
        let (stats, total) = count_dockerfile_lines(temp_dir.path().join("Dockerfile").as_path())?;
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
