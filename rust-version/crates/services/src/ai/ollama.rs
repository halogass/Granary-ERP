//! Port of backend/app/services/ai/ollama.py: Ollama (self-hosted) adapter. Ollama speaks the OpenAI wire
//! protocol natively under `/v1` (chat/completions, completions, embeddings, models, including
//! OpenAI-shaped error bodies), so this reuses `OpenAIProvider`'s implementation wholesale.
//!
//! Point `base_url` either directly at Ollama's own compat layer (e.g. http://ollama:11434) or, recommended
//! when Model Maintainer manages the GPU, at its authenticated proxy (http://<model-maintainer>/api/ollama/v1)
//! with one of its Ollama-scoped tokens as the API key. Either way `/v1` is appended automatically if the
//! given base_url doesn't already end in it.

use crate::ai::openai::OpenAIProvider;

/// `OllamaProvider(api_key=None, base_url=None)`: an OpenAIProvider named "ollama".
pub struct OllamaProvider;

impl OllamaProvider {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> OpenAIProvider {
        let base = base_url
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let base = base.trim_end_matches('/').to_string();
        let base = if base.ends_with("/v1") {
            base
        } else {
            format!("{base}/v1")
        };
        // Real Ollama ignores the key; a proxy in front of it may require one. The OpenAI SDK needs some
        // non-empty string regardless, so default to a placeholder rather than "".
        let key = api_key
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "ollama".to_string());
        OpenAIProvider::named("ollama", Some(key), Some(base))
    }
}
