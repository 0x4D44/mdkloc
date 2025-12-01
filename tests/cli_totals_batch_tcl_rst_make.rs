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
fn cli_totals_batch() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("run.bat"),
        "REM header\n:: also comment\n@echo on\nset X=1\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, _b) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("Batch")
        .copied()
        .expect("Batch totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 2);
}

#[test]
fn cli_totals_tcl() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("prog.tcl"),
        "#! /usr/bin/env tclsh\n# comment\nputs \"hello\"\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, _b) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("TCL")
        .copied()
        .expect("TCL totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 1);
}

#[test]
fn cli_totals_rst() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("doc.rst"),
        "Title\n=====\n\n.. comment\n\nParagraph text.\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, blank) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("ReStructuredText")
        .copied()
        .expect("RST totals");
    assert_eq!(code, 4);
    assert_eq!(comments, 0);
    assert_eq!(blank, 2);
}

#[test]
fn cli_totals_makefile() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("Makefile"),
        "# comment\n\nall:\n\t@echo done\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, blank) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("Makefile")
        .copied()
        .expect("Makefile totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 1);
    assert_eq!(blank, 1);
}
