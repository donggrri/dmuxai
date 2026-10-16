use std::process::{Command, Stdio};

const EXPECTED_NOTTY_MESSAGE: &str =
    "dmuxai needs an interactive terminal (TTY); run it in a terminal, or see dmuxai --help";

/// TC-380 / FR-380: `dmuxai --help` and `-h` print usage and exit 0 without requiring a TTY.
#[test]
fn help_flag_prints_usage_without_tty() {
    for flag in ["--help", "-h"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");

        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit code 0 for {flag}, got: {:?}",
            output.status
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("DmuxAI"),
            "stdout missing DmuxAI description for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("dmuxai [--help|--version]"),
            "stdout missing usage pattern for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("DMUXAI_FORCE_COLOR"),
            "stdout missing DMUXAI_FORCE_COLOR for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("NO_COLOR"),
            "stdout missing NO_COLOR for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("XDG_STATE_HOME"),
            "stdout missing XDG_STATE_HOME for {flag}: {stdout}"
        );
        assert!(
            stdout.contains("dmuxai.toml"),
            "stdout missing dmuxai.toml for {flag}: {stdout}"
        );
    }
}

/// TC-381 / FR-381: `dmuxai --version` and `-V` print version string and exit 0 without a TTY.
#[test]
fn version_flag_prints_version_without_tty() {
    let expected = format!("dmuxai {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("execute dmuxai");

        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit code 0 for {flag}, got: {:?}",
            output.status
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout, expected,
            "expected version string {expected}, got {stdout}"
        );
    }
}

/// TC-382 / FR-380: unknown flags print an error message, show usage, and exit 2.
#[test]
fn unknown_flag_exits_with_code_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .arg("--unknown-flag")
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: unexpected argument '--unknown-flag'"),
        "stderr missing error description: {stderr}"
    );
    assert!(
        stderr.contains("dmuxai [--help|--version]"),
        "stderr missing usage text: {stderr}"
    );
}

/// TC-382 / FR-382: starting without a TTY prints a clear error message to stderr and exits 1 without "Os code 6".
#[test]
fn non_tty_start_fails_with_clear_message() {
    let output = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .stdin(Stdio::null())
        .output()
        .expect("execute dmuxai");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for non-TTY start, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(EXPECTED_NOTTY_MESSAGE),
        "expected clear non-TTY error message, got: {stderr}"
    );
    assert!(
        !stderr.contains("Os { code: 6"),
        "stderr should not mention Os code 6, got: {stderr}"
    );
}
