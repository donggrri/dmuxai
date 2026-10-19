//! Plain-text judge / triage lines for the TUI and dashboard (UX-930).

use unicode_width::UnicodeWidthChar;

use super::judge::{FallbackCause, OverrideKind, Route, TriageSource};
use super::types::Triage;

/// Full judge line for a finished triage (before width ellipsizing).
pub fn judge_line_text(triage: &Triage) -> String {
    match triage.source {
        TriageSource::Judge => {
            let runner = plain_label_text(&triage.runner);
            let model = plain_label_text(&triage.model);
            let reason = plain_reason_text(&triage.reason);
            if reason.is_empty() {
                format!(
                    "Judge ({} {}): {}",
                    runner,
                    model,
                    route_label(triage.route),
                )
            } else {
                format!(
                    "Judge ({} {}): {} — {}",
                    runner,
                    model,
                    route_label(triage.route),
                    reason,
                )
            }
        }
        TriageSource::Fallback => {
            let cause = triage.cause.map(fallback_cause_label).unwrap_or("error");
            format!(
                "Judge fallback: {} (heuristic) — {}",
                cause,
                route_label(triage.route),
            )
        }
        TriageSource::Override => {
            override_line_text(triage.override_kind, &plain_label_text(&triage.runner))
        }
    }
}

/// Line shown while the judge turn is in flight (UX-930).
pub fn judging_line_text(runner: &str, model: &str) -> String {
    format!(
        "Judging with {} {}…",
        plain_label_text(runner),
        plain_label_text(model),
    )
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

/// Cuts `text` to at most `max_width` display columns, appending `…` when truncated.
/// Never splits a Unicode scalar value.
pub fn ellipsize_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(text) <= max_width {
        return text.to_owned();
    }
    if max_width == 1 {
        return "…".to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    let budget = max_width.saturating_sub(1);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > budget {
            out.push('…');
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

fn route_label(route: Route) -> &'static str {
    match route {
        Route::Simple => "simple",
        Route::Complex => "complex",
    }
}

fn fallback_cause_label(cause: FallbackCause) -> &'static str {
    match cause {
        FallbackCause::Timeout => "timeout",
        FallbackCause::Error => "error",
        FallbackCause::InvalidVerdict => "invalid_verdict",
        FallbackCause::NoReadOnlyMode => "no_read_only_mode",
    }
}

fn override_line_text(kind: Option<OverrideKind>, runner: &str) -> String {
    match kind {
        Some(OverrideKind::Plan) => "/plan".to_owned(),
        Some(OverrideKind::Direct) => "/direct".to_owned(),
        Some(OverrideKind::Agent) => format!("agent {}", runner),
        Some(OverrideKind::NoAllowedRunners) => "no allowed runners".to_owned(),
        None => "/direct".to_owned(),
    }
}

/// Collapses control characters and line separators to spaces for plain-text display.
fn plain_reason_text(raw: &str) -> String {
    plain_label_text(raw)
}

fn plain_label_text(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_control() || ch == '\u{2028}' || ch == '\u{2029}' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::judge::TriageSource;

    fn sample_triage(source: TriageSource, reason: &str) -> Triage {
        Triage {
            route: Route::Simple,
            reason: reason.to_owned(),
            source,
            cause: None,
            override_kind: None,
            runner: "cursor".to_owned(),
            model: "composer".to_owned(),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn judge_line_simple_and_complex() {
        let triage = sample_triage(TriageSource::Judge, "one file edit");
        assert_eq!(
            judge_line_text(&triage),
            "Judge (cursor composer): simple — one file edit"
        );
        let mut complex = triage;
        complex.route = Route::Complex;
        complex.reason = "multi-step".to_owned();
        assert_eq!(
            judge_line_text(&complex),
            "Judge (cursor composer): complex — multi-step"
        );
    }

    #[test]
    fn judge_line_normalizes_reason() {
        let triage = sample_triage(TriageSource::Judge, "line one\nline\ttwo");
        assert_eq!(
            judge_line_text(&triage),
            "Judge (cursor composer): simple — line one line two"
        );
    }

    #[test]
    fn judge_line_empty_reason_omits_separator() {
        let triage = sample_triage(TriageSource::Judge, "");
        assert_eq!(judge_line_text(&triage), "Judge (cursor composer): simple");
    }

    #[test]
    fn judge_line_fallback() {
        let triage = Triage {
            route: Route::Complex,
            reason: "heuristic fallback".to_owned(),
            source: TriageSource::Fallback,
            cause: Some(FallbackCause::Timeout),
            override_kind: None,
            runner: "cursor".to_owned(),
            model: String::new(),
            elapsed_ms: 1,
        };
        assert_eq!(
            judge_line_text(&triage),
            "Judge fallback: timeout (heuristic) — complex"
        );
    }

    #[test]
    fn judge_line_overrides() {
        let base = sample_triage(TriageSource::Override, "override");
        assert_eq!(judge_line_text(&base), "/direct");
        let mut direct = base.clone();
        direct.override_kind = Some(OverrideKind::Direct);
        assert_eq!(judge_line_text(&direct), "/direct");
        let mut plan = base.clone();
        plan.override_kind = Some(OverrideKind::Plan);
        assert_eq!(judge_line_text(&plan), "/plan");
        let mut agent = base.clone();
        agent.override_kind = Some(OverrideKind::Agent);
        agent.runner = "codex".to_owned();
        assert_eq!(judge_line_text(&agent), "agent codex");
        let mut blocked = base.clone();
        blocked.override_kind = Some(OverrideKind::NoAllowedRunners);
        assert_eq!(judge_line_text(&blocked), "no allowed runners");
    }

    #[test]
    fn judging_line_text_format() {
        assert_eq!(
            judging_line_text("cursor", "composer"),
            "Judging with cursor composer…"
        );
    }

    #[test]
    fn ellipsize_respects_display_width_and_korean() {
        let text = "가나다라";
        assert_eq!(ellipsize_to_width(text, 8), "가나다라");
        assert_eq!(ellipsize_to_width(text, 7), "가나다…");
        assert_eq!(ellipsize_to_width("hello", 3), "he…");
        assert_eq!(ellipsize_to_width("hello", 0), "");
        assert_eq!(ellipsize_to_width("hello", 1), "…");
        assert_eq!(ellipsize_to_width("abc", 2), "a…");
    }
}
