use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn mdkloc_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mdkloc")
}

fn write_file(path: &Path, contents: &str) {
    fs::write(path, contents).expect("failed to write test file");
}

fn parse_role_totals(stdout: &str, role: &str) -> HashMap<String, (u64, u64)> {
    let marker = format!("Totals by language ({role}):");
    let mut totals = HashMap::new();
    let mut lines = stdout.lines();
    while let Some(line) = lines.next() {
        if line.contains(&marker) {
            for row in lines.by_ref() {
                let trimmed = row.trim();
                if trimmed.is_empty() || trimmed.starts_with("Role breakdown") {
                    break;
                }
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() < 6 {
                    continue;
                }
                let lang = parts[0].to_string();
                let files = parts[1].parse::<u64>().unwrap_or(0);
                let code = parts[2].parse::<u64>().unwrap_or(0);
                totals.insert(lang, (files, code));
            }
            break;
        }
    }
    totals
}

#[test]
fn cli_role_breakdown_reports_mainline_and_test() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Mainline + inline tests
    write_file(
        &root.join("lib.rs"),
        r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_numbers() {
        assert_eq!(add(2, 2), 4);
    }
}
"#,
    );

    // Integration test (whole file should be test role)
    let tests_dir = root.join("tests");
    fs::create_dir_all(&tests_dir).expect("failed to create tests dir");
    write_file(
        &tests_dir.join("integration.rs"),
        r#"
#[test]
fn integration_runs() {
    assert_eq!(2 + 2, 4);
}
"#,
    );

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("-r")
        .output()
        .expect("failed to execute mdkloc");
    assert!(
        output.status.success(),
        "expected success: {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Role breakdown (Mainline)"),
        "expected mainline section in output:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Role breakdown (Test)"),
        "expected test section in output:\n{}",
        stdout
    );

    let mainline = parse_role_totals(&stdout, "Mainline");
    let test = parse_role_totals(&stdout, "Test");

    let (mainline_files, mainline_code) = mainline
        .get("Rust")
        .copied()
        .expect("expected Rust in mainline totals");
    assert!(
        mainline_files >= 1 && mainline_code >= 1,
        "mainline stats should be non-zero: files={mainline_files}, code={mainline_code}"
    );

    let (test_files, test_code) = test
        .get("Rust")
        .copied()
        .expect("expected Rust in test totals");
    assert!(
        test_files >= 1 && test_code >= 1,
        "test stats should be non-zero: files={test_files}, code={test_code}"
    );
}
