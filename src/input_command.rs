//! Input command classifier and path disambiguation (finding #10, PR-8d).

use std::path::Path;

/// Classified form of a user prompt or entry point input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind<'a> {
    /// Text after dropping the first '/' of an escaped leading "//".
    Escaped(&'a str),
    /// A slash command with command name (including leading '/') and trimmed trailing arguments.
    Slash { cmd: &'a str, rest: &'a str },
    /// Plain prompt text, image paths, or non-slash input.
    Plain,
}

/// Disambiguates whether the first token is a file path rather than a slash command.
///
/// `/home/u/shot.png describe this` is a path, not a slash command: the
/// first token has a second `/` or names an existing file.
pub(crate) fn first_token_is_path(prompt: &str) -> bool {
    let token = prompt.split_whitespace().next().unwrap_or("");
    token.get(1..).is_some_and(|s| s.contains('/')) || Path::new(token).exists()
}

/// Classifies a prompt string into [`InputKind`].
///
/// Follows standard DmuxAI input classification rules:
/// - Prompts starting with `"//"` are classified as [`InputKind::Escaped`] with the first `/` removed.
/// - Prompts starting with `'/'` that do not qualify as file paths via [`first_token_is_path`]
///   are classified as [`InputKind::Slash`] with the command token and trimmed remaining text.
/// - All other inputs are classified as [`InputKind::Plain`].
pub(crate) fn classify(prompt: &str) -> InputKind<'_> {
    if prompt.starts_with("//") {
        InputKind::Escaped(&prompt[1..])
    } else if prompt.starts_with('/') && !first_token_is_path(prompt) {
        let cmd = prompt.split_whitespace().next().unwrap_or(prompt);
        let rest = prompt[cmd.len()..].trim();
        InputKind::Slash { cmd, rest }
    } else {
        InputKind::Plain
    }
}

/// Returns whether `text` represents a slash command after trimming leading whitespace.
///
/// Headless and dashboard check: starts with '/', not "//", and first token is not a path.
pub(crate) fn is_slash_command(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('/') && !trimmed.starts_with("//") && !first_token_is_path(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_2060_classify_escaped_slash_plain() {
        // Escaped: drops leading slash
        assert_eq!(classify("//x"), InputKind::Escaped("/x"));
        assert_eq!(classify("///nested"), InputKind::Escaped("//nested"));

        // Slash commands: command name and trimmed rest
        assert_eq!(
            classify("/agent codex"),
            InputKind::Slash {
                cmd: "/agent",
                rest: "codex",
            }
        );
        assert_eq!(
            classify("/plan   do x"),
            InputKind::Slash {
                cmd: "/plan",
                rest: "do x",
            }
        );
        assert_eq!(
            classify("/mode"),
            InputKind::Slash {
                cmd: "/mode",
                rest: "",
            }
        );

        // Path is Plain
        assert_eq!(classify("/tmp/file.png"), InputKind::Plain);
        assert_eq!(classify("/home/u/shot.png describe this"), InputKind::Plain);

        // Normal text is Plain
        assert_eq!(classify("hello world"), InputKind::Plain);
        assert_eq!(classify(""), InputKind::Plain);
    }

    #[test]
    fn tc_2061_is_slash_command_trim_start() {
        assert!(is_slash_command("/agent codex"));
        assert!(is_slash_command("   /agent codex"));
        assert!(is_slash_command("\t/mode"));
        assert!(!is_slash_command("//x"));
        assert!(!is_slash_command("   //x"));
        assert!(!is_slash_command("/tmp/file.png"));
        assert!(!is_slash_command("   /tmp/file.png"));
        assert!(!is_slash_command("hello world"));
        assert!(!is_slash_command(""));
        assert!(!is_slash_command("   "));
    }
}
