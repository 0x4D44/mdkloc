use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn mdkloc_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mdkloc")
}

fn write_file(path: &Path, contents: &str) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    fs::write(path, contents).expect("failed to write test file");
}

fn run_mdkloc<I, S>(root: &Path, args: I) -> (std::process::ExitStatus, String, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new(mdkloc_bin())
        .arg(root)
        .args(args)
        .output()
        .expect("failed to execute mdkloc");
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn cli_edges_cover_trailing_comment_close_variants() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Rust: block comment closes with trailing code; and code before block + trailing line comment
    write_file(
        &root.join("edge.rs"),
        r#"
fn main() {}
/* comment */ let a = 1; // trailing line comment (code counted once)
let b = 2; /* comment */
/* open
*/ after
let c = 3; /* c */ more();
"#,
    );

    // Python: triple quotes open/close on same line with trailing code, and multi-line close with trailing code
    write_file(
        &root.join("edge.py"),
        "\"\"\"doc\"\"\" x = 1\n'''open\nend''' tail\n",
    );

    // C: end block with trailing whitespace only; and block then line comment ordering; also close-after-open on following line
    write_file(
        &root.join("edge.c"),
        "int x; /* block */   \n/* start */ code /* end */\n/* open\n*/   \n",
    );

    // JavaScript: block close with trailing code; JSX comment close with trailing code and multi-line close
    write_file(
        &root.join("edge.js"),
        "var a = 1; /* c */ x();\n<!-- jsx --> tail\n/* open\n*/ trail\n<!-- open\n--> trail\n",
    );

    // PHP: block close with trailing code (not starting with // or #); also multi-line close with trailing code
    write_file(
        &root.join("edge.php"),
        "/* c */ echo 1;\n/* open\n*/ echo 2;\n",
    );

    // Pascal: nested block styles with trailing code after close; plus multi-line block closes with trailing code
    write_file(
        &root.join("edge.pas"),
        r#"
begin { a { nested } done } x;
begin (* a (* nested *) done *) y;
// line
{ open
} after
(* open
*) after
end.
"#,
    );

    // Mustache: comment close with trailing code; and multi-line close
    write_file(
        &root.join("edge.mustache"),
        "{{! note }} hi\n{{! open\n}} tail\n",
    );

    // IPLAN: block close with trailing code
    write_file(&root.join("edge.ipl"), "a /* b */ c\n");

    // PowerShell: block-only close and multi-line close
    write_file(&root.join("edge.ps1"), "<# comment #>\n<# open\n#>   \n");

    // XML: comment-only line (no trailing code) and multi-line close
    write_file(
        &root.join("edge.html"),
        "<!-- only -->   \n<!-- open\n-->    \n",
    );

    // DCL: first non-empty determines DCL format
    write_file(&root.join("edge.com"), "$ write sys$output \"hi\"\n");

    let (status, stdout, _stderr) = run_mdkloc(root, ["--non-recursive"]);
    assert!(status.success(), "expected success: {:?}", status);
    assert!(
        stdout.contains("Totals by language:"),
        "missing totals section: {stdout}"
    );
    // Spot-check that languages are detected and reported (ensures parsers ran)
    for lang in [
        "Rust",
        "Python",
        "C/C++",
        "JavaScript",
        "PHP",
        "Pascal",
        "Mustache",
        "IPLAN",
        "PowerShell",
        "HTML",
        "DCL",
    ] {
        assert!(
            stdout
                .to_ascii_uppercase()
                .contains(&lang.to_ascii_uppercase()),
            "totals missing language {lang}: {stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn cli_error_paths_permission_denied_and_symlink_metadata_fail() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    // Failure injection is always enabled in tests; see FAULT_ENV_VAR.
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Permission denied file: readable name and extension, but 000 perms.
    let blocked = root.join("blocked.rs");
    write_file(&blocked, "fn x(){}\n");
    let mut perms = fs::metadata(&blocked).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&blocked, perms).unwrap();

    // Symlink with name that triggers metadata failure when reading symlink target.
    let real = root.join("real.rs");
    write_file(&real, "fn y(){}\n");
    let meta_fail_link = root.join("__mdkloc_metadata_fail__");
    symlink(&real, &meta_fail_link).expect("failed to create failing symlink");

    let output = Command::new(mdkloc_bin())
        .env("MDKLOC_ENABLE_FAULTS", "1")
        .arg(root)
        .arg("--verbose")
        .output()
        .expect("failed to execute mdkloc");
    let status = output.status;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(status.success(), "expected success: {:?}", status);

    // Expect errors were reported and summarized.
    assert!(
        stderr.contains("Error counting lines in"),
        "stderr should report file counting error (permission denied): {stderr}"
    );
    assert!(
        stderr.contains("Error resolving metadata for symlink"),
        "stderr should report metadata failure for symlink: {stderr}"
    );
    assert!(
        stdout.contains("Overall Summary") && stdout.contains("Warning"),
        "stdout should include warning summary when errors occur: {stdout}"
    );
}

#[test]
fn cli_scans_single_file_path_exercises_file_metadata_branch() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let file = temp_dir.path().join("solo.rs");
    write_file(&file, "fn solo(){}\n/* comment */ tail\n");

    // Pass a single file path to scan; exercises metadata.is_file() branch.
    let (status, stdout, _stderr) = run_mdkloc(&file, ["--non-recursive"]);
    assert!(status.success(), "expected success: {:?}", status);
    assert!(
        stdout.contains("Totals by language:") && stdout.to_ascii_uppercase().contains("RUST"),
        "stdout should include language totals for the single file: {stdout}"
    );
}
