// Requirement/TC traceability: docs/specs/SPEC-spec-index.md
// FR-470 / TC-470: every docs/specs/SPEC-*.md has a Status line in its first
//   10 lines -> `every_spec_has_status_line`.
// FR-471 / TC-471: an FR/NFR/UX/INT/TC ID defined as the first cell of a table
//   row appears in at most one SPEC file -> `spec_ids_are_unique_across_specs`.
// FR-472 / TC-472: scripts/spec-index.sh prints a markdown table (SPEC link,
//   Status, ID ranges) generated from the SPEC files
//   -> `spec_index_script_lists_every_spec`.
// FR-473 / TC-473: TEST-PLAN §0.1 has no per-spec rows and points to the generator and rule.
//   -> `tc_473_test_plan_section_0_1_has_no_per_spec_rows`.

// FR-1800 / TC-1800: `python3 scripts/check-spec.py --all` exits 0 on this repo
//   with the committed allowlist -> `tc_1800_check_spec_all_passes_with_allowlist`.
// FR-1801 / TC-1801: --all reports stale and unknown allowlist entries as failures
//   -> `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist`.
// FR-1803 / TC-1803: `python3 scripts/check-tc-links.py` exits 0 on this repo
//   with the committed allowlist -> `tc_1803_tc_links_have_tests_or_allowlist`.
// FR-1804 / TC-1804: check-tc-links.py reports orphans both ways (unlinked specs,
//   undefined tests, stale allowlist entries, undefined allowlist entries)
//   -> `tc_1804_tc_link_check_reports_orphans_both_ways`.
// FR-2552 / TC-2551: check-tc-links.py ratchet fails on raised baseline or exceeding counts
//   -> `tc_2551_tc_link_baseline_ratchet`.
// FR-2553 / TC-2552: check-tc-links.py --base rejects new allowlist entries and raised baseline
//   -> `tc_2552_tc_link_base_rejects_new_allowlist_and_raised_baseline`.
// FR-2554 / TC-2553: check-tc-links.py --base requires strong links for changed TCs
//   -> `tc_2553_tc_link_base_requires_strong_links_for_changed_tcs`.
// FR-2555 / TC-2554: scripts/gate.sh TC-link step uses --base origin/main when ref exists
//   -> `tc_2554_gate_tc_links_step_uses_base`.

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

/// Run `scripts/gate.sh` with the given arguments and return its stdout.
fn print_gate_cmd(root: &Path, args: &[&str]) -> String {
    let output = Command::new("bash")
        .arg("scripts/gate.sh")
        .args(args)
        .current_dir(root)
        .output()
        .expect("bash must be available to run scripts/gate.sh");
    assert!(
        output.status.success(),
        "gate.sh {args:?} failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn tc_470_every_spec_has_status_line() {
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
fn tc_471_spec_ids_are_unique_across_specs() {
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
fn tc_472_spec_index_script_lists_every_spec() {
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
// FR-473 / TC-473: TEST-PLAN §0.1 no longer has per-spec rows. It points to the generator and the rule.
fn tc_473_test_plan_section_0_1_has_no_per_spec_rows() {
    let test_plan_path = Path::new("docs/TEST-PLAN.md");
    let content =
        fs::read_to_string(test_plan_path).expect("docs/TEST-PLAN.md must exist and be readable");

    let section_0_1 = content
        .find("## 0.1 SPEC 색인")
        .or_else(|| content.find("## 0.1"))
        .expect("TEST-PLAN §0.1 section heading must exist");

    let section_end = content[section_0_1..]
        .find("\n## 1.")
        .map(|idx| section_0_1 + idx)
        .unwrap_or(content.len());

    let section_text = &content[section_0_1..section_end];

    // Assert it mentions scripts/spec-index.sh
    assert!(
        section_text.contains("scripts/spec-index.sh"),
        "§0.1 must mention scripts/spec-index.sh"
    );

    // Assert it contains no per-spec table rows (no `| SPEC-` / `SPEC-*.md` link rows)
    for line in section_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            assert!(
                !trimmed.contains("SPEC-"),
                "§0.1 must not contain per-spec table rows: {trimmed}"
            );
        }
    }
}

fn run_git_cmd(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must run");
    assert!(
        output.status.success(),
        "git {args:?} failed in {:?}:\nstdout:\n{}\nstderr:\n{}",
        repo,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn tc_2551_tc_link_baseline_ratchet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp_root = std::env::temp_dir().join(format!(
        "dmuxai-tc-link-2551-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_root.join("docs/specs")).expect("create temp docs/specs");
    fs::create_dir_all(temp_root.join("src")).expect("create temp src");
    fs::create_dir_all(temp_root.join("scripts")).expect("create temp scripts");

    // Fixture with one comment-only (TC-1) and one allowlisted (TC-2)
    fs::write(
        temp_root.join("docs/specs/SPEC-test.md"),
        "Status: Draft\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | Evidence |\n|---|---|---|---|---|---|\n\
         | FR-1 | Must | p | r | f | TC-1 |\n\
         | FR-2 | Must | p | r | f | TC-2 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-1 | FR-1 | unit | s | a | o | loc |\n\
         | TC-2 | FR-2 | unit | s | a | o | loc |\n",
    )
    .expect("write SPEC-test.md");

    // TC-1 is linked only by comment in src/lib.rs
    fs::write(temp_root.join("src/lib.rs"), "// TC-1 link in comment\n").expect("write src/lib.rs");

    // TC-2 is allowlisted
    let allowlist = temp_root.join("scripts/tc-link-allowlist.txt");
    fs::write(&allowlist, "TC-2 manual\n").expect("write allowlist");

    let script = root.join("scripts/check-tc-links.py");
    let baseline = temp_root.join("scripts/tc-link-baseline.json");

    // 1) Baseline 1/1: comment_only=1, allowlisted=1 -> exit 0
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 1}"#).expect("write baseline 1/1");
    let out_1_1 = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .output()
        .expect("run script");
    assert_eq!(
        out_1_1.status.code(),
        Some(0),
        "1/1 should exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_1_1.stdout),
        String::from_utf8_lossy(&out_1_1.stderr)
    );

    // 2) Baseline 0/1: comment_only=0, allowlisted=1 -> exit 1 naming comment-only count
    fs::write(&baseline, r#"{"comment_only": 0, "allowlisted": 1}"#).expect("write baseline 0/1");
    let out_0_1 = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .output()
        .expect("run script");
    assert_eq!(
        out_0_1.status.code(),
        Some(1),
        "0/1 should exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_0_1.stdout),
        String::from_utf8_lossy(&out_0_1.stderr)
    );
    let combined_0_1 = format!(
        "{}{}",
        String::from_utf8_lossy(&out_0_1.stdout),
        String::from_utf8_lossy(&out_0_1.stderr)
    );
    assert!(
        combined_0_1.contains("comment-only") && combined_0_1.contains("baseline"),
        "0/1 must name comment-only count and baseline:\n{combined_0_1}"
    );

    // 3) Baseline 1/0: comment_only=1, allowlisted=0 -> exit 1 naming allowlisted count
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 0}"#).expect("write baseline 1/0");
    let out_1_0 = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .output()
        .expect("run script");
    assert_eq!(
        out_1_0.status.code(),
        Some(1),
        "1/0 should exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_1_0.stdout),
        String::from_utf8_lossy(&out_1_0.stderr)
    );
    let combined_1_0 = format!(
        "{}{}",
        String::from_utf8_lossy(&out_1_0.stdout),
        String::from_utf8_lossy(&out_1_0.stderr)
    );
    assert!(
        combined_1_0.contains("allowlisted") && combined_1_0.contains("baseline"),
        "1/0 must name allowlisted count and baseline:\n{combined_1_0}"
    );

    // 4) Baseline 2/2: comment_only=2, allowlisted=2 -> exit 0 and prints `lower the baseline`
    fs::write(&baseline, r#"{"comment_only": 2, "allowlisted": 2}"#).expect("write baseline 2/2");
    let out_2_2 = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .output()
        .expect("run script");
    assert_eq!(
        out_2_2.status.code(),
        Some(0),
        "2/2 should exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_2_2.stdout),
        String::from_utf8_lossy(&out_2_2.stderr)
    );
    let combined_2_2 = format!(
        "{}{}",
        String::from_utf8_lossy(&out_2_2.stdout),
        String::from_utf8_lossy(&out_2_2.stderr)
    );
    assert!(
        combined_2_2.contains("lower the baseline"),
        "2/2 must print 'lower the baseline':\n{combined_2_2}"
    );

    // 5) Missing baseline file -> exit 1
    fs::remove_file(&baseline).expect("remove baseline");
    let out_missing = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_root)
        .output()
        .expect("run script");
    assert_eq!(
        out_missing.status.code(),
        Some(1),
        "missing baseline should exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_missing.stdout),
        String::from_utf8_lossy(&out_missing.stderr)
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn tc_2552_tc_link_base_rejects_new_allowlist_and_raised_baseline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp_repo = std::env::temp_dir().join(format!(
        "dmuxai-tc-link-2552-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_repo.join("docs/specs")).expect("create temp docs/specs");
    fs::create_dir_all(temp_repo.join("src")).expect("create temp src");
    fs::create_dir_all(temp_repo.join("scripts")).expect("create temp scripts");

    run_git_cmd(&temp_repo, &["init"]);

    fs::write(
        temp_repo.join("docs/specs/SPEC-a.md"),
        "Status: Draft\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | Evidence |\n|---|---|---|---|---|---|\n\
         | FR-1 | Must | p | r | f | TC-1 |\n\
         | FR-2 | Must | p | r | f | TC-2 |\n\
         | FR-3 | Must | p | r | f | TC-3 |\n\
         | FR-4 | Must | p | r | f | TC-4 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-1 | FR-1 | unit | s | a | o | loc |\n\
         | TC-2 | FR-2 | unit | s | a | o | loc |\n\
         | TC-3 | FR-3 | unit | s | a | o | loc |\n\
         | TC-4 | FR-4 | unit | s | a | o | loc |\n",
    )
    .expect("write SPEC-a.md");

    // TC-1 has strong test, TC-2 has comment link, TC-3 is allowlisted, TC-4 has strong test in base
    fs::write(
        temp_repo.join("src/lib.rs"),
        "#[test]\nfn tc_1_ok() {}\n// TC-2\n#[test]\nfn tc_4_ok() {}\n",
    )
    .expect("write src/lib.rs");

    let allowlist = temp_repo.join("scripts/tc-link-allowlist.txt");
    fs::write(&allowlist, "TC-3 manual\n").expect("write base allowlist");

    let baseline = temp_repo.join("scripts/tc-link-baseline.json");
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 1}"#).expect("write base baseline");

    run_git_cmd(&temp_repo, &["add", "."]);
    run_git_cmd(
        &temp_repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-m",
            "base commit",
        ],
    );

    // Create branch "base"
    run_git_cmd(&temp_repo, &["branch", "base_ref"]);

    let script = root.join("scripts/check-tc-links.py");

    // Case A: Head adds TC-4 to the allowlist (removing its test from src/lib.rs so it's not stale)
    fs::write(
        temp_repo.join("src/lib.rs"),
        "#[test]\nfn tc_1_ok() {}\n// TC-2\n",
    )
    .expect("update src/lib.rs");
    fs::write(&allowlist, "TC-3 manual\nTC-4 manual\n").expect("add TC-4 to allowlist");
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 2}"#).expect("update baseline");

    let out_new_allowlist = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_repo)
        .arg("--base")
        .arg("base_ref")
        .output()
        .expect("run script");
    assert_eq!(
        out_new_allowlist.status.code(),
        Some(1),
        "new allowlist entry should exit 1"
    );
    let combined_new_al = format!(
        "{}{}",
        String::from_utf8_lossy(&out_new_allowlist.stdout),
        String::from_utf8_lossy(&out_new_allowlist.stderr)
    );
    assert!(
        combined_new_al.contains("NEW allowlist entry: TC-4"),
        "output must name 'NEW allowlist entry: TC-4':\n{combined_new_al}"
    );

    // Case B: Separately raises the baseline (revert allowlist to {TC-3}, restore TC-4 test, but raise baseline to 2/2)
    fs::write(
        temp_repo.join("src/lib.rs"),
        "#[test]\nfn tc_1_ok() {}\n// TC-2\n#[test]\nfn tc_4_ok() {}\n",
    )
    .expect("restore src/lib.rs");
    fs::write(&allowlist, "TC-3 manual\n").expect("restore allowlist");
    fs::write(&baseline, r#"{"comment_only": 2, "allowlisted": 1}"#).expect("raise baseline");

    let out_raised = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_repo)
        .arg("--base")
        .arg("base_ref")
        .output()
        .expect("run script");
    assert_eq!(
        out_raised.status.code(),
        Some(1),
        "raised baseline should exit 1"
    );
    let combined_raised = format!(
        "{}{}",
        String::from_utf8_lossy(&out_raised.stdout),
        String::from_utf8_lossy(&out_raised.stderr)
    );
    assert!(
        combined_raised.contains("baseline raised"),
        "output must name 'baseline raised':\n{combined_raised}"
    );

    // Case C: Head that only removes entries exits 0 (e.g. TC-3 gets a test, removed from allowlist, baseline 1/0)
    fs::write(
        temp_repo.join("src/lib.rs"),
        "#[test]\nfn tc_1_ok() {}\n// TC-2\n#[test]\nfn tc_3_ok() {}\n#[test]\nfn tc_4_ok() {}\n",
    )
    .expect("lib.rs with tc_3 test");
    fs::write(&allowlist, "# empty\n").expect("empty allowlist");
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 0}"#).expect("lowered baseline");

    let out_shrink = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_repo)
        .arg("--base")
        .arg("base_ref")
        .output()
        .expect("run script");
    assert_eq!(
        out_shrink.status.code(),
        Some(0),
        "head removing allowlist entries and lowering baseline should exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_shrink.stdout),
        String::from_utf8_lossy(&out_shrink.stderr)
    );

    let _ = fs::remove_dir_all(&temp_repo);
}

#[test]
fn tc_2553_tc_link_base_requires_strong_links_for_changed_tcs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp_repo = std::env::temp_dir().join(format!(
        "dmuxai-tc-link-2553-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_repo.join("docs/specs")).expect("create temp docs/specs");
    fs::create_dir_all(temp_repo.join("src")).expect("create temp src");
    fs::create_dir_all(temp_repo.join("scripts")).expect("create temp scripts");

    run_git_cmd(&temp_repo, &["init"]);

    // Base commit has TC-7
    fs::write(
        temp_repo.join("docs/specs/SPEC-test.md"),
        "Status: Draft\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | Evidence |\n|---|---|---|---|---|---|\n\
         | FR-7 | Must | p | r | f | TC-7 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-7 | FR-7 | unit | old setup | old act | old exp | old loc |\n",
    )
    .expect("write base SPEC-test.md");

    // Base has comment link for TC-7
    fs::write(temp_repo.join("src/lib.rs"), "// TC-7 comment\n").expect("write base src/lib.rs");

    let allowlist = temp_repo.join("scripts/tc-link-allowlist.txt");
    fs::write(&allowlist, "# allowlist\n").expect("write base allowlist");

    let baseline = temp_repo.join("scripts/tc-link-baseline.json");
    fs::write(&baseline, r#"{"comment_only": 2, "allowlisted": 0}"#).expect("write base baseline");

    run_git_cmd(&temp_repo, &["add", "."]);
    run_git_cmd(
        &temp_repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-m",
            "base commit",
        ],
    );

    run_git_cmd(&temp_repo, &["branch", "base_ref"]);

    // Head adds TC-5 (comment-only) and TC-6 (strong) rows and edits the TC-7 row
    fs::write(
        temp_repo.join("docs/specs/SPEC-test.md"),
        "Status: Draft\n\n## 4. Requirements\n\n\
         | ID | Priority | P | R | F | Evidence |\n|---|---|---|---|---|---|\n\
         | FR-5 | Must | p | r | f | TC-5 |\n\
         | FR-6 | Must | p | r | f | TC-6 |\n\
         | FR-7 | Must | p | r | f | TC-7 |\n\n\
         ## 5. Acceptance scenarios and test design\n\n\
         | TC-5 | FR-5 | unit | new setup 5 | act 5 | exp 5 | loc 5 |\n\
         | TC-6 | FR-6 | unit | new setup 6 | act 6 | exp 6 | loc 6 |\n\
         | TC-7 | FR-7 | unit | edited setup 7 | act 7 | exp 7 | loc 7 |\n",
    )
    .expect("update SPEC-test.md");

    // lib.rs has comment for TC-5, strong for TC-6, comment for TC-7
    fs::write(
        temp_repo.join("src/lib.rs"),
        "// TC-5\n#[test]\nfn tc_6_strong() {}\n// TC-7\n",
    )
    .expect("update src/lib.rs");

    let script = root.join("scripts/check-tc-links.py");

    // Run --base base_ref: TC-5 and TC-7 lack strong links, TC-6 has one
    let out_missing_strong = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_repo)
        .arg("--base")
        .arg("base_ref")
        .output()
        .expect("run script");
    assert_eq!(
        out_missing_strong.status.code(),
        Some(1),
        "should exit 1 when changed TCs lack strong links\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_missing_strong.stdout),
        String::from_utf8_lossy(&out_missing_strong.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out_missing_strong.stdout),
        String::from_utf8_lossy(&out_missing_strong.stderr)
    );
    assert!(
        combined.contains("TC-5") && combined.contains("needs a strong link"),
        "must name TC-5:\n{combined}"
    );
    assert!(
        combined.contains("TC-7") && combined.contains("needs a strong link"),
        "must name TC-7:\n{combined}"
    );
    assert!(
        !combined.contains("TC-6 needs a strong link"),
        "TC-6 has strong link and must not be flagged:\n{combined}"
    );

    // After adding strong tests for TC-5 and TC-7, exit 0
    fs::write(
        temp_repo.join("src/lib.rs"),
        "#[test]\nfn tc_5_strong() {}\n#[test]\nfn tc_6_strong() {}\n#[test]\nfn tc_7_strong() {}\n",
    )
    .expect("add strong tests for all");

    let out_all_strong = Command::new("python3")
        .arg(&script)
        .arg("--root")
        .arg(&temp_repo)
        .arg("--base")
        .arg("base_ref")
        .output()
        .expect("run script");
    assert_eq!(
        out_all_strong.status.code(),
        Some(0),
        "after adding strong tests exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out_all_strong.stdout),
        String::from_utf8_lossy(&out_all_strong.stderr)
    );

    let _ = fs::remove_dir_all(&temp_repo);
}

#[test]
fn tc_2554_gate_tc_links_step_uses_base() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--print-tc-links-cmd")
        .current_dir(root)
        .output()
        .expect("bash must run scripts/gate.sh --print-tc-links-cmd");
    assert!(
        output.status.success(),
        "gate.sh --print-tc-links-cmd failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let cmd = String::from_utf8_lossy(&output.stdout);
    assert!(
        cmd.contains("check-tc-links.py"),
        "output must contain check-tc-links.py:\n{cmd}"
    );

    let origin_main_exists = Command::new("git")
        .args(["rev-parse", "--verify", "-q", "origin/main"])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if origin_main_exists {
        assert!(
            cmd.contains("--base origin/main"),
            "when origin/main exists, command must pass --base origin/main:\n{cmd}"
        );
    } else {
        assert!(
            !cmd.contains("--base"),
            "when origin/main does not exist, command must not pass --base:\n{cmd}"
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
// TC-1802 / FR-1802: single spec check exits 1 on failing spec and 0 on passing spec.
fn tc_1802_check_spec_single_file_mode() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let specs_dir = std::env::temp_dir().join(format!(
        "dmuxai-spec-gate-single-{}-{}",
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

    let _ = fs::remove_dir_all(&specs_dir);
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

    let baseline = temp_root.join("scripts/tc-link-baseline.json");
    fs::write(&baseline, r#"{"comment_only": 1, "allowlisted": 1}"#).expect("write baseline");

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

    // The test command must keep running the remaining test targets after one
    // target fails (issue #198).
    let full_cmd = print_gate_cmd(root, &["--print-test-cmd"]);
    assert_eq!(
        full_cmd.trim(),
        "cargo test --all-features --no-fail-fast",
        "full test command must use --no-fail-fast:\n{full_cmd}"
    );

    let quick_cmd = print_gate_cmd(root, &["--print-test-cmd", "--quick"]);
    assert_eq!(
        quick_cmd.trim(),
        "cargo test --all-features --no-fail-fast --test spec_index",
        "quick test command must use --no-fail-fast:\n{quick_cmd}"
    );
}

#[test]
fn tc_1806_gate_script_isolated_mode_unsets_dmuxai_vars() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let caller_xdg = "/caller/config-should-not-leak";
    let isolated = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--print-env")
        .env("DMUXAI_OPENCODE_BIN", "/nonexistent")
        .env("XDG_CONFIG_HOME", caller_xdg)
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
    assert!(
        isolated_out.contains("XDG_CONFIG_HOME="),
        "isolated --print-env must list the gate-owned XDG_CONFIG_HOME:\n{isolated_out}"
    );
    assert!(
        !isolated_out.contains(caller_xdg),
        "isolated --print-env must not use the caller's XDG_CONFIG_HOME:\n{isolated_out}"
    );

    let non_isolated = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--no-isolated")
        .arg("--print-env")
        .env("DMUXAI_OPENCODE_BIN", "/nonexistent")
        .env("XDG_CONFIG_HOME", caller_xdg)
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
    assert!(
        non_isolated_out.contains(&format!("XDG_CONFIG_HOME={caller_xdg}")),
        "non-isolated --print-env must keep the caller's XDG_CONFIG_HOME:\n{non_isolated_out}"
    );
}

#[test]
fn tc_1807_gate_script_browser_command_uses_web_targets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash")
        .arg("scripts/gate.sh")
        .arg("--print-browser-cmd")
        .current_dir(root)
        .output()
        .expect("bash must be available to run scripts/gate.sh --print-browser-cmd");
    assert!(
        output.status.success(),
        "gate.sh --print-browser-cmd failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let cmd = String::from_utf8_lossy(&output.stdout);
    assert!(
        cmd.contains("cargo test --all-features"),
        "browser command must be a cargo test invocation:\n{cmd}"
    );
    assert!(
        cmd.contains("--no-fail-fast"),
        "browser command must keep running the remaining web targets after one fails:\n{cmd}"
    );
    assert!(
        cmd.contains("--test web_followup_queue"),
        "browser command must use tests/web_*.rs integration targets:\n{cmd}"
    );
    assert!(
        cmd.contains("--ignored"),
        "browser command must run ignored tests:\n{cmd}"
    );
    assert!(
        cmd.find("--no-fail-fast") < cmd.find(" -- "),
        "--no-fail-fast must be passed to cargo, before the test-arg separator:\n{cmd}"
    );
}
