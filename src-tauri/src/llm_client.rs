use crate::managers::llm_engine::PostProcessProvider;
use log::debug;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct JsonSchema {
    name: String,
    strict: bool,
    schema: Value,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    json_schema: JsonSchema,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

/// Build headers for API requests based on provider type
fn build_headers(provider: &PostProcessProvider, api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();

    // Common headers
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://github.com/pohsuchenwork/vaporly"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Vaporly/1.0 (+https://github.com/pohsuchenwork/vaporly)"),
    );
    headers.insert("X-Title", HeaderValue::from_static("Vaporly"));

    // The bundled engine requires its per-session token (the user never
    // configures a key for it). A user-supplied key would win if ever set.
    let engine_token;
    let api_key = if provider.id == crate::managers::llm_engine::VAPORLY_ENGINE_PROVIDER_ID
        && api_key.is_empty()
    {
        engine_token = crate::managers::llm_engine::engine_token();
        engine_token.as_str()
    } else {
        api_key
    };

    // Bearer auth (the bundled engine's per-session token).
    if !api_key.is_empty() {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| format!("Invalid authorization header value: {}", e))?,
        );
    }

    Ok(headers)
}

/// Create an HTTP client with provider-specific headers
fn create_client(provider: &PostProcessProvider, api_key: &str) -> Result<reqwest::Client, String> {
    let headers = build_headers(provider, api_key)?;
    reqwest::Client::builder()
        .default_headers(headers)
        // A dead local server must fail the paste fallback fast, not hang it.
        .connect_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// Per-request timeout: local engines scale with input length (a long dictation
/// through a 7B on a weak CPU legitimately takes tens of seconds, but never
/// unbounded); cloud providers get a flat cap. Fixes the historical
/// no-timeout-anywhere bug where a slow-but-accepting server blocked the paste
/// indefinitely.
pub(crate) fn request_timeout(provider_id: &str, input_chars: usize) -> Duration {
    match provider_id {
        "custom" | "vaporly_engine" => {
            Duration::from_secs((20 + input_chars as u64 / 30).clamp(20, 90))
        }
        _ => Duration::from_secs(60),
    }
}

/// Greedy, seeded, hard-capped decoding for the LOCAL cleanup engines. The
/// bundled llama-server otherwise runs llama.cpp defaults (temperature 0.8,
/// unbounded generation), which produce inconsistent rewrites and let a
/// misbehaving model generate to the context limit and burn the whole request
/// timeout. temperature 0 selects the argmax; top_k 1 forces greedy even on
/// builds that treat temperature 0 as "very low"; the max_tokens cap sizes to
/// the input so cleanup can never run away. Cloud providers keep their own
/// defaults (their reasoning models reject temperature/max_tokens overrides),
/// so they get all-None and their serialized body is unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CleanupSampling {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub max_tokens: Option<u32>,
    pub seed: Option<i64>,
}

pub(crate) fn cleanup_sampling(provider_id: &str, input_bytes: usize) -> CleanupSampling {
    match provider_id {
        "custom" | "vaporly_engine" => CleanupSampling {
            temperature: Some(0.0),
            top_p: Some(1.0),
            top_k: Some(1),
            // input_bytes/3 approximates 1.3x the input token count (~chars/4),
            // so a legitimate cleanup (output <= input) never truncates, while
            // the runaway ceiling drops from -c 8192 to a few hundred. The 2048
            // ceiling stays well under the context window.
            max_tokens: Some(((input_bytes / 3) as u32 + 64).clamp(96, 2048)),
            seed: Some(42),
        },
        _ => CleanupSampling {
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            seed: None,
        },
    }
}

/// Send a chat completion request to an OpenAI-compatible API
/// Returns Ok(Some(content)) on success, Ok(None) if response has no content,
/// or Err on actual errors (HTTP, parsing, etc.)
pub async fn send_chat_completion(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    prompt: String,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    send_chat_completion_with_schema(
        provider,
        api_key,
        model,
        prompt,
        None,
        None,
        reasoning_effort,
        reasoning,
    )
    .await
}

/// Send a chat completion request with structured output support
/// When json_schema is provided, uses structured outputs mode
/// system_prompt is used as the system message when provided
/// reasoning_effort sets the OpenAI-style top-level field (e.g., "none", "low", "medium", "high")
/// reasoning sets the OpenRouter-style nested object (effort + exclude)
#[allow(clippy::too_many_arguments)]
pub async fn send_chat_completion_with_schema(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    user_content: String,
    system_prompt: Option<String>,
    json_schema: Option<Value>,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    // Bundled-engine providers carry a placeholder port; resolve to the live one.
    let provider = crate::managers::llm_engine::resolve_provider(provider);
    let provider = &provider;
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base_url);

    debug!("Sending chat completion request to: {}", url);

    let timeout = request_timeout(&provider.id, user_content.len());
    // Greedy + capped for the local engines; all-None (unchanged body) for cloud.
    let sampling = cleanup_sampling(&provider.id, user_content.len());
    let client = create_client(provider, &api_key)?;

    // Build messages vector
    let mut messages = Vec::new();

    // Add system prompt if provided
    if let Some(system) = system_prompt {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system,
        });
    }

    // Add user message
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_content,
    });

    // Build response_format if schema is provided
    let response_format = json_schema.map(|schema| ResponseFormat {
        format_type: "json_schema".to_string(),
        json_schema: JsonSchema {
            name: "transcription_output".to_string(),
            strict: true,
            schema,
        },
    });

    let request_body = ChatCompletionRequest {
        model: model.to_string(),
        messages,
        response_format,
        reasoning_effort,
        reasoning,
        temperature: sampling.temperature,
        top_p: sampling.top_p,
        top_k: sampling.top_k,
        max_tokens: sampling.max_tokens,
        seed: sampling.seed,
    };

    let response = client
        .post(&url)
        .timeout(timeout)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Failed to read error response".to_string());
        return Err(format!(
            "API request failed with status {}: {}",
            status, error_text
        ));
    }

    let completion: ChatCompletionResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse API response: {}", e))?;

    Ok(completion
        .choices
        .first()
        .and_then(|choice| choice.message.content.clone()))
}

#[cfg(test)]
mod sampling_tests {
    use super::*;

    #[test]
    fn local_engines_get_greedy_capped_sampling() {
        for id in ["vaporly_engine", "custom"] {
            let s = cleanup_sampling(id, 300);
            assert_eq!(s.temperature, Some(0.0));
            assert_eq!(s.top_p, Some(1.0));
            assert_eq!(s.top_k, Some(1));
            assert_eq!(s.seed, Some(42));
            assert!(s.max_tokens.is_some());
        }
    }

    #[test]
    fn cloud_providers_get_no_sampling_overrides() {
        for id in [
            "openai",
            "groq",
            "anthropic",
            "cerebras",
            "openrouter",
            "zai",
        ] {
            let s = cleanup_sampling(id, 300);
            assert_eq!(s.temperature, None);
            assert_eq!(s.top_p, None);
            assert_eq!(s.top_k, None);
            assert_eq!(s.max_tokens, None);
            assert_eq!(s.seed, None);
        }
    }

    #[test]
    fn max_tokens_scales_with_input_and_is_bounded() {
        // Floor covers tiny dictations (0/3 + 64 = 64 -> clamped up to 96).
        assert_eq!(cleanup_sampling("vaporly_engine", 0).max_tokens, Some(96));
        assert_eq!(cleanup_sampling("vaporly_engine", 10).max_tokens, Some(96));
        // Scales through the middle: 300/3 + 64 = 164.
        assert_eq!(
            cleanup_sampling("vaporly_engine", 300).max_tokens,
            Some(164)
        );
        // Ceiling stays well under the 8192 context window.
        assert_eq!(
            cleanup_sampling("vaporly_engine", 1_000_000).max_tokens,
            Some(2048)
        );
    }

    fn body_with(sampling: CleanupSampling) -> serde_json::Value {
        let body = ChatCompletionRequest {
            model: "m".to_string(),
            messages: vec![],
            response_format: None,
            reasoning_effort: None,
            reasoning: None,
            temperature: sampling.temperature,
            top_p: sampling.top_p,
            top_k: sampling.top_k,
            max_tokens: sampling.max_tokens,
            seed: sampling.seed,
        };
        serde_json::to_value(&body).unwrap()
    }

    #[test]
    fn cloud_request_body_omits_sampling_keys() {
        // Cloud bodies must be byte-identical to before the change.
        let v = body_with(cleanup_sampling("openai", 300));
        for key in ["temperature", "top_p", "top_k", "max_tokens", "seed"] {
            assert!(v.get(key).is_none(), "cloud body should omit {key}");
        }
    }

    #[test]
    fn local_request_body_includes_sampling_keys() {
        let v = body_with(cleanup_sampling("vaporly_engine", 300));
        assert_eq!(v.get("temperature").and_then(|x| x.as_f64()), Some(0.0));
        assert_eq!(v.get("top_k").and_then(|x| x.as_u64()), Some(1));
        assert_eq!(v.get("seed").and_then(|x| x.as_i64()), Some(42));
        assert_eq!(v.get("max_tokens").and_then(|x| x.as_u64()), Some(164));
    }
}
