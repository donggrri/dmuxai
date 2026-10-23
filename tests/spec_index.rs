// Requirement/TC traceability: docs/specs/SPEC-spec-index.md
// FR-470 / TC-470: every docs/specs/SPEC-*.md has a Status line in its first
//   10 lines -> `every_spec_has_status_line`.
// FR-471 / TC-471: an FR/NFR/UX/INT/TC ID defined as the first cell of a table
//   row appears in at most one SPEC file -> `spec_ids_are_unique_across_specs`.
// FR-472 / TC-472: scripts/spec-index.sh prints a markdown table (SPEC link,
//   Status, ID ranges) generated from the SPEC files
//   -> `spec_index_script_lists_every_spec`.
// FR-473 / TC-473 is a manual review of docs/TEST-PLAN.md §0.1 (kept as prose
//   only) and has no automated test.

// FR-1800 / TC-1800: `python3 scripts/check-spec.py --all` exits 0 on this repo
//   with the committed allowlist -> `tc_1800_check_spec_all_passes_with_allowlist`.
// FR-1801 / TC-1801: --all reports stale and unknown allowlist entries as failures
//   -> `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist`.
// FR-1803 / TC-1803: `python3 scripts/check-tc-links.py` exits 0 on this repo
//   with the committed allowlist -> `tc_1803_tc_links_have_tests_or_allowlist`.
// FR-1804 / TC-1804: check-tc-links.py reports orphans both ways (unlinked specs,
//   undefined tests, stale allowlist entries, undefined allowlist entries)
//   -> `tc_1804_tc_link_check_reports_orphans_both_ways`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use regex::Regex;

const SPECS_DIR: &str = "docs/specs";

fn spec_paths() -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(SPECS_DIR)
        .expect("docs/specs directory readable")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("SPEC-") && name.ends_with(".md"))
        })
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "expected at least one SPEC-* file in {SPECS_DIR}"
    );
    paths
}

fn status_line(contents: &str) -> Option<String> {
    contents
        .lines()
        .take(10)
        .find_map(|line| line.strip_prefix("Status:"))
        .map(|status| status.trim().to_string())
}

/// Min and max ID number per ID prefix, over table rows whose first cell is an
/// ID definition (the same contract FR-471 protects).
fn defined_id_ranges(contents: &str) -> BTreeMap<String, (u32, u32)> {
    let row_id = Regex::new(r"^\|[[:space:]]*((FR|NFR|UX|INT|TC)-([0-9]+))[[:space:]]*\|")
        .expect("valid ID regex");
    let mut ranges = BTreeMap::new();
    for line in contents.lines() {
        if let Some(capture) = row_id.captures(line) {
            let prefix = capture.get(2).unwrap().as_str().to_string();
            let number: u32 = capture.get(3).unwrap().as_str().parse().expect("digits");
            ranges
                .entry(prefix)
                .and_modify(|low_high: &mut (u32, u32)| {
                    low_high.0 = low_high.0.min(number);
                    low_high.1 = low_high.1.max(number);
                })
                .or_insert((number, number));
        }
    }
    ranges
}

fn format_id_ranges(ranges: &BTreeMap<String, (u32, u32)>) -> String {
    if ranges.is_empty() {
        return "—".to_string();
    }
    ranges
        .iter()
        .map(|(prefix, (low, high))| {
            if low == high {
                format!("{prefix}-{low}")
            } else {
                format!("{prefix}-{low}..{high}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn every_spec_has_status_line() {
    for path in spec_paths() {
        let contents = fs::read_to_string(&path).expect("SPEC file readable");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let status = status_line(&contents).unwrap_or_else(|| {
            panic!("{name} has no `Status:` line within its first 10 lines (FR-470)")
        });
        assert!(!status.is_empty(), "{name} has an empty `Status:` line");
    }
}

#[test]
fn spec_ids_are_unique_across_specs() {
    let row_id = Regex::new(r"^\|[[:space:]]*((FR|NFR|UX|INT|TC)-[0-9]+)[[:space:]]*\|")
        .expect("valid ID regex");
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in spec_paths() {
        let contents = fs::read_to_string(&path).expect("SPEC file readable");
        let owner = path.file_name().unwrap().to_string_lossy().into_owned();
        for line in contents.lines() {
            if let Some(capture) = row_id.captures(line) {
                owners
                    .entry(capture.get(1).unwrap().as_str().to_string())
                    .or_default()
                    .push(owner.clone());
            }
        }
    }
    let duplicates: Vec<String> = owners
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(id, files)| format!("{id} defined in {}", files.join(", ")))
        .collect();
    assert!(
        duplicates.is_empty(),
        "FR-471 violated; each ID must be defined as a table-row first cell in \
         at most one SPEC file:\n{}",
        duplicates.join("\n")
    );
}

#[test]
fn spec_index_script_lists_every_spec() {
    let output = Command::new("bash")
        .arg("scripts/spec-index.sh")
        .output()
        .expect("bash must be available to run scripts/spec-index.sh");
    assert!(
        output.status.success(),
        "spec-index.sh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("| SPEC | Status | ID ranges |"),
        "output must be a markdown table with SPEC link, Status, and ID range columns"
    );

    let row = Regex::new(
        r"^\|[[:space:]]*\[(SPEC-[^\]]+)\]\(docs/specs/[^\)]+\)[[:space:]]*\|([^|]+)\|([^|]+)\|$",
    )
    .expect("valid row regex");
    let mut rows = BTreeMap::new();
    for line in stdout.lines().skip(2) {
        if let Some(capture) = row.captures(line) {
            let status = capture.get(2).unwrap().as_str().trim().to_string();
            let ranges = capture.get(3).unwrap().as_str().trim().to_string();
            rows.insert(
                capture.get(1).unwrap().as_str().to_string(),
                (status, ranges),
            );
        }
    }

    for path in spec_paths() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let contents = fs::read_to_string(&path).expect("SPEC file readable");
        let expected_status =
            status_line(&contents).expect("FR-470 guarantees a Status line for every row");
        let expected_ranges = format_id_ranges(&defined_id_ranges(&contents));
        let (status, ranges) = rows
            .get(&name)
            .unwrap_or_else(|| panic!("spec-index.sh output is missing a row for {name}"));
        assert_eq!(status, &expected_status, "Status cell of the {name} row");
        assert_eq!(ranges, &expected_ranges, "ID ranges cell of the {name} row");
    }

    for name in rows.keys() {
        assert!(
            Path::new(SPECS_DIR).join(name).is_file(),
            "spec-index.sh emitted a row for nonexistent SPEC file {name}"
        );
    }
}

#[test]
fn tc_1800_check_spec_all_passes_with_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("python3")
        .arg("scripts/check-spec.py")
        .arg("--all")
        .current_dir(root)
        .output()
        .expect("python3 must be available to run scripts/check-spec.py --all");
    assert!(
        output.status.success(),
        "check-spec.py --all failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tc_1801_check_spec_all_flags_stale_and_unknown_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let specs_dir = std::env::temp_dir().join(format!(
        "dmuxai-spec-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&specs_dir).expect("create temp specs dir");

    let passing_spec = specs_dir.join("SPEC-passing.md");
    let failing_spec = specs_dir.join("SPEC-failing.md");
    fs::write(
        &passing_spec,
        "Status: Implemented\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | E |\n|---|---|---|---|---|---|\n\
         | FR-999 | Must | given | then | none | TC-999 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-999 | FR-999 | unit | s | a | o |\n",
    )
    .expect("write passing spec");
    fs::write(&failing_spec, "# failing spec\n").expect("write failing spec");

    let allowlist = std::env::temp_dir().join(format!(
        "dmuxai-spec-allowlist-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    // Bad allowlist: passing spec (stale) plus a nonexistent entry (unknown).
    fs::write(&allowlist, "SPEC-passing.md\nSPEC-does-not-exist.md\n")
        .expect("write bad allowlist");

    let bad_output = Command::new("python3")
        .arg("scripts/check-spec.py")
        .arg("--all")
        .arg("--specs-dir")
        .arg(&specs_dir)
        .arg("--allowlist")
        .arg(&allowlist)
        .current_dir(root)
        .output()
        .expect("python3 must be available to run scripts/check-spec.py --all");
    assert!(
        !bad_output.status.success(),
        "bad allowlist run unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bad_output.stdout),
        String::from_utf8_lossy(&bad_output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&bad_output.stdout),
        String::from_utf8_lossy(&bad_output.stderr)
    );
    assert!(
        combined.contains("STALE"),
        "output must mention STALE:\n{combined}"
    );
    assert!(
        combined.contains("SPEC-does-not-exist.md"),
        "output must mention the unknown entry:\n{combined}"
    );

    // Single-file mode checks (TC-1802).
    let single_fail = Command::new("python3")
        .arg("scripts/check-spec.py")
        .arg(&failing_spec)
        .current_dir(root)
        .output()
        .expect("python3 must be available");
    assert_eq!(
        single_fail.status.code(),
        Some(1),
        "failing spec single-file should exit 1"
    );

    let single_pass = Command::new("python3")
        .arg("scripts/check-spec.py")
        .arg(&passing_spec)
        .current_dir(root)
        .output()
        .expect("python3 must be available");
    assert_eq!(
        single_pass.status.code(),
        Some(0),
        "passing spec single-file should exit 0"
    );

    // Correct allowlist: only the failing spec.
    fs::write(&allowlist, "SPEC-failing.md\n").expect("write good allowlist");
    let good_output = Command::new("python3")
        .arg("scripts/check-spec.py")
        .arg("--all")
        .arg("--specs-dir")
        .arg(&specs_dir)
        .arg("--allowlist")
        .arg(&allowlist)
        .current_dir(root)
        .output()
        .expect("python3 must be available to run scripts/check-spec.py --all");
    assert!(
        good_output.status.success(),
        "good allowlist run failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        good_output.status.code(),
        String::from_utf8_lossy(&good_output.stdout),
        String::from_utf8_lossy(&good_output.stderr)
    );

    let _ = fs::remove_dir_all(&specs_dir);
    let _ = fs::remove_file(&allowlist);
}

#[test]
fn tc_1803_tc_links_have_tests_or_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("python3")
        .arg("scripts/check-tc-links.py")
        .current_dir(root)
        .output()
        .expect("python3 must be available to run scripts/check-tc-links.py");
    assert!(
        output.status.success(),
        "check-tc-links.py failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tc_1804_tc_link_check_reports_orphans_both_ways() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp_root = std::env::temp_dir().join(format!(
        "dmuxai-tc-link-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_root.join("docs/specs")).expect("create temp docs/specs");
    fs::create_dir_all(temp_root.join("src")).expect("create temp src");
    fs::create_dir_all(temp_root.join("scripts")).expect("create temp scripts");

    fs::write(
        temp_root.join("docs/specs/SPEC-x.md"),
        "Status: Draft\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | Evidence |\n|---|---|---|---|---|---|\n\
         | FR-1 | Must | p | r | f | TC-1 |\n\
         | FR-2 | Must | p | r | f | TC-2 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-1 | FR-1 | unit | s | a | o | loc |\n\
         | TC-2 | FR-2 | unit | s | a | o | loc |\n",
    )
    .expect("write SPEC-x.md");

    fs::write(
        temp_root.join("src/a.rs"),
        "#[test]\nfn tc_1_ok() {}\n\n#[test]\nfn tc_9_bad() {}\n",
    )
    .expect("write src/a.rs");

    let allowlist = temp_root.join("scripts/tc-link-allowlist.txt");
    fs::write(&allowlist, "TC-1 manual\nTC-3 planned\n").expect("write allowlist");

    let script = root.join("scripts/check-tc-links.py");
    let bad_output = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .arg("--allowlist")
        .arg(&allowlist)
        .output()
        .expect("python3 must run scripts/check-tc-links.py");
    assert_eq!(
        bad_output.status.code(),
        Some(1),
        "bad fixture should exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bad_output.stdout),
        String::from_utf8_lossy(&bad_output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&bad_output.stdout),
        String::from_utf8_lossy(&bad_output.stderr)
    );
    assert!(
        combined.contains("TC-2") && combined.to_lowercase().contains("orphan"),
        "output must name TC-2 (orphan):\n{combined}"
    );
    assert!(
        combined.contains("TC-9") && combined.contains("undefined"),
        "output must name TC-9 (undefined test):\n{combined}"
    );
    assert!(
        combined.contains("TC-1") && combined.to_lowercase().contains("stale"),
        "output must name TC-1 (stale):\n{combined}"
    );
    assert!(
        combined.contains("TC-3") && combined.contains("undefined"),
        "output must name TC-3 (undefined allowlist):\n{combined}"
    );

    // Clean fixture: TC-1 has test tc_1_ok; TC-2 is allowlisted; no undefined test or allowlist.
    fs::write(temp_root.join("src/a.rs"), "#[test]\nfn tc_1_ok() {}\n")
        .expect("write clean src/a.rs");
    fs::write(&allowlist, "TC-2 manual\n").expect("write clean allowlist");

    let good_output = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .arg("--allowlist")
        .arg(&allowlist)
        .output()
        .expect("python3 must run scripts/check-tc-links.py");
    assert!(
        good_output.status.success(),
        "clean fixture should exit 0, got {:?}\nstdout:\n{}\nstderr:\n{}",
        good_output.status.code(),
        String::from_utf8_lossy(&good_output.stdout),
        String::from_utf8_lossy(&good_output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn tc_1805_gate_script_help_and_linux_guard() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--help")
        .current_dir(root)
        .output()
        .expect("bash must be available to run scripts/gate.sh --help");
    assert!(
        output.status.success(),
        "gate.sh --help failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for opt in ["--isolated", "--browser", "--quick", "--keep-going"] {
        assert!(
            combined.contains(opt),
            "help text must mention {opt}:\n{combined}"
        );
    }
}

#[test]
fn tc_1806_gate_script_isolated_mode_unsets_dmuxai_vars() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let isolated = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--print-env")
        .env("DMUXAI_OPENCODE_BIN", "/nonexistent")
        .current_dir(root)
        .output()
        .expect("bash must be available to run scripts/gate.sh --print-env");
    assert!(
        isolated.status.success(),
        "isolated --print-env failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        isolated.status.code(),
        String::from_utf8_lossy(&isolated.stdout),
        String::from_utf8_lossy(&isolated.stderr)
    );
    let isolated_out = String::from_utf8_lossy(&isolated.stdout);
    assert!(
        !isolated_out.contains("DMUXAI_OPENCODE_BIN"),
        "isolated --print-env must not list DMUXAI_OPENCODE_BIN:\n{isolated_out}"
    );

    let non_isolated = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--no-isolated")
        .arg("--print-env")
        .env("DMUXAI_OPENCODE_BIN", "/nonexistent")
        .current_dir(root)
        .output()
        .expect("bash must be available to run scripts/gate.sh --print-env");
    assert!(
        non_isolated.status.success(),
        "non-isolated --print-env failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        non_isolated.status.code(),
        String::from_utf8_lossy(&non_isolated.stdout),
        String::from_utf8_lossy(&non_isolated.stderr)
    );
    let non_isolated_out = String::from_utf8_lossy(&non_isolated.stdout);
    assert!(
        non_isolated_out.contains("DMUXAI_OPENCODE_BIN"),
        "non-isolated --print-env must list DMUXAI_OPENCODE_BIN:\n{non_isolated_out}"
    );
}
