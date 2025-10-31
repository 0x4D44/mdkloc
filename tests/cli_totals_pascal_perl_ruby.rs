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
fn cli_totals_perl_ruby_pascal() {
    let td = TempDir::new().unwrap();
    // Perl: shebang + POD block + code
    write_file(
        &td.path().join("t.pl"),
        "#!/usr/bin/perl\n# Line comment\n=pod\nDocumentation block\n=cut\nprint \"Hello\";\n\n",
    );
    // Ruby: shebang + line + block + code
    write_file(
        &td.path().join("t.rb"),
        "#!/usr/bin/env ruby\n# c\nputs 'Hello'\n=begin\nblock\n=end\nputs 'Goodbye'\n",
    );
    // Pascal: line + brace + paren block + code
    write_file(&td.path().join("t.pas"), "program Test;\n// line\n{ block }\nwriteln('Hello');\n(* another\nblock *)\nwriteln('Goodbye');\n");

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));

    let (_f, p_code, p_comments, _m, _b) = totals.get("Perl").copied().expect("Perl totals");
    assert!(p_code >= 2);
    assert!(p_comments >= 3);

    let (_f, r_code, r_comments, _m, _b) = totals.get("Ruby").copied().expect("Ruby totals");
    assert!(r_code >= 2);
    assert!(r_comments >= 2);

    let (_f, pas_code, pas_comments, _m, _b) =
        totals.get("Pascal").copied().expect("Pascal totals");
    assert!(pas_code >= 3);
    assert!(pas_comments >= 3);
}
