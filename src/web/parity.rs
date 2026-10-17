//! TUI ↔ dashboard parity matrix (SPEC-dashboard-parity §3, FR-784).
//!
//! Every TUI slash command and key binding maps to a dashboard control,
//! identified by the `data-parity` attribute in `assets/session.js`.
//! `tui_commands_have_dashboard_controls` (TC-787) fails when a TUI command
//! has no entry here or an entry names a control the page does not have.

/// One row of the parity matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParityEntry {
    /// TUI slash command or key, e.g. `/agent` or `Ctrl+C`.
    pub tui: &'static str,
    /// `data-parity` id of the dashboard control.
    pub control: &'static str,
    /// Server action: a `POST /api/command` name, `prompt` for
    /// `POST /api/prompt`, or `client` for browser-only behavior.
    pub action: &'static str,
}

pub const PARITY: &[ParityEntry] = &[
    ParityEntry {
        tui: "Enter",
        control: "prompt-send",
        action: "prompt",
    },
    ParityEntry {
        tui: "Shift+Enter",
        control: "prompt-newline",
        action: "client",
    },
    ParityEntry {
        tui: "/agent",
        control: "agent",
        action: "select_agent",
    },
    ParityEntry {
        tui: "/mode",
        control: "mode",
        action: "set_mode",
    },
    ParityEntry {
        tui: "F2",
        control: "model",
        action: "load_models, select_model",
    },
    ParityEntry {
        tui: "/effort",
        control: "effort",
        action: "set_effort",
    },
    ParityEntry {
        tui: "/status",
        control: "status",
        action: "status",
    },
    ParityEntry {
        tui: "/quit",
        control: "quit",
        action: "quit",
    },
    ParityEntry {
        tui: "/new",
        control: "new",
        action: "new_conversation",
    },
    ParityEntry {
        tui: "/conversations",
        control: "conversations",
        action: "client",
    },
    ParityEntry {
        tui: "F3",
        control: "conversations",
        action: "client",
    },
    ParityEntry {
        tui: "/resume",
        control: "resume",
        action: "load_conversation",
    },
    ParityEntry {
        tui: "/summarize",
        control: "summarize",
        action: "prompt",
    },
    ParityEntry {
        tui: "/compact",
        control: "compact",
        action: "prompt",
    },
    ParityEntry {
        tui: "/share",
        control: "share",
        action: "share",
    },
    ParityEntry {
        tui: "/image",
        control: "image",
        action: "prompt",
    },
    ParityEntry {
        tui: "/details",
        control: "details",
        action: "toggle_details",
    },
    ParityEntry {
        tui: "Ctrl+O",
        control: "details",
        action: "toggle_details",
    },
    ParityEntry {
        tui: "/graph",
        control: "graph",
        action: "prompt",
    },
    ParityEntry {
        tui: "/remember",
        control: "remember",
        action: "prompt",
    },
    ParityEntry {
        tui: "/memory",
        control: "memory",
        action: "prompt",
    },
    ParityEntry {
        tui: "Ctrl+C",
        control: "cancel",
        action: "cancel",
    },
    ParityEntry {
        tui: "Esc",
        control: "escape",
        action: "client",
    },
];

/// The dashboard control for a TUI command or key, if mapped.
pub fn control_for(tui: &str) -> Option<&'static str> {
    PARITY
        .iter()
        .find(|entry| entry.tui == tui)
        .map(|entry| entry.control)
}

/// Whether the page declares a control with this `data-parity` id: as a
/// `setAttribute('data-parity', '<id>')` call, as the `parity` argument of the
/// `button`/`picker` helpers (followed by the click/change handler), or as a
/// value of the palette's `PARITY_IDS` table.
pub fn page_has_control(session_js: &str, control: &str) -> bool {
    [
        format!("'data-parity', '{control}'"),
        format!("'{control}', function"),
        format!("'{control}', submitPrompt"),
        format!(": '{control}'"),
    ]
    .iter()
    .any(|pattern| session_js.contains(pattern.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-787 (FR-784): matrix rows are unique and every control exists on the page.
    #[test]
    fn parity_matrix_controls_exist_in_session_js() {
        let js = crate::web::assets::SESSION_JS;
        assert!(js.contains("data-parity"));
        for (index, entry) in PARITY.iter().enumerate() {
            assert!(
                page_has_control(js, entry.control),
                "{} maps to missing control {}",
                entry.tui,
                entry.control
            );
            assert!(
                PARITY[..index].iter().all(|other| other.tui != entry.tui),
                "duplicate row for {}",
                entry.tui
            );
        }
        assert_eq!(control_for("/agent"), Some("agent"));
        assert_eq!(control_for("/nope"), None);
        assert!(!page_has_control(js, "not-a-control"));
        // A bare string that is not a declaration does not count.
        assert!(!page_has_control("var x = 'agent';", "agent"));
    }
}
