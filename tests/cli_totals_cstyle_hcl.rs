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

fn parse_totals(stdout: &str) -> HashMap<String, (u64, u64, u64, u64, u64)> {
    let mut out = HashMap::new();
    let mut it = stdout.lines();
    for line in it.by_ref() {
        if line.contains("Totals by language:") {
            break;
        }
    }
    for line in it {
        if line.trim().is_empty() || line.contains("Overall Summary:") {
            break;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        out.insert(
            cols[0].to_string(),
            (
                parse(cols[1]),
                parse(cols[2]),
                parse(cols[3]),
                parse(cols[4]),
                parse(cols[5]),
            ),
        );
    }
    out
}

#[test]
fn cli_totals_cstyle_mixed_patterns() {
    let td = TempDir::new().expect("tmp");
    // C-style with inline block and trailing line comment on same line
    write_file(&td.path().join("a.c"), "int v=0; /* block */ // trailing\n");
    // C-style with multiline block that closes then code continues
    write_file(&td.path().join("b.c"), "/* start\ncontinues */ int x=1;\n");

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (files, code, comments, _mixed, _blank) =
        totals.get("C/C++").copied().expect("C/C++ totals");
    assert_eq!(files, 2);
    assert!(code >= 2);
    assert!(comments >= 2);
}

#[test]
fn cli_totals_hcl_mixed_patterns() {
    let td = TempDir::new().expect("tmp");
    // HCL with block then hash on same line
    write_file(
        &td.path().join("main.tf"),
        "resource \"x\" \"y\" {\n  value = 1 /* block */ # trailing\n}\n",
    );

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (files, code, comments, _mixed, _blank) = totals.get("HCL").copied().expect("HCL totals");
    assert_eq!(files, 1);
    assert!(code >= 2);
    assert!(comments >= 1);
}
