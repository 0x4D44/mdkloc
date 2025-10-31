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

#[test]
fn cstyle_break_after_close_and_inside_line_marker() {
    // Hit the 783 break path: both // and /* present, with // inside the block; after */ nothing remains.
    let t = TempDir::new().unwrap();
    write_file(
        t.path().join("case.c").as_path(),
        "int x; /* inside // line */   \n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success(), "status: {:?}", out.status);
}

#[test]
fn cstyle_break_after_close_no_trailing_tokens() {
    // Hit the 720 break path: in_block_comment close and remainder is whitespace only.
    let t = TempDir::new().unwrap();
    write_file(t.path().join("b.c").as_path(), "/* open\n*/   \n");
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn powershell_break_after_close_with_hash_inside_block() {
    // Hit 1759 break path in (Some(pl), Some(pb)) arm: <# ... #> where `#` appears inside the block and nothing after close.
    let t = TempDir::new().unwrap();
    write_file(t.path().join("e.ps1").as_path(), "<# # inside #>    \n");
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn xml_in_comment_close_with_trailing_code_continues() {
    // Hit 1864 continue after closing comment with trailing code.
    let t = TempDir::new().unwrap();
    write_file(t.path().join("x.html").as_path(), "<!-- open\n--> tail\n");
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn xml_continue_after_block_close_on_following_line() {
    // Ensure we hit the continue path after closing an XML comment with trailing code.
    let t = TempDir::new().unwrap();
    write_file(t.path().join("y.html").as_path(), "<!-- open\n--> tail\n");
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn direct_file_scan_exercises_metadata_is_file_branch() {
    // Hit 2083 call-site by passing a single file path to the CLI.
    let t = TempDir::new().unwrap();
    let file = t.path().join("solo.rs");
    write_file(&file, "/* open\n*/ tail\n");
    let out = Command::new(mdkloc_bin()).arg(&file).output().unwrap();
    assert!(out.status.success());
}

#[test]
fn dcl_first_non_empty_line_not_dcl_then_dcl() {
    // First non-empty line is not DCL (no $ or !), second line is a DCL comment.
    // Ensures the is_dcl detection path (trim_start non-empty) is exercised.
    let t = TempDir::new().unwrap();
    write_file(
        t.path().join("m.com").as_path(),
        "   \n echo not dcl\n$! now dcl\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn iplan_block_comment_opens_without_close_on_line() {
    // Exercise IPLAN: line with /* opens but no */ on same line, so in_block becomes true.
    let t = TempDir::new().unwrap();
    write_file(
        t.path().join("x.ipl").as_path(),
        "code /* open\n! comment\n/* close */\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(t.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
}
