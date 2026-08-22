use std::{io, mem};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::agent::{Agent, AgentAdapter, AgentEvent, FakeAgent};

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

pub struct App {
    exit: bool,
    selected: usize,
    input: String,
    messages: Vec<Message>,
    adapters: Vec<Box<dyn AgentAdapter>>,
    streaming_agent: Option<Agent>,
}

impl App {
    pub fn new() -> Self {
        let adapters = Agent::ALL
            .into_iter()
            .map(|agent| Box::new(FakeAgent::new(agent)) as Box<dyn AgentAdapter>)
            .collect();

        Self {
            exit: false,
            selected: 0,
            input: String::new(),
            messages: vec![Message::new(
                Role::System,
                "Local UI prototype: choose an agent and send a prompt.",
            )],
            adapters,
            streaming_agent: None,
        }
    }

    pub fn run<B>(&mut self, terminal: &mut Terminal<B>) -> Result<(), B::Error>
    where
        B: Backend,
        B::Error: From<io::Error>,
    {
        terminal.draw(|frame| self.draw(frame))?;

        while !self.exit {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
                Event::Paste(text) => self.input.push_str(&text),
                _ => {}
            }

            if !self.exit {
                terminal.draw(|frame| self.draw(frame))?;
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.exit = true,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.adapters.len() - 1);
            }
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

    fn submit_prompt(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }

        let prompt = mem::take(&mut self.input);
        let agent = self.adapters[self.selected].agent();

        // The adapter only borrows the prompt. Once it returns owned events, the
        // original String can move into history without an unnecessary clone.
        let events = self.adapters[self.selected].send(&prompt);
        self.messages.push(Message::new(Role::User, prompt));

        for event in events {
            self.apply_agent_event(agent, event);
        }
    }

    fn apply_agent_event(&mut self, agent: Agent, event: AgentEvent) {
        match event {
            AgentEvent::TextDelta(delta) => {
                if self.streaming_agent == Some(agent)
                    && let Some(Message {
                        role: Role::Agent(message_agent),
                        content,
                    }) = self.messages.last_mut()
                    && *message_agent == agent
                {
                    content.push_str(&delta);
                } else {
                    self.messages.push(Message::new(Role::Agent(agent), delta));
                    self.streaming_agent = Some(agent);
                }
            }
            AgentEvent::Completed => self.streaming_agent = None,
            AgentEvent::Error(error) => {
                self.messages.push(Message::new(
                    Role::System,
                    format!("{agent} error: {error}"),
                ));
                self.streaming_agent = None;
            }
        }
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
    }

    fn draw_agents(&self, frame: &mut Frame, area: Rect) {
        let items = self
            .adapters
            .iter()
            .enumerate()
            .map(|(index, adapter)| {
                let marker = if index == self.selected { "* " } else { "  " };
                let style = if index == self.selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{marker}{}", adapter.agent())).style(style)
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
        let scroll = lines.len().saturating_sub(visible_height) as u16;
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
        frame.set_cursor_position((cursor_x, area.y.saturating_add(1)));
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let selected = self.adapters[self.selected].agent();
        let status = Line::from(vec![
            Span::styled(
                format!(" {selected} "),
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  Up/Down select  Enter send  Backspace edit  Esc quit"),
        ]);
        frame.render_widget(Paragraph::new(status), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submitting_routes_to_the_selected_agent() {
        let mut app = App::new();
        app.selected = 1;
        app.input = "Explain ownership".to_owned();

        app.submit_prompt();

        assert_eq!(app.input, "");
        assert_eq!(app.messages[1].role, Role::User);
        assert_eq!(app.messages[1].content, "Explain ownership");
        assert_eq!(app.messages[2].role, Role::Agent(Agent::Cursor));
        assert!(app.messages[2].content.contains("Cursor"));
    }

    #[test]
    fn switching_agents_does_not_change_history() {
        let mut app = App::new();
        app.input = "First prompt".to_owned();
        app.submit_prompt();
        app.selected = 2;

        assert_eq!(app.messages[2].role, Role::Agent(Agent::Codex));
    }

    #[test]
    fn blank_prompts_are_ignored() {
        let mut app = App::new();
        let initial_message_count = app.messages.len();
        app.input = "   ".to_owned();

        app.submit_prompt();

        assert_eq!(app.messages.len(), initial_message_count);
        assert_eq!(app.input, "   ");
    }

    #[test]
    fn streaming_deltas_join_one_agent_message() {
        let mut app = App::new();

        app.apply_agent_event(Agent::Codex, AgentEvent::TextDelta("Hello".to_owned()));
        app.apply_agent_event(Agent::Codex, AgentEvent::TextDelta(" world".to_owned()));
        app.apply_agent_event(Agent::Codex, AgentEvent::Completed);

        assert_eq!(app.messages.last().unwrap().content, "Hello world");
        assert_eq!(app.streaming_agent, None);
    }

    #[test]
    fn errors_become_visible_system_messages() {
        let mut app = App::new();

        app.apply_agent_event(Agent::Agy, AgentEvent::Error("offline".to_owned()));

        assert_eq!(app.messages.last().unwrap().role, Role::System);
        assert_eq!(app.messages.last().unwrap().content, "AGY error: offline");
    }
}
