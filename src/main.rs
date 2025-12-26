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
use std::collections::{HashMap, HashSet};
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
use terminal_size::{terminal_size, Width};

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
const CODE_ROLE_COUNT: usize = 2;

struct PerformanceMetrics {
    files_processed: Arc<AtomicU64>,
    lines_processed: Arc<AtomicU64>,
    start_time: Instant,
    last_update: Instant,
    writer: Box<dyn Write + Send>,
    progress_enabled: bool,
    role_files: [AtomicU64; CODE_ROLE_COUNT],
    role_lines: [AtomicU64; CODE_ROLE_COUNT],
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Source code analyser for multiple programming languages",
    long_about = "Supported languages: Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript, PHP, Perl, Ruby, Shell, Pascal, Scala, YAML, XML, JSON, HTML, TOML, Makefile, Dockerfile, INI, HCL, CMake, PowerShell, Batch, TCL, ReStructuredText, Velocity, Mustache, Protobuf, SVG, XSL, Algol, COBOL, Fortran, Assembly, DCL, IPLAN, mdhavers.",
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

    #[arg(short = 'r', long)]
    role_breakdown: bool,

    #[arg(short = 'l', long)]
    languages: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct LanguageStats {
    code_lines: u64,
    comment_lines: u64,
    blank_lines: u64,
    overlap_lines: u64,
}

impl LanguageStats {
    fn add_assign(&mut self, other: &LanguageStats) {
        self.code_lines += other.code_lines;
        self.comment_lines += other.comment_lines;
        self.blank_lines += other.blank_lines;
        self.overlap_lines += other.overlap_lines;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileRoleHint {
    Unknown,
    TestFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CodeRole {
    Mainline = 0,
    Test = 1,
}

impl CodeRole {
    const ALL: [CodeRole; CODE_ROLE_COUNT] = [CodeRole::Mainline, CodeRole::Test];

    fn as_index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            CodeRole::Mainline => "Mainline",
            CodeRole::Test => "Test",
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RoleBucket {
    stats: LanguageStats,
    total_lines: u64,
}

#[derive(Debug, Clone)]
struct RoleSplit {
    buckets: [Option<RoleBucket>; CODE_ROLE_COUNT],
    total_lines: u64,
}

impl Default for RoleSplit {
    fn default() -> Self {
        Self {
            buckets: [None; CODE_ROLE_COUNT],
            total_lines: 0,
        }
    }
}

impl RoleSplit {
    fn single(role: CodeRole, stats: LanguageStats, total_lines: u64) -> Self {
        let mut split = RoleSplit::default();
        split.push(role, stats, total_lines);
        split
    }

    fn push(&mut self, role: CodeRole, stats: LanguageStats, total_lines: u64) {
        self.buckets[role.as_index()] = Some(RoleBucket { stats, total_lines });
        self.total_lines += total_lines;
    }

    fn iter(&self) -> impl Iterator<Item = (CodeRole, RoleBucket)> + '_ {
        CodeRole::ALL
            .iter()
            .copied()
            .filter_map(|role| self.buckets[role.as_index()].map(|bucket| (role, bucket)))
    }

    #[cfg(test)]
    fn bucket(&self, role: CodeRole) -> Option<RoleBucket> {
        self.buckets[role.as_index()]
    }

    fn total_lines(&self) -> u64 {
        self.total_lines
    }

    fn role_count(&self) -> usize {
        CodeRole::ALL
            .iter()
            .filter(|role| self.buckets[role.as_index()].is_some())
            .count()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RoleStats {
    files: u64,
    totals: LanguageStats,
}

impl RoleStats {
    fn add_file(&mut self, stats: &LanguageStats) {
        self.files += 1;
        self.totals.add_assign(stats);
    }

    #[cfg(test)]
    fn add_aggregate(&mut self, files: u64, stats: &LanguageStats) {
        self.files += files;
        self.totals.add_assign(stats);
    }

    fn merge(&mut self, other: &RoleStats) {
        self.files += other.files;
        self.totals.add_assign(&other.totals);
    }
}

#[derive(Debug, Default, Clone)]
struct LanguageEntry {
    per_role: [RoleStats; CODE_ROLE_COUNT],
    total_files: u64,
}

impl LanguageEntry {
    fn record_roles(&mut self, role_stats: &[(CodeRole, LanguageStats)]) {
        if role_stats.is_empty() {
            return;
        }
        self.total_files += 1;
        for (role, stats) in role_stats {
            self.per_role[role.as_index()].add_file(stats);
        }
    }

    #[cfg(test)]
    fn record_aggregate(&mut self, role: CodeRole, files: u64, stats: LanguageStats) {
        self.total_files += files;
        self.per_role[role.as_index()].add_aggregate(files, &stats);
    }

    fn absorb(&mut self, other: LanguageEntry) {
        self.total_files += other.total_files;
        for role in CodeRole::ALL {
            self.per_role[role.as_index()].merge(&other.per_role[role.as_index()]);
        }
    }

    fn total_files(&self) -> u64 {
        self.total_files
    }

    fn total_stats(&self) -> LanguageStats {
        let mut totals = LanguageStats::default();
        for role in &self.per_role {
            totals.add_assign(&role.totals);
        }
        totals
    }

    fn summary(&self) -> (u64, LanguageStats) {
        (self.total_files(), self.total_stats())
    }

    fn role_summary(&self, role: CodeRole) -> Option<(u64, LanguageStats)> {
        let role_stats = &self.per_role[role.as_index()];
        if role_stats.files == 0 {
            None
        } else {
            Some((role_stats.files, role_stats.totals))
        }
    }
}

#[derive(Debug, Default)]
struct DirectoryStats {
    language_stats: HashMap<String, LanguageEntry>,
}

fn infer_role_from_path(root_path: &Path, file_path: &Path) -> FileRoleHint {
    // Prefer path-based hints first (e.g., files under tests/ directories)
    if let Ok(relative) = file_path.strip_prefix(root_path) {
        for component in relative.components() {
            if let std::path::Component::Normal(name) = component {
                let is_tests_dir =
                    name.eq_ignore_ascii_case("tests") || name.eq_ignore_ascii_case("__tests__");
                let is_testdata_rust = name.eq_ignore_ascii_case("testdata")
                    && file_path.extension().and_then(|e| e.to_str()) == Some("rs");
                if is_tests_dir || is_testdata_rust {
                    return FileRoleHint::TestFile;
                }
            }
        }
    }

    if let Some(file_name) = file_path.file_name().and_then(|n| n.to_str()) {
        let lower = file_name.to_lowercase();
        if lower.starts_with("test_")
            || lower.ends_with("_test.rs")
            || lower.ends_with("_test.py")
            || lower.ends_with("_test.go")
            || lower.ends_with("_test.ts")
            || lower.contains(".test.")
            || lower.contains(".spec.")
        {
            return FileRoleHint::TestFile;
        }
    }

    FileRoleHint::Unknown
}

#[derive(Clone, Copy)]
enum StringMode {
    Normal(char),
    Raw(usize),
}

#[derive(Default)]
struct BraceScanState {
    in_block_comment: bool,
    string_mode: Option<StringMode>,
}

impl BraceScanState {
    fn scan_line(&mut self, line: &str, tracker: &mut RustRoleTracker) {
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if self.in_block_comment {
                if c == '*' && matches!(chars.peek(), Some('/')) {
                    self.in_block_comment = false;
                    chars.next();
                }
                continue;
            }
            if let Some(mode) = &mut self.string_mode {
                match mode {
                    StringMode::Normal(delim) => {
                        if c == '\\' {
                            chars.next();
                            continue;
                        }
                        if c == *delim {
                            self.string_mode = None;
                        }
                    }
                    StringMode::Raw(hashes) => {
                        if c == '"' {
                            let mut remaining = *hashes;
                            let mut matched = true;
                            while remaining > 0 {
                                if matches!(chars.peek(), Some('#')) {
                                    chars.next();
                                    remaining -= 1;
                                } else {
                                    matched = false;
                                    break;
                                }
                            }
                            if matched {
                                self.string_mode = None;
                            }
                        }
                    }
                }
                continue;
            }
            match c {
                '/' => {
                    if matches!(chars.peek(), Some('/')) {
                        break;
                    } else if matches!(chars.peek(), Some('*')) {
                        self.in_block_comment = true;
                        chars.next();
                    }
                }
                '"' => {
                    self.string_mode = Some(StringMode::Normal('"'));
                }
                '\'' => {
                    self.string_mode = Some(StringMode::Normal('\''));
                }
                'r' => {
                    let mut clone = chars.clone();
                    let mut hashes = 0usize;
                    while matches!(clone.peek(), Some('#')) {
                        hashes += 1;
                        clone.next();
                    }
                    if matches!(clone.peek(), Some('"')) {
                        for _ in 0..hashes {
                            chars.next();
                        }
                        if matches!(chars.peek(), Some('"')) {
                            chars.next();
                        }
                        self.string_mode = Some(StringMode::Raw(hashes));
                    }
                }
                '{' => tracker.open_scope(),
                '}' => tracker.close_scope(),
                _ => {}
            }
        }
    }
}

#[derive(Default)]
struct RustRoleTracker {
    scope_stack: Vec<CodeRole>,
    pending_scope_role: Option<CodeRole>,
    pending_line_role: Option<CodeRole>,
}

impl RustRoleTracker {
    fn new(hint: FileRoleHint) -> Self {
        let base_role = if matches!(hint, FileRoleHint::TestFile) {
            CodeRole::Test
        } else {
            CodeRole::Mainline
        };
        Self {
            scope_stack: vec![base_role],
            pending_scope_role: None,
            pending_line_role: None,
        }
    }

    fn current_role(&self) -> CodeRole {
        *self.scope_stack.last().unwrap()
    }

    fn mark_pending_test(&mut self) {
        let role = CodeRole::Test;
        self.pending_scope_role = Some(role);
        self.pending_line_role = Some(role);
    }

    fn take_line_role(&mut self) -> Option<CodeRole> {
        self.pending_line_role.take()
    }

    fn clear_pending_scope(&mut self) {
        self.pending_scope_role = None;
    }

    fn open_scope(&mut self) {
        let role = self
            .pending_scope_role
            .take()
            .unwrap_or_else(|| self.current_role());
        self.scope_stack.push(role);
    }

    fn close_scope(&mut self) {
        if self.scope_stack.len() > 1 {
            self.scope_stack.pop();
        }
    }
}

fn attribute_indicates_test(attr: &str) -> bool {
    let lower = attr.trim().to_ascii_lowercase();
    if lower.starts_with("#[cfg(") {
        if lower.contains("not(test") {
            return false;
        }
        return lower.contains("test");
    }
    lower.starts_with("#[test") || lower.contains("::test]")
}

fn detect_rust_line_roles(lines: &[String], hint: FileRoleHint) -> Vec<CodeRole> {
    let mut tracker = RustRoleTracker::new(hint);
    let mut brace_state = BraceScanState::default();
    let mut roles = Vec::with_capacity(lines.len());
    for line in lines {
        let trimmed = line.trim();
        let mut role = if trimmed.is_empty() {
            tracker.current_role()
        } else {
            tracker
                .take_line_role()
                .unwrap_or_else(|| tracker.current_role())
        };
        if trimmed.starts_with("#[") && attribute_indicates_test(trimmed) {
            tracker.mark_pending_test();
            role = CodeRole::Test;
        }
        roles.push(role);
        if tracker.pending_scope_role.is_some() && trimmed.ends_with(';') && !trimmed.contains('{')
        {
            tracker.clear_pending_scope();
        }
        brace_state.scan_line(line, &mut tracker);
    }
    roles
}

// Internal processing context to shorten repetitive call sites in scanning.
struct ProcCtx<'a> {
    args: &'a Args,
    root_path: &'a Path,
    metrics: &'a mut PerformanceMetrics,
    stats: &'a mut HashMap<PathBuf, DirectoryStats>,
    error_count: &'a mut usize,
    filespec: Option<&'a Pattern>,
    visited_real_paths: &'a mut HashSet<PathBuf>,
}

fn process_entry_file(ctx: &mut ProcCtx<'_>, p: &Path) -> io::Result<()> {
    process_file(
        p,
        ctx.args,
        ctx.root_path,
        ctx.metrics,
        ctx.stats,
        ctx.error_count,
        ctx.filespec,
        ctx.visited_real_paths,
    )
}

fn handle_symlink(ctx: &mut ProcCtx<'_>, entry_path: &Path) -> io::Result<()> {
    match fetch_metadata(entry_path) {
        Ok(target_metadata) => {
            if target_metadata.is_dir() {
                if ctx.args.verbose {
                    println!("Skipping symlinked directory: {}", entry_path.display());
                }
                Ok(())
            } else if target_metadata.is_file() {
                process_entry_file(ctx, entry_path)
            } else {
                Ok(())
            }
        }
        Err(err) => {
            eprintln!(
                "Error resolving metadata for symlink {}: {}",
                entry_path.display(),
                err
            );
            *ctx.error_count += 1;
            Ok(())
        }
    }
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
    match target.get_mut(&dir) {
        Some(existing) => {
            for (lang, entry) in stat.language_stats {
                existing
                    .language_stats
                    .entry(lang)
                    .or_default()
                    .absorb(entry);
            }
        }
        None => {
            target.insert(dir, stat);
        }
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
            role_files: std::array::from_fn(|_| AtomicU64::new(0)),
            role_lines: std::array::from_fn(|_| AtomicU64::new(0)),
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
            "\rProcessed {} files ({} files/sec) and {} lines ({} lines/sec)...",
            format_number(files),
            format_rate(files as f64 / elapsed),
            format_number(lines),
            format_rate(lines as f64 / elapsed)
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
            format_number(files).bright_yellow(),
            format!("{} files/sec", format_rate(safe_rate(files, elapsed))).bright_yellow()
        );
        let _ = writeln!(
            writer,
            "Lines processed: {} ({})",
            format_number(lines).bright_yellow(),
            format!("{} lines/sec", format_rate(safe_rate(lines, elapsed))).bright_yellow()
        );
    }

    fn record_role(&self, role: CodeRole, lines: u64) {
        self.role_files[role.as_index()].fetch_add(1, Ordering::Relaxed);
        self.role_lines[role.as_index()].fetch_add(lines, Ordering::Relaxed);
    }

    fn role_counters(&self) -> [(u64, u64); CODE_ROLE_COUNT] {
        std::array::from_fn(|idx| {
            (
                self.role_files[idx].load(Ordering::Relaxed),
                self.role_lines[idx].load(Ordering::Relaxed),
            )
        })
    }

    fn has_role_data(&self) -> bool {
        self.role_files
            .iter()
            .zip(&self.role_lines)
            .any(|(files, lines)| {
                files.load(Ordering::Relaxed) > 0 || lines.load(Ordering::Relaxed) > 0
            })
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

fn read_file_lines_vec(file_path: &Path) -> io::Result<Vec<String>> {
    read_file_lines_lossy(file_path)?.collect()
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
        "dart" => Some("Dart"),
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
        // mdhavers (Scots programming language)
        "braw" => Some("mdhavers"),
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

fn format_number(n: u64) -> String {
    let s = n.to_string();
    if s.len() < 4 {
        return s;
    }
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    let offset = s.len() % 3;
    if offset > 0 {
        result.push_str(&s[..offset]);
        result.push(',');
    }
    for (i, c) in s[offset..].chars().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

fn format_rate(rate: f64) -> String {
    let s = format!("{:.1}", rate);
    if let Some((integer, decimal)) = s.split_once('.') {
        if integer.len() > 3 {
            let mut result = String::with_capacity(s.len() + s.len() / 3);
            let offset = integer.len() % 3;
            if offset > 0 {
                result.push_str(&integer[..offset]);
                result.push(',');
            }
            for (i, c) in integer[offset..].chars().enumerate() {
                if i > 0 && i % 3 == 0 {
                    result.push(',');
                }
                result.push(c);
            }
            result.push('.');
            result.push_str(decimal);
            return result;
        }
    }
    s
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
        return Err(io::Error::other("simulated metadata read failure"));
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
        let injected_error = inject_entry_error
            .then(|| io::Error::other("simulated directory entry iteration failure"));
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
        return Err(io::Error::other("simulated read_dir failure"));
    }
    let iter = fs::read_dir(path)?;
    Ok(ReadDirStream::new(
        iter,
        should_simulate_path_failure(path, ENTRY_ITER_FAIL_TAG),
    ))
}

fn entry_file_type(entry: &fs::DirEntry) -> io::Result<fs::FileType> {
    if should_simulate_entry_failure(entry, FILE_TYPE_FAIL_TAG) {
        return Err(io::Error::other("simulated file_type failure"));
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
        "dart" => count_c_style_lines(file_path),
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
        // mdhavers uses # for comments (like Python/Shell)
        "braw" => count_mdhavers_lines(file_path),
        _ => count_generic_lines(file_path),
    }
}

fn count_lines_with_roles(file_path: &Path, role_hint: FileRoleHint) -> io::Result<RoleSplit> {
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    if extension == "rs" {
        return count_rust_lines_role_aware(file_path, role_hint);
    }
    // TODO: Extend with Go/Python/JS-specific role splits once heuristics mature.
    let (stats, total_lines) = count_lines_with_stats(file_path)?;
    Ok(RoleSplit::single(CodeRole::Mainline, stats, total_lines))
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
                let after_comment = trimmed.split("*/").nth(1).unwrap_or("");
                if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                    stats.code_lines += 1;
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
            let before_comment = trimmed.split("/*").next().unwrap_or("");
            if !before_comment.trim().is_empty() {
                stats.code_lines += 1;
            }
            if !trimmed.contains("*/") {
                in_block_comment = true;
            } else {
                let after_comment = trimmed.split("*/").nth(1).unwrap_or("");
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

/// Count lines for mdhavers (.braw files) - a Scots programming language.
/// mdhavers uses # for single-line comments (like Python/Shell).
/// https://github.com/0x4d44/mdhavers
fn count_mdhavers_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
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

fn count_rust_lines_role_aware(file_path: &Path, hint: FileRoleHint) -> io::Result<RoleSplit> {
    let lines = read_file_lines_vec(file_path)?;
    if lines.is_empty() {
        let default_role = if matches!(hint, FileRoleHint::TestFile) {
            CodeRole::Test
        } else {
            CodeRole::Mainline
        };
        return Ok(RoleSplit::single(default_role, LanguageStats::default(), 0));
    }
    let roles = detect_rust_line_roles(&lines, hint);
    let mut stats_per_role = [LanguageStats::default(); CODE_ROLE_COUNT];
    let mut in_block_comment = false;
    for (line, &role) in lines.iter().zip(roles.iter()) {
        let trimmed = line.trim();
        let bucket = &mut stats_per_role[role.as_index()];
        if trimmed.is_empty() {
            bucket.blank_lines += 1;
            continue;
        }
        if in_block_comment {
            bucket.comment_lines += 1;
            if let Some(end) = trimmed.find("*/") {
                in_block_comment = false;
                let rest = trimmed[end + 2..].trim();
                if rest.is_empty() {
                    continue;
                }
            } else {
                continue;
            }
        }
        if trimmed.starts_with("#[") {
            bucket.code_lines += 1;
            continue;
        }
        if let Some(pos) = trimmed.find("/*") {
            bucket.comment_lines += 1;
            let before = &trimmed[..pos];
            if !before.trim().is_empty() {
                bucket.code_lines += 1;
            }
            if !trimmed.contains("*/") {
                in_block_comment = true;
            } else {
                let after_comment = trimmed.split("*/").nth(1).unwrap_or("");
                if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                    bucket.code_lines += 1;
                }
            }
            continue;
        }
        if trimmed.starts_with("///") || trimmed.starts_with("//!") || trimmed.starts_with("//") {
            bucket.comment_lines += 1;
            continue;
        }
        bucket.code_lines += 1;
    }
    let mut split = RoleSplit::default();
    for role in CodeRole::ALL {
        let stats = stats_per_role[role.as_index()];
        if stats.code_lines + stats.comment_lines + stats.blank_lines > 0 {
            let role_total = stats.code_lines + stats.comment_lines + stats.blank_lines;
            split.push(role, stats, role_total);
        }
    }
    Ok(split)
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
                let code = trimmed.split(&quote).nth(1).unwrap_or("");
                if !code.trim().is_empty() && !code.trim_start().starts_with("#") {
                    stats.code_lines += 1;
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
                let code = trimmed.split(quote).nth(2).unwrap_or("");
                if !code.trim().is_empty() && !code.trim_start().starts_with("#") {
                    stats.code_lines += 1;
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
            let close = trimmed.find("*/");
            if close.is_some() {
                in_block_comment = false;
            }
            let after_comment = close.map(|end| &trimmed[end + 2..]).unwrap_or("");
            if !after_comment.trim().is_empty() && !after_comment.trim().starts_with("//") {
                stats.code_lines += 1;
            }
            continue;
        }
        if in_jsx_comment {
            stats.comment_lines += 1;
            let close = trimmed.find("-->");
            if close.is_some() {
                in_jsx_comment = false;
            }
            let after_comment = close.map(|end| &trimmed[end + 3..]).unwrap_or("");
            if !after_comment.trim().is_empty() {
                stats.code_lines += 1;
            }
            continue;
        }
        let line_pos = trimmed.find("//");
        let block_pos = trimmed.find("/*");
        let jsx_pos = trimmed.find("<!--");

        if let Some(pl) = line_pos {
            if block_pos.is_none_or(|pb| pl < pb) && jsx_pos.is_none_or(|pj| pl < pj) {
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
            let close = after.find("*/");
            if close.is_none() {
                in_block_comment = true;
            }
            let trailing = close.map(|end| &after[(end + 2)..]).unwrap_or("");
            if !trailing.trim().is_empty()
                && !trailing.trim_start().starts_with("//")
                && !trailing.trim_start().starts_with("<#")
            {
                stats.code_lines += 1;
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
            let close = after.find("-->");
            if close.is_none() {
                in_jsx_comment = true;
            }
            let trailing = close.map(|end| &after[(end + 3)..]).unwrap_or("");
            if !trailing.trim().is_empty() {
                stats.code_lines += 1;
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
                let code_trimmed = trimmed
                    .split("*/")
                    .nth(1)
                    .map(|s| s.trim_start())
                    .unwrap_or("");
                if !code_trimmed.is_empty()
                    && !code_trimmed.starts_with("//")
                    && !code_trimmed.starts_with('#')
                {
                    stats.code_lines += 1;
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
                    let after = trimmed.rsplit('}').next().unwrap_or("");
                    if !after.trim().is_empty() && !after.trim().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }

            // Count nested parenthesis comments
            if parenthesis_comment_level > 0 {
                parenthesis_comment_level += trimmed.matches("(*").count() as i32;
                parenthesis_comment_level -= trimmed.matches("*)").count() as i32;

                // If we've closed all parenthesis comments, check for code after
                if parenthesis_comment_level == 0 {
                    let after = trimmed.rsplit("*)").next().unwrap_or("");
                    if !after.trim().is_empty() && !after.trim().starts_with("//") {
                        stats.code_lines += 1;
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
            let before = trimmed.split('{').next().unwrap_or("");
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }

            brace_comment_level += trimmed.matches("{").count() as i32;
            brace_comment_level -= trimmed.matches("}").count() as i32;

            // If comment ends on same line
            if brace_comment_level == 0 {
                let after = trimmed.rsplit('}').next().unwrap_or("");
                if !after.trim().is_empty() && !after.trim().starts_with("//") {
                    stats.code_lines += 1;
                }
            }

            continue;
        }

        // Start of parenthesis comment
        if trimmed.contains("(*") {
            stats.comment_lines += 1;

            // Check for code before the comment
            let before = trimmed.split("(*").next().unwrap_or("");
            if !before.trim().is_empty() {
                stats.code_lines += 1;
            }

            parenthesis_comment_level += trimmed.matches("(*").count() as i32;
            parenthesis_comment_level -= trimmed.matches("*)").count() as i32;

            // If comment ends on same line
            if parenthesis_comment_level == 0 {
                let after = trimmed.rsplit("*)").next().unwrap_or("");
                if !after.trim().is_empty() && !after.trim().starts_with("//") {
                    stats.code_lines += 1;
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
                    if token == "//" || token == "#" {
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
            } else {
                let after = trimmed[pos..]
                    .find("}}")
                    .map(|end| &trimmed[(pos + end + 2)..])
                    .unwrap_or("");
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
            let after = &trimmed[(pos + 2)..];
            let close = after.find("*/");
            if close.is_none() {
                in_block = true;
            }
            let trailing = close.map(|end| &after[(end + 2)..]).unwrap_or("");
            if !trailing.trim().is_empty() && !trailing.trim_start().starts_with('!') {
                stats.code_lines += 1;
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

fn increment_entries(entries_count: &mut usize, args: &Args, entry_path: &Path) -> io::Result<()> {
    *entries_count += 1;
    if *entries_count > args.max_entries {
        return Err(io::Error::other(format!(
            "Maximum entry limit ({}) exceeded while scanning {}",
            args.max_entries,
            entry_path.display()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_file(
    file_path: &Path,
    args: &Args,
    root_path: &Path,
    metrics: &mut PerformanceMetrics,
    stats: &mut HashMap<PathBuf, DirectoryStats>,
    error_count: &mut usize,
    filespec: Option<&Pattern>,
    visited_real_paths: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    if !should_process_file(filespec, root_path, file_path) {
        return Ok(());
    }

    let real_path = match fs::canonicalize(file_path) {
        Ok(path) => path,
        Err(err) => {
            eprintln!(
                "Error resolving real path for {}: {}",
                file_path.display(),
                err
            );
            *error_count += 1;
            return Ok(());
        }
    };

    if !visited_real_paths.insert(real_path.clone()) {
        if args.verbose {
            println!(
                "Skipping duplicate target for symlinked file: {} -> {}",
                file_path.display(),
                real_path.display()
            );
        }
        return Ok(());
    }

    let Some(language) = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(get_language_from_extension)
    else {
        return Ok(());
    };

    let role_hint = infer_role_from_path(root_path, file_path);
    match count_lines_with_roles(file_path, role_hint) {
        Ok(role_split) => {
            metrics.update(role_split.total_lines());
            let dir_path = file_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let dir_stats = stats.entry(dir_path).or_default();
            let show_role = role_split.role_count() > 1;
            let mut pending: Vec<(CodeRole, LanguageStats)> = Vec::new();

            for (role, bucket) in role_split.iter() {
                let normalized_stats = normalize_stats(bucket.stats, bucket.total_lines);
                let total_line_kinds = normalized_stats.code_lines
                    + normalized_stats.comment_lines
                    + normalized_stats.blank_lines;
                if total_line_kinds > 0 || bucket.total_lines == 0 {
                    let normalized_total = normalized_stats.code_lines
                        + normalized_stats.comment_lines
                        + normalized_stats.blank_lines;
                    metrics.record_role(role, normalized_total);
                    pending.push((role, normalized_stats));

                    if args.verbose {
                        println!("File: {}", file_path.display());
                        if show_role {
                            println!("  Role: {:?}", role);
                        }
                        println!(
                            "  Code lines: {}",
                            format_number(normalized_stats.code_lines)
                        );
                        println!(
                            "  Comment lines: {}",
                            format_number(normalized_stats.comment_lines)
                        );
                        println!(
                            "  Blank lines: {}",
                            format_number(normalized_stats.blank_lines)
                        );
                        println!(
                            "  Mixed code/comment lines: {}",
                            format_number(normalized_stats.overlap_lines)
                        );
                        println!();
                    }
                }
            }

            if !pending.is_empty() {
                let entry = dir_stats
                    .language_stats
                    .entry(language.to_string())
                    .or_default();
                entry.record_roles(&pending);
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
    visited_real_paths: &mut HashSet<PathBuf>,
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
        increment_entries(entries_count, args, path)?;
        {
            let mut ctx = ProcCtx {
                args,
                root_path,
                metrics,
                stats: &mut stats,
                error_count,
                filespec,
                visited_real_paths,
            };
            process_entry_file(&mut ctx, path)?;
        }
        return Ok(stats);
    } else if !metadata.is_dir() {
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

        increment_entries(entries_count, args, &entry_path)?;

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
                visited_real_paths,
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
            let mut ctx = ProcCtx {
                args,
                root_path,
                metrics,
                stats: &mut stats,
                error_count,
                filespec,
                visited_real_paths,
            };
            process_entry_file(&mut ctx, &entry_path)?;
        } else if file_type.is_symlink() {
            let mut ctx = ProcCtx {
                args,
                root_path,
                metrics,
                stats: &mut stats,
                error_count,
                filespec,
                visited_real_paths,
            };
            handle_symlink(&mut ctx, &entry_path)?;
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
    let mut visited_real_paths = HashSet::new();

    scan_directory_impl(
        &root_path,
        args,
        &root_path,
        metrics,
        current_depth,
        entries_count,
        error_count,
        filespec_pattern.as_ref(),
        &mut visited_real_paths,
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
        format_number(file_count),
        format_number(stats.code_lines),
        format_number(stats.comment_lines),
        format_number(stats.overlap_lines),
        format_number(stats.blank_lines),
        width = LANG_WIDTH
    )
}

fn write_language_table_header(output: &mut String) {
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
}

fn build_analysis_report(
    current_dir: &Path,
    stats: &HashMap<PathBuf, DirectoryStats>,
    files_processed: u64,
    lines_processed: u64,
    error_count: usize,
    role_breakdown: bool,
) -> String {
    let mut output = String::new();
    let mut sorted_stats: Vec<_> = stats.iter().collect();
    sorted_stats.sort_by(|(a, _), (b, _)| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let mut total_by_language: HashMap<String, (u64, LanguageStats)> = HashMap::new();

    let _ = writeln!(output, "\n\nDetailed source code analysis:");
    write_language_table_header(&mut output);

    for (path, dir_stats) in &sorted_stats {
        let display_path = format_directory_display(path, current_dir);
        let mut languages: Vec<_> = dir_stats.language_stats.iter().collect();
        languages.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (lang, entry) in languages {
            let (file_count, lang_stats) = entry.summary();
            let line = format_language_stats_line(&display_path, lang, file_count, &lang_stats);
            let _ = writeln!(output, "{}", line);
            let (total_count, total_stats) = total_by_language
                .entry(lang.to_string())
                .or_insert((0, LanguageStats::default()));
            *total_count += file_count;
            total_stats.add_assign(&lang_stats);
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

    if role_breakdown {
        append_role_breakdown_sections(&mut output, current_dir, &sorted_stats);
    }

    if files_processed > 0 || lines_processed > 0 {
        let _ = writeln!(output, "\n{}", "Overall Summary:".blue().bold());
        let _ = writeln!(
            output,
            "Total files processed: {}",
            format_number(files_processed).bright_yellow()
        );
        let _ = writeln!(
            output,
            "Total lines processed: {}",
            format_number(lines_processed).bright_yellow()
        );
        let _ = writeln!(
            output,
            "Code lines:     {} ({})",
            format_number(grand_total.code_lines).bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.code_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Comment lines:  {} ({})",
            format_number(grand_total.comment_lines).bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.comment_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Mixed lines:    {} ({})",
            format_number(grand_total.overlap_lines).bright_yellow(),
            format!(
                "{:.1}%",
                safe_percentage(grand_total.overlap_lines, lines_processed)
            )
            .bright_yellow()
        );
        let _ = writeln!(
            output,
            "Blank lines:    {} ({})",
            format_number(grand_total.blank_lines).bright_yellow(),
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

fn append_role_breakdown_sections(
    output: &mut String,
    current_dir: &Path,
    sorted_stats: &[(&PathBuf, &DirectoryStats)],
) {
    for role in CodeRole::ALL {
        append_single_role_section(output, current_dir, sorted_stats, role);
    }
}

fn append_single_role_section(
    output: &mut String,
    current_dir: &Path,
    sorted_stats: &[(&PathBuf, &DirectoryStats)],
    role: CodeRole,
) {
    let mut totals_by_language: HashMap<String, (u64, LanguageStats)> = HashMap::new();
    let mut has_rows = false;
    for (path, dir_stats) in sorted_stats {
        let display_path = format_directory_display(path, current_dir);
        let mut languages: Vec<_> = dir_stats.language_stats.iter().collect();
        languages.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (lang, entry) in languages {
            if let Some((file_count, lang_stats)) = entry.role_summary(role) {
                if !has_rows {
                    let _ = writeln!(output, "\nRole breakdown ({})", role.label());
                    write_language_table_header(output);
                    has_rows = true;
                }
                let line = format_language_stats_line(&display_path, lang, file_count, &lang_stats);
                let _ = writeln!(output, "{}", line);
                let (total_count, total_stats) = totals_by_language
                    .entry(lang.to_string())
                    .or_insert((0, LanguageStats::default()));
                *total_count += file_count;
                total_stats.add_assign(&lang_stats);
            }
        }
    }

    if has_rows {
        let _ = writeln!(output, "{:-<112}", "");
        let _ = writeln!(output, "Totals by language ({}):", role.label());
        let mut sorted_totals: Vec<_> = totals_by_language.iter().collect();
        sorted_totals.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (lang, (file_count, stats)) in sorted_totals {
            let line = format_language_stats_line("", lang, *file_count, stats);
            let _ = writeln!(output, "{}", line);
        }
    } else {
        let _ = writeln!(output, "\nRole breakdown ({})", role.label());
        let _ = writeln!(output, "No {} data collected.", role.label().to_lowercase());
    }
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

fn print_supported_languages() {
    let languages = [
        ("Algol", colored::Color::White),
        ("Assembly", colored::Color::Cyan),
        ("Batch", colored::Color::White),
        ("C#", colored::Color::Magenta),
        ("C/C++", colored::Color::Blue),
        ("CMake", colored::Color::Green),
        ("COBOL", colored::Color::Blue),
        ("DCL", colored::Color::White),
        ("Dockerfile", colored::Color::Cyan),
        ("Fortran", colored::Color::Magenta),
        ("Go", colored::Color::Cyan),
        ("HCL", colored::Color::Magenta),
        ("HTML", colored::Color::Red),
        ("INI", colored::Color::White),
        ("IPLAN", colored::Color::White),
        ("JSON", colored::Color::Yellow),
        ("JSX", colored::Color::Yellow),
        ("Java", colored::Color::Red),
        ("JavaScript", colored::Color::Yellow),
        ("Makefile", colored::Color::Red),
        ("Mustache", colored::Color::Red),
        ("PHP", colored::Color::Magenta),
        ("Pascal", colored::Color::Green),
        ("Perl", colored::Color::Cyan),
        ("PowerShell", colored::Color::Blue),
        ("Protobuf", colored::Color::Magenta),
        ("Python", colored::Color::Yellow),
        ("ReStructuredText", colored::Color::Green),
        ("Ruby", colored::Color::Red),
        ("Rust", colored::Color::Red),
        ("SVG", colored::Color::Yellow),
        ("Scala", colored::Color::Red),
        ("Shell", colored::Color::Green),
        ("TCL", colored::Color::Magenta),
        ("TOML", colored::Color::Yellow),
        ("TSX", colored::Color::Blue),
        ("TypeScript", colored::Color::Blue),
        ("Velocity", colored::Color::Cyan),
        ("XML", colored::Color::Yellow),
        ("XSL", colored::Color::Yellow),
        ("YAML", colored::Color::Green),
        ("mdhavers", colored::Color::Red),
    ];

    println!("Supported languages:");

    let term_width = if let Some((Width(w), _)) = terminal_size() {
        w as usize
    } else {
        80
    };

    let mut current_line_len = 0;
    let mut first = true;
    for (lang, color) in languages {
        let lang_display = lang.color(color);
        if !first {
            if current_line_len + 2 + lang.len() > term_width {
                println!(",");
                print!("{}", lang_display);
                current_line_len = lang.len();
            } else {
                print!(", {}", lang_display);
                current_line_len += 2 + lang.len();
            }
        } else {
            print!("{}", lang_display);
            current_line_len += lang.len();
        }
        first = false;
    }
    println!();
}

fn run_cli_with_metrics(args: Args, metrics: &mut PerformanceMetrics) -> io::Result<()> {
    if args.languages {
        print_supported_languages();
        return Ok(());
    }

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
        args.role_breakdown,
    );
    print!("{}", report);

    if (args.role_breakdown || args.verbose) && metrics.has_role_data() {
        println!("\n{}", "Role Summary:".blue().bold());
        for (idx, (files, lines)) in metrics.role_counters().iter().enumerate() {
            let role = CodeRole::ALL[idx];
            println!(
                "{}: {} file occurrences, {} lines",
                role.label().bright_cyan(),
                format_number(*files).bright_yellow(),
                format_number(*lines).bright_yellow()
            );
        }
        if files_processed < metrics.role_counters().iter().map(|(f, _)| f).sum::<u64>() {
            println!(
                "{}",
                "(Note: files can appear in multiple roles; counts above are per-role occurrences.)"
                    .bright_black()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    include!("tests_included.rs");

    #[test]
    fn test_format_number() {
        use super::format_number;
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(10), "10");
        assert_eq!(format_number(100), "100");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(10000), "10,000");
        assert_eq!(format_number(100000), "100,000");
        assert_eq!(format_number(1000000), "1,000,000");
        assert_eq!(format_number(123456789), "123,456,789");
    }

    #[test]
    fn test_format_rate() {
        use super::format_rate;
        assert_eq!(format_rate(0.0), "0.0");
        assert_eq!(format_rate(123.456), "123.5");
        assert_eq!(format_rate(1234.56), "1,234.6");
        assert_eq!(format_rate(12345.67), "12,345.7");
        assert_eq!(format_rate(1234567.89), "1,234,567.9");
    }
}
