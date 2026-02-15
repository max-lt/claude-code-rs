mod event;
mod markdown;
mod render;

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
    pub max_scroll: u16,
    pub input: String,
    pub cursor: usize,
    pub state: AppState,
    pub pending_perm: Option<PendingPermission>,
    pub spinner_frame: usize,
    pub last_spinner_update: Instant,
    #[cfg(feature = "voice")]
    pub pending_voice_recording: bool,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    session_tx: mpsc::UnboundedSender<SessionCmd>,
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
            max_scroll: 0,
            input: String::new(),
            cursor: 0,
            state: AppState::Idle,
            pending_perm: None,
            spinner_frame: 0,
            last_spinner_update: Instant::now(),
            #[cfg(feature = "voice")]
            pending_voice_recording: false,
            ui_rx,
            session_tx,
        }
    }

    // -- Key handling -------------------------------------------------------

    /// Returns `true` if the app should quit.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        // Ctrl+C: stop Claude if busy, quit if idle
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.state == AppState::Busy {
                let _ = self.session_tx.send(SessionCmd::Stop);
                return false;
            } else {
                return true;
            }
        }

        // Esc: stop Claude if busy, do nothing if idle
        if key.code == KeyCode::Esc && self.state == AppState::Busy {
            let _ = self.session_tx.send(SessionCmd::Stop);
            return false;
        }

        // Permission prompt captures y/n
        if self.pending_perm.is_some() {
            return self.handle_perm_key(key.code);
        }

        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() && self.state != AppState::Busy {
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
                CommandResult::SendMessage(msg) => {
                    // Send the transcribed message as if user typed it
                    self.messages.push(DisplayMessage::User(msg.clone()));
                    self.state = AppState::Busy;
                    self.auto_scroll = true;
                    let _ = self.session_tx.send(SessionCmd::SendMessage(msg));
                    return false;
                }

                #[cfg(feature = "voice")]
                CommandResult::RecordVoice => {
                    self.messages.push(DisplayMessage::Info(
                        "Entering voice recording mode...".to_string(),
                    ));
                    self.pending_voice_recording = true;
                    return false;
                }
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

            UiEvent::ToolStart { name, input } => {
                self.messages.push(DisplayMessage::ToolUse {
                    name,
                    input: Some(input),
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
            }

            UiEvent::Failed(msg) => {
                self.messages.push(DisplayMessage::Error(msg));
                self.state = AppState::Idle;
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
                let cancel = CancellationToken::new();
                let token = cancel.clone();

                let message_future = session.send_message(&text, &mut handler, &token);
                tokio::pin!(message_future);

                // Race message completion against stop commands
                let result = loop {
                    tokio::select! {
                        res = &mut message_future => break res,
                        Some(cmd) = cmd_rx.recv() => {
                            if matches!(cmd, SessionCmd::Stop) {
                                cancel.cancel();
                            }
                            // Other commands ignored while busy
                        }
                    }
                };

                match result {
                    Ok(usage) => {
                        let _ = ui_tx.send(UiEvent::Done(usage));
                    }
                    Err(e) => {
                        let msg = e.to_string();

                        if msg == "Cancelled" {
                            let _ = ui_tx.send(UiEvent::Failed("Stopped.".to_string()));
                        } else {
                            let _ = ui_tx.send(UiEvent::Failed(msg));
                        }
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
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )?;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Restore terminal on panic
    let original_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(
            stdout,
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
        );
        let _ = crossterm::terminal::disable_raw_mode();
        original_hook(info);
    }));

    let mut app = App::new(cwd, model, ui_rx, session_tx);

    // Start with a clean alternate screen
    terminal.clear()?;

    loop {
        // Handle voice recording if requested
        #[cfg(feature = "voice")]
        if app.pending_voice_recording {
            app.pending_voice_recording = false;

            // Exit TUI temporarily - rec::run() handles terminal state
            drop(terminal);

            // Run voice recording (async, blocks until done)
            let rec_result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { crate::commands::rec::run().await })
            });

            // Recreate terminal and re-enable raw mode
            crossterm::terminal::enable_raw_mode()?;
            crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::EnterAlternateScreen,
                crossterm::event::EnableMouseCapture,
            )?;
            let backend = CrosstermBackend::new(std::io::stdout());
            terminal = Terminal::new(backend)?;
            terminal.clear()?;

            // Process result
            match rec_result {
                Ok(CommandResult::SendMessage(msg)) => {
                    app.messages.push(DisplayMessage::User(msg.clone()));
                    app.state = AppState::Busy;
                    app.auto_scroll = true;
                    let _ = app.session_tx.send(SessionCmd::SendMessage(msg));
                }
                Err(e) => {
                    app.messages.push(DisplayMessage::Error(format!(
                        "Voice recording failed: {e}"
                    )));
                }
                _ => {}
            }
        }

        // Update spinner frame if busy (~10 fps for spinner animation)
        if app.state == AppState::Busy
            && app.last_spinner_update.elapsed() >= Duration::from_millis(100)
        {
            app.spinner_frame = (app.spinner_frame + 1) % 10;
            app.last_spinner_update = Instant::now();
        }

        terminal.draw(|f| render::render(&mut app, f))?;

        // Poll crossterm events (~30 fps)
        if crossterm::event::poll(Duration::from_millis(33))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if app.handle_key(key) {
                        break;
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        app.scroll = app.scroll.saturating_sub(3);
                        app.auto_scroll = false;
                    }
                    MouseEventKind::ScrollDown => {
                        app.scroll = app.scroll.saturating_add(3);
                        // Re-enable auto-scroll if we've reached the bottom
                        if app.scroll >= app.max_scroll {
                            app.auto_scroll = true;
                        }
                    }
                    _ => {}
                },
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
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
    )?;

    Ok(())
}
