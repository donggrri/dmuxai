// Requirement/TC traceability: docs/specs/SPEC-security-reporting.md
// FR-2516, FR-2517 / TC-2516: SECURITY.md documents reporting channels.
// FR-2518 / TC-2517: RELEASE-PROCESS.md verifies private vulnerability reporting before release.

use std::fs;

fn reporting_section(security_md: &str) -> &str {
    let start = security_md
        .find("## Reporting a vulnerability")
        .expect("SECURITY.md must have a Reporting a vulnerability section");
    let rest = &security_md[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn tc_2516_security_md_documents_reporting_channels() {
    let security = fs::read_to_string("SECURITY.md").expect("SECURITY.md readable");
    let section = reporting_section(&security);

    assert!(
        section.contains("**Security**") && section.contains("Report a vulnerability"),
        "SECURITY.md must name the Security tab Report a vulnerability form (FR-2516)"
    );
    assert!(
        section.contains("minimal public issue"),
        "SECURITY.md must document the public-issue fallback (FR-2517)"
    );
    assert!(
        section.contains("Do **not** include vulnerability details"),
        "SECURITY.md must forbid vulnerability details in the fallback issue (FR-2517)"
    );
    assert!(
        !section.to_lowercase().contains("mailto:") && !section.contains('@'),
        "SECURITY.md reporting section must not invent email or mailto contacts (FR-2517)"
    );
}

#[test]
fn tc_2517_release_process_checks_private_vulnerability_reporting() {
    let release = fs::read_to_string("docs/RELEASE-PROCESS.md").expect("RELEASE-PROCESS readable");
    assert!(
        release.contains("Verify private vulnerability reporting"),
        "RELEASE-PROCESS step 0 must verify private vulnerability reporting (FR-2518)"
    );
    assert!(
        release.contains("Report a vulnerability"),
        "RELEASE-PROCESS checklist must reference the Security form (FR-2518)"
    );
}
