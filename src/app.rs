use std::{
    collections::{HashMap, HashSet},
    io, mem,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{sleep, timeout},
};

use crate::agent::{
    Agent, AgentAdapter, AgentEvent, AgentOutput, ModelInfo, event_channel, spawn_agy_adapter,
    spawn_codex_adapter, spawn_cursor_adapter,
};

const RENDER_INTERVAL: Duration = Duration::from_millis(25);
const EVENT_LOOP_PAUSE: Duration = Duration::from_millis(5);

#[derive(Debug, PartialEq, Eq)]
enum Role {
    User,
    Agent(Agent),
    System,
}

#[derive(Debug, PartialEq, Eq)]
struct Message {
    role: Role,
    content: String,
}

impl Message {
    fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Default)]
struct ModelCatalog {
    models: Vec<ModelInfo>,
    selected: Option<String>,
    loading: bool,
}

struct ModelPicker {
    agent: Agent,
    cursor: usize,
}

pub struct App {
    exit: bool,
    selected: usize,
    input: String,
    messages: Vec<Message>,
    adapters: Vec<Box<dyn AgentAdapter>>,
    event_rx: mpsc::Receiver<AgentOutput>,
    workers: Vec<JoinHandle<()>>,
    busy_agents: HashSet<Agent>,
    streaming_messages: HashMap<Agent, usize>,
    model_catalogs: HashMap<Agent, ModelCatalog>,
    model_picker: Option<ModelPicker>,
}

impl App {
    pub fn new() -> Self {
        let (event_tx, event_rx) = event_channel();
        let mut adapters = Vec::with_capacity(Agent::ALL.len());
        let mut workers = Vec::with_capacity(Agent::ALL.len());

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        for agent in Agent::ALL {
            let (adapter, worker) = match agent {
                Agent::Codex => spawn_codex_adapter(event_tx.clone(), cwd.clone()),
                Agent::Cursor => spawn_cursor_adapter(event_tx.clone(), cwd.clone()),
                Agent::Agy => spawn_agy_adapter(event_tx.clone(), cwd.clone()),
            };
            adapters.push(adapter);
            workers.push(worker);
        }
        drop(event_tx);

        Self::with_runtime(adapters, event_rx, workers)
    }

    fn with_runtime(
        adapters: Vec<Box<dyn AgentAdapter>>,
        event_rx: mpsc::Receiver<AgentOutput>,
        workers: Vec<JoinHandle<()>>,
    ) -> Self {
        Self {
            exit: false,
            selected: 0,
            input: String::new(),
            messages: vec![Message::new(
                Role::System,
                "Codex, Cursor, and AGY use real structured headless protocols. Press F2 to choose a model or Ctrl+C to cancel the selected agent.",
            )],
            adapters,
            event_rx,
            workers,
            busy_agents: HashSet::new(),
            streaming_messages: HashMap::new(),
            model_catalogs: Agent::ALL
                .into_iter()
                .map(|agent| (agent, ModelCatalog::default()))
                .collect(),
            model_picker: None,
        }
    }

    pub async fn run<B>(&mut self, terminal: &mut Terminal<B>) -> Result<(), B::Error>
    where
        B: Backend,
        B::Error: From<io::Error>,
    {
        terminal.draw(|frame| self.draw(frame))?;
        let mut dirty = false;
        let mut last_render = Instant::now();

        while !self.exit {
            dirty |= self.drain_agent_events();

            while event::poll(Duration::ZERO)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
                    Event::Paste(text) if self.model_picker.is_none() => self.input.push_str(&text),
                    _ => {}
                }
                dirty = true;
            }

            if self.exit {
                break;
            }

            if dirty && last_render.elapsed() >= RENDER_INTERVAL {
                terminal.draw(|frame| self.draw(frame))?;
                dirty = false;
                last_render = Instant::now();
            }

            // This await yields the runtime thread so process and reader tasks
            // continue running even when the terminal has no keyboard input.
            sleep(EVENT_LOOP_PAUSE).await;
        }

        Ok(())
    }

    pub async fn shutdown(&mut self) {
        // Dropping every command sender closes each actor's receiver. Each
        // actor then cleans up the child/process task it exclusively owns.
        self.adapters.clear();
        for mut worker in self.workers.drain(..) {
            if timeout(Duration::from_secs(2), &mut worker).await.is_err() {
                // Defensive fallback: aborting drops the actor-owned Child,
                // whose kill_on_drop setting asks Tokio to terminate it.
                worker.abort();
                let _ = worker.await;
            }
        }
        self.busy_agents.clear();
        self.streaming_messages.clear();
    }

    fn drain_agent_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(output) = self.event_rx.try_recv() {
            self.apply_agent_event(output.agent, output.event);
            changed = true;
        }
        changed
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.model_picker.is_some() {
            self.handle_model_picker_key(key);
            return;
        }

        match key.code {
            KeyCode::Esc => self.exit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_selected();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.adapters.len() - 1);
            }
            KeyCode::F(2) => self.open_model_picker(),
            KeyCode::Enter => self.submit_prompt(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character);
            }
            _ => {}
        }
    }

    fn open_model_picker(&mut self) {
        let agent = self.adapters[self.selected].agent();
        if self.busy_agents.contains(&agent) {
            self.messages.push(Message::new(
                Role::System,
                format!("Finish or cancel the {agent} turn before changing models."),
            ));
            return;
        }

        let catalog = self
            .model_catalogs
            .get_mut(&agent)
            .expect("every agent has a model catalog");
        let cursor = catalog
            .selected
            .as_ref()
            .and_then(|selected| {
                catalog
                    .models
                    .iter()
                    .position(|model| &model.id == selected)
            })
            .unwrap_or(0);
        let should_load = catalog.models.is_empty() && !catalog.loading;
        if should_load {
            catalog.loading = true;
        }
        self.model_picker = Some(ModelPicker { agent, cursor });

        if should_load && let Err(error) = self.adapters[self.selected].request_models() {
            catalog.loading = false;
            self.messages.push(Message::new(
                Role::System,
                format!("Could not load {agent} models: {error}"),
            ));
        }
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let catalog = self
            .model_catalogs
            .get(&picker.agent)
            .expect("every agent has a model catalog");

        match key.code {
            KeyCode::Esc => self.model_picker = None,
            KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
            KeyCode::Down if !catalog.models.is_empty() => {
                picker.cursor = (picker.cursor + 1).min(catalog.models.len() - 1);
            }
            KeyCode::Enter if !catalog.models.is_empty() => {
                let agent = picker.agent;
                let model = catalog.models[picker.cursor].clone();
                let adapter_index = Agent::ALL
                    .iter()
                    .position(|candidate| *candidate == agent)
                    .expect("picker agent belongs to Agent::ALL");
                match self.adapters[adapter_index].select_model(model.id.clone()) {
                    Ok(()) => self.model_picker = None,
                    Err(error) => self.messages.push(Message::new(
                        Role::System,
                        format!("Could not select {} for {agent}: {error}", model.label),
                    )),
                }
            }
            _ => {}
        }
    }

    fn submit_prompt(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }

        let agent = self.adapters[self.selected].agent();
        if self.busy_agents.contains(&agent) {
            self.messages.push(Message::new(
                Role::System,
                format!("{agent} is busy; cancel the active turn before sending another prompt."),
            ));
            return;
        }

        let prompt = mem::take(&mut self.input);

        // History and the asynchronous process task both need owned Strings.
        // Sharing one allocation would require Arc<str>; a small prompt clone is
        // clearer at this stage and represents genuinely separate ownership.
        match self.adapters[self.selected].send(prompt.clone()) {
            Ok(()) => {
                self.messages.push(Message::new(Role::User, prompt));
                self.busy_agents.insert(agent);
            }
            Err(error) => {
                self.input = prompt;
                self.messages.push(Message::new(
                    Role::System,
                    format!("Could not send to {agent}: {error}"),
                ));
            }
        }
    }

    fn cancel_selected(&mut self) {
        let agent = self.adapters[self.selected].agent();
        if !self.busy_agents.contains(&agent) {
            return;
        }

        if let Err(error) = self.adapters[self.selected].cancel() {
            self.messages.push(Message::new(
                Role::System,
                format!("Could not cancel {agent}: {error}"),
            ));
        }
    }

    fn apply_agent_event(&mut self, agent: Agent, event: AgentEvent) {
        match event {
            AgentEvent::Started => {
                self.busy_agents.insert(agent);
                self.streaming_messages.remove(&agent);
            }
            AgentEvent::TextDelta(delta) => self.append_delta(agent, delta),
            AgentEvent::Notice(notice) => {
                self.messages
                    .push(Message::new(Role::System, format!("{agent}: {notice}")));
            }
            AgentEvent::ModelsListed { models, selected } => {
                let catalog = self
                    .model_catalogs
                    .get_mut(&agent)
                    .expect("every agent has a model catalog");
                catalog.models = models;
                catalog.selected = selected;
                catalog.loading = false;
                if let Some(picker) = self.model_picker.as_mut()
                    && picker.agent == agent
                {
                    picker.cursor = catalog
                        .selected
                        .as_ref()
                        .and_then(|selected| {
                            catalog
                                .models
                                .iter()
                                .position(|model| &model.id == selected)
                        })
                        .unwrap_or(0);
                }
            }
            AgentEvent::ModelSelected(model) => {
                let catalog = self
                    .model_catalogs
                    .get_mut(&agent)
                    .expect("every agent has a model catalog");
                catalog.selected = Some(model.clone());
                let label = catalog
                    .models
                    .iter()
                    .find(|candidate| candidate.id == model)
                    .map(|candidate| candidate.label.as_str())
                    .unwrap_or(&model);
                self.messages.push(Message::new(
                    Role::System,
                    format!("{agent} model selected: {label}"),
                ));
            }
            AgentEvent::Completed => self.finish_turn(agent),
            AgentEvent::Cancelled => {
                self.finish_turn(agent);
                self.messages.push(Message::new(
                    Role::System,
                    format!("{agent} turn cancelled."),
                ));
            }
            AgentEvent::Error(error) => {
                if let Some(catalog) = self.model_catalogs.get_mut(&agent) {
                    catalog.loading = false;
                }
                self.finish_turn(agent);
                self.messages.push(Message::new(
                    Role::System,
                    format!("{agent} error: {error}"),
                ));
            }
        }
    }

    fn append_delta(&mut self, agent: Agent, delta: String) {
        if let Some(index) = self.streaming_messages.get(&agent).copied()
            && let Some(Message {
                role: Role::Agent(message_agent),
                content,
            }) = self.messages.get_mut(index)
            && *message_agent == agent
        {
            content.push_str(&delta);
            return;
        }

        let index = self.messages.len();
        self.messages.push(Message::new(Role::Agent(agent), delta));
        self.streaming_messages.insert(agent, index);
    }

    fn finish_turn(&mut self, agent: Agent) {
        self.busy_agents.remove(&agent);
        self.streaming_messages.remove(&agent);
    }

    fn draw(&self, frame: &mut Frame) {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(frame.area());
        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(20)])
            .split(vertical[0]);

        self.draw_agents(frame, main[0]);
        self.draw_chat(frame, main[1]);
        self.draw_input(frame, vertical[1]);
        self.draw_status(frame, vertical[2]);
        if let Some(picker) = &self.model_picker {
            self.draw_model_picker(frame, picker);
        }
    }

    fn draw_agents(&self, frame: &mut Frame, area: Rect) {
        let items = self
            .adapters
            .iter()
            .enumerate()
            .map(|(index, adapter)| {
                let agent = adapter.agent();
                let selection = if index == self.selected { "*" } else { " " };
                let activity = if self.busy_agents.contains(&agent) {
                    "●"
                } else {
                    " "
                };
                let style = if index == self.selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{selection}{activity} {agent}")).style(style)
            })
            .collect::<Vec<_>>();

        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Agents "));
        frame.render_widget(list, area);
    }

    fn draw_chat(&self, frame: &mut Frame, area: Rect) {
        let mut lines = Vec::new();

        for message in &self.messages {
            let (label, color) = match message.role {
                Role::User => ("You".to_owned(), Color::Cyan),
                Role::Agent(agent) => (agent.to_string(), Color::Green),
                Role::System => ("DmuxAI".to_owned(), Color::DarkGray),
            };

            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            lines.extend(message.content.lines().map(Line::from));
            lines.push(Line::default());
        }

        let visible_height = area.height.saturating_sub(2) as usize;
        let content_width = area.width.saturating_sub(2);
        let rendered_height = Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(content_width);
        let scroll = rendered_height.saturating_sub(visible_height) as u16;
        let chat = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Chat "))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(chat, area);
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(" Prompt "));
        frame.render_widget(input, area);

        let cursor_offset = self.input.chars().count() as u16;
        let cursor_x = area
            .x
            .saturating_add(1)
            .saturating_add(cursor_offset)
            .min(area.right().saturating_sub(2));
        if self.model_picker.is_none() {
            frame.set_cursor_position((cursor_x, area.y.saturating_add(1)));
        }
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let selected = self.adapters[self.selected].agent();
        let state = if self.busy_agents.contains(&selected) {
            "BUSY"
        } else {
            "IDLE"
        };
        let model = self
            .model_catalogs
            .get(&selected)
            .and_then(|catalog| {
                catalog.selected.as_ref().map(|selected| {
                    catalog
                        .models
                        .iter()
                        .find(|model| &model.id == selected)
                        .map(|model| model.label.as_str())
                        .unwrap_or(selected)
                })
            })
            .unwrap_or("provider default");
        let status = Line::from(vec![
            Span::styled(
                format!(" {selected} {state} "),
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(
                "  Model: {model}  F2 model  Enter send  Ctrl+C cancel  Esc quit"
            )),
        ]);
        frame.render_widget(Paragraph::new(status), area);
    }

    fn draw_model_picker(&self, frame: &mut Frame, picker: &ModelPicker) {
        let area = centered_rect(72, 76, frame.area());
        frame.render_widget(Clear, area);
        let catalog = self
            .model_catalogs
            .get(&picker.agent)
            .expect("every agent has a model catalog");
        let title = format!(" {} models ", picker.agent);
        if catalog.loading {
            frame.render_widget(
                Paragraph::new("Loading models from the provider CLI...")
                    .block(Block::default().borders(Borders::ALL).title(title)),
                area,
            );
            return;
        }

        let items = catalog
            .models
            .iter()
            .map(|model| {
                let marker = if catalog.selected.as_deref() == Some(model.id.as_str()) {
                    "●"
                } else {
                    " "
                };
                ListItem::new(format!("{marker} {}  [{}]", model.label, model.id))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_bottom(" Up/Down move  Enter select  Esc close "),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(
            (!catalog.models.is_empty()).then_some(picker.cursor.min(catalog.models.len() - 1)),
        );
        frame.render_stateful_widget(list, area, &mut state);
    }
}

fn centered_rect(horizontal_percent: u16, vertical_percent: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - vertical_percent) / 2),
        Constraint::Percentage(vertical_percent),
        Constraint::Percentage((100 - vertical_percent) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - horizontal_percent) / 2),
        Constraint::Percentage(horizontal_percent),
        Constraint::Percentage((100 - horizontal_percent) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::agent::AdapterError;

    type SentPrompts = Arc<Mutex<Vec<(Agent, String)>>>;

    struct RecordingAdapter {
        agent: Agent,
        sent: SentPrompts,
    }

    impl AgentAdapter for RecordingAdapter {
        fn agent(&self) -> Agent {
            self.agent
        }

        fn send(&self, prompt: String) -> Result<(), AdapterError> {
            self.sent.lock().unwrap().push((self.agent, prompt));
            Ok(())
        }

        fn cancel(&self) -> Result<(), AdapterError> {
            Ok(())
        }

        fn request_models(&self) -> Result<(), AdapterError> {
            Ok(())
        }

        fn select_model(&self, _model: String) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    fn test_app() -> (App, SentPrompts) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let adapters = Agent::ALL
            .into_iter()
            .map(|agent| {
                Box::new(RecordingAdapter {
                    agent,
                    sent: Arc::clone(&sent),
                }) as Box<dyn AgentAdapter>
            })
            .collect();
        let (event_tx, event_rx) = event_channel();
        drop(event_tx);

        (App::with_runtime(adapters, event_rx, Vec::new()), sent)
    }

    #[test]
    fn submitting_routes_to_the_selected_agent() {
        let (mut app, sent) = test_app();
        app.selected = 1;
        app.input = "Explain ownership".to_owned();

        app.submit_prompt();

        assert_eq!(app.input, "");
        assert_eq!(app.messages[1].role, Role::User);
        assert_eq!(app.messages[1].content, "Explain ownership");
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[(Agent::Cursor, "Explain ownership".to_owned())]
        );
    }

    #[test]
    fn switching_agents_does_not_change_history() {
        let (mut app, _) = test_app();
        app.apply_agent_event(Agent::Codex, AgentEvent::Started);
        app.apply_agent_event(
            Agent::Codex,
            AgentEvent::TextDelta("First reply".to_owned()),
        );
        app.selected = 2;

        assert_eq!(app.messages[1].role, Role::Agent(Agent::Codex));
    }

    #[test]
    fn blank_prompts_are_ignored() {
        let (mut app, sent) = test_app();
        let initial_message_count = app.messages.len();
        app.input = "   ".to_owned();

        app.submit_prompt();

        assert_eq!(app.messages.len(), initial_message_count);
        assert_eq!(app.input, "   ");
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn interleaved_streams_keep_separate_messages() {
        let (mut app, _) = test_app();

        app.apply_agent_event(Agent::Codex, AgentEvent::TextDelta("A".to_owned()));
        app.apply_agent_event(Agent::Agy, AgentEvent::TextDelta("B".to_owned()));
        app.apply_agent_event(Agent::Codex, AgentEvent::TextDelta("C".to_owned()));

        assert_eq!(app.messages[1].content, "AC");
        assert_eq!(app.messages[2].content, "B");
    }

    #[test]
    fn cancellation_clears_busy_state_and_becomes_visible() {
        let (mut app, _) = test_app();
        app.apply_agent_event(Agent::Agy, AgentEvent::Started);

        app.apply_agent_event(Agent::Agy, AgentEvent::Cancelled);

        assert!(!app.busy_agents.contains(&Agent::Agy));
        assert_eq!(app.messages.last().unwrap().role, Role::System);
        assert_eq!(app.messages.last().unwrap().content, "AGY turn cancelled.");
    }

    #[test]
    fn errors_become_visible_system_messages() {
        let (mut app, _) = test_app();

        app.apply_agent_event(Agent::Agy, AgentEvent::Error("offline".to_owned()));

        assert_eq!(app.messages.last().unwrap().role, Role::System);
        assert_eq!(app.messages.last().unwrap().content, "AGY error: offline");
    }

    #[test]
    fn model_catalog_and_selection_are_kept_per_agent() {
        let (mut app, _) = test_app();
        app.apply_agent_event(
            Agent::Cursor,
            AgentEvent::ModelsListed {
                models: vec![
                    ModelInfo::new("cursor-a", "Cursor A"),
                    ModelInfo::new("cursor-b", "Cursor B"),
                ],
                selected: Some("cursor-a".to_owned()),
            },
        );
        app.apply_agent_event(
            Agent::Cursor,
            AgentEvent::ModelSelected("cursor-b".to_owned()),
        );

        let cursor = app.model_catalogs.get(&Agent::Cursor).unwrap();
        assert_eq!(cursor.selected.as_deref(), Some("cursor-b"));
        assert!(app.model_catalogs[&Agent::Codex].selected.is_none());
    }

    #[test]
    fn lowercase_m_remains_prompt_text_and_f2_opens_models() {
        let (mut app, _) = test_app();

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert_eq!(app.input, "m");
        assert!(app.model_picker.is_none());

        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert!(app.model_picker.is_some());
        assert!(app.model_catalogs[&Agent::Codex].loading);
    }
}
