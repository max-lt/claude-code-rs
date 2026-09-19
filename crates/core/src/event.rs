/// Receives streaming events from an API interaction.
///
/// New methods can be added with default impls without breaking existing code.
pub trait EventHandler: Send {
    fn on_text(&mut self, text: &str);
    fn on_error(&mut self, message: &str);

    /// Reasoning tokens, when the model streams them separately from its
    /// answer. Anthropic models do not reach this; Qwen on Cerebras does.
    fn on_thinking(&mut self, _text: &str) {}

    fn on_tool_use_start(&mut self, _name: &str, _id: &str, _input: &serde_json::Value) {}
    fn on_tool_use_end(&mut self, _name: &str) {}
    fn on_tool_executing(&mut self, _name: &str, _input: &serde_json::Value) {}
    fn on_tool_result(&mut self, _name: &str, _output: &str, _is_error: bool) {}
}
