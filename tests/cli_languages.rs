use std::process::Command;

fn mdkloc_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mdkloc")
}

#[test]
fn cli_prints_supported_languages() {
    let output = Command::new(mdkloc_bin())
        .arg("--languages")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success, got status {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Supported languages:"),
        "stdout missing 'Supported languages:' header: {stdout}"
    );
    assert!(stdout.contains("Rust"), "stdout missing Rust: {stdout}");
    assert!(stdout.contains("Python"), "stdout missing Python: {stdout}");
    assert!(stdout.contains("C/C++"), "stdout missing C/C++: {stdout}");
}

#[test]
fn cli_languages_short_flag() {
    let output = Command::new(mdkloc_bin())
        .arg("-l")
        .output()
        .expect("failed to execute mdkloc");

    assert!(
        output.status.success(),
        "expected success with -l, got status {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Supported languages:"),
        "stdout missing 'Supported languages:' header with -l: {stdout}"
    );
}
