use std::{
    collections::{HashMap, HashSet},
    io, mem,
    time::{Duration, Instant},
};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
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
use unicode_width::UnicodeWidthStr;

use dmuxai::{
    chat_selection::{CellPosition, ChatSelection},
    clipboard::copy_text,
};

use crate::agent::{
    Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ConversationId, ExecutionMode,
    ModelInfo, TurnId, event_channel, spawn_agy_adapter, spawn_codex_adapter, spawn_cursor_adapter,
};

const RENDER_INTERVAL: Duration = Duration::from_millis(25);
const EVENT_LOOP_PAUSE: Duration = Duration::from_millis(5);
const DEFAULT_INPUT_HEIGHT: u16 = 3;
const MIN_INPUT_HEIGHT: u16 = 3;
const MIN_MAIN_HEIGHT: u16 = 5;
const STATUS_HEIGHT: u16 = 1;

#[derive(Clone, Copy)]
enum SlashAction {
    Agent,
    Mode,
}

#[derive(Clone, Copy)]
struct SlashCommand {
    name: &'static str,
    description: &'static str,
    action: SlashAction,
}

const SLASH_COMMANDS: [SlashCommand; 2] = [
    SlashCommand {
        name: "/agent",
        description: "Switch the active agent",
        action: SlashAction::Agent,
    },
    SlashCommand {
        name: "/mode",
        description: "Choose Ask, Plan, or Implement",
        action: SlashAction::Mode,
    },
];

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

struct AgentPicker {
    cursor: usize,
}

struct ModePicker {
    agent: Agent,
    cursor: usize,
}

#[derive(Default)]
struct SlashPreview {
    cursor: usize,
}

#[derive(Clone, Copy)]
struct UiAreas {
    chat: Rect,
    input: Rect,
    status: Rect,
}

pub struct App {
    exit: bool,
    selected: usize,
    input: String,
    messages: Vec<Message>,
    adapters: Vec<Box<dyn AgentAdapter>>,
    event_rx: mpsc::Receiver<AgentOutput>,
    workers: Vec<JoinHandle<()>>,
    conversation_id: ConversationId,
    next_turn_id: u64,
    active_turns: HashMap<Agent, TurnId>,
    provider_sessions: HashMap<Agent, String>,
    busy_agents: HashSet<Agent>,
    streaming_messages: HashMap<Agent, usize>,
    model_catalogs: HashMap<Agent, ModelCatalog>,
    model_picker: Option<ModelPicker>,
    agent_picker: Option<AgentPicker>,
    mode_picker: Option<ModePicker>,
    slash_preview: Option<SlashPreview>,
    modes: HashMap<Agent, ExecutionMode>,
    chat_scroll: usize,
    chat_selection: Option<ChatSelection>,
    selecting_chat: bool,
    selection_dragged: bool,
    visible_chat_lines: Vec<String>,
    chat_content_area: Rect,
    copy_notice: Option<String>,
    input_height: u16,
    resizing_input: bool,
    terminal_area: Rect,
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
                "Codex, Cursor, and AGY use real structured headless protocols. Type /agent to switch providers, /mode to choose Ask, Plan, or Implement, F2 to choose a model, or Ctrl+C to cancel the selected turn. Implement mode may modify files inside the workspace.",
            )],
            adapters,
            event_rx,
            workers,
            conversation_id: ConversationId::new("default"),
            next_turn_id: 1,
            active_turns: HashMap::new(),
            provider_sessions: HashMap::new(),
            busy_agents: HashSet::new(),
            streaming_messages: HashMap::new(),
            model_catalogs: Agent::ALL
                .into_iter()
                .map(|agent| (agent, ModelCatalog::default()))
                .collect(),
            model_picker: None,
            agent_picker: None,
            mode_picker: None,
            slash_preview: None,
            modes: Agent::ALL
                .into_iter()
                .map(|agent| (agent, ExecutionMode::default()))
                .collect(),
            chat_scroll: 0,
            chat_selection: None,
            selecting_chat: false,
            selection_dragged: false,
            visible_chat_lines: Vec::new(),
            chat_content_area: Rect::default(),
            copy_notice: None,
            input_height: DEFAULT_INPUT_HEIGHT,
            resizing_input: false,
            terminal_area: Rect::default(),
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
                    Event::Mouse(mouse) => self.handle_mouse(mouse),
                    Event::Paste(text)
                        if self.model_picker.is_none()
                            && self.agent_picker.is_none()
                            && self.mode_picker.is_none() =>
                    {
                        self.input.push_str(&text);
                        self.refresh_slash_preview();
                    }
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
        self.active_turns.clear();
        self.streaming_messages.clear();
    }

    fn drain_agent_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(output) = self.event_rx.try_recv() {
            changed |= self.apply_agent_output(output);
        }
        changed
    }

    fn apply_agent_output(&mut self, output: AgentOutput) -> bool {
        if let (Some(conversation_id), Some(turn_id)) = (&output.conversation_id, &output.turn_id) {
            if conversation_id.as_str() != self.conversation_id.as_str()
                || self
                    .active_turns
                    .get(&output.agent)
                    .is_none_or(|active| active.as_str() != turn_id.as_str())
            {
                return false;
            }
        } else if output.conversation_id.is_some()
            || output.turn_id.is_some()
            || event_requires_turn_context(&output.event)
        {
            return false;
        }

        self.apply_agent_event(output.agent, output.event);
        true
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.agent_picker.is_some() {
            self.handle_agent_picker_key(key);
            return;
        }
        if self.mode_picker.is_some() {
            self.handle_mode_picker_key(key);
            return;
        }
        if self.model_picker.is_some() {
            self.handle_model_picker_key(key);
            return;
        }
        if self.slash_preview.is_some() {
            self.handle_slash_preview_key(key);
            return;
        }

        match key.code {
            KeyCode::Esc => self.exit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_selected();
            }
            KeyCode::F(2) => self.open_model_picker(),
            KeyCode::Enter => self.submit_prompt(),
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_slash_preview();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character);
                self.refresh_slash_preview();
            }
            _ => {}
        }
    }

    fn handle_slash_preview_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.slash_preview = None,
            KeyCode::Up => {
                if let Some(preview) = self.slash_preview.as_mut() {
                    preview.cursor = preview.cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                let match_count = matching_slash_commands(&self.input).len();
                if let Some(preview) = self.slash_preview.as_mut() {
                    preview.cursor = (preview.cursor + 1).min(match_count.saturating_sub(1));
                }
            }
            KeyCode::Enter | KeyCode::Tab => self.execute_selected_slash_command(),
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_slash_preview();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character);
                self.refresh_slash_preview();
            }
            _ => {}
        }
    }

    fn refresh_slash_preview(&mut self) {
        let matches = matching_slash_commands(&self.input);
        if matches.is_empty() {
            self.slash_preview = None;
            return;
        }

        let cursor = self
            .slash_preview
            .as_ref()
            .map(|preview| preview.cursor.min(matches.len() - 1))
            .unwrap_or(0);
        self.slash_preview = Some(SlashPreview { cursor });
    }

    fn execute_selected_slash_command(&mut self) {
        let Some(preview) = self.slash_preview.take() else {
            return;
        };
        let Some(command) = matching_slash_commands(&self.input)
            .get(preview.cursor)
            .copied()
        else {
            return;
        };

        self.input.clear();
        self.execute_slash_action(command.action);
    }

    fn execute_slash_action(&mut self, action: SlashAction) {
        match action {
            SlashAction::Agent => self.open_agent_picker(),
            SlashAction::Mode => self.open_mode_picker(),
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            if self.resizing_input {
                self.resizing_input = false;
            } else if self.selecting_chat {
                self.finish_chat_selection();
            }
            return;
        }

        if self.agent_picker.is_some() || self.mode_picker.is_some() || self.model_picker.is_some()
        {
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let areas = self.ui_areas(self.terminal_area);
                if mouse.row == areas.input.y {
                    self.resizing_input = true;
                    self.chat_selection = None;
                    self.selecting_chat = false;
                } else if self.slash_preview.is_none()
                    && let Some(position) = self.chat_cell_at(mouse.column, mouse.row, false)
                {
                    self.chat_selection = Some(ChatSelection::new(position, position));
                    self.selecting_chat = true;
                    self.selection_dragged = false;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing_input => {
                self.resize_input_to_row(mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selecting_chat => {
                if let Some(position) = self.chat_cell_at(mouse.column, mouse.row, true)
                    && let Some(selection) = self.chat_selection.as_mut()
                {
                    selection.set_cursor(position);
                    self.selection_dragged = true;
                }
            }
            MouseEventKind::ScrollUp => {
                self.clear_chat_selection();
                self.chat_scroll = self.chat_scroll.saturating_add(3);
            }
            MouseEventKind::ScrollDown => {
                self.clear_chat_selection();
                self.chat_scroll = self.chat_scroll.saturating_sub(3);
            }
            _ => {}
        }
    }

    fn chat_cell_at(&self, column: u16, row: u16, clamp: bool) -> Option<CellPosition> {
        let area = self.chat_content_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }

        if !clamp
            && (column < area.x || column >= area.right() || row < area.y || row >= area.bottom())
        {
            return None;
        }

        let column = column.clamp(area.x, area.right().saturating_sub(1));
        let row = row.clamp(area.y, area.bottom().saturating_sub(1));
        Some(CellPosition::new(
            usize::from(row.saturating_sub(area.y)),
            usize::from(column.saturating_sub(area.x)),
        ))
    }

    fn finish_chat_selection(&mut self) {
        self.selecting_chat = false;
        if !self.selection_dragged {
            self.chat_selection = None;
            return;
        }

        let Some(selection) = self.chat_selection else {
            return;
        };
        let text = selection.text(&self.visible_chat_lines);
        if text.is_empty() {
            self.chat_selection = None;
            return;
        }

        self.copy_notice = Some(match copy_text(text.clone()) {
            Ok(()) => format!("Copied {} characters", text.chars().count()),
            Err(error) => format!("Copy failed: {error}"),
        });
    }

    fn clear_chat_selection(&mut self) {
        self.chat_selection = None;
        self.selecting_chat = false;
        self.selection_dragged = false;
    }

    fn resize_input_to_row(&mut self, row: u16) {
        let status_y = self.terminal_area.bottom().saturating_sub(STATUS_HEIGHT);
        let requested_height = status_y.saturating_sub(row);
        self.input_height = clamp_input_height(requested_height, self.terminal_area.height);
    }

    fn open_agent_picker(&mut self) {
        self.agent_picker = Some(AgentPicker {
            cursor: self.selected,
        });
    }

    fn handle_agent_picker_key(&mut self, key: KeyEvent) {
        let Some(mut picker) = self.agent_picker.take() else {
            return;
        };

        match key.code {
            KeyCode::Esc => {}
            KeyCode::Up => {
                picker.cursor = picker.cursor.saturating_sub(1);
                self.agent_picker = Some(picker);
            }
            KeyCode::Down => {
                picker.cursor = (picker.cursor + 1).min(Agent::ALL.len() - 1);
                self.agent_picker = Some(picker);
            }
            KeyCode::Enter => {
                self.selected = picker.cursor;
                let agent = Agent::ALL[self.selected];
                self.messages.push(Message::new(
                    Role::System,
                    format!("Active agent: {agent}."),
                ));
            }
            _ => self.agent_picker = Some(picker),
        }
    }

    fn open_mode_picker(&mut self) {
        let agent = self.adapters[self.selected].agent();
        if self.busy_agents.contains(&agent) {
            self.messages.push(Message::new(
                Role::System,
                format!("Finish or cancel the {agent} turn before changing modes."),
            ));
            return;
        }

        let selected_mode = self.modes.get(&agent).copied().unwrap_or_default();
        let cursor = ExecutionMode::ALL
            .iter()
            .position(|mode| *mode == selected_mode)
            .unwrap_or(0);
        self.mode_picker = Some(ModePicker { agent, cursor });
    }

    fn handle_mode_picker_key(&mut self, key: KeyEvent) {
        let Some(mut picker) = self.mode_picker.take() else {
            return;
        };

        match key.code {
            KeyCode::Esc => {}
            KeyCode::Up => {
                picker.cursor = picker.cursor.saturating_sub(1);
                self.mode_picker = Some(picker);
            }
            KeyCode::Down => {
                picker.cursor = (picker.cursor + 1).min(ExecutionMode::ALL.len() - 1);
                self.mode_picker = Some(picker);
            }
            KeyCode::Enter => {
                let mode = ExecutionMode::ALL[picker.cursor];
                let adapter_index = Agent::ALL
                    .iter()
                    .position(|agent| *agent == picker.agent)
                    .expect("picker agent belongs to Agent::ALL");
                match self.adapters[adapter_index].select_mode(mode) {
                    Ok(()) => {
                        self.modes.insert(picker.agent, mode);
                        self.messages.push(Message::new(
                            Role::System,
                            format!("{} mode selected: {mode}.", picker.agent),
                        ));
                    }
                    Err(error) => {
                        self.messages.push(Message::new(
                            Role::System,
                            format!("Could not select {mode} for {}: {error}", picker.agent),
                        ));
                        self.mode_picker = Some(picker);
                    }
                }
            }
            _ => self.mode_picker = Some(picker),
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

        let prompt = mem::take(&mut self.input);
        self.slash_preview = None;
        match prompt.as_str() {
            "/agent" => {
                self.execute_slash_action(SlashAction::Agent);
                return;
            }
            "/mode" => {
                self.execute_slash_action(SlashAction::Mode);
                return;
            }
            _ => {}
        }

        let agent = self.adapters[self.selected].agent();
        if self.busy_agents.contains(&agent) {
            self.input = prompt;
            self.messages.push(Message::new(
                Role::System,
                format!("{agent} is busy; cancel the active turn before sending another prompt."),
            ));
            return;
        }

        self.chat_scroll = 0;

        // History and the asynchronous process task both need owned Strings.
        // Sharing one allocation would require Arc<str>; a small prompt clone is
        // clearer at this stage and represents genuinely separate ownership.
        let turn_id = TurnId::new(format!("turn-{}", self.next_turn_id));
        let request = AgentRequest {
            conversation_id: self.conversation_id.clone(),
            turn_id: turn_id.clone(),
            prompt: prompt.clone(),
            resume_session: self.provider_sessions.get(&agent).cloned(),
        };
        match self.adapters[self.selected].send(request) {
            Ok(()) => {
                self.next_turn_id += 1;
                self.messages.push(Message::new(Role::User, prompt));
                self.active_turns.insert(agent, turn_id);
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
            AgentEvent::ProviderSessionBound(session_id) => {
                self.provider_sessions.insert(agent, session_id);
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
            AgentEvent::ModeSelected(mode) => {
                self.modes.insert(agent, mode);
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
        self.active_turns.remove(&agent);
        self.streaming_messages.remove(&agent);
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.terminal_area = frame.area();
        let areas = self.ui_areas(self.terminal_area);

        self.draw_chat(frame, areas.chat);
        self.draw_input(frame, areas.input);
        self.draw_status(frame, areas.status);
        if let Some(picker) = &self.model_picker {
            self.draw_model_picker(frame, picker);
        } else if let Some(picker) = &self.agent_picker {
            self.draw_agent_picker(frame, picker);
        } else if let Some(picker) = &self.mode_picker {
            self.draw_mode_picker(frame, picker);
        } else if let Some(preview) = &self.slash_preview {
            self.draw_slash_preview(frame, preview, areas.input);
        }
    }

    fn ui_areas(&self, area: Rect) -> UiAreas {
        let input_height = clamp_input_height(self.input_height, area.height);
        let main_height = area
            .height
            .saturating_sub(input_height.saturating_add(STATUS_HEIGHT));
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(main_height),
                Constraint::Length(input_height),
                Constraint::Length(STATUS_HEIGHT),
            ])
            .split(area);
        UiAreas {
            chat: vertical[0],
            input: vertical[1],
            status: vertical[2],
        }
    }

    fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
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
        let max_scroll = rendered_height.saturating_sub(visible_height);
        let scroll = max_scroll
            .saturating_sub(self.chat_scroll.min(max_scroll))
            .min(u16::MAX as usize) as u16;
        let chat = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Chat "))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(chat, area);

        self.chat_content_area = Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        self.visible_chat_lines = visible_buffer_lines(frame, self.chat_content_area);
        if let Some(selection) = self.chat_selection {
            highlight_chat_selection(frame, self.chat_content_area, selection);
        }
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let title = if self.resizing_input {
            " Prompt (resizing) "
        } else {
            " Prompt (drag top border) "
        };
        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(input, area);

        let cursor_offset = input_cursor_offset(self.input.as_str());
        let cursor_x = area
            .x
            .saturating_add(1)
            .saturating_add(cursor_offset)
            .min(area.right().saturating_sub(2));
        if self.model_picker.is_none() && self.agent_picker.is_none() && self.mode_picker.is_none()
        {
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
                "  Model: {model}  Mode: {}  /mode  /agent  F2 model  Enter send  Ctrl+C cancel  Esc quit{}",
                self.modes.get(&selected).copied().unwrap_or_default(),
                self.copy_notice
                    .as_ref()
                    .map(|notice| format!("  {notice}"))
                    .unwrap_or_default(),
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

    fn draw_agent_picker(&self, frame: &mut Frame, picker: &AgentPicker) {
        let area = centered_fixed_rect(48, 7, frame.area());
        frame.render_widget(Clear, area);
        let items = Agent::ALL
            .into_iter()
            .enumerate()
            .map(|(index, agent)| {
                let current = if index == self.selected { "*" } else { " " };
                let mode = self.modes.get(&agent).copied().unwrap_or_default();
                ListItem::new(format!("{current} {agent}  [{mode}]"))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Select agent ")
                    .title_bottom(" Up/Down move  Enter select  Esc close "),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(Some(picker.cursor));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_mode_picker(&self, frame: &mut Frame, picker: &ModePicker) {
        let area = centered_fixed_rect(56, 7, frame.area());
        frame.render_widget(Clear, area);
        let selected_mode = self.modes.get(&picker.agent).copied().unwrap_or_default();
        let items = ExecutionMode::ALL
            .into_iter()
            .map(|mode| {
                let current = if mode == selected_mode { "*" } else { " " };
                let description = match mode {
                    ExecutionMode::Ask => "read-only questions and explanations",
                    ExecutionMode::Plan => "read-only implementation planning",
                    ExecutionMode::Implement => "workspace edits and sandboxed commands",
                };
                ListItem::new(format!("{current} {mode:<9} {description}"))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} mode ", picker.agent))
                    .title_bottom(" Up/Down move  Enter select  Esc close "),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(Some(picker.cursor));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_slash_preview(&self, frame: &mut Frame, preview: &SlashPreview, input_area: Rect) {
        let matches = matching_slash_commands(&self.input);
        if matches.is_empty() || input_area.y < 3 || input_area.width < 8 {
            return;
        }

        let height = (matches.len() as u16 + 2).min(input_area.y);
        if height < 3 {
            return;
        }
        let area = Rect::new(
            input_area.x,
            input_area.y.saturating_sub(height),
            input_area.width.min(64),
            height,
        );
        frame.render_widget(Clear, area);
        let items = matches
            .iter()
            .map(|command| ListItem::new(format!("{:<10} {}", command.name, command.description)))
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Commands "))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut state = ListState::default()
            .with_selected(Some(preview.cursor.min(matches.len().saturating_sub(1))));
        frame.render_stateful_widget(list, area, &mut state);
    }
}

fn input_cursor_offset(input: &str) -> u16 {
    UnicodeWidthStr::width(input).min(u16::MAX as usize) as u16
}

fn matching_slash_commands(input: &str) -> Vec<&'static SlashCommand> {
    if !input.starts_with('/') || input.chars().any(char::is_whitespace) {
        return Vec::new();
    }

    SLASH_COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(input))
        .collect()
}

fn visible_buffer_lines(frame: &mut Frame, area: Rect) -> Vec<String> {
    let buffer = frame.buffer_mut();
    (area.y..area.bottom())
        .map(|row| {
            let mut line = String::new();
            for column in area.x..area.right() {
                line.push_str(buffer[(column, row)].symbol());
            }
            line.trim_end().to_owned()
        })
        .collect()
}

fn highlight_chat_selection(frame: &mut Frame, area: Rect, selection: ChatSelection) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (start, end) = selection.endpoints();
    let last_row = usize::from(area.height.saturating_sub(1));
    let last_column = usize::from(area.width.saturating_sub(1));
    let end_row = end.row.min(last_row);
    let style = Style::default()
        .fg(Color::Black)
        .bg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    let buffer = frame.buffer_mut();

    for row in start.row.min(last_row)..=end_row {
        let first_column = if row == start.row {
            start.column.min(last_column)
        } else {
            0
        };
        let final_column = if row == end.row {
            end.column.min(last_column)
        } else {
            last_column
        };
        for column in first_column..=final_column {
            buffer[(
                area.x.saturating_add(column as u16),
                area.y.saturating_add(row as u16),
            )]
                .set_style(style);
        }
    }
}

fn event_requires_turn_context(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::Started
            | AgentEvent::ProviderSessionBound(_)
            | AgentEvent::TextDelta(_)
            | AgentEvent::Completed
            | AgentEvent::Cancelled
    )
}

fn clamp_input_height(requested_height: u16, terminal_height: u16) -> u16 {
    let max_height = terminal_height.saturating_sub(MIN_MAIN_HEIGHT + STATUS_HEIGHT);
    let min_height = MIN_INPUT_HEIGHT.min(max_height);
    requested_height.clamp(min_height, max_height)
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

fn centered_fixed_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ratatui::backend::TestBackend;

    use super::*;
    use crate::agent::AdapterError;

    type SentRequests = Arc<Mutex<Vec<(Agent, AgentRequest)>>>;

    struct RecordingAdapter {
        agent: Agent,
        sent: SentRequests,
    }

    impl AgentAdapter for RecordingAdapter {
        fn agent(&self) -> Agent {
            self.agent
        }

        fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
            self.sent.lock().unwrap().push((self.agent, request));
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

        fn select_mode(&self, _mode: ExecutionMode) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    fn test_app() -> (App, SentRequests) {
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

    fn render_text_at(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();

        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, Agent::Cursor);
        assert_eq!(sent[0].1.prompt, "Explain ownership");
        assert_eq!(sent[0].1.conversation_id, app.conversation_id);
        assert_eq!(sent[0].1.turn_id, TurnId::new("turn-1"));
        assert_eq!(sent[0].1.resume_session, None);
    }

    #[test]
    fn provider_session_binding_is_reused_by_the_same_agent() {
        let (mut app, sent) = test_app();
        app.input = "first".to_owned();
        app.submit_prompt();

        let first_request = sent.lock().unwrap()[0].1.clone();
        assert!(app.apply_agent_output(AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(first_request.conversation_id.clone()),
            turn_id: Some(first_request.turn_id.clone()),
            event: AgentEvent::ProviderSessionBound("codex-thread".to_owned()),
        }));
        assert!(app.apply_agent_output(AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(first_request.conversation_id),
            turn_id: Some(first_request.turn_id),
            event: AgentEvent::Completed,
        }));

        app.input = "second".to_owned();
        app.submit_prompt();

        let sent = sent.lock().unwrap();
        assert_eq!(sent[1].1.resume_session.as_deref(), Some("codex-thread"));
        assert_eq!(sent[1].1.turn_id, TurnId::new("turn-2"));
    }

    #[test]
    fn stale_or_incomplete_turn_outputs_are_not_rendered() {
        let (mut app, _) = test_app();
        app.input = "hello".to_owned();
        app.submit_prompt();
        let initial_message_count = app.messages.len();

        assert!(!app.apply_agent_output(AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(app.conversation_id.clone()),
            turn_id: Some(TurnId::new("stale-turn")),
            event: AgentEvent::TextDelta("stale".to_owned()),
        }));
        assert!(!app.apply_agent_output(AgentOutput {
            agent: Agent::Codex,
            conversation_id: None,
            turn_id: None,
            event: AgentEvent::TextDelta("unscoped".to_owned()),
        }));

        assert_eq!(app.messages.len(), initial_message_count);
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

    #[test]
    fn startup_message_explains_provider_safety_modes() {
        let (app, _) = test_app();

        assert!(app.messages[0].content.contains("/mode"));
        assert!(app.messages[0].content.contains("Implement mode"));
    }

    #[test]
    fn input_cursor_offset_uses_terminal_width_for_korean() {
        assert_eq!(input_cursor_offset("abc"), 3);
        assert_eq!(input_cursor_offset("한글"), 4);
        assert_eq!(input_cursor_offset("a한글"), 5);
    }

    #[test]
    fn agent_command_opens_picker_without_sending_prompt() {
        let (mut app, sent) = test_app();
        app.input = "/agent".to_owned();

        app.submit_prompt();

        assert!(app.agent_picker.is_some());
        assert!(app.input.is_empty());
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn slash_preview_filters_and_executes_mode_with_tab() {
        let (mut app, _) = test_app();

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(matching_slash_commands(&app.input).len(), 2);
        assert!(app.slash_preview.is_some());

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert_eq!(matching_slash_commands(&app.input)[0].name, "/mode");

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.slash_preview.is_none());
        assert!(app.mode_picker.is_some());
        assert!(app.input.is_empty());
    }

    #[test]
    fn slash_preview_escape_closes_preview_before_exiting() {
        let (mut app, _) = test_app();
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.exit);
        assert!(app.slash_preview.is_none());
        assert_eq!(app.input, "/");

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.exit);
    }

    #[test]
    fn whitespace_after_command_sends_it_as_regular_prompt() {
        let (mut app, sent) = test_app();
        app.input = "/agent ".to_owned();

        app.submit_prompt();

        assert!(app.agent_picker.is_none());
        assert_eq!(sent.lock().unwrap()[0].1.prompt, "/agent ");
    }

    #[test]
    fn slash_preview_renders_commands_on_regular_and_small_terminals() {
        let (mut app, _) = test_app();
        app.input = "/".to_owned();
        app.refresh_slash_preview();

        let regular = render_text_at(&mut app, 80, 24);
        assert!(regular.contains("Commands"));
        assert!(regular.contains("/agent"));
        assert!(regular.contains("/mode"));

        let small = render_text_at(&mut app, 40, 10);
        assert!(small.contains("Commands"));
        assert!(small.contains("/agent"));
    }

    #[test]
    fn mode_picker_keeps_a_separate_mode_for_each_agent() {
        let (mut app, _) = test_app();
        app.input = "/mode".to_owned();
        app.submit_prompt();

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.modes[&Agent::Codex], ExecutionMode::Implement);
        assert_eq!(app.modes[&Agent::Cursor], ExecutionMode::Ask);
        assert!(app.mode_picker.is_none());
    }

    #[test]
    fn busy_agent_cannot_open_mode_picker() {
        let (mut app, _) = test_app();
        app.busy_agents.insert(Agent::Codex);
        app.input = "/mode".to_owned();

        app.submit_prompt();

        assert!(app.mode_picker.is_none());
        assert!(
            app.messages
                .last()
                .unwrap()
                .content
                .contains("before changing modes")
        );
    }

    #[test]
    fn agent_picker_replaces_the_persistent_sidebar() {
        let (mut app, _) = test_app();

        let initial = render_text_at(&mut app, 80, 24);
        assert!(!initial.contains("Agents"));

        app.input = "/agent".to_owned();
        app.submit_prompt();

        let with_picker = render_text_at(&mut app, 80, 24);
        assert!(with_picker.contains("Select agent"));

        let small_terminal = render_text_at(&mut app, 40, 10);
        assert!(small_terminal.contains("Select agent"));
    }

    #[test]
    fn agent_picker_changes_active_agent() {
        let (mut app, _) = test_app();
        app.input = "/agent".to_owned();
        app.submit_prompt();

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.selected, 1);
        assert!(app.agent_picker.is_none());
        assert_eq!(
            app.messages.last().unwrap().content,
            "Active agent: Cursor."
        );
    }

    #[test]
    fn arrow_keys_do_not_change_agent_outside_picker() {
        let (mut app, _) = test_app();

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.selected, 0);
    }

    #[test]
    fn mouse_scroll_moves_chat_without_changing_agent() {
        let (mut app, _) = test_app();
        app.selected = 1;
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 30,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(mouse);

        assert_eq!(app.selected, 1);
        assert_eq!(app.chat_scroll, 3);
    }

    #[test]
    fn dragging_inside_chat_selects_rendered_text() {
        let (mut app, _) = test_app();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let content = app.chat_content_area;

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: content.x,
            row: content.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: content.x + 4,
            row: content.y + 1,
            modifiers: KeyModifiers::NONE,
        });

        let selected = app
            .chat_selection
            .expect("drag should create a selection")
            .text(&app.visible_chat_lines);
        assert_eq!(selected, "Codex");
        assert!(app.selection_dragged);
    }

    #[test]
    fn dragging_prompt_border_resizes_chat_and_input_panes() {
        let (mut app, _) = test_app();
        app.terminal_area = Rect::new(0, 0, 100, 30);
        let input_top = app.ui_areas(app.terminal_area).input.y;

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 40,
            row: input_top,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 40,
            row: input_top - 5,
            modifiers: KeyModifiers::NONE,
        });

        assert!(app.resizing_input);
        assert_eq!(app.input_height, DEFAULT_INPUT_HEIGHT + 5);
        let areas = app.ui_areas(app.terminal_area);
        assert_eq!(areas.input.height, DEFAULT_INPUT_HEIGHT + 5);
        assert_eq!(areas.chat.height, 30 - STATUS_HEIGHT - app.input_height);

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 40,
            row: input_top - 5,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.resizing_input);
    }

    #[test]
    fn prompt_resize_keeps_minimum_chat_and_input_heights() {
        assert_eq!(clamp_input_height(1, 30), MIN_INPUT_HEIGHT);
        assert_eq!(
            clamp_input_height(u16::MAX, 30),
            30 - MIN_MAIN_HEIGHT - STATUS_HEIGHT
        );
    }
}
