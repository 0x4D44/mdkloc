//! Source Code Analysis Tool
//! 
//! This tool performs comprehensive analysis of source code across multiple programming languages,
//! providing detailed statistics about code, comment, and blank line distribution.
//! 
//! Supported languages: Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript, PHP, Perl, Ruby, Shell, Pascal.

use std::collections::HashMap;
use clap::{Parser, ArgAction};
use std::fs;
use std::env;
use std::io; // No BufReader here.
use std::path::{Path, PathBuf};

use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::io::Read; // Needed for reading file contents
use glob::glob;
use colored::*;

// Fixed width for the directory column.
const DIR_WIDTH: usize = 40;

// Performance metrics structure
#[derive(Debug)]
struct PerformanceMetrics {
    files_processed: Arc<AtomicU64>,
    lines_processed: Arc<AtomicU64>,
    start_time: Instant,
    last_update: Instant,
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Source code analyser for multiple programming languages. Supported languages: Rust, Go, Python, Java, C/C++, C#, JavaScript, TypeScript, PHP, Perl, Ruby, Shell, Pascal.",
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

#[derive(Debug, Default)]
struct LanguageStats {
    code_lines: u64,
    comment_lines: u64,
    blank_lines: u64,
}

#[derive(Debug, Default)]
struct DirectoryStats {
    language_stats: HashMap<String, (u64, LanguageStats)>, // (file_count, stats) per language
}

impl PerformanceMetrics {
    fn new() -> Self {
        PerformanceMetrics {
            files_processed: Arc::new(AtomicU64::new(0)),
            lines_processed: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            last_update: Instant::now(),
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

    fn print_progress(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let files = self.files_processed.load(Ordering::Relaxed);
        let lines = self.lines_processed.load(Ordering::Relaxed);
        
        print!("\rProcessed {} files ({:.1} files/sec) and {} lines ({:.1} lines/sec)...", 
            files,
            files as f64 / elapsed,
            lines,
            lines as f64 / elapsed
        );
        let _ = io::Write::flush(&mut io::stdout()); // Ignore errors instead of unwrap
    }

    fn print_final_stats(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let files = self.files_processed.load(Ordering::Relaxed);
        let lines = self.lines_processed.load(Ordering::Relaxed);
        
        println!("\n\n{}", "Performance Summary:".blue().bold());
        println!("Total time: {:.2} seconds", elapsed.to_string().bright_yellow());
        println!("Files processed: {} ({:.1} files/sec)", files.to_string().bright_yellow(), (files as f64 / elapsed).to_string().bright_yellow());
        println!("Lines processed: {} ({:.1} lines/sec)", lines.to_string().bright_yellow(), (lines as f64 / elapsed).to_string().bright_yellow());
    }
}



/// Reads a file’s entire content as lines, converting invalid UTF‑8 sequences using replacement characters.
fn read_file_lines_lossy(file_path: &Path) -> io::Result<Vec<String>> {
    let mut file = fs::File::open(file_path)?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    let content = String::from_utf8_lossy(&content);
    Ok(content.lines().map(|line| line.to_string()).collect())
}

/// Identify the language based on the file extension (case-insensitive).
/// Uses static strings to avoid unnecessary allocations.
fn get_language_from_extension(file_name: &str) -> Option<&'static str> {
    // Extract extension first, then normalize only if needed
    let ext = file_name.rsplit('.').next()?;
    // Convert to lowercase for case-insensitive comparison
    let lowercase_ext = ext.to_lowercase();
    
    match lowercase_ext.as_str() {
        "rs"   => Some("Rust"),
        "go"   => Some("Go"),
        "py"   => Some("Python"),
        "java" => Some("Java"),
        "cpp" | "c" | "h" | "hpp" => Some("C/C++"),
        "cs"   => Some("C#"),
        "js"   => Some("JavaScript"),
        "ts"   => Some("TypeScript"),
        "jsx"  => Some("JSX"),
        "tsx"  => Some("TSX"),
        "php"  => Some("PHP"),
        "pl" | "pm" | "t" => Some("Perl"),
        "rb"   => Some("Ruby"),
        "sh"   => Some("Shell"),
        "pas"  => Some("Pascal"),
        "toml" => Some("TOML"),
        _      => None,
    }
}

fn is_ignored_dir(path: &Path) -> bool {
    let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ignored = [
        "target", "node_modules", "build", "dist", ".git",  
        "venv", "__pycache__", "bin", "obj"
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

/// Delegate counting to the appropriate parser based on file extension.
fn count_lines_with_stats(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    // Get extension in lowercase for case-insensitive matching.
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "rs"  => count_rust_lines(file_path),
        "go"  => count_c_style_lines(file_path),
        "py"  => count_python_lines(file_path),
        "java" | "c" | "cpp" | "h" | "hpp" | "cs" => count_c_style_lines(file_path),
        "js" | "ts" | "jsx" | "tsx" => count_javascript_lines(file_path),
        "php" => count_php_lines(file_path),
        "pl" | "pm" | "t" => count_perl_lines(file_path),
        "rb"  => count_ruby_lines(file_path),
        "sh"  => count_shell_lines(file_path),
        "pas" => count_pascal_lines(file_path),
        "toml" => count_toml_lines(file_path),
        _     => count_generic_lines(file_path),
    }
}

fn count_generic_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let total_lines = lines.len() as u64;
    for line in lines {
        if line.trim().is_empty() {
            stats.blank_lines += 1;
        } else {
            stats.code_lines += 1;
        }
    }
    Ok((stats, total_lines))
}

fn count_rust_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_multiline_string = false;
    let mut multiline_quote_char = '"';
    let mut prev_line_continued = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
        if (trimmed.starts_with("'''") || trimmed.starts_with(r#"""""#)) && !prev_line_continued {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
                    if !code.trim().is_empty() && !code.trim_start().starts_with("//") {
                        stats.code_lines += 1;
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block_comment = true;
            stats.comment_lines += 1;
            if trimmed.contains("*/") {
                in_block_comment = false;
                if let Some(code) = trimmed.split("*/").nth(1) {
                    if !code.trim().is_empty() {
                        stats.code_lines += 1;
                    }
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

fn count_javascript_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let mut in_jsx_comment = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
        if trimmed.starts_with("/*") {
            in_block_comment = true;
            stats.comment_lines += 1;
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_pod_comment = false;
    let total_lines = lines.len() as u64;
    for line in lines {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let mut in_block_comment = false;
    let total_lines = lines.len() as u64;
    let mut line_number = 0;
    for line in lines {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let total_lines = lines.len() as u64;
    let mut line_number = 0;
    for line in lines {
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
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let total_lines = lines.len() as u64;
    
    // Track both comment type and nesting level
    let mut brace_comment_level = 0;      // For { } comments
    let mut parenthesis_comment_level = 0; // For (* *) comments
    
    for line in lines {
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

/// TOML: supports line comments with '#'.
fn count_toml_lines(file_path: &Path) -> io::Result<(LanguageStats, u64)> {
    let lines = read_file_lines_lossy(file_path)?;
    let mut stats = LanguageStats::default();
    let total_lines = lines.len() as u64;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stats.blank_lines += 1;
            continue;
        }
        if trimmed.starts_with("#") {
            stats.comment_lines += 1;
            continue;
        }
        stats.code_lines += 1;
    }
    Ok((stats, total_lines))
}


/// Recursively scan directories and collect statistics.
/// Added error tracking and directory depth limiting to prevent stack overflow.
fn scan_directory(
    path: &Path, 
    args: &Args,
    current_dir: &Path,
    metrics: &mut PerformanceMetrics,
    current_depth: usize,
    error_count: &mut usize
) -> io::Result<HashMap<PathBuf, DirectoryStats>> {
    // Check max depth to prevent stack overflow
    if current_depth > args.max_depth {
        eprintln!("Warning: Maximum directory depth ({}) reached at {}", args.max_depth, path.display());
        *error_count += 1;
        return Ok(HashMap::new());
    }

    if args.non_recursive && current_depth > 0 {
        return Ok(HashMap::new());
    }

    // Dynamically size HashMap based on expected entries
    let estimate_size = if path.is_dir() { 128 } else { 1 };
    let mut stats: HashMap<PathBuf, DirectoryStats> = HashMap::with_capacity(estimate_size);
    
    if is_ignored_dir(path) || args.ignore.iter().any(|d| path.ends_with(Path::new(d))) {
        return Ok(stats);
    }
    
    if path.is_file() {
        if let Some(language) = path.file_name().and_then(|n| n.to_str()).and_then(get_language_from_extension) {
            // Safely handle parent path without unwrapping
            let dir_path = match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => PathBuf::from(""),
            };
            
            if let Ok((ref file_stats, total_lines)) = count_lines_with_stats(path) {
                metrics.update(total_lines);
                let dir_stats = stats.entry(dir_path).or_default();
                let (count, lang_stats) = dir_stats.language_stats.entry(language.to_string()).or_insert((0, LanguageStats::default()));
                *count += 1;
                lang_stats.code_lines += file_stats.code_lines;
                lang_stats.comment_lines += file_stats.comment_lines;
                lang_stats.blank_lines += file_stats.blank_lines;
                if args.verbose {
                    println!("File: {}", path.display());
                    println!("  Code lines: {}", file_stats.code_lines);
                    println!("  Comment lines: {}", file_stats.comment_lines);
                    println!("  Blank lines: {}", file_stats.blank_lines);
                    println!();
                }
            }
        }
        return Ok(stats);
    }

    if let Some(filespec) = &args.filespec {
        let pattern = path.join(filespec);
        for entry in glob(pattern.to_str().unwrap()).expect("Failed to read glob pattern ") {
            match entry {
                Ok(path) => {
                    if path.is_file() {
                        if let Some(language) = path.file_name().and_then(|n| n.to_str()).and_then(get_language_from_extension) {
                            let dir_path = match path.parent() {
                                Some(parent) => parent.to_path_buf(),
                                None => PathBuf::from(""),
                            };
                            
                            match count_lines_with_stats(&path) {
                                Ok((ref file_stats, total_lines)) => {
                                    metrics.update(total_lines);
                                    let dir_stats = stats.entry(dir_path).or_default();
                                    let (count, lang_stats) = dir_stats.language_stats.entry(language.to_string()).or_insert((0, LanguageStats::default()));
                                    *count += 1;
                                    lang_stats.code_lines += file_stats.code_lines;
                                    lang_stats.comment_lines += file_stats.comment_lines;
                                    lang_stats.blank_lines += file_stats.blank_lines;
                                    if args.verbose {
                                        println!("File: {}", path.display());
                                        println!("  Code lines: {}", file_stats.code_lines);
                                        println!("  Comment lines: {}", file_stats.comment_lines);
                                        println!("  Blank lines: {}", file_stats.blank_lines);
                                        println!();
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Error counting lines in {}: {}", path.display(), e);
                                    *error_count += 1;
                                }
                            }
                        }
                    }
                }
                Err(e) => println!("{:?}", e),
            }
        }
    } else {
        let read_dir = fs::read_dir(path)?;
        for entry_result in read_dir {
            let entry = match entry_result {
                Ok(entry) => entry,
                Err(e) => {
                    eprintln!("Error reading entry in {}: {}", path.display(), e);
                    *error_count += 1;
                    continue;
                }
            };
            
            let file_type = entry.file_type()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                if !args.non_recursive {
                    match scan_directory(&entry.path(), args, current_dir, metrics, current_depth + 1, error_count) {
                        Ok(sub_stats) => {
                            for (path, stat) in sub_stats {
                                if let Some(existing) = stats.get_mut(&path) {
                                    for (lang, (count, lang_stats)) in stat.language_stats {
                                        let (existing_count, existing_stats) = existing.language_stats.entry(lang).or_insert((0, LanguageStats::default()));
                                        *existing_count += count;
                                        existing_stats.code_lines += lang_stats.code_lines;
                                        existing_stats.comment_lines += lang_stats.comment_lines;
                                        existing_stats.blank_lines += lang_stats.blank_lines;
                                    }
                                } else {
                                    stats.insert(path, stat);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error scanning directory {}: {}", entry.path().display(), e);
                            *error_count += 1;
                        }
                    }
                }
            } else if file_type.is_file() && !file_type.is_symlink() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if let Some(language) = get_language_from_extension(&file_name) {
                    let dir_path = match entry.path().parent() {
                        Some(parent) => parent.to_path_buf(),
                        None => PathBuf::from(""),
                    };
                    
                    match count_lines_with_stats(&entry.path()) {
                        Ok((ref file_stats, total_lines)) => {
                            metrics.update(total_lines);
                            let dir_stats = stats.entry(dir_path).or_default();
                            let (count, lang_stats) = dir_stats.language_stats.entry(language.to_string()).or_insert((0, LanguageStats::default()));
                            *count += 1;
                            lang_stats.code_lines += file_stats.code_lines;
                            lang_stats.comment_lines += file_stats.comment_lines;
                            lang_stats.blank_lines += file_stats.blank_lines;
                            if args.verbose {
                                println!("File: {}", entry.path().display());
                                println!("  Code lines: {}", file_stats.code_lines);
                                println!("  Comment lines: {}", file_stats.comment_lines);
                                println!("  Blank lines: {}", file_stats.blank_lines);
                                println!();
                            }
                        }
                        Err(e) => {
                            eprintln!("Error counting lines in {}: {}", entry.path().display(), e);
                            *error_count += 1;
                        }
                    }
                }
            }
        }
    }
    
    Ok(stats)
}

/// Helper function to print stats for a language
fn print_language_stats(prefix: &str, lang: &str, file_count: u64, stats: &LanguageStats) {
    println!("{:<40} {:<12} {:>8} {:>10} {:>10} {:>10}", 
        prefix.white(), lang.white(), file_count.to_string().bright_yellow(), stats.code_lines.to_string().bright_yellow(), stats.comment_lines.to_string().bright_yellow(), stats.blank_lines.to_string().bright_yellow());
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let path = Path::new(&args.path);
    let current_dir = env::current_dir()?;
    let mut metrics = PerformanceMetrics::new();
    let mut error_count = 0;
    
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Path does not exist: {}", path.display())));
    }
    
    println!("Starting source code analysis...");
    // Start with depth 0 and track errors
    let stats = scan_directory(path, &args, &current_dir, &mut metrics, 0, &mut error_count)?;
    metrics.print_final_stats();
    
    // Print detailed analysis with fixed-width directory field.
    let mut total_by_language: HashMap<String, (u64, LanguageStats)> = HashMap::new();
    let mut sorted_stats: Vec<_> = stats.iter().collect();
    sorted_stats.sort_by(|(a, _), (b, _)| a.to_string_lossy().cmp(&b.to_string_lossy()));
    
    println!("\n\n{}", "Detailed source code analysis:".blue().bold());
    println!("{}", "-".repeat(100).truecolor(100, 100, 100));
    let dir_header = "Directory".white().bold();
    let lang_header = "Language".white().bold();
    let files_header = "Files".white().bold();
    let code_header = "Code".white().bold();
    let comments_header = "Comments".white().bold();
    let blank_header = "Blank".white().bold();

    println!("{:<40} {:<12} {:>8} {:>10} {:>10} {:>10}",
        dir_header, lang_header, files_header, code_header, comments_header, blank_header);
    println!("{}", "-".repeat(100).truecolor(100, 100, 100));
    
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
            
            let (total_count, total_stats) = total_by_language.entry(lang.to_string()).or_insert((0, LanguageStats::default()));
            *total_count += file_count;
            total_stats.code_lines += lang_stats.code_lines;
            total_stats.comment_lines += lang_stats.comment_lines;
            total_stats.blank_lines += lang_stats.blank_lines;
        }
    }
    
    println!("{:-<100}", "".truecolor(100, 100, 100));
    println!("{}", "Totals by language:".blue().bold());
    
    let mut sorted_totals: Vec<_> = total_by_language.iter().collect();
    sorted_totals.sort_by(|(a, _), (b, _)| a.cmp(b));
    
    for (lang, (file_count, stats)) in sorted_totals {
        print_language_stats("", lang, *file_count, stats);
    }
    
    let mut grand_total = LanguageStats::default();
    let mut total_files = 0;
    
    for (_, (files, stats)) in total_by_language.iter() {
        total_files += files;
        grand_total.code_lines += stats.code_lines;
        grand_total.comment_lines += stats.comment_lines;
        grand_total.blank_lines += stats.blank_lines;
    }
    
    let total_lines = grand_total.code_lines + grand_total.comment_lines + grand_total.blank_lines;
    
    if total_lines > 0 {
        println!("\n{}", "Overall Summary:".blue().bold());
        println!("Total files processed: {}", total_files.to_string().bright_yellow());
        println!("Total lines processed: {}", total_lines.to_string().bright_yellow());
        println!("Code lines:     {} ({:.1}%)", grand_total.code_lines.to_string().bright_yellow(), ((grand_total.code_lines as f64 / total_lines as f64) * 100.0).to_string().bright_yellow());
        println!("Comment lines:  {} ({:.1}%)", grand_total.comment_lines.to_string().bright_yellow(), ((grand_total.comment_lines as f64 / total_lines as f64) * 100.0).to_string().bright_yellow());
        println!("Blank lines:    {} ({:.1}%)", grand_total.blank_lines.to_string().bright_yellow(), ((grand_total.blank_lines as f64 / total_lines as f64) * 100.0).to_string().bright_yellow());
        
        if error_count > 0 {
            println!("\n{}: {}", "Warning".red().bold(), error_count.to_string().bright_yellow());
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
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
        PerformanceMetrics::new()
    }

    fn create_test_file(dir: &Path, name: &str, content: &str) -> io::Result<()> {
        let path = dir.join(name);
        let mut file = File::create(path)?;
        write!(file, "{}", content)?;
        Ok(())
    }

    #[test]
    fn test_directory_scanning() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let args = test_args();
        let mut metrics = test_metrics();
        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir(&sub_dir)?;
        create_test_file(&temp_dir.path(), "main.rs", "fn main() {\n// Comment\nprintln!(\"Hello\");\n}\n")?;
        create_test_file(&sub_dir, "lib.rs", "pub fn add(a: i32, b: i32) -> i32 {\n/* Block comment */\na + b\n}\n")?;
        create_test_file(&temp_dir.path(), "readme.md", "# Test Project")?;
        let mut error_count = 0;
        let stats = scan_directory(temp_dir.path(), &args, temp_dir.path(), &mut metrics, 0, &mut error_count)?;
        let main_stats = stats.get(temp_dir.path()).unwrap();
        let main_rust_stats = main_stats.language_stats.get("Rust").unwrap();
        assert_eq!(main_rust_stats.0, 1);
        assert_eq!(main_rust_stats.1.code_lines, 3);
        assert_eq!(main_rust_stats.1.comment_lines, 1);
        let sub_stats = stats.get(&sub_dir).unwrap();
        let sub_rust_stats = sub_stats.language_stats.get("Rust").unwrap();
        assert_eq!(sub_rust_stats.0, 1);
        assert_eq!(sub_rust_stats.1.code_lines, 3);
        assert_eq!(sub_rust_stats.1.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_rust_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.rs", "fn main() {\n// Line comment\n/* Block comment */\n/// Doc comment\n//! Module comment\nprintln!(\"Hello\");\n}\n")?;
        let (stats, _total_lines) = count_rust_lines(&temp_dir.path().join("test.rs"))?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_python_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.py", "def main():\n# Line comment\n'''Block\ncomment'''\nprint('Hello')\n\n")?;
        let (stats, _total_lines) = count_python_lines(&temp_dir.path().join("test.py"))?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 3);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_javascript_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.js", "function main() {\n// Line comment\n/* Block comment */\n/* Multi-line\ncomment */\n<!-- JSX comment -->\nconsole.log('Hello');\n}\n")?;
        let (stats, _total_lines) = count_javascript_lines(&temp_dir.path().join("test.js"))?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 5);
        assert_eq!(stats.blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_perl_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.pl", "#!/usr/bin/perl\n# Line comment\n=pod\nDocumentation block\n=cut\nprint \"Hello\";\n\n")?;
        let (stats, _total_lines) = count_perl_lines(&temp_dir.path().join("test.pl"))?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 4);
        assert_eq!(stats.blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_ruby_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.rb", "#!/usr/bin/env ruby\n# This is a comment\nputs 'Hello, world!'\n=begin\nThis is a block comment\n=end\nputs 'Goodbye'\n")?;
        let (stats, _total_lines) = count_ruby_lines(&temp_dir.path().join("test.rb"))?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        Ok(())
    }

    #[test]
    fn test_shell_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.sh", "#!/bin/bash\n# This is a comment\necho \"Hello, world!\"\n")?;
        let (stats, _total_lines) = count_shell_lines(&temp_dir.path().join("test.sh"))?;
        assert_eq!(stats.code_lines, 2);
        assert_eq!(stats.comment_lines, 1);
        Ok(())
    }

    #[test]
    fn test_pascal_line_counting() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_file(&temp_dir.path(), "test.pas", "program Test;\n// This is a line comment\n{ This is a block comment }\nwriteln('Hello, world!');\n(* Another block comment\nspanning multiple lines *)\nwriteln('Goodbye');\n")?;
        let (stats, _total_lines) = count_pascal_lines(&temp_dir.path().join("test.pas"))?;
        assert_eq!(stats.code_lines, 3);
        assert_eq!(stats.comment_lines, 4);
        Ok(())
    }

    // --- New Tests ---

    #[test]
    fn test_case_insensitive_extension() {
        // Test that uppercase or mixed-case extensions are correctly recognized.
        assert_eq!(get_language_from_extension("TEST.RS"), Some("Rust"));
        assert_eq!(get_language_from_extension("example.Js"), Some("JavaScript"));
        assert_eq!(get_language_from_extension("module.Py"), Some("Python"));
        assert_eq!(get_language_from_extension("FOO.TS"), Some("TypeScript"));
    }

    #[test]
    fn test_invalid_utf8_handling() -> io::Result<()> {
        // Create a file with invalid UTF-8 bytes.
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("invalid.txt");
        // Write valid UTF-8 text, then an invalid byte (0xFF), then more valid text.
        fs::write(&file_path, b"hello\n\xFFworld\n")?;
        // read_file_lines_lossy should not error and should replace the invalid byte.
        let lines = read_file_lines_lossy(&file_path)?;
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
        create_test_file(&temp_dir.path(), "file.xyz", content)?;

        let (stats, _total_lines) = count_generic_lines(&temp_dir.path().join("file.xyz"))?;
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
        let expected_ending: String = long_str.chars().rev().take(DIR_WIDTH - 3).collect::<Vec<_>>().into_iter().rev().collect();
        assert!(truncated.ends_with(&expected_ending));
    }
}
