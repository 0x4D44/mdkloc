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
fn cli_totals_cstyle_more_mixed() {
    let td = TempDir::new().unwrap();
    // code before line comment
    write_file(
        &td.path().join("line_comment.c"),
        "int value = 42; // trailing comment\n",
    );
    // mixed line + block sequences
    write_file(
        &td.path().join("mixed.c"),
        "int a=0; // c /* ignored */\nint b=0; /* block */ // trailing\n",
    );
    // multiple block pairs on a single line
    write_file(
        &td.path().join("pairs.c"),
        "int a; /* c1 */ mid /* c2 */ end;\n",
    );

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_files, code, comments, _mixed, _blank) =
        parse_totals(&String::from_utf8_lossy(&out.stdout))
            .get("C/C++")
            .copied()
            .expect("C/C++ totals");
    assert!(code >= 4);
    assert!(comments >= 3);
}

#[test]
fn cli_totals_hcl_more_mixed() {
    let td = TempDir::new().unwrap();
    // multiline block then hash line
    write_file(
        &td.path().join("block_then_hash_line.tf"),
        "resource \"x\" \"y\" {\n  /* block\n     still comment */\n  # trailing hash\n}\n",
    );
    // block then line comment
    write_file(
        &td.path().join("block_then_line.tf"),
        "resource \"x\" \"y\" {\n  attr = 1 /* block */ // trailing line comment\n}\n",
    );
    // resumes code before hash
    write_file(
        &td.path().join("combo.tf"),
        "resource \"x\" \"y\" {\n  primary = 1 /* block */ secondary = 2 # trailing hash\n}\n",
    );

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (files, code, comments, _mixed, _blank) =
        parse_totals(&String::from_utf8_lossy(&out.stdout))
            .get("HCL")
            .copied()
            .expect("HCL totals");
    assert_eq!(files, 3);
    assert!(code >= 6);
    assert!(comments >= 3);
}
