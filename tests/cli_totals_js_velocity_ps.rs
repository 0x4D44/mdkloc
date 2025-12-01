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
fn cli_totals_javascript() {
    let td = TempDir::new().unwrap();
    // .js inline block with trailing code
    write_file(
        &td.path().join("a.js"),
        "const a=1; /* block */ const b=2;\n",
    );
    // .js line comment
    write_file(&td.path().join("b.js"), "let x=3; // note\n");
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (files, code, comments, _mixed, _blank) =
        parse_totals(&String::from_utf8_lossy(&out.stdout))
            .get("JavaScript")
            .copied()
            .expect("JavaScript totals");
    assert_eq!(files, 2);
    assert!(code >= 2);
    assert!(comments >= 1);
}

#[test]
fn cli_totals_velocity() {
    let td = TempDir::new().unwrap();
    // Velocity line and block
    write_file(
        &td.path().join("t.vm"),
        "## line\nHello #* block *# World\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_files, code, comments, _mixed, _blank) =
        parse_totals(&String::from_utf8_lossy(&out.stdout))
            .get("Velocity")
            .copied()
            .expect("Velocity totals");
    assert!(code >= 1);
    assert!(comments >= 1);
}

#[test]
fn cli_totals_powershell() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("s.ps1"),
        "# c\nWrite-Host 'x'\n<# block\ncomment #> Write-Host 'after'\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_files, code, comments, _mixed, _blank) =
        parse_totals(&String::from_utf8_lossy(&out.stdout))
            .get("PowerShell")
            .copied()
            .expect("PowerShell totals");
    assert!(code >= 2);
    assert!(comments >= 2);
}
