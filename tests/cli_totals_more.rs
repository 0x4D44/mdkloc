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
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let lang = cols[0].to_string();
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        out.insert(
            lang,
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
fn cli_totals_ini_language() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // INI with two code lines, two comment lines, one blank
    write_file(
        &root.join("config.ini"),
        "; top comment\n# another\n[core]\nname = demo\n\n",
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
    let totals = parse_totals(&String::from_utf8_lossy(&output.stdout));
    let (_, code, comments, mixed, blank) = totals.get("INI").copied().expect("INI totals");
    assert_eq!(code, 2);
    assert_eq!(comments, 2);
    assert_eq!(mixed, 0);
    assert_eq!(blank, 1);
}

#[test]
fn cli_totals_xml_and_html_languages() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let root = temp_dir.path();

    // XML: code >=3, comments >=2 (rough structure)
    write_file(
        &root.join("data.xml"),
        "<root>\n<!-- c1 -->\n<!--\n block\n-->\n<child/>\n</root>\n",
    );
    // HTML: code >=5, comments >=2
    write_file(
        &root.join("index.html"),
        "<html>\n<body>\n<!-- banner -->\n<div>hi</div>\n<!--\n multi\n-->\n</body>\n</html>\n",
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
    let totals = parse_totals(&String::from_utf8_lossy(&output.stdout));

    let (_, xml_code, xml_comments, _xml_mixed, _xml_blank) =
        totals.get("XML").copied().expect("XML totals");
    assert!(xml_code >= 3);
    assert!(xml_comments >= 2);

    let (_, html_code, html_comments, _html_mixed, _html_blank) =
        totals.get("HTML").copied().expect("HTML totals");
    assert!(html_code >= 5);
    assert!(html_comments >= 2);
}
