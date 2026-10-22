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
        tui: "/plan",
        control: "route",
        action: "prompt",
    },
    ParityEntry {
        tui: "/direct",
        control: "route",
        action: "prompt",
    },
    ParityEntry {
        tui: "/graph approve",
        control: "plan-approve",
        action: "plan_decision",
    },
    ParityEntry {
        tui: "/graph edit",
        control: "plan-edit",
        action: "plan_decision",
    },
    ParityEntry {
        tui: "/graph reject",
        control: "plan-reject",
        action: "plan_decision",
    },
    ParityEntry {
        tui: "/graph replan",
        control: "plan-replan",
        action: "plan_replan",
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
        tui: "/skills",
        control: "skills",
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

/// Plan-gate and graph-header controls may be rendered in `app.js` or `index.html`
/// (via `dmuxaiSession.parityButton`) instead of hidden session stubs.
pub fn dashboard_has_control(
    session_js: &str,
    app_js: &str,
    index_html: &str,
    control: &str,
) -> bool {
    page_has_control(session_js, control)
        || app_js.contains(&format!("data-parity=\"{control}\""))
        || index_html.contains(&format!("data-parity=\"{control}\""))
        || (app_js.contains(&format!("'{control}'")) && app_js.contains("parityButton"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-787 (FR-784): matrix rows are unique and every control exists on the page.
    #[test]
    fn parity_matrix_controls_exist_in_session_js() {
        let js = crate::web::assets::SESSION_JS;
        let app_js = crate::web::assets::APP_JS;
        let index_html = crate::web::assets::INDEX_HTML;
        assert!(js.contains("data-parity"));
        for (index, entry) in PARITY.iter().enumerate() {
            assert!(
                dashboard_has_control(js, app_js, index_html, entry.control),
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

    // TC-841 (SPEC-todo-sidebar FR-840, FR-841): moving the to-do UI into the
    // sidebar keeps every to-do action reachable: create, run, done, delete
    // and MCP import, each with its control in the To-dos panel and its
    // handler in app.js.
    #[test]
    fn todo_actions_still_covered() {
        let html = crate::web::assets::INDEX_HTML;
        let js = crate::web::assets::APP_JS;
        let panel = html
            .find(r#"id="tab-panel-todos""#)
            .expect("to-dos tab panel");
        let end = html[panel..].find("</aside>").expect("sidebar end") + panel;
        let in_panel = |id: &str| {
            html.find(&format!(r#"id="{id}""#))
                .is_some_and(|at| at > panel && at < end)
        };
        // create
        assert!(in_panel("todo-form") && in_panel("todo-add"));
        assert!(js.contains("todoApi('POST', '/api/todos', body)"));
        // import
        assert!(in_panel("import-form") && js.contains("function submitImport()"));
        // run / done / delete render into #todo-list inside the panel
        assert!(in_panel("todo-list"));
        assert!(js.contains("function runTodo(id, body)"));
        assert!(js.contains("function todoRunControls(id, status)"));
        assert!(js.contains("patchTodo(id, { status: box.checked ? 'done' : 'open' }, false)"));
        assert!(js.contains("function deleteTodo(id)"));
        assert!(js.contains("function updateTodoViewOnlyHint(snap)"));
    }
}
