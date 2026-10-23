use super::judge::Route;
use super::types::NodeRole;

/// FR-936: pure route heuristic with no I/O. Keeps the English keywords and
/// adds the Korean equivalents. It only picks the route; it never builds a
/// multi-step plan (FR-948).
pub fn heuristic_route(prompt: &str) -> Route {
    if heuristic_roles(prompt).len() <= 1 {
        Route::Simple
    } else {
        Route::Complex
    }
}

/// Keyword roles in today's order: research, explore, test, plan, review,
/// implement. A prompt with no keyword maps to implement so prompts that infer
/// at most one role still route simple.
///
/// The Korean additions map to the fixed-order adjacency: 조사/탐색/찾아 join
/// the research keyword family (their closest English equivalents are
/// search/understand), 테스트/검증 join test, 계획/설계 join plan, 리뷰/검토
/// join review, and 구현/수정/고쳐/추가/리팩터 join implement.
fn heuristic_roles(prompt: &str) -> Vec<NodeRole> {
    let lower = prompt.to_ascii_lowercase();
    let mut roles = Vec::new();
    if contains_any(
        &lower,
        &[
            "research",
            "explore",
            "investigate",
            "search",
            "understand",
            "조사",
            "탐색",
            "찾아",
        ],
    ) {
        roles.push(NodeRole::Research);
    }
    if contains_any(&lower, &["explore", "trace", "grep"]) && !roles.contains(&NodeRole::Explore) {
        roles.push(NodeRole::Explore);
    }
    if contains_any(
        &lower,
        &["test", "verify", "spec", "coverage", "테스트", "검증"],
    ) {
        roles.push(NodeRole::Test);
    }
    if contains_any(
        &lower,
        &["plan", "design", "architect", "route", "계획", "설계"],
    ) {
        roles.push(NodeRole::Plan);
    }
    if contains_any(&lower, &["review", "audit", "checklist", "리뷰", "검토"]) {
        roles.push(NodeRole::Review);
    }
    if contains_any(
        &lower,
        &[
            "implement",
            "fix",
            "build",
            "draft",
            "code",
            "refactor",
            "add ",
            "구현",
            "수정",
            "고쳐",
            "추가",
            "리팩터",
        ],
    ) {
        roles.push(NodeRole::Implement);
    }
    if roles.is_empty() {
        roles.push(NodeRole::Implement);
    }
    roles
}

/// FR-944 / FR-1662: whether `text` matches a sensitive keyword. Pure, no role
/// filter; quoted and fenced regions are ignored; matches whole words only.
pub fn sensitive_keyword_match(text: &str) -> bool {
    let masked = mask_ignored_regions(text);
    let lower = masked.to_ascii_lowercase();
    const KEYWORDS: &[&str] = &[
        "merge",
        "database",
        "db",
        "auth",
        "login",
        "credential",
        "migration",
        "deploy",
        "production",
    ];
    KEYWORDS
        .iter()
        .any(|word| contains_whole_word(&lower, word))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn mask_ignored_regions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i..].starts_with(&['`', '`', '`']) {
            i += 3;
            while i < chars.len() {
                if chars[i..].starts_with(&['`', '`', '`']) {
                    i += 3;
                    out.push(' ');
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i] == '`' {
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(' ');
            continue;
        }
        if chars[i] == '"' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(' ');
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn contains_whole_word(haystack: &str, word: &str) -> bool {
    let mut start = 0usize;
    while let Some(rel) = haystack[start..].find(word) {
        let at = start + rel;
        let before_ok = haystack[..at]
            .chars()
            .last()
            .is_none_or(|c| !is_word_char(c));
        let after = at + word.len();
        let after_ok = haystack[after..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-1662 / FR-1662
    #[test]
    fn tc_1662_sensitive_keyword_match() {
        assert!(sensitive_keyword_match("please login now"));
        assert!(sensitive_keyword_match("deploy to staging"));
        assert!(!sensitive_keyword_match("list all authors"));
        assert!(!sensitive_keyword_match("enable dbg output"));
        assert!(!sensitive_keyword_match("use `login` helper"));
        assert!(!sensitive_keyword_match(r#"say "production""#));
        assert!(!sensitive_keyword_match("```\nlogin\n```"));
        assert!(sensitive_keyword_match("touch the db schema"));
    }

    // TC-936 / FR-934, FR-936: the fallback heuristic is pure and returns
    // simple when at most one executable role is inferred (FR-937); it never
    // builds a plan (FR-948).
    #[test]
    fn heuristic_route_handles_korean_and_english() {
        assert_eq!(heuristic_route("README 오타 고쳐줘"), Route::Simple);
        assert_eq!(
            heuristic_route("조사하고 설계한 다음 구현하고 테스트해줘"),
            Route::Complex
        );
        assert_eq!(heuristic_route("fix typo"), Route::Simple);
        assert_eq!(
            heuristic_route("research, plan, implement and test X"),
            Route::Complex
        );
    }
}
