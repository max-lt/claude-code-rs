use super::CommandResult;

pub fn run() -> CommandResult {
    #[allow(unused_mut)]
    let mut text = String::from(
        "\
Available commands:
  /help /h   — Show this help message
  /quit /q   — Exit the application
  /clear     — Clear conversation history
  /model     — Pick a model with ↑/↓, LM Studio included if up
  !<command> — Execute a bash command (e.g., !ls, !git status)

While a turn is running:
  Enter      — Queue the typed line; it runs when the turn ends (commands too)
  Backspace  — (on an empty line) Remove the last queued line for editing
  Ctrl+C/Esc — Stop the current turn; queued lines come back to the input",
    );

    #[cfg(feature = "voice")]
    text.push_str("\n  /rec       — Record and transcribe voice input");

    CommandResult::Info(text)
}
