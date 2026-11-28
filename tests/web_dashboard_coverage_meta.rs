//! TC-3764 (NFR-3750, SPEC-dashboard-feature-coverage): the new browser tests use
//! the shared temp-env helper and a probe deadline under 60 s, skip without
//! Chrome/Playwright, are `#[ignore]`d, and never touch the fixed dashboard
//! ports, `--allow-sensitive` or `--trust-workspace-config`. Also checks that
//! every TC-3750..TC-3764 named in the spec exists where §5 says it does.

use std::path::Path;

/// (file, test function name prefix, uses `common::` helpers)
const BROWSER_TESTS: &[(&str, &str, bool)] = &[
    ("tests/web_chat_copy.rs", "tc_3750_", false),
    ("tests/web_chat_single_send.rs", "tc_3751_", false),
    ("tests/web_model_menu.rs", "tc_3752_", false),
    (
        "tests/web_followup_queue.rs",
        "tc_1230_1231_1232_1233_1234_1235_1236_3753_",
        false,
    ),
    ("tests/web_dashboard_views.rs", "tc_3754_", true),
    ("tests/web_dashboard_views.rs", "tc_3755_", true),
    ("tests/web_board_e2e.rs", "tc_3756_", false),
    ("tests/web_board_e2e.rs", "tc_3757_", false),
    ("tests/web_attachments.rs", "tc_3758_", true),
    ("tests/web_session_sidebar.rs", "tc_3759_", true),
    ("tests/web_workspace_dir.rs", "tc_3760_", true),
    ("tests/web_memory_panel.rs", "tc_3763_", true),
];

/// New headless-Chrome tests written for this spec (TC-3754..TC-3763): these
/// must use the shared temp-env helper, not hand-rolled environments.
const NEW_BROWSER_TESTS: &[(&str, &str)] = &[
    ("tests/web_dashboard_views.rs", "tc_3754_"),
    ("tests/web_dashboard_views.rs", "tc_3755_"),
    ("tests/web_board_e2e.rs", "tc_3756_"),
    ("tests/web_board_e2e.rs", "tc_3757_"),
    ("tests/web_attachments.rs", "tc_3758_"),
    ("tests/web_session_sidebar.rs", "tc_3759_"),
    ("tests/web_workspace_dir.rs", "tc_3760_"),
    ("tests/web_memory_panel.rs", "tc_3763_"),
];

fn read(path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

/// Text of the test function whose name starts with `prefix`, with its
/// attributes: from the preceding `#[test]`/`#[tokio::test` line to the next
/// top-level `fn`/`async fn`/`// TC-` marker.
fn function_text(source: &str, prefix: &str) -> Option<String> {
    let needle = format!("fn {prefix}");
    let at = source.find(&needle)?;
    let start = source[..at]
        .rfind("\n#[test]")
        .or_else(|| source[..at].rfind("\n#[tokio::test"))
        .map(|i| i + 1)
        .unwrap_or(at);
    let rest = &source[at + needle.len()..];
    let end = [
        "\nfn ",
        "\nasync fn ",
        "\n// TC-",
        "\n#[test]",
        "\n#[tokio::test",
    ]
    .iter()
    .filter_map(|marker| rest.find(marker))
    .min()
    .map(|i| at + needle.len() + i)
    .unwrap_or(source.len());
    Some(source[start..end].to_owned())
}

// TC-3764 (NFR-3750)
#[test]
fn tc_3764_new_browser_tests_use_shared_env_and_deadline() {
    for (file, prefix, _) in BROWSER_TESTS {
        let source = read(file);
        let body = function_text(&source, prefix)
            .unwrap_or_else(|| panic!("{file}: no test named {prefix}*"));
        let at = source.find(&format!("fn {prefix}")).unwrap();
        let attrs = &source[at.saturating_sub(300)..at];
        assert!(
            attrs.contains("#[ignore"),
            "{file} {prefix}: browser tests are #[ignore]d"
        );
        assert!(
            !body.contains("--allow-sensitive") && !body.contains("trust-workspace-config"),
            "{file} {prefix}: must not use --allow-sensitive or --trust-workspace-config"
        );
        for port in ["7420", "7472"] {
            assert!(
                !body.contains(port),
                "{file} {prefix}: must not use the fixed port {port}"
            );
        }
    }

    for (file, prefix) in NEW_BROWSER_TESTS {
        let source = read(file);
        let body = function_text(&source, prefix).unwrap();
        assert!(
            body.contains("prepare()"),
            "{file} {prefix}: must SKIP via prepare() without Chrome/Playwright"
        );
        assert!(
            body.contains("setup_env()"),
            "{file} {prefix}: must build its workspace/state/config with setup_env()"
        );
        assert!(
            body.contains("run_probe("),
            "{file} {prefix}: probes must run through run_probe (deadline)"
        );
        assert!(
            body.contains("start_web(") || body.contains("start_serve("),
            "{file} {prefix}: must start dmuxai on port 0 via start_web/start_serve"
        );
    }

    // The shared helper: temp dirs, port 0, deadline < 60 s, child kill on drop.
    let common = read("tests/common/mod.rs");
    assert!(common.contains("tempfile::tempdir()"), "temp root");
    assert!(common.contains("\"--port\", \"0\""), "port 0");
    assert!(
        common.contains("impl Drop for ChildGuard"),
        "no leftover process"
    );
    let deadline = common
        .lines()
        .find(|l| l.contains("PROBE_DEADLINE: Duration"))
        .expect("PROBE_DEADLINE");
    let secs: u64 = deadline
        .split("from_secs(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.trim().parse().ok())
        .expect("deadline seconds");
    assert!(secs < 60, "probe deadline must be under 60 s, got {secs}");
    for (env_key, label) in [
        ("DMUXAI_STATE_DIR", "state dir"),
        ("XDG_CONFIG_HOME", "config dir"),
        ("HOME", "home"),
    ] {
        assert!(
            common.contains(env_key),
            "common helper isolates the {label}"
        );
    }

    // web_board_e2e keeps its own helper with the same limits.
    let board = read("tests/web_board_e2e.rs");
    assert!(board.contains("fn setup_env() -> Env"));
    // After #372, run_probe delegates to run_probe_for(..., Duration::from_secs(55)).
    assert!(
        board.contains("recv_timeout(Duration::from_secs(55))")
            || (board.contains("run_probe_for(")
                && board.contains("Duration::from_secs(55)")
                && board.contains("recv_timeout(timeout)")),
        "web_board_e2e probe deadline is 55 s"
    );
    assert!(board.contains("\"--port\""));
}

// TC-3764: the renamed existing tests keep their evidence names (FR-3750..FR-3753).
#[test]
fn tc_3764_spec_names_exist_in_the_listed_files() {
    let spec = read("docs/specs/SPEC-dashboard-feature-coverage.md");
    for tc in 3750..=3764 {
        assert!(spec.contains(&format!("| TC-{tc} |")), "spec lists TC-{tc}");
    }
    for (file, prefix, _) in BROWSER_TESTS {
        let source = read(file);
        assert!(
            source.contains(&format!("fn {prefix}")),
            "{file} has {prefix}*"
        );
        assert!(
            spec.contains(file.trim_start_matches("tests/")) || spec.contains(file),
            "spec §5 lists {file}"
        );
    }
}
