//! Composer agent/model menu in headless Chrome: the "Search models" box
//! filters agents and models, arrow keys and Enter pick a model or an effort,
//! Escape closes the menu and returns focus to the chip, focus and the typed
//! query survive snapshot re-renders, and the menu never offers a fast model
//! or a Fast toggle (fast variants are blocked by policy). Needs Chrome or
//! Chromium and python Playwright, so it is `#[ignore]`d; run with
//! `cargo test --test web_model_menu -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dmuxai::web::commands::{DashboardCommand, SessionCommand};
use dmuxai::web::{
    DashboardHub, DashboardServices, ServerConfig, SessionAgent, SessionModel, SessionView,
    SnapshotSource, start_server,
};
use tokio::sync::mpsc;

fn find_chrome() -> Option<PathBuf> {
    for var in ["DMUXAI_CHROME", "CHROME_BIN"] {
        if let Some(path) = std::env::var_os(var) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ];
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn model(id: &str, label: &str, efforts: &[&str], default_effort: Option<&str>) -> SessionModel {
    SessionModel {
        id: id.to_owned(),
        label: label.to_owned(),
        efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
        default_effort: default_effort.map(str::to_owned),
    }
}

fn agent(id: &str, label: &str, models: Vec<SessionModel>, selected: Option<&str>) -> SessionAgent {
    SessionAgent {
        id: id.to_owned(),
        label: label.to_owned(),
        mode: "agent".to_owned(),
        model: selected.map(str::to_owned),
        default_model: selected.map(str::to_owned),
        allowed: true,
        models,
        ..SessionAgent::default()
    }
}

/// A session whose models include fast variants the server would normally
/// drop, so the menu's own fast filter is exercised too.
fn demo_session() -> SessionView {
    let mut codex = agent(
        "codex",
        "Codex",
        vec![
            model(
                "composer-2.5",
                "Composer 2.5",
                &["low", "medium", "high", "xhigh", "fast"],
                Some("medium"),
            ),
            model("composer-2.5-fast", "Composer 2.5 Fast", &["low"], None),
        ],
        Some("composer-2.5"),
    );
    codex.effort = Some("medium".to_owned());
    SessionView {
        conversation_id: "c1".to_owned(),
        conversation_title: "Model menu".to_owned(),
        selected_agent: "codex".to_owned(),
        agents: vec![
            codex,
            agent(
                "cursor",
                "Cursor",
                vec![model("composer-2.5", "Composer 2.5", &[], None)],
                Some("composer-2.5"),
            ),
            agent(
                "grok",
                "Grok",
                vec![
                    model("grok-4.7", "Grok 4.7", &[], None),
                    model("grok-4.6", "Grok 4.6", &[], None),
                    model("grok-4.7-build-fast", "Grok 4.7 Build", &[], None),
                ],
                Some("grok-4.7"),
            ),
        ],
        ..SessionView::default()
    }
}

fn apply(session: &mut SessionView, command: &SessionCommand) {
    match command {
        SessionCommand::SelectAgent { agent } => session.selected_agent = agent.clone(),
        SessionCommand::SelectModel { agent, model } => {
            let id = agent
                .clone()
                .unwrap_or_else(|| session.selected_agent.clone());
            if let Some(a) = session.agents.iter_mut().find(|a| a.id == id) {
                a.effort = a
                    .models
                    .iter()
                    .find(|m| &m.id == model)
                    .and_then(|m| m.default_effort.clone());
                a.model = Some(model.clone());
            }
        }
        SessionCommand::SetEffort { agent, effort } => {
            let id = agent
                .clone()
                .unwrap_or_else(|| session.selected_agent.clone());
            if let Some(a) = session.agents.iter_mut().find(|a| a.id == id) {
                a.effort = Some(effort.clone());
            }
        }
        _ => {}
    }
}

const SCRIPT: &str = r##"
import sys, json, time
from playwright.sync_api import sync_playwright

out = {}
def menu_text(pg):
    return pg.eval_on_selector(".session-agent-popover", "e => e.innerText")
def visible_models(pg):
    return pg.eval_on_selector_all(".session-agent-popover .session-model-row:not(.session-model-default) .session-model-name", "es => es.map(e => e.textContent)")
def active(pg):
    return pg.evaluate("() => { const a = document.querySelector('.session-menu-active'); return a ? a.getAttribute('data-nav-key') : null; }")
def focused(pg):
    return pg.evaluate("() => { const a = document.activeElement; if (!a) return null; if (a.classList.contains('session-agent-search')) return 'search'; if (a.classList.contains('session-agent-picker')) return 'chip'; return a.getAttribute('data-nav-key') || a.className; }")
def no_fast(pg):
    html = pg.eval_on_selector(".session-agent-popover", "e => e.outerHTML")
    toggles = pg.eval_on_selector_all(".session-agent-popover input[type=checkbox], .session-agent-popover [role=switch]", "es => es.length")
    return ("fast" not in html.lower()) and toggles == 0

with sync_playwright() as p:
    b = p.chromium.launch(executable_path=sys.argv[1], args=["--no-sandbox"])
    pg = b.new_page(viewport={"width": 1280, "height": 900})
    pg.goto(sys.argv[2])
    pg.wait_for_selector("#session:not([hidden]) .session-agent-picker:not([disabled])", timeout=15000)

    # Open: search box focused, Codex models listed with the current effort.
    pg.click(".session-agent-picker")
    pg.wait_for_selector(".session-agent-search", timeout=5000)
    out["open_focus"] = focused(pg)
    out["open_models"] = visible_models(pg)
    out["open_text"] = menu_text(pg)
    out["open_no_fast"] = no_fast(pg)

    # Escape closes and returns focus to the chip.
    pg.keyboard.press("Escape")
    out["esc_closed"] = pg.query_selector(".session-agent-popover") is None
    out["esc_focus"] = focused(pg)

    # Search filters across agents; Enter picks the first match.
    pg.click(".session-agent-picker")
    pg.keyboard.type("4.6")
    out["search_models"] = visible_models(pg)
    out["search_active"] = active(pg)
    out["search_no_fast"] = no_fast(pg)
    pg.keyboard.press("Enter")
    pg.wait_for_selector(".session-agent-popover", state="detached", timeout=5000)
    out["enter_focus"] = focused(pg)

    # Back on Codex: ArrowRight opens the effort submenu, arrows + Enter pick one.
    pg.click(".session-agent-picker")
    pg.keyboard.type("codex")
    out["codex_models"] = visible_models(pg)
    pg.keyboard.press("Enter")
    pg.wait_for_selector(".session-agent-popover", state="detached", timeout=5000)
    pg.click(".session-agent-picker")
    pg.keyboard.type("composer 2.5")
    time.sleep(0.2)
    out["composer_active_before"] = active(pg)
    pg.keyboard.press("ArrowRight")
    pg.wait_for_selector(".session-model-sub", timeout=5000)
    out["efforts"] = pg.eval_on_selector_all(".session-effort-row .session-model-name", "es => es.map(e => e.textContent)")
    out["effort_no_fast"] = no_fast(pg)
    out["effort_text"] = menu_text(pg)
    # Re-renders from snapshots keep the query, the submenu and search focus.
    time.sleep(1.5)
    out["rerender_query"] = pg.eval_on_selector(".session-agent-search", "e => e.value")
    out["rerender_focus"] = focused(pg)
    out["rerender_sub"] = pg.query_selector(".session-model-sub") is not None
    for _ in range(3):
        pg.keyboard.press("ArrowDown")
    out["effort_active"] = active(pg)
    # Focus a row: it keeps focus across re-renders (not the panel).
    pg.evaluate("() => document.querySelector('.session-menu-active').focus()")
    time.sleep(1.5)
    out["row_focus"] = focused(pg)
    pg.keyboard.press("Enter")
    pg.wait_for_selector(".session-agent-popover", state="detached", timeout=5000)
    out["final_focus"] = focused(pg)
    # "Default model" re-selects the runner default (resets the effort).
    pg.click(".session-agent-picker")
    pg.click('[data-nav-key="default:codex"]')
    pg.wait_for_selector(".session-agent-popover", state="detached", timeout=5000)
    pg.click(".session-agent-picker")
    time.sleep(0.5)
    out["after_default_text"] = menu_text(pg)
    b.close()
print(json.dumps(out))
"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Chrome/Chromium and python playwright; run with --ignored"]
async fn model_menu_search_keyboard_and_no_fast_in_headless_chrome() {
    let Some(chrome) = find_chrome() else {
        println!("SKIP: no Chrome/Chromium found (set DMUXAI_CHROME)");
        return;
    };
    let python = if cfg!(windows) { "python" } else { "python3" };
    let probe = Command::new(python)
        .args(["-c", "import playwright"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|status| status.success()).unwrap_or(false) {
        println!("SKIP: python playwright not installed");
        return;
    }

    let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_owned());
    let state = Arc::new(Mutex::new(demo_session()));
    hub.set_session(Some(state.lock().unwrap().clone()));

    let (tx, mut rx) = mpsc::channel::<DashboardCommand>(16);
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let (state, log, hub) = (state.clone(), log.clone(), hub.clone());
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                if let DashboardCommand::Session { command, reply } = command {
                    log.lock().unwrap().push(format!("{command:?}"));
                    let session = {
                        let mut session = state.lock().unwrap();
                        apply(&mut session, &command);
                        session.clone()
                    };
                    hub.set_session(Some(session.clone()));
                    let _ = reply.send(Ok(session));
                }
            }
        });
    }
    // Snapshot churn: the title flips every 300 ms, so the menu re-renders.
    {
        let (state, hub) = (state.clone(), hub.clone());
        tokio::spawn(async move {
            let mut tick = 0_u64;
            loop {
                tokio::time::sleep(Duration::from_millis(300)).await;
                tick += 1;
                let session = {
                    let mut session = state.lock().unwrap();
                    session.conversation_title = format!("Model menu {}", tick % 2);
                    session.clone()
                };
                hub.set_session(Some(session));
            }
        });
    }

    let config = ServerConfig {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        allow_remote: false,
        token: "test-token-model-menu".to_owned(),
        services: DashboardServices {
            events_path: None,
            todos: None,
            commands: Some(tx),
            mcp: None,
            web_attached: true,
            memory: None,
            docs: None,
        },
    };
    let server = start_server(config, hub).await.expect("start server");
    let url = server.url.clone();

    let output = tokio::task::spawn_blocking(move || {
        Command::new(python)
            .args(["-c", SCRIPT])
            .arg(&chrome)
            .arg(&url)
            .stdin(Stdio::null())
            .output()
            .expect("run playwright")
    });
    let output = tokio::time::timeout(Duration::from_secs(90), output)
        .await
        .expect("playwright timed out")
        .expect("join playwright");
    server.abort();
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "no JSON from playwright:\n{text}\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    let v: serde_json::Value = serde_json::from_str(line).expect("json result");
    let strs = |key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} missing: {v}"))
            .iter()
            .map(|s| s.as_str().unwrap_or_default().to_owned())
            .collect()
    };

    assert_eq!(v["open_focus"], "search", "{v}");
    assert_eq!(strs("open_models"), vec!["Composer 2.5"], "{v}");
    let open_text = v["open_text"].as_str().unwrap_or_default();
    assert!(open_text.contains("Default model"), "{v}");
    assert!(
        open_text.contains("Medium"),
        "current effort next to the model: {v}"
    );
    for key in ["open_no_fast", "search_no_fast", "effort_no_fast"] {
        assert_eq!(v[key], true, "{key}: the menu offered a fast option: {v}");
    }
    assert_eq!(v["esc_closed"], true, "{v}");
    assert_eq!(v["esc_focus"], "chip", "{v}");
    assert_eq!(strs("search_models"), vec!["Grok 4.6"], "{v}");
    assert_eq!(v["search_active"], "model:grok:grok-4.6", "{v}");
    assert_eq!(v["enter_focus"], "chip", "{v}");
    assert_eq!(
        v["composer_active_before"], "model:codex:composer-2.5",
        "{v}"
    );
    assert_eq!(
        strs("efforts"),
        vec!["Low", "Medium", "High", "Extra High"],
        "{v}"
    );
    assert_eq!(v["rerender_query"], "composer 2.5", "{v}");
    assert_eq!(v["rerender_focus"], "search", "{v}");
    assert_eq!(v["rerender_sub"], true, "{v}");
    assert_eq!(v["effort_active"], "effort:codex:composer-2.5:high", "{v}");
    assert_eq!(v["row_focus"], "effort:codex:composer-2.5:high", "{v}");
    assert_eq!(v["final_focus"], "chip", "{v}");

    let log = log.lock().unwrap().join("\n");
    assert!(log.contains(r#"SelectAgent { agent: "grok" }"#), "{log}");
    assert!(
        log.contains(r#"SelectModel { agent: Some("grok"), model: "grok-4.6" }"#),
        "{log}"
    );
    assert!(log.contains(r#"SelectAgent { agent: "codex" }"#), "{log}");
    assert!(
        log.contains(r#"SetEffort { agent: Some("codex"), effort: "high" }"#),
        "{log}"
    );
    assert!(
        log.contains(r#"SelectModel { agent: Some("codex"), model: "composer-2.5" }"#),
        "Default model must re-send select_model: {log}"
    );
    let after_default = v["after_default_text"].as_str().unwrap_or_default();
    assert!(
        after_default.contains("Medium") && !after_default.contains("High"),
        "{v}"
    );
    assert!(!log.to_ascii_lowercase().contains("fast"), "{log}");
}
