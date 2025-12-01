use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn mdkloc_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mdkloc")
}
fn write_file(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write");
}

fn parse_totals(stdout: &str) -> HashMap<String, (u64, u64, u64, u64, u64)> {
    let mut out = HashMap::new();
    let mut it = stdout.lines();
    for l in it.by_ref() {
        if l.contains("Totals by language:") {
            break;
        }
    }
    for l in it {
        if l.trim().is_empty() || l.contains("Overall Summary:") {
            break;
        }
        let cols: Vec<&str> = l.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let p = |s: &str| s.parse::<u64>().unwrap_or(0);
        out.insert(
            cols[0].to_string(),
            (p(cols[1]), p(cols[2]), p(cols[3]), p(cols[4]), p(cols[5])),
        );
    }
    out
}

#[test]
fn cli_totals_cstyle_unterminated_variants() {
    let td = TempDir::new().unwrap();
    // Unterminated-looking sequence then proper close on next line
    write_file(
        &td.path().join("block_unterminated.c"),
        "int start = 0; /* begin // still comment\n*/ int done = 1;\n",
    );
    // Blank + multiline block
    write_file(
        &td.path().join("blank_block.c"),
        "int a = 0;\n\n/* block starts\nstill comment\n*/ int b = 1;\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (_files, code, comments, _mixed, _blank) =
        totals.get("C/C++").copied().expect("C/C++ totals");
    assert!(code >= 4);
    assert!(comments >= 3);
}

#[test]
fn cli_totals_hcl_line_vs_hash() {
    let td = TempDir::new().unwrap();
    // Line comment precedes hash on same line
    write_file(
        &td.path().join("line_then_hash.tf"),
        "resource \"x\" \"y\" {\n  attr = 1 // primary # trailing\n}\n",
    );
    // Hash before block
    write_file(&td.path().join("hash_first.tf"), "resource \"x\" \"y\" {\n  value = 1 # hash before block /* still comment */\n  another = 2\n}\n");
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (files, code, comments, _mixed, _blank) = totals.get("HCL").copied().expect("HCL totals");
    assert_eq!(files, 2);
    assert!(code >= 4);
    assert!(comments >= 2);
}
