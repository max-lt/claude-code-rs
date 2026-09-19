use std::process::Command;
use super::CommandResult;

/// Execute a bash command directly from the TUI.
pub fn run(command: &str) -> CommandResult {
    if command.trim().is_empty() {
        return CommandResult::Info("No command provided".to_string());
    }

    // Execute the command
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output();

    match output {
        Ok(output) => {
            let mut result = String::new();
            
            // Add stdout if present
            if !output.stdout.is_empty() {
                if let Ok(stdout) = String::from_utf8(output.stdout) {
                    result.push_str(&stdout);
                }
            }
            
            // Add stderr if present
            if !output.stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                if let Ok(stderr) = String::from_utf8(output.stderr) {
                    result.push_str(&stderr);
                }
            }
            
            // If both empty, indicate success
            if result.is_empty() {
                result = format!("Command executed successfully (exit code: {})", output.status.code().unwrap_or(0));
            }
            
            CommandResult::Info(result)
        }
        Err(e) => CommandResult::Info(format!("Failed to execute command: {}", e)),
    }
}
