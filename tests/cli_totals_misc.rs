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
    while let Some(l) = it.next() {
        if l.contains("Totals by language:") {
            break;
        }
    }
    for l in it {
        if l.trim().is_empty() || l.contains("Overall Summary:") {
            break;
        }
        let cols: Vec<&str> = l.trim_start().split_whitespace().collect();
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
fn cli_totals_mustache() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("view.mustache"),
        "{{! top }}\nHello {{name}}\n{{! multi\n line }}\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, _b) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("Mustache")
        .copied()
        .expect("Mustache totals");
    assert!(code >= 1);
    assert!(comments >= 2);
}

#[test]
fn cli_totals_cmake() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("CMakeLists.txt"),
        "# top\ncmake_minimum_required(VERSION 3.25)\nproject(demo)\n# end\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, _b) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("CMake")
        .copied()
        .expect("CMake totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 2);
}

#[test]
fn cli_totals_dockerfile() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("Dockerfile"),
        "FROM alpine\n# comment\n\nRUN echo hi\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, blank) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("Dockerfile")
        .copied()
        .expect("Dockerfile totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 1);
    assert_eq!(blank, 1);
}

#[test]
fn cli_totals_shell() {
    let td = TempDir::new().unwrap();
    write_file(
        &td.path().join("test.sh"),
        "#!/bin/bash\n# This is a comment\necho \"Hello, world!\"\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (_f, code, comments, _m, _b) = parse_totals(&String::from_utf8_lossy(&out.stdout))
        .get("Shell")
        .copied()
        .expect("Shell totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 1);
}
