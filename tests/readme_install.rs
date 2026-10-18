//! Documentation contract for the install steps (SPEC-first-run TC-815,
//! FR-815, FR-816): the README matches the release archive layout.

use std::path::Path;

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn readme_matches_release_layout() {
    let readme = read("README.md");
    let release = read(".github/workflows/release.yml");

    // The Linux archive's top directory, as staged by the release workflow.
    let directory = release
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("staging_directory=\"$RUNNER_TEMP/")?
                .strip_suffix('"')
        })
        .expect("release.yml stages the Linux archive in a named directory");
    assert_eq!(directory, "dmuxai-linux-x86_64", "archive directory");
    assert!(
        release.contains("cp README.md \"$staging_directory/README.md\""),
        "release.yml must ship README.md"
    );
    assert!(
        release.contains("\"$staging_directory/CHANGELOG.md\""),
        "release.yml must ship CHANGELOG.md"
    );
    assert!(
        release.contains(&format!(
            "tar -C \"$RUNNER_TEMP\" -czf \"$archive_name\" {directory}"
        )),
        "the archive keeps its top directory"
    );

    // The README names the archive contents and the install steps.
    assert!(
        readme.contains(&format!("{directory}/{{dmuxai,README.md,CHANGELOG.md}}")),
        "README must describe the archive contents"
    );
    for step in [
        "## First run",
        "dmuxai init",
        "mkdir -p ~/.local/bin",
        &format!("install -m 0755 {directory}/dmuxai ~/.local/bin/dmuxai"),
        &format!(
            "tar -xzf dmuxai-linux-x86_64.tar.gz -C ~/.local/bin --strip-components=1 {directory}/dmuxai"
        ),
        "export PATH=\"$HOME/.local/bin:$PATH\"",
        "dmuxai --version",
    ] {
        assert!(readme.contains(step), "README is missing: {step}");
    }
}

// TC-905 / FR-900
#[test]
fn readme_and_changelog_mention_builtin_grok() {
    let readme = read("README.md");
    let first_run = readme
        .split("## First run")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("First run section");
    for needle in [
        "DMUXAI_GROK_BIN",
        "~/.grok/bin/grok",
        "grok-4.7",
        "fast",
        "grok login",
        "builtin_grok",
        "allowed_runners",
    ] {
        assert!(
            first_run.contains(needle),
            "First run is missing {needle}: {first_run}"
        );
    }
    let changelog = read("CHANGELOG.md");
    let unreleased = changelog
        .split("## Unreleased")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("Unreleased section");
    assert!(
        unreleased.contains("grok"),
        "CHANGELOG must state the new runner"
    );
    assert!(
        unreleased.to_ascii_lowercase().contains("graph"),
        "CHANGELOG must state the graph exception"
    );
}
