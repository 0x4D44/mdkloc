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
fn cli_totals_shell_dotfiles_and_special_names() {
    let td = TempDir::new().unwrap();
    // Dotfiles that should detect as Shell
    for name in [
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".zshrc",
        ".zprofile",
        ".zshenv",
        ".kshrc",
        ".cshrc",
    ] {
        write_file(&td.path().join(name), "echo hi\n# note\n");
    }
    // Special names
    write_file(&td.path().join("Makefile"), "all:\n\t@echo hi\n");
    write_file(&td.path().join("CMakeLists.txt"), "project(x) # note\n");
    write_file(&td.path().join("Dockerfile"), "FROM alpine\n# c\n");

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));
    assert!(
        totals.contains_key("Shell"),
        "expected Shell totals for dotfiles"
    );
    assert!(totals.contains_key("Makefile"), "expected Makefile totals");
    assert!(totals.contains_key("CMake"), "expected CMake totals");
    assert!(
        totals.contains_key("Dockerfile"),
        "expected Dockerfile totals"
    );
}
