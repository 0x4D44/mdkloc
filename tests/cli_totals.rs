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
    // Map: lang -> (files, code, comments, mixed, blank)
    let mut out = HashMap::new();
    let mut iter = stdout.lines();
    // Seek to the totals section
    while let Some(line) = iter.next() {
        if line.contains("Totals by language:") {
            break;
        }
    }
    // Read until a blank line or "Overall Summary:" appears
    for line in iter {
        if line.trim().is_empty() || line.contains("Overall Summary:") {
            break;
        }
        // Each totals row has fixed columns; language sits after ~40 spaces
        // Example: "                                        YAML             1         2          1          0          1"
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // Split into tokens keeping language (single token, no spaces in names we use)
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let lang = parts[0].to_string();
        // Files, Code, Comments, Mixed, Blank follow
        let parse_u64 = |s: &str| s.parse::<u64>().unwrap_or(0);
        let files = parse_u64(parts[1]);
        let code = parse_u64(parts[2]);
        let comments = parse_u64(parts[3]);
        let mixed = parse_u64(parts[4]);
        let blank = parse_u64(parts[5]);
        out.insert(lang, (files, code, comments, mixed, blank));
    }
    out
}

#[test]
fn cli_totals_hash_and_json_languages() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // YAML: 3 total lines => code=1, comments=1, blank=1
    write_file(&root.join("a.yaml"), "\n# comment line\nkey: value\n");

    // TOML: 3 total lines => code=1, comments=1, blank=1
    write_file(
        &root.join("b.toml"),
        "# header comment\n\nname = 'demo' # trailing\n",
    );

    // JSON: 5 total lines => code=4, comments=0, blank=1
    write_file(
        &root.join("c.json"),
        "{\n  \"k\": 1,\n  \"arr\": [1,2]\n}\n\n",
    );

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
    let totals = parse_totals(&stdout);

    // YAML
    let (_, code, comments, mixed, blank) = totals
        .get("YAML")
        .copied()
        .expect("expected YAML in totals");
    assert_eq!(code, 1, "yaml code");
    assert_eq!(comments, 1, "yaml comments");
    assert_eq!(mixed, 0, "yaml mixed");
    assert_eq!(blank, 1, "yaml blank");

    // TOML
    let (_, code, comments, mixed, blank) = totals
        .get("TOML")
        .copied()
        .expect("expected TOML in totals");
    assert_eq!(code, 1, "toml code");
    assert_eq!(comments, 1, "toml comments");
    assert_eq!(mixed, 0, "toml mixed");
    assert_eq!(blank, 1, "toml blank");

    // JSON
    let (_, code, comments, mixed, blank) = totals
        .get("JSON")
        .copied()
        .expect("expected JSON in totals");
    assert_eq!(code, 4, "json code");
    assert_eq!(comments, 0, "json comments");
    assert_eq!(mixed, 0, "json mixed");
    assert_eq!(blank, 1, "json blank");
}
