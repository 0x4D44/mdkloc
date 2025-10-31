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
    while let Some(line) = it.next() {
        if line.contains("Totals by language:") {
            break;
        }
    }
    for line in it {
        if line.trim().is_empty() || line.contains("Overall Summary:") {
            break;
        }
        let parts: Vec<&str> = line.trim_start().split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        out.insert(
            parts[0].to_string(),
            (
                parse(parts[1]),
                parse(parts[2]),
                parse(parts[3]),
                parse(parts[4]),
                parse(parts[5]),
            ),
        );
    }
    out
}

#[test]
fn cli_totals_yaml_comment_only() {
    let td = TempDir::new().expect("tmp");
    write_file(
        &td.path().join("comment.yaml"),
        "# only comment\n# another\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (_, code, comments, mixed, blank) = totals.get("YAML").copied().expect("YAML totals");
    assert_eq!(code, 0);
    assert_eq!(comments, 2);
    assert_eq!(mixed, 0);
    assert_eq!(blank, 0);
}

#[test]
fn cli_totals_toml_comment_only() {
    let td = TempDir::new().expect("tmp");
    write_file(&td.path().join("comment.toml"), "# header\n# detail\n");
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (_, code, comments, mixed, blank) = totals.get("TOML").copied().expect("TOML totals");
    assert_eq!(code, 0);
    assert_eq!(comments, 2);
    assert_eq!(mixed, 0);
    assert_eq!(blank, 0);
}

#[test]
fn cli_totals_ini_mixed_comment_styles() {
    let td = TempDir::new().expect("tmp");
    write_file(
        &td.path().join("settings.ini"),
        "name=value\n; comment\nvalue = other # trailing\n\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (_, code, comments, mixed, blank) = totals.get("INI").copied().expect("INI totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 1);
    assert_eq!(mixed, 0);
    assert_eq!(blank, 1);
}

#[test]
fn cli_totals_ini_hash_comments_no_blank() {
    let td = TempDir::new().expect("tmp");
    write_file(
        &td.path().join("config.ini"),
        "[section]\n# note\n; another\nkey=value\n",
    );
    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .expect("run");
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    let (_, code, comments, mixed, blank) = totals.get("INI").copied().expect("INI totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 2);
    assert_eq!(mixed, 0);
    assert_eq!(blank, 0);
}
