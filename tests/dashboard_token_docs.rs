//! Documentation contract for dashboard access tokens (SPEC-dashboard-token-docs).

use std::path::Path;

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn section_after_heading<'a>(doc: &'a str, heading: &str) -> &'a str {
    let after = doc
        .split(heading)
        .nth(1)
        .unwrap_or_else(|| panic!("missing heading: {heading}"));
    after.split("\n### ").next().unwrap_or(after)
}

// TC-2513 / FR-2513
#[test]
fn tc_2513_readme_documents_dashboard_token() {
    let readme = read("README.md");
    let section = section_after_heading(&readme, "### Dashboard token");
    for needle in [
        "dmuxai --web",
        "dmuxai serve",
        "require_token",
        "treat",
        "scrollback",
        "browser history",
        "redact",
        "?token=",
        "DMUXAI_WEB_TOKEN",
        "DMUXAI_WEB_REQUIRE_TOKEN",
    ] {
        assert!(
            section
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
                || section.contains(needle),
            "Dashboard token section is missing: {needle}"
        );
    }
}

// TC-2514 / FR-2514
#[test]
fn tc_2965_docs_state_tokenless_default_tradeoff_wsl_and_opt_out() {
    let readme = read("README.md");
    let section = section_after_heading(&readme, "### Dashboard token");
    for needle in [
        "without",
        "require_token",
        "trust_wsl_host",
        "WSL",
        "localhost",
        "tradeoff",
        "DMUXAI_WEB_REQUIRE_TOKEN",
    ] {
        assert!(
            section
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase()),
            "README dashboard section missing: {needle}"
        );
    }
    let security = read("SECURITY.md");
    let sec = section_after_heading(&security, "## Dashboard access token");
    assert!(sec.to_ascii_lowercase().contains("wsl"));
    assert!(sec.to_ascii_lowercase().contains("require_token"));
    let api = read("docs/API.md");
    assert!(api.contains("X-Dmuxai-Client"));
    assert!(api.contains("token_required"));
}

// TC-2514 / FR-2514
#[test]
fn tc_2514_security_documents_dashboard_token() {
    let security = read("SECURITY.md");
    let section = section_after_heading(&security, "## Dashboard access token");
    for needle in [
        "require_token",
        "dmuxai --web",
        "dmuxai serve",
        "scrollback",
        "redact",
        "trust_wsl_host",
        "DMUXAI_WEB_TOKEN",
        "bug report",
    ] {
        assert!(
            section
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase()),
            "SECURITY.md dashboard section is missing: {needle}"
        );
    }
}

// TC-2515 / FR-2515
#[test]
fn tc_2515_bug_report_reminds_redact_dashboard_tokens() {
    let template = read(".github/ISSUE_TEMPLATE/bug_report.yml");
    let logs_block = template
        .split("id: logs")
        .nth(1)
        .expect("bug_report.yml must define logs field");
    let description = logs_block
        .split("description:")
        .nth(1)
        .and_then(|rest| {
            rest.lines()
                .find(|line| line.trim_start().starts_with("Redact"))
        })
        .expect("logs field must have a Redact description line");
    let lower = description.to_ascii_lowercase();
    assert!(lower.contains("dashboard"), "{description}");
    assert!(
        lower.contains("token") || lower.contains("?token"),
        "{description}"
    );
}
