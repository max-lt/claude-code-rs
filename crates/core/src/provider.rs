//! Backend selection.
//!
//! The internal message model is Anthropic-shaped. Providers that speak a
//! different wire format translate at the edge, in their own module.

/// An upstream inference backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    /// Cerebras, via its OpenAI-compatible chat completions endpoint.
    Cerebras,
}

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub provider: Provider,
}

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5";

pub const AVAILABLE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-sonnet-4-5",
        label: "Sonnet 4.5",
        provider: Provider::Anthropic,
    },
    ModelInfo {
        id: "claude-opus-4-6",
        label: "Opus 4.6",
        provider: Provider::Anthropic,
    },
    ModelInfo {
        id: "claude-haiku-4-5",
        label: "Haiku 4.5",
        provider: Provider::Anthropic,
    },
    ModelInfo {
        id: "qwen-3.8-27b",
        label: "Qwen 3.8 27B (Cerebras)",
        provider: Provider::Cerebras,
    },
    ModelInfo {
        id: "gpt-oss-120b",
        label: "GPT-OSS 120B (Cerebras)",
        provider: Provider::Cerebras,
    },
];

impl Provider {
    /// Resolve the provider for a model id.
    ///
    /// Known ids come from [`AVAILABLE_MODELS`]. An unknown id is routed by
    /// prefix, so a new Cerebras model works without a code change.
    pub fn for_model(model: &str) -> Self {
        if let Some(info) = AVAILABLE_MODELS.iter().find(|m| m.id == model) {
            return info.provider;
        }

        if model.starts_with("qwen") || model.starts_with("gpt-oss") {
            return Self::Cerebras;
        }

        Self::Anthropic
    }

    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1/messages",
            Self::Cerebras => "https://api.cerebras.ai/v1/chat/completions",
        }
    }

    /// Upper bound on tokens the model may generate in one turn.
    pub fn max_output_tokens(self) -> u32 {
        match self {
            Self::Anthropic => 16384,
            // The free tier gives a 64k context. A large output cap would eat
            // it, so keep the reply short and leave room for the history.
            Self::Cerebras => 8192,
        }
    }

    /// Upper bound on the serialized request body, in bytes. This is a byte
    /// proxy for a token budget, at roughly 4 bytes per token.
    pub fn max_request_bytes(self) -> usize {
        match self {
            Self::Anthropic => 4 * 1024 * 1024,
            // Stay under the 64k free-tier context, with room for the reply.
            Self::Cerebras => 200 * 1024,
        }
    }

    /// Upper bound on a single tool result, in bytes.
    pub fn max_tool_result_bytes(self) -> usize {
        match self {
            Self::Anthropic => 500_000,
            Self::Cerebras => 24 * 1024,
        }
    }

    /// The first line of the system prompt. Naming the wrong vendor degrades
    /// an open-weights model, so each backend states what it actually is.
    pub fn identity(self) -> &'static str {
        match self {
            Self::Anthropic => "You are Claude Code, Anthropic's official CLI for Claude.",
            Self::Cerebras => {
                "You are ccrs, a coding assistant running in a terminal. \
                 Call the provided tools to inspect and change the codebase."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_known_models() {
        assert_eq!(
            Provider::for_model("claude-sonnet-4-5"),
            Provider::Anthropic
        );
        assert_eq!(Provider::for_model("qwen-3.8-27b"), Provider::Cerebras);
        assert_eq!(Provider::for_model("gpt-oss-120b"), Provider::Cerebras);
    }

    #[test]
    fn routes_unknown_models_by_prefix() {
        assert_eq!(Provider::for_model("qwen-4-next"), Provider::Cerebras);
        assert_eq!(Provider::for_model("claude-opus-9"), Provider::Anthropic);
        assert_eq!(Provider::for_model("something-else"), Provider::Anthropic);
    }

    #[test]
    fn every_listed_model_routes_to_its_own_provider() {
        for m in AVAILABLE_MODELS {
            assert_eq!(Provider::for_model(m.id), m.provider, "{}", m.id);
        }
    }

    #[test]
    fn default_model_is_listed() {
        assert!(AVAILABLE_MODELS.iter().any(|m| m.id == DEFAULT_MODEL));
    }
}
