use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn mdkloc_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mdkloc")
}

#[cfg(unix)]
fn create_symlink(src: &Path, dst: &Path) {
    use std::os::unix::fs::symlink;

    symlink(src, dst).expect("failed to create symlink");
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

#[cfg(unix)]
#[test]
fn cli_counts_symlinked_files_once() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();
    let actual = root.join("actual.rs");
    let alias = root.join("alias.rs");
    write_file(&actual, "fn main() {}\n// real file\n");
    create_symlink(&actual, &alias);

    let output = Command::new(mdkloc_bin())
        .arg(root)
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
        stdout.contains("Total files processed: 1"),
        "symlinked file should count once, stdout: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn cli_skips_symlinked_directories_verbose() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Real directory with a real file
    let real_dir = root.join("real");
    fs::create_dir(&real_dir).expect("failed to create real dir");
    write_file(&real_dir.join("main.rs"), "fn main(){}\n");

    // Symlinked directory pointing to the real directory
    let alias_dir = root.join("alias");
    symlink(&real_dir, &alias_dir).expect("failed to create symlinked dir");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("--verbose")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Symlinked directories are silently skipped - verify file is only counted once
    assert!(
        stdout.contains("Total files processed: 1"),
        "symlinked directory should be skipped, only real file counted: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn cli_reports_duplicate_symlinked_file_target_verbose() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();
    let actual = root.join("actual.rs");
    let alias = root.join("alias.rs");
    write_file(&actual, "fn main() {}\n// real file\n");
    symlink(&actual, &alias).expect("failed to create file symlink");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("--verbose")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Symlinked files are silently skipped - verify only 1 file processed
    assert!(
        stdout.contains("Total files processed: 1"),
        "symlinked file should be skipped, only real file counted: {stdout}"
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
fn cli_enforces_max_entries_after_filters() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();
    // Create 2 matching files to exceed max-entries=1
    write_file(&root.join("one.rs"), "fn main() {}\n");
    write_file(&root.join("two.rs"), "fn other() {}\n");
    write_file(&root.join("skip.txt"), "// not counted\n");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("--filespec")
        .arg("*.rs")
        .arg("--max-entries")
        .arg("1")
        .output()
        .expect("failed to execute mdkloc");

    // max-entries counts filtered entries, so 2 .rs files > 1 should fail
    assert!(
        !output.status.success(),
        "max entries guard should fail the run, status: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Too many entries"),
        "stderr missing max entry message: {stderr}"
    );
}

#[test]
fn cli_prints_language_totals() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("main.rs"),
        "fn main() {}\n// comment\n",
    );
    write_file(&temp_dir.path().join("script.py"), "print('hi')\n# note\n");

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--non-recursive")
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
        stdout.contains("Totals by language:"),
        "stdout missing totals section: {stdout}"
    );
    assert!(
        stdout.contains("Rust") && stdout.contains("Python"),
        "stdout totals missing expected languages: {stdout}"
    );
}

#[test]
fn cli_exercises_language_parsers() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // JavaScript with block and JSX-style comments
    write_file(
        &root.join("app.js"),
        "const a = 1; /* block */ const b = 2;\n<!-- jsx open\ncontinues --> tail\n",
    );
    // PHP with block then trailing code and hash
    write_file(
        &root.join("index.php"),
        "<?php\n$y = 1; /* c */ echo $y; # note\n",
    );
    // Perl POD
    write_file(
        &root.join("script.pl"),
        "print 'x';\n=pod\nPOD body\n=cut\nprint 'y';\n",
    );
    // Ruby block and shebang
    write_file(
        &root.join("script.rb"),
        "#!/usr/bin/env ruby\n=begin\nblock\n=end\nputs 'hi'\n",
    );
    // Shell shebang
    write_file(&root.join("run.sh"), "#!/bin/sh\n# c\necho hi\n");
    // Pascal nested comments
    write_file(
        &root.join("p.pas"),
        "{c1} (*c2*) code\n(* multi\nline *) code2\n",
    );
    // Makefile
    write_file(&root.join("Makefile"), "all:\n\t@echo hi\n");
    // HCL / Terraform
    write_file(&root.join("main.tf"), "a=1 /*c*/ b=2 # tail\n");
    // CMake
    write_file(&root.join("CMakeLists.txt"), "project(x) # note\n");
    // INI
    write_file(&root.join("settings.ini"), "name=value ; inline\n# x\n");
    // TOML
    write_file(&root.join("Cargo.toml"), "[p]\nname='x' # i\n");
    // JSON
    write_file(&root.join("a.json"), "{\n  \"k\": 1\n}\n");
    // XML
    write_file(&root.join("a.xml"), "<a><!-- c --><b/></a>\n");
    // HTML
    write_file(
        &root.join("index.html"),
        "<html><!-- c --><body>t</body></html>\n",
    );
    // Velocity and Mustache
    write_file(&root.join("t.vm"), "## line\n#* block *# tail\n");
    write_file(&root.join("view.mustache"), "{{! c }} Hello {{name}}\n");
    // Algol, COBOL, Fortran, ASM, DCL, IPLAN, Protobuf
    write_file(&root.join("a.alg"), "begin\nCOMMENT x;\nend\n");
    write_file(
        &root.join("c.cob"),
        "       IDENTIFICATION DIVISION.\n      *> c\n       PROGRAM-ID. X.\n",
    );
    write_file(&root.join("f.f90"), "! c\nprogram x\nend\n");
    write_file(&root.join("x.asm"), "; c\nmov eax, eax\n");
    write_file(&root.join("proc.com"), "$! c\n$ exit\n");
    write_file(&root.join("x.ipl"), "/* c */\nVALUE\n");
    write_file(&root.join("m.proto"), "// c\nsyntax = \"proto3\";\n");
    // SVG + XSL (XML family)
    write_file(&root.join("pic.svg"), "<svg><!-- c --></svg>\n");
    write_file(
        &root.join("sheet.xsl"),
        "<xsl:stylesheet><!-- c --></xsl:stylesheet>\n",
    );
    // PowerShell, Batch, TCL
    write_file(&root.join("a.ps1"), "# c\nWrite-Host 'x'\n");
    write_file(&root.join("b.bat"), "REM c\n@echo on\n");
    write_file(&root.join("c.tcl"), "# c\nputs \"x\"\n");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .arg("--non-recursive")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Spot check a few languages appeared, implying their parsers ran
    for lang in [
        "JavaScript",
        "PHP",
        "Perl",
        "Ruby",
        "Shell",
        "Pascal",
        "HCL",
        "CMake",
        "INI",
        "TOML",
        "JSON",
        "XML",
        "HTML",
    ] {
        assert!(
            stdout.contains(lang),
            "expected language '{lang}' in totals; stdout: {stdout}"
        );
    }
}

#[test]
fn cli_reports_warning_summary_when_errors_occur() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();
    write_file(&root.join("main.rs"), "fn main() {}\n// comment\n");
    let sentinel = root.join("__mdkloc_metadata_fail__");
    fs::create_dir(&sentinel).expect("failed to create sentinel directory");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .env("MDKLOC_ENABLE_FAULTS", "1")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "metadata failure should only warn, status: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error reading metadata"),
        "stderr missing metadata warning: {stderr}"
    );
    assert!(
        stdout.contains("Warning") && stdout.contains("Performance Summary"),
        "stdout missing warning summary or performance section: {stdout}"
    );
}

#[test]
fn cli_injected_read_dir_failure() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Create a subdirectory whose name triggers a simulated read_dir failure.
    let fail_dir = root.join("__mdkloc_read_dir_fail__");
    fs::create_dir(&fail_dir).expect("failed to create failing dir");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .env("MDKLOC_ENABLE_FAULTS", "1")
        .output()
        .expect("failed to execute mdkloc");

    assert!(output.status.success(), "status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error reading directory"),
        "stderr should contain read_dir error: {stderr}"
    );
}

#[test]
fn cli_injected_file_type_failure() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Create an entry whose name triggers a simulated file_type failure.
    let fail_entry = root.join("__mdkloc_file_type_fail__.rs");
    write_file(&fail_entry, "fn z(){}\n");

    let output = Command::new(mdkloc_bin())
        .arg(root)
        .env("MDKLOC_ENABLE_FAULTS", "1")
        .output()
        .expect("failed to execute mdkloc");

    assert!(output.status.success(), "status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error reading type for"),
        "stderr should contain file_type error: {stderr}"
    );
}

#[test]
fn cli_errors_when_max_entries_zero() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(&temp_dir.path().join("main.rs"), "fn main() {}\n");

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

#[test]
fn cli_filespec_handles_uppercase_extensions() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("KEEP.RS"),
        "fn keep() {}\n// comment\n",
    );
    write_file(
        &temp_dir.path().join("skip.py"),
        "print('skip')\n# comment\n",
    );
    write_file(&temp_dir.path().join("note.txt"), "note\n");

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--filespec")
        .arg("*.RS")
        .arg("--non-recursive")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, status: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.to_ascii_uppercase().contains("RUST"),
        "stdout should report Rust totals when uppercase filespec matches: {stdout}"
    );
    assert!(
        !stdout.contains("skip.py"),
        "stdout should omit non-matching files when filespec filters: {stdout}"
    );
}
#[test]
fn cli_filespec_and_ignore_combination() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("keep.rs"),
        "fn keep() {}\n// comment\n",
    );
    let ignore_dir = temp_dir.path().join("ignore_me");
    fs::create_dir(&ignore_dir).expect("failed to create ignore dir");
    write_file(&ignore_dir.join("skip.rs"), "fn skip() {}\n");
    write_file(&temp_dir.path().join("note.txt"), "note\n");

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--filespec")
        .arg("*.rs")
        .arg("--ignore")
        .arg("ignore_me")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, status: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.to_ascii_uppercase().contains("RUST"),
        "stdout should report Rust totals when filespec matches: {stdout}"
    );
    assert!(
        !stdout.contains("ignore_me"),
        "stdout should omit ignored directory from the report: {stdout}"
    );
}
#[test]
fn cli_verbose_color_combination() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("main.rs"),
        "fn main() {}\n// comment\n",
    );

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--verbose")
        .arg("--ignore")
        .arg("nonexistent")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, status: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("File:"),
        "verbose output should list processed files: {stdout}"
    );
    assert!(
        stdout.contains("main.rs"),
        "verbose output should include the processed Rust file: {stdout}"
    );
}
#[test]
fn cli_color_filespec_ignore_combination() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    write_file(
        &temp_dir.path().join("keep.rs"),
        "fn keep() {}\n// comment\n",
    );
    let ignore_dir = temp_dir.path().join("ignore_this");
    fs::create_dir(&ignore_dir).expect("failed to create ignore dir");
    write_file(&ignore_dir.join("skip.rs"), "fn skip() {}\n");
    write_file(&temp_dir.path().join("note.txt"), "note\n");

    let output = Command::new(mdkloc_bin())
        .arg(temp_dir.path())
        .arg("--max-entries")
        .arg("5")
        .arg("--filespec")
        .arg("*.rs")
        .arg("--ignore")
        .arg("ignore_this")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, status: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Performance Summary"),
        "stdout should include performance summary when color is disabled: {stdout}"
    );
    assert!(
        stdout.to_ascii_uppercase().contains("RUST"),
        "stdout should report Rust totals when filespec matches: {stdout}"
    );
    assert!(
        !stdout.contains("ignore_this"),
        "stdout should omit ignored directory from the report: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn cli_processes_symlink_to_external_file() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Create the real file outside the scanned root.
    let ext_dir = TempDir::new().expect("failed to create external dir");
    let real = ext_dir.path().join("real.rs");
    write_file(&real, "fn outside(){}\n// note\n");

    // Create a symlink inside the root pointing to the external file.
    let link = root.join("link.rs");
    symlink(&real, &link).expect("failed to create symlink to external file");

    // Scan the root; symlinks are skipped for safety (avoid following external paths)
    let output = Command::new(mdkloc_bin())
        .arg(root)
        .output()
        .expect("failed to execute mdkloc");

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Symlinked files are skipped - no files should be counted
    // Note: Overall Summary is not printed when 0 files, so check Performance Summary
    assert!(
        stdout.contains("Files processed: 0"),
        "symlinked external file should be skipped: {stdout}"
    );
}
