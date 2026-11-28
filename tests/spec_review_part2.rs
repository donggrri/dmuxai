//! Part 2 placeholders for SPEC-spec-review-gate (board, dashboard, TUI).
//!
//! Part 1 (core, CLI, MCP flag) is implemented and tested elsewhere. These
//! requirements are not built yet; each placeholder is `#[ignore]`d and fails if
//! it is ever run, so it cannot pass by accident. The part 2 change replaces each
//! one with the real test at the location named in the spec (section 5) and
//! deletes this file.

#[test]
#[ignore = "FR-3711 board item spec_review: part 2, src/board/runner.rs"]
fn tc_3711_board_item_spec_review_updates() {
    unimplemented!("FR-3711 board item spec_review: part 2, src/board/runner.rs");
}

#[test]
#[ignore = "FR-3712 board child timeout: part 2, src/board/runner.rs"]
fn tc_3712_board_child_not_killed_while_review_pending() {
    unimplemented!("FR-3712 board child timeout: part 2, src/board/runner.rs");
}

#[test]
#[ignore = "FR-3713 dashboard endpoints: part 2, src/web/board_api.rs"]
fn tc_3713_board_spec_review_http_endpoints() {
    unimplemented!("FR-3713 dashboard endpoints: part 2, src/web/board_api.rs");
}

#[test]
#[ignore = "FR-3714/FR-3715 browser e2e: part 2, tests/web_board_e2e.rs"]
fn tc_3714_board_spec_review_e2e() {
    unimplemented!("FR-3714/FR-3715 browser e2e: part 2, tests/web_board_e2e.rs");
}

#[test]
#[ignore = "FR-3715 Plan review badge: part 2, src/web/assets.rs"]
fn tc_3715_board_plan_review_badge_assets() {
    unimplemented!("FR-3715 Plan review badge: part 2, src/web/assets.rs");
}

#[test]
#[ignore = "FR-3716 TUI panel: part 2, src/app.rs"]
fn tc_3716_tui_spec_review_panel() {
    unimplemented!("FR-3716 TUI panel: part 2, src/app.rs");
}
