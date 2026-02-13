use super::CommandResult;

pub fn run() -> CommandResult {
    #[allow(unused_mut)]
    let mut text = String::from(
        "\
Available commands:
  /help /h   — Show this help message
  /quit /q   — Exit the application
  /clear     — Clear conversation history
  /model     — List or switch models",
    );

    #[cfg(feature = "voice")]
    text.push_str("\n  /rec       — Record and transcribe voice input");

    CommandResult::Info(text)
}
