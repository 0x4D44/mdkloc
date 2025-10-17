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

#[test]
fn cli_prints_summary_for_basic_run() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("main.rs"),
        "fn main() {}\n// comment\n",
    );

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, got status {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Performance Summary"),
        "stdout missing summary: {stdout}"
    );
    assert!(
        stdout.contains("Detailed source code analysis"),
        "stdout missing detailed table: {stdout}"
    );
    assert!(
        stdout.contains("Rust"),
        "stdout missing Rust language totals: {stdout}"
    );
}

#[test]
fn cli_respects_non_recursive_and_ignore() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("root.rs"),
        "fn root() {}\n// root comment\n",
    );
    let sub_dir = temp_dir.path().join("sub");
    fs::create_dir(&sub_dir).expect("failed to create sub directory");
    write_file(
        &sub_dir.join("nested.rs"),
        "fn nested() {}\n// nested comment\n",
    );

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--non-recursive")
        .arg("--ignore")
        .arg("sub")
        .arg("--filespec")
        .arg("*.rs")
        .arg("--max-entries")
        .arg("10")
        .arg("--max-depth")
        .arg("1")
        .arg("--verbose")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, got status {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("sub"),
        "non-recursive + ignore should skip sub dir, stdout: {stdout}"
    );
    assert!(
        stdout.contains("File:"),
        "verbose mode should list files, stdout: {stdout}"
    );
}

#[test]
fn cli_invalid_path_returns_error() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let missing_path = temp_dir.path().join("missing");
    let output = Command::new(mdkloc_bin())
        .arg(missing_path)
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        !output.status.success(),
        "expected failure for missing path, status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Path does not exist"),
        "stderr did not mention missing path: {stderr}"
    );
}

#[test]
fn cli_warns_on_max_depth_exceeded() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();
    let level1 = root.join("level1");
    let level2 = level1.join("level2");
    fs::create_dir(&level1).expect("failed to create level1 directory");
    fs::create_dir(&level2).expect("failed to create level2 directory");
    write_file(&root.join("root.rs"), "fn root() {}\n");
    write_file(&level2.join("nested.rs"), "fn nested() {}\n");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("--max-depth")
        .arg("0")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success with warning, got status {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Warning"),
        "stdout should include warning summary when max depth exceeded: {stdout}"
    );
    assert!(
        stderr.contains("Maximum directory depth"),
        "stderr should include depth warning, stderr: {stderr}"
    );
}

#[test]
fn cli_invalid_filespec_pattern_errors() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("main.rs"),
        "fn main() {}\n// comment\n",
    );

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--filespec")
        .arg("[")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        !output.status.success(),
        "invalid filespec should fail, status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid filespec pattern"),
        "stderr missing filespec error: {stderr}"
    );
}

#[test]
fn cli_errors_when_max_entries_zero() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("main.rs"),
        "fn main() {}\n",
    );

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--max-entries")
        .arg("0")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        !output.status.success(),
        "max-entries=0 should fail, status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Too many entries"),
        "stderr missing max entries message: {stderr}"
    );
}
