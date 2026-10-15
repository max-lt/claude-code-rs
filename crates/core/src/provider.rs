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
    /// A local LM Studio server, also OpenAI-compatible. Its model ids are
    /// free-form, so they are marked with [`LM_STUDIO_PREFIX`] internally.
    LmStudio,
}

/// Internal marker for a model served by LM Studio. LM Studio ids are
/// arbitrary (`qwen3.8-27b-obliterated`, `google/gemma-4-12b-qat`), so prefix
/// matching on the bare id would misroute them. The marker is stripped before
/// the id goes on the wire, see [`wire_model`].
pub const LM_STUDIO_PREFIX: &str = "lmstudio:";

/// Default address of the LM Studio local server.
pub const LM_STUDIO_DEFAULT_URL: &str = "http://localhost:1234";

/// Base URL of the LM Studio server. `LMSTUDIO_BASE_URL` overrides the default.
pub fn lm_studio_base_url() -> String {
    std::env::var("LMSTUDIO_BASE_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| LM_STUDIO_DEFAULT_URL.to_string())
}

/// The internal id for a model name reported by LM Studio.
pub fn lm_studio_model_id(name: &str) -> String {
    format!("{LM_STUDIO_PREFIX}{name}")
}

/// The model name to send to the backend: the internal marker is dropped.
pub fn wire_model(model: &str) -> &str {
    model.strip_prefix(LM_STUDIO_PREFIX).unwrap_or(model)
}

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub provider: Provider,
    /// Total context window, in tokens. Cerebras figures are the free tier;
    /// the paid tier doubles them.
    pub context_window: u64,
}

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

pub const AVAILABLE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-sonnet-5-5",
        label: "Sonnet 5.5",
        provider: Provider::Anthropic,
        context_window: 200_000,
    },
    ModelInfo {
        id: "claude-opus-5-5",
        label: "Opus 5.5",
        provider: Provider::Anthropic,
        context_window: 200_000,
    },
    ModelInfo {
        id: "claude-haiku-5-5",
        label: "Haiku 5.5",
        provider: Provider::Anthropic,
        context_window: 200_000,
    },
    ModelInfo {
        id: "qwen-3.8-27b",
        label: "Qwen 3.8 27B (Cerebras)",
        provider: Provider::Cerebras,
        context_window: 64_000,
    },
    ModelInfo {
        id: "gpt-oss-120b",
        label: "GPT-OSS 120B (Cerebras)",
        provider: Provider::Cerebras,
        context_window: 65_536,
    },
];

/// Context window for a model id. An id not in [`AVAILABLE_MODELS`] falls back
/// to its provider's default, matching how [`Provider::for_model`] routes it.
pub fn context_window_for(model: &str) -> u64 {
    AVAILABLE_MODELS
        .iter()
        .find(|m| m.id == model)
        .map(|m| m.context_window)
        .unwrap_or_else(|| Provider::for_model(model).default_context_window())
}

impl Provider {
    /// Resolve the provider for a model id.
    ///
    /// Known ids come from [`AVAILABLE_MODELS`]. An unknown id is routed by
    /// prefix, so a new Cerebras model works without a code change.
    pub fn for_model(model: &str) -> Self {
        if let Some(info) = AVAILABLE_MODELS.iter().find(|m| m.id == model) {
            return info.provider;
        }

        if model.starts_with(LM_STUDIO_PREFIX) {
            return Self::LmStudio;
        }

        if model.starts_with("qwen") || model.starts_with("gpt-oss") {
            return Self::Cerebras;
        }

        Self::Anthropic
    }

    pub fn endpoint(self) -> String {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
            Self::Cerebras => "https://api.cerebras.ai/v1/chat/completions".to_string(),
            Self::LmStudio => format!("{}/v1/chat/completions", lm_studio_base_url()),
        }
    }

    /// Whether the backend speaks the OpenAI chat completions wire format.
    pub fn is_openai_compatible(self) -> bool {
        matches!(self, Self::Cerebras | Self::LmStudio)
    }

    /// Context window assumed for a model this build does not list.
    pub fn default_context_window(self) -> u64 {
        match self {
            Self::Anthropic => 200_000,
            Self::Cerebras => 64_000,
            // The real window depends on how the model was loaded; LM Studio
            // defaults to a few thousand tokens. This is a middle estimate.
            Self::LmStudio => 32_768,
        }
    }

    /// Upper bound on tokens the model may generate in one turn.
    pub fn max_output_tokens(self) -> u32 {
        match self {
            Self::Anthropic => 16384,
            // The free tier gives a 64k context. A large output cap would eat
            // it, so keep the reply short and leave room for the history.
            Self::Cerebras => 8192,
            Self::LmStudio => 4096,
        }
    }

    /// Upper bound on the serialized request body, in bytes. This is a byte
    /// proxy for a token budget, at roughly 4 bytes per token.
    pub fn max_request_bytes(self) -> usize {
        match self {
            Self::Anthropic => 4 * 1024 * 1024,
            // Stay under the 64k free-tier context, with room for the reply.
            Self::Cerebras => 200 * 1024,
            Self::LmStudio => 96 * 1024,
        }
    }

    /// Upper bound on a single tool result, in bytes.
    pub fn max_tool_result_bytes(self) -> usize {
        match self {
            Self::Anthropic => 500_000,
            Self::Cerebras => 24 * 1024,
            Self::LmStudio => 16 * 1024,
        }
    }

    /// The first line of the system prompt. Naming the wrong vendor degrades
    /// an open-weights model, so each backend states what it actually is.
    pub fn identity(self) -> &'static str {
        match self {
            Self::Anthropic => "You are Claude Code, Anthropic's official CLI for Claude.",
            Self::Cerebras | Self::LmStudio => {
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
            Provider::for_model("claude-sonnet-5-5"),
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
    fn lm_studio_ids_route_by_marker_not_by_name() {
        // A bare `qwen…` id would go to Cerebras; the marker wins.
        let id = lm_studio_model_id("qwen3.8-27b-obliterated");
        assert_eq!(Provider::for_model(&id), Provider::LmStudio);
        assert_eq!(Provider::for_model("qwen3.8-27b-obliterated"), Provider::Cerebras);
        assert_eq!(wire_model(&id), "qwen3.8-27b-obliterated");
        assert_eq!(wire_model("claude-sonnet-5-5"), "claude-sonnet-5-5");
    }

    #[test]
    fn lm_studio_window_is_the_provider_default() {
        assert_eq!(context_window_for("lmstudio:google/gemma-4-12b-qat"), 32_768);
    }

    #[test]
    fn every_listed_model_routes_to_its_own_provider() {
        for m in AVAILABLE_MODELS {
            assert_eq!(Provider::for_model(m.id), m.provider, "{}", m.id);
        }
    }

    #[test]
    fn context_window_comes_from_the_table() {
        assert_eq!(context_window_for("claude-sonnet-5-5"), 200_000);
        assert_eq!(context_window_for("qwen-3.8-27b"), 64_000);
    }

    #[test]
    fn unknown_model_falls_back_to_its_own_provider() {
        // A Cerebras id must not inherit the Anthropic window.
        assert_eq!(context_window_for("qwen-4-next"), 64_000);
        assert_eq!(context_window_for("claude-opus-9"), 200_000);
    }

    #[test]
    fn cerebras_windows_all_assume_the_free_tier() {
        for m in AVAILABLE_MODELS {
            if m.provider == Provider::Cerebras {
                assert!(
                    m.context_window <= 65_536,
                    "{} claims a paid-tier window",
                    m.id
                );
            }
        }
    }

    #[test]
    fn no_context_window_is_zero() {
        // render.rs divides by this value.
        for m in AVAILABLE_MODELS {
            assert!(m.context_window > 0, "{}", m.id);
        }
    }

    #[test]
    fn default_model_is_listed() {
        assert!(AVAILABLE_MODELS.iter().any(|m| m.id == DEFAULT_MODEL));
    }
}
