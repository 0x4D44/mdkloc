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
fn cli_totals_algol_cobol_fortran_dcl() {
    let td = TempDir::new().unwrap();

    // Algol
    write_file(
        &td.path().join("a.alg"),
        "begin\nCOMMENT this is a comment;\nend\n",
    );
    // COBOL
    write_file(&td.path().join("p.cob"), "       IDENTIFICATION DIVISION.\n      * comment in col 7\n       PROGRAM-ID. DEMO.\n       *> free comment\n");
    // Fortran
    write_file(
        &td.path().join("m.f90"),
        "! comment\nprogram x\nprint *, 'hi'\nend\n",
    );
    // DCL
    write_file(
        &td.path().join("proc.com"),
        "$! comment\n$ write sys$output \"hi\"\n",
    );

    let out = Command::new(mdkloc_bin())
        .arg(td.path())
        .arg("--non-recursive")
        .output()
        .unwrap();
    assert!(out.status.success());
    let totals = parse_totals(&String::from_utf8_lossy(&out.stdout));

    let (_f, a_code, a_comments, _m, _b) = totals.get("Algol").copied().expect("Algol totals");
    assert!(a_code >= 2);
    assert!(a_comments >= 1);

    let (_f, cob_code, cob_comments, _m, _b) = totals.get("COBOL").copied().expect("COBOL totals");
    assert!(cob_code >= 2);
    assert!(cob_comments >= 2);

    let (_f, f_code, f_comments, _m, _b) = totals.get("Fortran").copied().expect("Fortran totals");
    assert!(f_code >= 3);
    assert!(f_comments >= 1);

    let (_f, d_code, d_comments, _m, _b) = totals.get("DCL").copied().expect("DCL totals");
    assert!(d_code >= 1);
    assert!(d_comments >= 1);
}
