mod event;
mod markdown;
mod render;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use claude_code_core::api::{
    AVAILABLE_MODELS, Content, ContentBlock, DEFAULT_MODEL, Message, Usage,
};
use claude_code_core::history;
use claude_code_core::lmstudio::{self, LmModel};
use claude_code_core::provider::{context_window_for, lm_studio_model_id};
use claude_code_core::session::Session;

use crate::commands::{self, CommandResult};
use crate::permissions::ChannelPermissions;

pub use event::{ChannelEventHandler, SessionCmd, UiEvent};

// ---------------------------------------------------------------------------
// Display model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum AppState {
    Idle,
    Busy,
}

impl AppState {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Busy)
    }
}

/// What is known about the local LM Studio server.
#[derive(Clone, PartialEq)]
pub enum LmStatus {
    /// Never probed.
    Unknown,
    Checking,
    /// Unreachable; carries the reason.
    Down(String),
    Up,
}

/// One row of the model picker.
pub struct PickerEntry {
    pub id: String,
    pub label: String,
    /// Shown dimmed after the label.
    pub detail: String,
    pub is_default: bool,
    /// Overrides the table lookup, for models whose window is only known live.
    pub context_window: Option<u64>,
}

pub struct PendingPermission {
    pub description: String,
    pub respond: std_mpsc::SyncSender<bool>,
}

pub enum DisplayMessage {
    User(String),
    AssistantText(String),
    Thinking(String),
    ToolUse {
        name: String,
        input: Option<serde_json::Value>,
        output: Option<String>,
        is_error: bool,
    },
    Error(String),
    Info(String),
}

/// Rebuild what the screen showed from a saved conversation.
/// Thinking blocks are not stored, so they do not come back.
pub fn display_history(messages: &[Message]) -> Vec<DisplayMessage> {
    let mut out = Vec::new();
    // tool_use id -> index of its DisplayMessage, to attach the result later.
    let mut tool_slots = std::collections::HashMap::new();

    for msg in messages {
        let is_user = msg.role == "user";

        let blocks = match &msg.content {
            Content::Text(text) => {
                out.push(if is_user {
                    DisplayMessage::User(text.clone())
                } else {
                    DisplayMessage::AssistantText(text.clone())
                });
                continue;
            }
            Content::Blocks(blocks) => blocks,
        };

        for block in blocks {
            match block {
                ContentBlock::Text { text } => out.push(if is_user {
                    DisplayMessage::User(text.clone())
                } else {
                    DisplayMessage::AssistantText(text.clone())
                }),

                ContentBlock::ToolUse { id, name, input } => {
                    tool_slots.insert(id.clone(), out.len());
                    out.push(DisplayMessage::ToolUse {
                        name: name.clone(),
                        input: Some(input.clone()),
                        output: None,
                        is_error: false,
                    });
                }

                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some(&slot) = tool_slots.get(tool_use_id)
                        && let Some(DisplayMessage::ToolUse {
                            output, is_error: err, ..
                        }) = out.get_mut(slot)
                    {
                        *output = Some(content.clone());
                        *err = is_error.unwrap_or(false);
                    }
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct App {
    pub cwd: PathBuf,
    pub model: String,
    pub usage: Usage,
    pub context_window: u64,
    /// Conversation size after the last turn, in tokens.
    pub context_used: u64,
    pub messages: Vec<DisplayMessage>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub max_scroll: u16,
    pub input: String,
    pub cursor: usize,
    pub state: AppState,
    pub pending_perm: Option<PendingPermission>,
    /// Index into `AVAILABLE_MODELS` while the model picker is open.
    pub model_picker: Option<usize>,
    /// Messages typed while a turn is running, sent in order after it ends.
    pub queue: VecDeque<String>,
    /// Stop was pressed during the current turn. The queue is then handed
    /// back to the input instead of being sent.
    stopping: bool,
    /// A queued `/quit` ran after a turn ended; the main loop exits.
    quit_requested: bool,
    /// Models last reported by LM Studio.
    pub lm_models: Vec<LmModel>,
    pub lm_status: LmStatus,
    pub spinner_frame: usize,
    pub last_spinner_update: Instant,
    /// Resident memory of this process, in bytes. `None` if the OS refused.
    pub memory: Option<usize>,
    last_memory_update: Instant,
    #[cfg(feature = "voice")]
    pub pending_voice_recording: bool,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    session_tx: mpsc::UnboundedSender<SessionCmd>,
}

impl App {
    fn new(
        cwd: PathBuf,
        model: String,
        restored: Vec<DisplayMessage>,
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        ui_tx: mpsc::UnboundedSender<UiEvent>,
        session_tx: mpsc::UnboundedSender<SessionCmd>,
    ) -> Self {
        let context_window = context_window_for(&model);

        let mut messages = vec![DisplayMessage::Info(
            "Type your message to start.\nWhile it is working, Enter queues a follow-up.\nBackspace on an empty line pops the last one out. Ctrl+C to exit.".to_string(),
        )];

        if !restored.is_empty() {
            messages.push(DisplayMessage::Info("Resumed previous session.".to_string()));
            messages.extend(restored);
        }

        Self {
            cwd,
            model,
            usage: Usage::default(),
            context_window,
            context_used: 0,
            messages,
            scroll: 0,
            auto_scroll: true,
            max_scroll: 0,
            input: String::new(),
            cursor: 0,
            state: AppState::Idle,
            pending_perm: None,
            model_picker: None,
            queue: VecDeque::new(),
            stopping: false,
            quit_requested: false,
            lm_models: Vec::new(),
            lm_status: LmStatus::Unknown,
            spinner_frame: 0,
            last_spinner_update: Instant::now(),
            memory: current_memory(),
            last_memory_update: Instant::now(),
            #[cfg(feature = "voice")]
            pending_voice_recording: false,
            ui_rx,
            ui_tx,
            session_tx,
        }
    }

    /// Everything the picker offers: the built-in models, then LM Studio's.
    pub fn picker_entries(&self) -> Vec<PickerEntry> {
        let mut entries: Vec<PickerEntry> = AVAILABLE_MODELS
            .iter()
            .map(|m| PickerEntry {
                id: m.id.to_string(),
                label: m.label.to_string(),
                detail: m.id.to_string(),
                is_default: m.id == DEFAULT_MODEL,
                context_window: None,
            })
            .collect();

        entries.extend(self.lm_models.iter().map(|m| {
            let state = match m.loaded {
                Some(true) => "loaded",
                Some(false) => "not loaded",
                None => "",
            };

            PickerEntry {
                id: lm_studio_model_id(&m.id),
                label: format!("[lm-studio] {}", m.id),
                detail: state.to_string(),
                is_default: false,
                // Only a loaded model has a window worth trusting.
                context_window: m.context_length.filter(|_| m.loaded != Some(false)),
            }
        }));

        entries
    }

    /// Probe LM Studio in the background; the answer comes back as a `UiEvent`.
    fn probe_lm_studio(&mut self) {
        self.lm_status = LmStatus::Checking;

        let tx = self.ui_tx.clone();

        tokio::spawn(async move {
            let result = lmstudio::list_models().await.map_err(|e| e.to_string());
            let _ = tx.send(UiEvent::LmStudio(result));
        });
    }
    // -- Key handling -------------------------------------------------------

    /// Returns `true` if the app should quit.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        // Ctrl+C: stop Claude if busy, quit if idle
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.state == AppState::Busy {
                self.stop();
                return false;
            } else {
                return true;
            }
        }

        // Esc: stop Claude if busy, do nothing if idle
        if key.code == KeyCode::Esc {
            if self.state == AppState::Busy {
                self.stop();
                return false;
            }
        }

        // Permission prompt captures y/n
        if self.pending_perm.is_some() {
            return self.handle_perm_key(key.code);
        }

        // Model picker captures navigation keys
        if self.model_picker.is_some() {
            self.handle_picker_key(key.code);
            return false;
        }

        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    if self.state.is_busy() {
                        // Queue it: the session sends it when the current
                        // turn ends.
                        self.queue.push_back(self.input.trim_end().to_string());
                        self.input.clear();
                        self.cursor = 0;
                        self.auto_scroll = true;
                        return false;
                    }
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
                if self.input.is_empty() {
                    // Input is empty: pop the last queued message back for
                    // editing.
                    if let Some(text) = self.queue.pop_back() {
                        self.input = text;
                        self.cursor = self.input.chars().count();
                    }
                    return false;
                }
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
                if self.input.is_empty() {
                    if let Some(text) = self.queue.pop_front() {
                        self.input = text;
                        self.cursor = 0;
                    }
                } else if self.cursor < self.input.chars().count() {
                    let byte_pos = self
                        .input
                        .char_indices()
                        .nth(self.cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.input.len());
                    self.input.remove(byte_pos);
                }
            }

            KeyCode::Tab => {
                // Complete the first matching slash command
                if let Some(name) = first_suggestion(&self.input) {
                    self.input = format!("{name} ");
                    self.cursor = self.input.chars().count();
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

    fn handle_picker_key(&mut self, code: KeyCode) {
        let Some(selected) = self.model_picker else {
            return;
        };
        let entries = self.picker_entries();
        let count = entries.len();
        // The list can shrink under the cursor when a probe comes back empty.
        let selected = selected.min(count.saturating_sub(1));

        match code {
            KeyCode::Up => {
                self.model_picker = Some((selected + count - 1) % count);
            }
            KeyCode::Down | KeyCode::Tab => {
                self.model_picker = Some((selected + 1) % count);
            }
            KeyCode::Enter => {
                self.model_picker = None;
                let Some(entry) = entries.get(selected) else {
                    return;
                };

                self.set_model(entry.id.clone(), entry.label.clone());

                if let Some(window) = entry.context_window {
                    self.context_window = window;
                }

                self.resume_queue();
            }
            KeyCode::Esc => {
                self.model_picker = None;
                self.resume_queue();
            }
            _ => {}
        }
    }

    fn set_model(&mut self, id: String, label: String) {
        self.context_window = context_window_for(&id);
        let _ = self.session_tx.send(SessionCmd::SetModel(id.clone()));
        self.model = id;
        self.messages
            .push(DisplayMessage::Info(format!("Switched to {label}.")));
    }

    /// Process the input line: slash command or message. Returns `true` to quit.
    fn submit_input(&mut self) -> bool {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.submit_text(text)
    }

    /// Run one entry, typed or queued: a slash command, a `!` command, or a
    /// message for the model. Returns `true` to quit.
    fn submit_text(&mut self, text: String) -> bool {
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
                    self.set_model(id, label);
                }

                CommandResult::PickModel => {
                    // Open at once; LM Studio's models join the list when the
                    // probe answers, after the built-in ones, so indices hold.
                    self.probe_lm_studio();

                    let current = self
                        .picker_entries()
                        .iter()
                        .position(|e| e.id == self.model)
                        .unwrap_or(0);
                    self.model_picker = Some(current);
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

            UiEvent::Thinking(text) => {
                if let Some(DisplayMessage::Thinking(existing)) = self.messages.last_mut() {
                    existing.push_str(&text);
                } else {
                    self.messages.push(DisplayMessage::Thinking(text));
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

            UiEvent::Done {
                usage,
                context_used,
            } => {
                self.usage.add(&usage);
                self.context_used = context_used;

                // Stop can lose the race to a turn that was already ending:
                // the user still asked to stop, so do not send the queue.
                if std::mem::take(&mut self.stopping) {
                    self.hand_back_queue();
                } else {
                    self.drain_queue();
                }
            }

            UiEvent::Failed(msg) => {
                self.messages.push(DisplayMessage::Error(msg));
                // Stopped, or an API error: either way, do not fire the queue
                // into a broken or unwanted state.
                self.stopping = false;
                self.hand_back_queue();
            }

            UiEvent::LmStudio(result) => match result {
                Ok(models) => {
                    self.lm_models = models;
                    self.lm_status = LmStatus::Up;
                }
                Err(reason) => {
                    self.lm_models.clear();
                    self.lm_status = LmStatus::Down(reason);
                }
            },

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

    /// Picker closed while idle: carry on with whatever is still queued.
    fn resume_queue(&mut self) {
        if self.state == AppState::Idle && !self.queue.is_empty() {
            self.drain_queue();
        }
    }

    /// Ask the session to stop the current turn. The queue is not sent
    /// afterwards: it goes back to the input, see `hand_back_queue`.
    fn stop(&mut self) {
        self.stopping = true;
        let _ = self.session_tx.send(SessionCmd::Stop);
    }

    /// After a turn has ended: run queued entries in order until one starts a
    /// new turn, or the queue is empty. Slash and `!` commands run as if typed.
    fn drain_queue(&mut self) {
        self.state = AppState::Idle;

        while self.state == AppState::Idle
            && let Some(text) = self.queue.pop_front()
        {
            self.auto_scroll = true;

            if self.submit_text(text) {
                self.quit_requested = true;
                return;
            }

            // `/rec` leaves the TUI; let the main loop do that first.
            #[cfg(feature = "voice")]
            if self.pending_voice_recording {
                return;
            }

            // A queued `/model` opened the picker: wait for the choice, so
            // the rest of the queue runs on the model picked. Closing the
            // picker resumes the queue.
            if self.model_picker.is_some() {
                return;
            }
        }
    }

    /// After a stop or an error: put the queue back in the input for editing
    /// instead of sending it. Several entries are joined into one line; the
    /// input holds a single line.
    fn hand_back_queue(&mut self) {
        self.state = AppState::Idle;

        if self.queue.is_empty() {
            return;
        }

        let mut parts: Vec<String> = self.queue.drain(..).collect();

        // Keep what was being typed, after the queued text.
        if !self.input.trim().is_empty() {
            parts.push(std::mem::take(&mut self.input));
        }

        self.input = parts.join(" ");
        self.cursor = self.input.chars().count();
    }
}

/// Resident memory (RSS) of this process, in bytes.
fn current_memory() -> Option<usize> {
    memory_stats::memory_stats().map(|s| s.physical_mem)
}

/// First matching slash command for the current input, if any.
/// Mirrors the filter used to render the suggestion list.
fn first_suggestion(input: &str) -> Option<&'static str> {
    let trimmed = input.trim();

    if !trimmed.starts_with('/') || trimmed.contains(' ') {
        return None;
    }

    commands::matching_commands(trimmed)
        .first()
        .map(|cmd| cmd.name)
}

// ---------------------------------------------------------------------------
// Session background task
// ---------------------------------------------------------------------------

async fn session_loop(
    mut session: Session<ChannelPermissions>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    last_saved: Arc<Mutex<Option<String>>>,
) {
    let mut handler = ChannelEventHandler { tx: ui_tx.clone() };

    // Commands that arrived while a turn was running. The turn's future pins
    // `&mut session`, so they cannot be applied until it is dropped.
    let mut deferred = VecDeque::new();

    while let Some(cmd) = {
        match deferred.pop_front() {
            Some(cmd) => Some(cmd),
            None => cmd_rx.recv().await,
        }
    } {
        match cmd {
            SessionCmd::SendMessage(text) => {
                let cancel = CancellationToken::new();
                let token = cancel.clone();

                // Scoped so the future (and its `&mut session` borrow) is
                // dropped before the session is read back below. Commands
                // that arrive while the turn is running are held in
                // `deferred` so they can be applied right after.
                let result = {
                    let message_future = session.send_message(&text, &mut handler, &token);
                    let mut message_future = Box::pin(message_future);

                    loop {
                        tokio::select! {
                            res = &mut message_future => break res,
                            Some(cmd) = cmd_rx.recv() => {
                                match cmd {
                                    SessionCmd::Stop => cancel.cancel(),
                                    _ => deferred.push_back(cmd),
                                }
                            }
                        }
                    }
                };

                match result {
                    Ok(summary) => {
                        // Persist after every completed turn so --resume works
                        // even if the process is killed later.
                        if let Some(stored) = session.to_stored() {
                            match history::save(&stored) {
                                Ok(()) => {
                                    if let Ok(mut slot) = last_saved.lock() {
                                        *slot = Some(stored.id);
                                    }
                                }
                                Err(e) => {
                                    let _ = ui_tx.send(UiEvent::Error(format!(
                                        "Could not save session: {e}"
                                    )));
                                }
                            }
                        }

                        let _ = ui_tx.send(UiEvent::Done {
                            usage: summary.usage,
                            context_used: summary.context_used,
                        });
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

                // The fresh session has no file yet: nothing to hint at.
                if let Ok(mut slot) = last_saved.lock() {
                    *slot = None;
                }
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
    restored: Vec<DisplayMessage>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
) -> Result<()> {
    let model = session.model().to_string();
    let app_ui_tx = ui_tx.clone();

    // Channel for UI → session commands
    let (session_tx, session_rx) = mpsc::unbounded_channel();

    // Id of the last session written to disk, read back on exit for the
    // "resume with" hint. A resumed session is already on disk.
    let last_saved = Arc::new(Mutex::new(
        session.to_stored().map(|_| session.id().to_string()),
    ));

    // Spawn session loop in background
    tokio::spawn(session_loop(
        session,
        session_rx,
        ui_tx,
        Arc::clone(&last_saved),
    ));

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

    let mut app = App::new(cwd, model, restored, ui_rx, app_ui_tx, session_tx);

    // Start with a clean alternate screen
    terminal.clear()?;

    loop {
        // Handle voice recording if requested
        #[cfg(feature = "voice")]
        if app.pending_voice_recording {
            app.pending_voice_recording = false;

            // Exit TUI temporarily
            drop(terminal);
            crossterm::terminal::disable_raw_mode()?;

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

        // Refresh memory usage about once per second
        if app.last_memory_update.elapsed() >= Duration::from_secs(1) {
            app.memory = current_memory();
            app.last_memory_update = Instant::now();
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

        // A queued `/quit` ran when its turn came.
        if app.quit_requested {
            break;
        }
    }

    // Cleanup
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
    )?;

    // Printed after leaving the alternate screen, or it would vanish with it.
    let saved_id = last_saved.lock().ok().and_then(|slot| slot.clone());

    if let Some(id) = saved_id {
        println!("\nResume this conversation with:\n  ccrs --resume {id}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app wired to channels the test can read, starting mid-turn.
    fn busy_app() -> (App, mpsc::UnboundedReceiver<SessionCmd>) {
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (session_tx, session_rx) = mpsc::unbounded_channel();
        let mut app = App::new(
            PathBuf::from("/tmp"),
            DEFAULT_MODEL.to_string(),
            Vec::new(),
            ui_rx,
            ui_tx,
            session_tx,
        );
        app.state = AppState::Busy;
        (app, session_rx)
    }

    fn done() -> UiEvent {
        UiEvent::Done {
            usage: Usage::default(),
            context_used: 0,
        }
    }

    fn sent(rx: &mut mpsc::UnboundedReceiver<SessionCmd>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            out.push(match cmd {
                SessionCmd::SendMessage(t) => format!("send:{t}"),
                SessionCmd::SetModel(m) => format!("model:{m}"),
                SessionCmd::Clear => "clear".to_string(),
                SessionCmd::Stop => "stop".to_string(),
            });
        }
        out
    }

    #[test]
    fn queued_messages_run_one_turn_at_a_time() {
        let (mut app, mut rx) = busy_app();
        app.queue.extend(["first".to_string(), "second".to_string()]);

        app.handle_ui_event(done());
        assert_eq!(sent(&mut rx), ["send:first"]);
        assert!(app.state.is_busy());

        app.handle_ui_event(done());
        assert_eq!(sent(&mut rx), ["send:second"]);

        app.handle_ui_event(done());
        assert!(sent(&mut rx).is_empty());
        assert_eq!(app.state, AppState::Idle);
    }

    #[test]
    fn queued_commands_run_as_commands() {
        let (mut app, mut rx) = busy_app();
        app.queue.extend([
            "/clear".to_string(),
            "/model opus".to_string(),
            "hello".to_string(),
        ]);

        app.handle_ui_event(done());

        let cmds = sent(&mut rx);
        assert_eq!(cmds[0], "clear");
        assert!(cmds[1].starts_with("model:") && cmds[1].contains("opus"));
        assert_eq!(cmds[2], "send:hello");
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn queued_quit_exits() {
        let (mut app, _rx) = busy_app();
        app.queue.push_back("/quit".to_string());
        app.handle_ui_event(done());
        assert!(app.quit_requested);
    }

    #[test]
    fn stop_hands_queue_back_to_input() {
        let (mut app, mut rx) = busy_app();
        app.queue.extend(["one".to_string(), "two".to_string()]);
        app.input = "three".to_string();

        app.stop();
        app.handle_ui_event(UiEvent::Failed("Stopped.".to_string()));

        assert_eq!(sent(&mut rx), ["stop"]);
        assert_eq!(app.input, "one two three");
        assert_eq!(app.cursor, app.input.chars().count());
        assert!(app.queue.is_empty());
        assert_eq!(app.state, AppState::Idle);
    }

    #[test]
    fn stop_that_loses_the_race_still_holds_the_queue() {
        let (mut app, mut rx) = busy_app();
        app.queue.push_back("next".to_string());

        app.stop();
        app.handle_ui_event(done()); // turn finished before Stop landed

        assert_eq!(sent(&mut rx), ["stop"]);
        assert_eq!(app.input, "next");
        assert!(!app.stopping);
    }

    #[test]
    fn api_error_does_not_fire_the_queue() {
        let (mut app, mut rx) = busy_app();
        app.queue.push_back("next".to_string());

        app.handle_ui_event(UiEvent::Failed("overloaded".to_string()));

        assert!(sent(&mut rx).is_empty());
        assert_eq!(app.input, "next");
    }

    /// The screen as text, one string per row.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render::render(app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn queue_is_listed_above_the_input() {
        let (mut app, _rx) = busy_app();
        app.queue.extend(["first".to_string(), "second".to_string()]);
        app.input = "typing".to_string();

        let rows = screen(&mut app, 40, 20);
        let first = rows.iter().position(|r| r.contains("❯ first")).unwrap();
        let second = rows.iter().position(|r| r.contains("❯ second")).unwrap();
        let input = rows.iter().position(|r| r.contains("typing")).unwrap();

        assert_eq!(second, first + 1);
        assert!(input > second);
    }

    #[test]
    fn long_queue_collapses_oldest_entries() {
        let (mut app, _rx) = busy_app();
        app.queue.extend((1..=8).map(|n| format!("msg{n}")));

        let rows = screen(&mut app, 40, 20).join("\n");
        assert!(rows.contains("… 3 earlier queued"));
        assert!(!rows.contains("msg3"));
        assert!(rows.contains("msg4") && rows.contains("msg8"));
    }

    #[test]
    fn long_queued_line_is_truncated() {
        let (mut app, _rx) = busy_app();
        app.queue.push_back("x".repeat(100));

        let rows = screen(&mut app, 40, 20);
        let row = rows.iter().find(|r| r.contains("❯")).unwrap();
        assert!(row.contains('…'));
    }

    #[test]
    fn no_queue_no_panel() {
        let (mut app, _rx) = busy_app();
        assert!(!screen(&mut app, 40, 20).join("\n").contains("❯"));
    }

    // `/model` probes LM Studio with `tokio::spawn`, so this needs a runtime.
    #[tokio::test]
    async fn queued_model_picker_pauses_the_queue() {
        let (mut app, mut rx) = busy_app();
        app.queue.extend(["/model".to_string(), "after".to_string()]);

        app.handle_ui_event(done());
        assert!(app.model_picker.is_some());
        assert!(sent(&mut rx).is_empty());

        app.handle_picker_key(KeyCode::Esc);
        assert_eq!(sent(&mut rx), ["send:after"]);
    }
}
