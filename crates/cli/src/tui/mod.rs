mod event;
mod markdown;
mod render;

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use claude_code_core::api::Usage;
use claude_code_core::session::Session;

use crate::commands::{self, CommandResult};
use crate::permissions::ChannelPermissions;

pub use event::{ChannelEventHandler, SessionCmd, UiEvent};

// ---------------------------------------------------------------------------
// Display model
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub enum AppState {
    Idle,
    Busy,
}

pub struct PendingPermission {
    pub description: String,
    pub respond: std_mpsc::SyncSender<bool>,
}

pub enum DisplayMessage {
    User(String),
    AssistantText(String),
    ToolUse {
        name: String,
        input: Option<serde_json::Value>,
        output: Option<String>,
        is_error: bool,
    },
    Error(String),
    Info(String),
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct App {
    pub cwd: PathBuf,
    pub model: String,
    pub usage: Usage,
    pub messages: Vec<DisplayMessage>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub input: String,
    pub cursor: usize,
    pub state: AppState,
    pub pending_perm: Option<PendingPermission>,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    session_tx: mpsc::UnboundedSender<SessionCmd>,
    ctrl_c_count: u8,
}

impl App {
    fn new(
        cwd: PathBuf,
        model: String,
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        session_tx: mpsc::UnboundedSender<SessionCmd>,
    ) -> Self {
        Self {
            cwd,
            model,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
            },
            messages: vec![DisplayMessage::Info(
                "Type your message to start. Ctrl+C to exit.".to_string(),
            )],
            scroll: 0,
            auto_scroll: true,
            input: String::new(),
            cursor: 0,
            state: AppState::Idle,
            pending_perm: None,
            ui_rx,
            session_tx,
            ctrl_c_count: 0,
        }
    }

    // -- Key handling -------------------------------------------------------

    /// Returns `true` if the app should quit.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        // Esc or Ctrl+C: stop Claude if busy, else quit
        let is_stop_key = key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));

        if is_stop_key {
            if self.state == AppState::Busy {
                // First interrupt: stop Claude
                self.ctrl_c_count += 1;
                if self.ctrl_c_count == 1 {
                    let _ = self.session_tx.send(SessionCmd::Stop);
                    self.messages.push(DisplayMessage::Info(
                        "⚠ Stopping... (Ctrl+C again to exit)".to_string(),
                    ));
                    return false;
                } else {
                    // Second Ctrl+C while still busy: force quit
                    return true;
                }
            } else {
                // Not busy: quit immediately
                return true;
            }
        }

        // Permission prompt captures y/n
        if self.pending_perm.is_some() {
            return self.handle_perm_key(key.code);
        }

        // Ignore input while busy
        if self.state == AppState::Busy {
            return false;
        }

        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    return self.submit_input();
                }
            }

            KeyCode::Char(c) => {
                let byte_pos = self
                    .input
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(self.input.len());
                self.input.insert(byte_pos, c);
                self.cursor += 1;
            }

            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let byte_pos = self
                        .input
                        .char_indices()
                        .nth(self.cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.input.len());
                    self.input.remove(byte_pos);
                }
            }

            KeyCode::Delete => {
                if self.cursor < self.input.chars().count() {
                    let byte_pos = self
                        .input
                        .char_indices()
                        .nth(self.cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.input.len());
                    self.input.remove(byte_pos);
                }
            }

            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
            }

            KeyCode::Right => {
                if self.cursor < self.input.chars().count() {
                    self.cursor += 1;
                }
            }

            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),

            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll = self.scroll.saturating_sub(1);
                self.auto_scroll = false;
            }

            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll = self.scroll.saturating_add(1);
                self.auto_scroll = true; // re-enable when scrolling down
            }

            _ => {}
        }

        false
    }

    fn handle_perm_key(&mut self, code: KeyCode) -> bool {
        let respond = match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(false),
            _ => None,
        };

        if let Some(allowed) = respond
            && let Some(perm) = self.pending_perm.take()
        {
            let _ = perm.respond.send(allowed);
        }

        false
    }

    /// Process input: slash command or message. Returns `true` to quit.
    fn submit_input(&mut self) -> bool {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;

        // Slash commands
        if let Some(result) = commands::handle_command(&text, &self.model) {
            match result {
                CommandResult::Exit => return true,

                CommandResult::Clear => {
                    let _ = self.session_tx.send(SessionCmd::Clear);
                    self.messages.clear();
                    self.messages
                        .push(DisplayMessage::Info("Conversation cleared.".to_string()));
                }

                CommandResult::SetModel { id, label } => {
                    let _ = self.session_tx.send(SessionCmd::SetModel(id.clone()));
                    self.model = id;
                    self.messages
                        .push(DisplayMessage::Info(format!("Switched to {label}.")));
                }

                CommandResult::Info(info) => {
                    self.messages.push(DisplayMessage::Info(info));
                }

                CommandResult::Continue => {}

                #[cfg(feature = "voice")]
                CommandResult::SendMessage(_) => {}
            }

            return false;
        }

        // Regular message
        self.messages.push(DisplayMessage::User(text.clone()));
        self.state = AppState::Busy;
        self.auto_scroll = true;
        let _ = self.session_tx.send(SessionCmd::SendMessage(text));

        false
    }

    // -- UI event handling --------------------------------------------------

    fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Text(text) => {
                if let Some(DisplayMessage::AssistantText(existing)) = self.messages.last_mut() {
                    existing.push_str(&text);
                } else {
                    self.messages.push(DisplayMessage::AssistantText(text));
                }
            }

            UiEvent::Error(msg) => {
                self.messages.push(DisplayMessage::Error(msg));
            }

            UiEvent::ToolStart { name } => {
                self.messages.push(DisplayMessage::ToolUse {
                    name,
                    input: None,
                    output: None,
                    is_error: false,
                });
            }

            UiEvent::ToolExecuting { input } => {
                if let Some(DisplayMessage::ToolUse { input: inp, .. }) = self.messages.last_mut() {
                    *inp = Some(input);
                }
            }

            UiEvent::ToolResult { output, is_error } => {
                if let Some(DisplayMessage::ToolUse {
                    output: out,
                    is_error: err,
                    ..
                }) = self.messages.last_mut()
                {
                    *out = Some(output);
                    *err = is_error;
                }
            }

            UiEvent::ToolEnd => {}

            UiEvent::Done(usage) => {
                self.usage.input_tokens += usage.input_tokens;
                self.usage.output_tokens += usage.output_tokens;
                self.state = AppState::Idle;
                self.ctrl_c_count = 0; // Reset stop counter
            }

            UiEvent::Failed(msg) => {
                self.messages.push(DisplayMessage::Error(msg));
                self.state = AppState::Idle;
                self.ctrl_c_count = 0; // Reset stop counter
            }

            UiEvent::PermissionRequest {
                description,
                respond,
            } => {
                self.pending_perm = Some(PendingPermission {
                    description,
                    respond,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session background task
// ---------------------------------------------------------------------------

async fn session_loop(
    mut session: Session<ChannelPermissions>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) {
    let mut handler = ChannelEventHandler { tx: ui_tx.clone() };

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            SessionCmd::SendMessage(text) => {
                let message_future = session.send_message(&text, &mut handler);
                tokio::pin!(message_future);

                let result = tokio::select! {
                    res = &mut message_future => Some(res),
                    Some(SessionCmd::Stop) = cmd_rx.recv() => {
                        // Stop received, cancel the message
                        None
                    }
                };

                match result {
                    Some(Ok(usage)) => {
                        let _ = ui_tx.send(UiEvent::Done(usage));
                    }
                    Some(Err(e)) => {
                        let _ = ui_tx.send(UiEvent::Failed(e.to_string()));
                    }
                    None => {
                        // Stopped by user
                        let _ = ui_tx.send(UiEvent::Failed("Stopped by user.".to_string()));
                    }
                }
            }

            SessionCmd::Stop => {
                // Stop command received while idle, ignore
            }

            SessionCmd::SetModel(id) => {
                session.set_model(id);
            }

            SessionCmd::Clear => {
                session.clear();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(
    cwd: PathBuf,
    session: Session<ChannelPermissions>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
) -> Result<()> {
    let model = session.model().to_string();

    // Channel for UI → session commands
    let (session_tx, session_rx) = mpsc::unbounded_channel();

    // Spawn session loop in background
    tokio::spawn(session_loop(session, session_rx, ui_tx));

    // Terminal setup
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Restore terminal on panic
    let original_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
        original_hook(info);
    }));

    let mut app = App::new(cwd, model, ui_rx, session_tx);

    // Start with a clean alternate screen
    terminal.clear()?;

    loop {
        terminal.draw(|f| render::render(&app, f))?;

        // Poll crossterm events (~30 fps)
        if crossterm::event::poll(Duration::from_millis(33))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if app.handle_key(key) {
                        break;
                    }
                }
                Event::Resize(_, _) => {
                    // Force full redraw after resize
                    terminal.clear()?;
                }
                _ => {}
            }
        }

        // Drain all pending UI events (batches fast streaming)
        while let Ok(ev) = app.ui_rx.try_recv() {
            app.handle_ui_event(ev);
        }
    }

    // Cleanup
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;

    Ok(())
}
