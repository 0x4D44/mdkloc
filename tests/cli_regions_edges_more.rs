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
fn cli_edges_target_specific_uncovered_paths() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // Rust: close block while in-block, with trailing code
    write_file(&root.join("r_inblock.rs"), "/* open\n*/ trailing_code()\n");

    // Python: open+close on same line and multi-line close with trailing code
    write_file(
        &root.join("p_same_and_multi.py"),
        "'''x''' a=1\n'''open\nend''' b=2\n",
    );

    // C/C++: close after being in-block with trailing whitespace only
    write_file(&root.join("c_inblock.c"), "/* open\n*/    \n");

    // JS: close after being in block with trailing code; JSX multi-line close with trailing code
    write_file(
        &root.join("js_blocks.js"),
        "/* open\n*/ tail\n<!-- open\n--> tail\n",
    );

    // PHP: close after being in block with trailing code
    write_file(&root.join("php_block.php"), "/* open\n*/ echo 1;\n");

    // Pascal: multi-line brace and parenthesis comment closes with trailing code
    write_file(
        &root.join("pas_multiline.pas"),
        "{ open\n} code\n(* open\n*) code\n",
    );

    // PowerShell: multi-line block comment, close with no trailing code after #>
    write_file(&root.join("ps1_multiline.ps1"), "<# open\n#>   \n");

    // XML: multi-line block comment, close with no trailing code after -->
    write_file(&root.join("xml_multi.html"), "<!-- open\n-->   \n");

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
    assert!(
        stdout.contains("Totals by language:"),
        "missing totals: {stdout}"
    );
}
