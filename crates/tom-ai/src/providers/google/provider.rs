//! Google `generateContent` streaming — one implementation for
//! `google-generative-ai` and `google-vertex`.
//!
//! Both APIs take the same request body and return the same
//! `streamGenerateContent` SSE frames. What differs is authentication and the
//! endpoint:
//!
//! | | Generative Language | Vertex |
//! | --- | --- | --- |
//! | Auth | `x-goog-api-key` | `Authorization: Bearer` (API key or ADC token) |
//! | Path | `{base}/models/{model}:streamGenerateContent?alt=sse` | `{base}/publishers/google/models/{model}:streamGenerateContent?alt=sse` |
//! | Default host | `generativelanguage.googleapis.com` | `{location}-aiplatform.googleapis.com` |
//!
//! Vertex's Application Default Credentials flow (§ metadata server, service
//! account JWT → access token) is *not* implemented here: an access token is
//! read from `GOOGLE_VERTEX_ACCESS_TOKEN` or supplied as the API key, which
//! covers gcloud/Workload-Identity setups that export a token. Anything more
//! would need a JWT signer and a metadata client, and belongs in a package
//! rather than the core adapter.
//!
//! Mirrors `packages/ai/src/api/google-generative-ai.ts` and
//! `google-vertex.ts`, with the shared wire code in [`super::shared`].

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::event_stream::{
    create_assistant_message_event_stream_with_cancellation, AssistantMessageEventStream,
    AssistantMessageEventStreamProducer,
};
use crate::model::Model;
use crate::provider::{Provider, SimpleStreamOptions};
use crate::providers::anthropic::cost::calculate_cost;
use crate::providers::anthropic::retry::retry_provider_request;
use crate::providers::anthropic::sse::SseEventStream;
use crate::providers::google::shared;
use crate::providers::{DELTA_FLUSH_BYTES, DELTA_FLUSH_INTERVAL};
use crate::types::{
    Api, AssistantMessage, AssistantMessageEvent, Content, Context, StopReason, TextContent,
    TextContentType, ThinkingContent, ThinkingContentType, ThinkingLevel, UserContent,
};
use crate::AiError;

/// Which Google API this provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleApi {
    /// `generativelanguage.googleapis.com` — an API key in `x-goog-api-key`.
    GenerativeAi,
    /// `*-aiplatform.googleapis.com` — a bearer token and a project/location.
    Vertex,
}

impl GoogleApi {
    fn api(&self) -> Api {
        match self {
            GoogleApi::GenerativeAi => Api::GoogleGenerativeAi,
            GoogleApi::Vertex => Api::GoogleVertex,
        }
    }

    fn default_base_url(&self) -> &'static str {
        match self {
            GoogleApi::GenerativeAi => "https://generativelanguage.googleapis.com/v1beta",
            // Vertex has no single host: it is per project location. The
            // `global` location is the one shared endpoint.
            GoogleApi::Vertex => "https://aiplatform.googleapis.com/v1",
        }
    }
}

/// Provider options shared by both Google APIs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoogleOptions {
    /// `auto` / `none` / `any`, mirroring the other adapters.
    pub tool_choice: Option<String>,
    /// Vertex project; falls back to `GOOGLE_CLOUD_PROJECT` / `GCLOUD_PROJECT`.
    pub project: Option<String>,
    /// Vertex location; falls back to `GOOGLE_CLOUD_LOCATION`.
    pub location: Option<String>,
}

pub struct GoogleProvider {
    id: String,
    api: GoogleApi,
    api_key: Option<String>,
    allow_env_api_key: bool,
    http: reqwest::Client,
    models: Vec<Model>,
    options: GoogleOptions,
}

impl GoogleProvider {
    pub fn new(
        id: impl Into<String>,
        api: GoogleApi,
        api_key: Option<String>,
        http: reqwest::Client,
        models: Vec<Model>,
    ) -> Self {
        Self {
            id: id.into(),
            api,
            api_key,
            // Only the first-party Google provider entries may read the shared
            // process environment; a `models.json` entry must bring its own key.
            allow_env_api_key: false,
            http,
            models,
            options: GoogleOptions::default(),
        }
    }

    /// Allow the process-wide Google credential env vars (first-party only).
    pub fn with_env_api_key(mut self) -> Self {
        self.allow_env_api_key = true;
        self
    }

    pub fn with_options(mut self, options: GoogleOptions) -> Self {
        self.options = options;
        self
    }

    fn resolve_key(&self, opts: &SimpleStreamOptions) -> Option<String> {
        opts.api_key
            .clone()
            .or_else(|| self.api_key.clone())
            .or_else(|| {
                self.allow_env_api_key
                    .then(|| {
                        [
                            "GOOGLE_VERTEX_ACCESS_TOKEN",
                            "GOOGLE_API_KEY",
                            "GEMINI_API_KEY",
                        ]
                        .iter()
                        .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
                    })
                    .flatten()
            })
    }
}

/// The request URL for one model.
pub fn stream_url(api: GoogleApi, model: &Model, options: &GoogleOptions) -> String {
    let base = {
        let configured = model.base_url.trim().trim_end_matches('/');
        if configured.is_empty() {
            api.default_base_url().to_string()
        } else {
            configured.to_string()
        }
    };
    match api {
        GoogleApi::GenerativeAi => {
            format!("{base}/models/{}:streamGenerateContent?alt=sse", model.id)
        }
        GoogleApi::Vertex => {
            let location = options
                .location
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .or_else(|| env("GOOGLE_CLOUD_LOCATION"));
            // A `global` location and a custom base URL both use the shared
            // host, so only a regional location rewrites the hostname.
            let base = match location.as_deref() {
                Some(location) if !location.eq_ignore_ascii_case("global") => {
                    if base.contains("-aiplatform.googleapis.com") || base.contains("aiplatform") {
                        replace_vertex_location(&base, location)
                    } else {
                        base
                    }
                }
                _ => base,
            };
            format!(
                "{base}/publishers/google/models/{}:streamGenerateContent?alt=sse",
                model.id
            )
        }
    }
}

/// Swap the location segment of a Vertex host:
/// `us-central1-aiplatform.googleapis.com` → `europe-west4-aiplatform...`.
fn replace_vertex_location(base: &str, location: &str) -> String {
    let (scheme, rest) = match base.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), base),
    };
    let Some((_, tail)) = rest.split_once("aiplatform.googleapis.com") else {
        return base.to_string();
    };
    // Drop any existing location prefix (`us-central1-`); the shared host has
    // none, and a regional one must be replaced rather than duplicated.
    format!("{scheme}{location}-aiplatform.googleapis.com{tail}")
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The request body (everything except auth).
pub fn build_body(
    api: GoogleApi,
    model: &Model,
    ctx: &Context,
    opts: &SimpleStreamOptions,
    options: &GoogleOptions,
) -> Result<Value, String> {
    let contents = shared::convert_messages(model, ctx);
    let mut generation_config = serde_json::Map::new();
    if let Some(temperature) = opts.temperature {
        generation_config.insert("temperature".into(), json!(temperature));
    }
    let max_tokens = opts.max_tokens.unwrap_or(model.max_tokens);
    if max_tokens > 0 {
        generation_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }

    let supports_strict = shared::supports_strict_tool_sampling(&model.id);
    let mode =
        shared::function_calling_mode(&ctx.tools, options.tool_choice.as_deref(), supports_strict);

    let mut config = serde_json::Map::new();
    if !generation_config.is_empty() {
        config.insert("generationConfig".into(), Value::Object(generation_config));
    }
    if let Some(system_prompt) = &ctx.system_prompt {
        if !system_prompt.trim().is_empty() {
            config.insert(
                "systemInstruction".into(),
                json!({
                    "parts": [{ "text": system_prompt }],
                }),
            );
        }
    }
    if let Some(tools) = shared::convert_tools(&ctx.tools, false) {
        config.insert("tools".into(), tools);
    }
    if let Some(mode) = mode {
        config.insert(
            "toolConfig".into(),
            json!({ "functionCallingConfig": { "mode": mode } }),
        );
    }

    // Thinking. `Off` disables it; every other level either sets a discrete
    // level (Gemini 3+) or a token budget (2.x).
    if model.reasoning {
        let level = opts.reasoning_level();
        if level == ThinkingLevel::Off {
            config.insert(
                "thinkingConfig".into(),
                shared::disabled_thinking_config(model)?,
            );
        } else {
            let mut thinking = serde_json::Map::new();
            thinking.insert("includeThoughts".into(), Value::Bool(true));
            if shared::uses_thinking_level(model) {
                thinking.insert(
                    "thinkingLevel".into(),
                    Value::String(shared::thinking_level_for(model, level)?.to_string()),
                );
            } else {
                thinking.insert(
                    "thinkingBudget".into(),
                    json!(shared::thinking_budget(
                        model,
                        level,
                        opts.thinking_budgets.as_ref()
                    )),
                );
            }
            config.insert("thinkingConfig".into(), Value::Object(thinking));
        }
    }

    if let Some(params) = &model.sampling_params {
        for (key, value) in params {
            config.insert(key.clone(), value.clone());
        }
    }

    let mut body = serde_json::Map::new();
    body.insert("contents".into(), Value::Array(contents));
    if !config.is_empty() {
        body.insert("config".into(), Value::Object(config));
    }
    let _ = api;
    Ok(Value::Object(body))
}

fn auth_headers(
    api: GoogleApi,
    key: Option<String>,
    vertex_token: Option<String>,
) -> std::collections::BTreeMap<String, String> {
    let mut headers = std::collections::BTreeMap::new();
    match api {
        GoogleApi::GenerativeAi => {
            if let Some(key) = key {
                headers.insert("x-goog-api-key".into(), key);
            }
        }
        GoogleApi::Vertex => {
            // A bearer token wins: that is what ADC yields. An API key is also
            // accepted (Vertex supports API-key auth for the shared endpoints).
            let token = vertex_token.or(key);
            if let Some(token) = token {
                headers.insert("authorization".into(), format!("Bearer {token}"));
            }
        }
    }
    headers
}

#[async_trait]
impl Provider for GoogleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    async fn stream_simple(
        &self,
        model: &Model,
        ctx: &Context,
        opts: &SimpleStreamOptions,
    ) -> AssistantMessageEventStream {
        let (mut producer, stream) =
            create_assistant_message_event_stream_with_cancellation(&opts.signal);
        let api = self.api;
        // Vertex reads a bearer token first: it is the ADC shape, and a project
        // set up that way has no API key at all.
        let vertex_token = (api == GoogleApi::Vertex)
            .then(|| env("GOOGLE_VERTEX_ACCESS_TOKEN"))
            .flatten();
        let key = self.resolve_key(opts);
        let headers = auth_headers(api, key.clone(), vertex_token.clone());
        if headers.is_empty() {
            let mut out = shared::empty_message(api.api(), model);
            shared::emit_error(
                &mut producer,
                &mut out,
                "No Google credential. Set GOOGLE_API_KEY / GEMINI_API_KEY (Generative AI) or \
                 GOOGLE_VERTEX_ACCESS_TOKEN (Vertex), or pass an API key."
                    .to_string(),
                false,
            )
            .await;
            return stream;
        }
        let body = match build_body(api, model, ctx, opts, &self.options) {
            Ok(body) => body,
            Err(message) => {
                let mut out = shared::empty_message(api.api(), model);
                shared::emit_error(&mut producer, &mut out, message, false).await;
                return stream;
            }
        };
        let url = stream_url(api, model, &self.options);
        let http = self.http.clone();
        let model = model.clone();
        let mut opts = opts.clone();
        opts.signal = producer.cancellation_token();
        tokio::spawn(async move {
            run_stream(&mut producer, http, &model, url, headers, body, &opts, api).await;
        });
        stream
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
    producer: &mut AssistantMessageEventStreamProducer,
    http: reqwest::Client,
    model: &Model,
    url: String,
    headers: std::collections::BTreeMap<String, String>,
    body: Value,
    opts: &SimpleStreamOptions,
    api: GoogleApi,
) {
    let api_type = api.api();
    let mut out = shared::empty_message(api_type.clone(), model);

    let mut builder = http.post(&url).timeout(opts.request_timeout());
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    for extra in [model.headers.as_ref(), opts.headers.as_ref()]
        .into_iter()
        .flatten()
    {
        for (name, value) in extra {
            builder = builder.header(name, value);
        }
    }
    let request = match super::super::request::PreparedJsonRequest::new(builder, &body) {
        Ok(request) => request,
        Err(error) => {
            shared::emit_error(
                producer,
                &mut out,
                format!("failed to encode request: {error}"),
                false,
            )
            .await;
            return;
        }
    };
    let signal = opts.signal.clone();
    let response = retry_provider_request(
        move || {
            let request = request.clone();
            let signal = signal.clone();
            async move {
                let response = tokio::select! {
                    r = request.send() => r?,
                    _ = signal.cancelled() => return Err(AiError::Abort { message: "Request aborted".into() }),
                };
                if response.status().is_success() {
                    Ok(response)
                } else {
                    let status = response.status().as_u16();
                    let message = response.text().await.unwrap_or_default();
                    Err(AiError::Http { status: Some(status), message })
                }
            }
        },
        opts.max_retries,
        opts.max_retry_delay,
        &opts.signal,
    )
    .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            shared::emit_error(producer, &mut out, error.to_string(), error.is_abort()).await;
            return;
        }
    };

    let mut events = SseEventStream::new(response, opts.signal.clone())
        .with_idle_timeout(opts.stream_idle_timeout());
    producer
        .push(AssistantMessageEvent::Start {
            partial: Arc::new(out.clone()),
        })
        .await;

    let mut state = StreamState::default();
    loop {
        let event = match events.next_event().await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => {
                state.flush(producer, &mut out, true).await;
                shared::emit_error(producer, &mut out, error.to_string(), error.is_abort()).await;
                return;
            }
        };
        let data = event.data.trim();
        if data.is_empty() || data == "[DONE]" {
            if data == "[DONE]" {
                break;
            }
            continue;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(error) => {
                state.flush(producer, &mut out, true).await;
                shared::emit_error(
                    producer,
                    &mut out,
                    format!("invalid Google stream chunk: {error}; data={data}"),
                    false,
                )
                .await;
                return;
            }
        };
        if let Some(response_id) = chunk.get("responseId").and_then(Value::as_str) {
            if out.response_id.is_none() && !response_id.is_empty() {
                out.response_id = Some(response_id.to_string());
            }
        }
        if let Some(metadata) = chunk.get("usageMetadata") {
            if let Some(usage) = shared::usage_from(metadata) {
                out.usage = usage;
            }
        }
        let candidate = chunk
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first());
        if let Some(candidate) = candidate {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|content| content.get("parts"))
                .and_then(Value::as_array)
            {
                for part in parts {
                    state.apply_part(producer, &mut out, part, model).await;
                }
            }
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                out.raw_stop_reason = Some(reason.to_string());
                out.stop_reason = shared::stop_reason(reason);
                // A response that stopped with tool calls did not stop: the
                // harness must run them.
                if out.stop_reason == StopReason::Stop
                    && out
                        .content
                        .iter()
                        .any(|block| matches!(block, Content::ToolCall(_)))
                {
                    out.stop_reason = StopReason::ToolUse;
                }
            }
        }
    }

    state.flush(producer, &mut out, true).await;

    if opts.signal.is_cancelled() {
        shared::emit_error(producer, &mut out, "Request was aborted", true).await;
        return;
    }
    if matches!(out.stop_reason, StopReason::Pending) {
        shared::emit_error(
            producer,
            &mut out,
            "Google stream ended without a finish reason",
            false,
        )
        .await;
        return;
    }
    if matches!(out.stop_reason, StopReason::Error | StopReason::Aborted) {
        let message = out
            .raw_stop_reason
            .as_deref()
            .map(|reason| format!("Provider stopped with: {reason}"))
            .unwrap_or_else(|| "An unknown error occurred".to_string());
        shared::emit_error(producer, &mut out, message, false).await;
        return;
    }

    out.usage.cost = calculate_cost(&model.cost, &out.usage);
    let reason = match out.stop_reason {
        StopReason::ToolUse => crate::types::DoneReason::ToolUse,
        StopReason::Length => crate::types::DoneReason::Length,
        StopReason::Deferred => crate::types::DoneReason::Deferred,
        _ => crate::types::DoneReason::Stop,
    };
    producer
        .push(AssistantMessageEvent::Done {
            reason,
            message: out,
        })
        .await;
}

/// Streaming state: which content block is open, and any batched delta.
#[derive(Default)]
struct StreamState {
    open: Option<OpenBlock>,
    taken_ids: HashSet<String>,
    response_index: usize,
    /// Bytes batched but not yet emitted, with the block they belong to.
    pending: String,
    pending_index: Option<usize>,
    last_flush: Option<std::time::Instant>,
}

enum OpenBlock {
    Text { index: usize },
    Thinking { index: usize },
}

impl StreamState {
    fn should_flush(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = self
            .last_flush
            .map(|last| now.duration_since(last) >= DELTA_FLUSH_INTERVAL)
            .unwrap_or(false);
        self.pending.len() >= DELTA_FLUSH_BYTES || elapsed
    }

    async fn flush(
        &mut self,
        producer: &mut AssistantMessageEventStreamProducer,
        out: &mut AssistantMessage,
        force_end: bool,
    ) {
        if !self.pending.is_empty() {
            if let Some(index) = self.pending_index {
                let delta = std::mem::take(&mut self.pending);
                let thinking = matches!(self.content_kind(out, index), Some(true));
                producer
                    .push(if thinking {
                        AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    } else {
                        AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    })
                    .await;
                self.last_flush = Some(std::time::Instant::now());
            } else {
                self.pending.clear();
            }
        }
        if force_end {
            self.end_block(producer, out).await;
        }
    }

    fn content_kind(&self, out: &AssistantMessage, index: usize) -> Option<bool> {
        match out.content.get(index) {
            Some(Content::Thinking(_)) => Some(true),
            Some(Content::Text(_)) => Some(false),
            _ => None,
        }
    }

    /// Close whichever text/thinking block is open, emitting its `*End` event
    /// only after the batch is flushed so no delta is lost or reordered.
    async fn end_block(
        &mut self,
        producer: &mut AssistantMessageEventStreamProducer,
        out: &mut AssistantMessage,
    ) {
        // Flush first, without recursing into another end.
        if !self.pending.is_empty() {
            if let Some(index) = self.pending_index {
                let delta = std::mem::take(&mut self.pending);
                let thinking = matches!(self.content_kind(out, index), Some(true));
                producer
                    .push(if thinking {
                        AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    } else {
                        AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    })
                    .await;
            } else {
                self.pending.clear();
            }
        }
        match self.open.take() {
            Some(OpenBlock::Text { index }) => {
                let content = match out.content.get(index) {
                    Some(Content::Text(text)) => text.text.clone(),
                    _ => String::new(),
                };
                producer
                    .push(AssistantMessageEvent::TextEnd {
                        content_index: index,
                        content,
                        partial: Arc::new(out.clone()),
                    })
                    .await;
            }
            Some(OpenBlock::Thinking { index }) => {
                let content = match out.content.get(index) {
                    Some(Content::Thinking(thinking)) => thinking.thinking.clone(),
                    _ => String::new(),
                };
                producer
                    .push(AssistantMessageEvent::ThinkingEnd {
                        content_index: index,
                        content,
                        partial: Arc::new(out.clone()),
                    })
                    .await;
            }
            None => {}
        }
        self.pending_index = None;
    }

    async fn apply_part(
        &mut self,
        producer: &mut AssistantMessageEventStreamProducer,
        out: &mut AssistantMessage,
        part: &Value,
        model: &Model,
    ) {
        // Tool calls are atomic in Google's stream: the whole `functionCall`
        // arrives in one part, so any open text block ends first.
        if let Some((name, args, provided_id)) = shared::part_function_call(part) {
            self.flush(producer, out, true).await;
            let id = shared::tool_call_id(
                provided_id,
                name,
                self.response_index,
                self.taken_ids.len(),
                &mut self.taken_ids,
            );
            let signature = part
                .get("thoughtSignature")
                .and_then(Value::as_str)
                .map(str::to_string);
            let call = shared::tool_call(id, name.to_string(), args.clone(), signature);
            let index = out.content.len();
            out.content.push(call.clone());
            producer
                .push(AssistantMessageEvent::ToolCallStart {
                    content_index: index,
                    partial: Arc::new(out.clone()),
                })
                .await;
            producer
                .push(AssistantMessageEvent::ToolCallEnd {
                    content_index: index,
                    tool_call: match &call {
                        Content::ToolCall(call) => call.clone(),
                        _ => unreachable!("just pushed a tool call"),
                    },
                    partial: Arc::new(out.clone()),
                })
                .await;
            let _ = model;
            return;
        }

        let Some(text) = part.get("text").and_then(Value::as_str) else {
            return;
        };
        let thinking = shared::is_thinking_part(part);
        let signature = part.get("thoughtSignature").and_then(Value::as_str);
        let wants_thinking = thinking;

        // Switching channel (or opening the first block) closes the previous one.
        let matches_open = matches!(
            (&self.open, wants_thinking),
            (Some(OpenBlock::Text { .. }), false) | (Some(OpenBlock::Thinking { .. }), true)
        );
        if !matches_open {
            self.flush(producer, out, true).await;
            let index = out.content.len();
            if wants_thinking {
                out.content.push(Content::Thinking(ThinkingContent {
                    kind: ThinkingContentType,
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: false,
                }));
                producer
                    .push(AssistantMessageEvent::ThinkingStart {
                        content_index: index,
                        partial: Arc::new(out.clone()),
                    })
                    .await;
                self.open = Some(OpenBlock::Thinking { index });
            } else {
                out.content.push(Content::Text(TextContent {
                    kind: TextContentType,
                    text: String::new(),
                    text_signature: None,
                }));
                producer
                    .push(AssistantMessageEvent::TextStart {
                        content_index: index,
                        partial: Arc::new(out.clone()),
                    })
                    .await;
                self.open = Some(OpenBlock::Text { index });
            }
        }

        let index = match &self.open {
            Some(OpenBlock::Text { index }) | Some(OpenBlock::Thinking { index }) => *index,
            None => return,
        };
        // Buffer the text into the block immediately (the events are batched,
        // the message is not) and remember which block the batch belongs to.
        match out.content.get_mut(index) {
            Some(Content::Thinking(thinking_block)) => {
                thinking_block.thinking.push_str(text);
                thinking_block.thinking_signature = shared::retain_signature(
                    thinking_block.thinking_signature.as_deref(),
                    signature,
                );
            }
            Some(Content::Text(text_block)) => {
                text_block.text.push_str(text);
                text_block.text_signature =
                    shared::retain_signature(text_block.text_signature.as_deref(), signature);
            }
            _ => return,
        }
        self.pending.push_str(text);
        self.pending_index = Some(index);
        if self.should_flush() {
            if let Some(index) = self.pending_index {
                let delta = std::mem::take(&mut self.pending);
                let thinking = matches!(self.content_kind(out, index), Some(true));
                producer
                    .push(if thinking {
                        AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    } else {
                        AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta,
                            partial: Arc::new(out.clone()),
                        }
                    })
                    .await;
                self.last_flush = Some(std::time::Instant::now());
            }
        }
    }
}

/// Catalog entries for the Generative Language API.
pub fn google_generative_ai_models() -> Vec<Model> {
    vec![
        google_model("gemini-2.5-pro", "Gemini 2.5 Pro", 2, true),
        google_model("gemini-2.5-flash", "Gemini 2.5 Flash", 3, true),
        google_model("gemini-2.5-flash-lite", "Gemini 2.5 Flash Lite", 4, true),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn google_model(id: &str, name: &str, _rank: u8, reasoning: bool) -> Option<Model> {
    let mut model = Model::new(
        id,
        name,
        Api::GoogleGenerativeAi,
        "google",
        GoogleApi::GenerativeAi.default_base_url(),
    );
    model.reasoning = reasoning;
    model.input = vec![
        crate::types::InputModality::Text,
        crate::types::InputModality::Image,
    ];
    model.context_window = 1_048_576;
    model.max_tokens = 65_536;
    Some(model)
}

/// A `UserContent` with no text at all still has to produce a valid turn; an
/// image-only message is the case that matters.
pub fn user_has_content(content: &UserContent) -> bool {
    shared::user_text(content).is_some() || matches!(content, UserContent::Blocks(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, UserMessage};

    fn model(api: GoogleApi, id: &str) -> Model {
        let mut model = Model::new(id, id, api.api(), "google", "");
        model.reasoning = true;
        model
    }

    #[test]
    fn the_generative_ai_url_names_the_model_on_the_models_path() {
        let url = stream_url(
            GoogleApi::GenerativeAi,
            &model(GoogleApi::GenerativeAi, "gemini-2.5-pro"),
            &GoogleOptions::default(),
        );
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn the_vertex_url_uses_the_publishers_path_and_the_regional_host() {
        let options = GoogleOptions {
            location: Some("europe-west4".into()),
            ..Default::default()
        };
        let url = stream_url(
            GoogleApi::Vertex,
            &model(GoogleApi::Vertex, "gemini-2.5-pro"),
            &options,
        );
        assert!(url.starts_with("https://europe-west4-aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-pro:streamGenerateContent"), "{url}");
        assert!(url.ends_with("?alt=sse"), "{url}");
    }

    #[test]
    fn the_global_vertex_location_keeps_the_shared_host() {
        let options = GoogleOptions {
            location: Some("global".into()),
            ..Default::default()
        };
        let url = stream_url(
            GoogleApi::Vertex,
            &model(GoogleApi::Vertex, "gemini-2.5-pro"),
            &options,
        );
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/v1/publishers"),
            "{url}"
        );
    }

    #[test]
    fn a_custom_base_url_is_used_verbatim() {
        let mut model = model(GoogleApi::Vertex, "gemini-2.5-pro");
        model.base_url = "https://proxy.example/v1".into();
        let url = stream_url(GoogleApi::Vertex, &model, &GoogleOptions::default());
        assert!(
            url.starts_with("https://proxy.example/v1/publishers/google/models/"),
            "{url}"
        );
    }

    #[test]
    fn generative_ai_authenticates_with_a_goog_header() {
        let headers = auth_headers(GoogleApi::GenerativeAi, Some("k".into()), None);
        assert_eq!(headers.get("x-goog-api-key").map(String::as_str), Some("k"));
        assert!(!headers.contains_key("authorization"));
    }

    #[test]
    fn vertex_authenticates_with_a_bearer_token() {
        let headers = auth_headers(GoogleApi::Vertex, Some("k".into()), Some("t".into()));
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer t")
        );
        // An API key is also accepted as a bearer token on Vertex.
        let keyed = auth_headers(GoogleApi::Vertex, Some("k".into()), None);
        assert_eq!(
            keyed.get("authorization").map(String::as_str),
            Some("Bearer k")
        );
    }

    #[test]
    fn the_body_carries_contents_config_and_system_instruction() {
        let model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        let ctx = Context {
            system_prompt: Some("be brief".into()),
            messages: vec![Message::User(UserMessage::new(
                UserContent::Text("hi".into()),
                0,
            ))],
            tools: Vec::new(),
        };
        let body = build_body(
            GoogleApi::GenerativeAi,
            &model,
            &ctx,
            &SimpleStreamOptions::default(),
            &GoogleOptions::default(),
        )
        .expect("body");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(
            body["config"]["systemInstruction"]["parts"][0]["text"],
            "be brief"
        );
    }

    #[test]
    fn an_off_thinking_level_sends_a_disabled_config() {
        let model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        let opts = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Off),
            ..Default::default()
        };
        let body = build_body(
            GoogleApi::GenerativeAi,
            &model,
            &Context::default(),
            &opts,
            &GoogleOptions::default(),
        )
        .expect("body");
        assert_eq!(body["config"]["thinkingConfig"]["thinkingBudget"], 0);
    }

    #[test]
    fn a_gemini_3_model_sends_a_discrete_thinking_level() {
        let model = model(GoogleApi::GenerativeAi, "gemini-3-pro-preview");
        let opts = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_body(
            GoogleApi::GenerativeAi,
            &model,
            &Context::default(),
            &opts,
            &GoogleOptions::default(),
        )
        .expect("body");
        assert_eq!(body["config"]["thinkingConfig"]["thinkingLevel"], "HIGH");
        assert_eq!(
            body["config"]["thinkingConfig"]["includeThoughts"],
            Value::Bool(true)
        );
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serve one SSE response and hand back the request body the adapter sent.
    async fn serve(body: String) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 16384];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (address, handle)
    }

    fn model(api: GoogleApi, id: &str) -> Model {
        let mut model = Model::new(id, id, api.api(), "google", "");
        model.reasoning = true;
        model
    }

    fn sse(frames: &[serde_json::Value]) -> String {
        frames
            .iter()
            .map(|frame| format!("data: {frame}\r\n\r\n"))
            .collect()
    }

    async fn collect(provider: &GoogleProvider, model: &Model) -> (AssistantMessage, Vec<String>) {
        let mut stream = provider
            .stream_simple(
                model,
                &Context::new(Vec::new()),
                &SimpleStreamOptions::default(),
            )
            .await;
        let mut events = Vec::new();
        let mut message = None;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = stream.next().await {
                events.push(event.type_tag().to_string());
                match event {
                    crate::types::AssistantMessageEvent::Done { message: done, .. } => {
                        message = Some(done)
                    }
                    crate::types::AssistantMessageEvent::Error { error, .. } => {
                        message = Some(error)
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("stream finishes");
        (message.expect("a terminal event"), events)
    }

    #[tokio::test]
    async fn a_text_stream_reaches_the_model_as_text_and_usage() {
        let (address, server) = serve(sse(&[
            json!({"responseId":"resp-1","candidates":[{"content":{"parts":[{"text":"hel"}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"text":"lo"}]}}]}),
            json!({"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],
                   "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}}),
        ]))
        .await;
        let mut test_model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        test_model.base_url = format!("http://{address}/v1beta");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            Some("test-key".into()),
            reqwest::Client::new(),
            vec![test_model.clone()],
        );

        let (message, events) = collect(&provider, &test_model).await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.response_id.as_deref(), Some("resp-1"));
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                Content::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "hello");
        assert_eq!(message.usage.input, 10);
        assert_eq!(message.usage.output, 2);
        assert!(events.iter().any(|kind| kind == "text_start"));
        assert!(events.iter().any(|kind| kind == "text_delta"));
        assert!(events.iter().any(|kind| kind == "text_end"));

        let request = server.await.unwrap();
        assert!(request.contains("x-goog-api-key: test-key"), "{request}");
        assert!(request.contains(":streamGenerateContent"), "{request}");
    }

    #[tokio::test]
    async fn thinking_parts_become_thinking_blocks_and_keep_their_signature() {
        let (address, _server) = serve(sse(&[
            json!({"candidates":[{"content":{"parts":[
                {"text":"pondering","thought":true,"thoughtSignature":"c2ln"},
            ]}}]}),
            json!({"candidates":[{"content":{"parts":[
                {"text":"the answer","thoughtSignature":"c2ln"},
            ]},"finishReason":"STOP"}]}),
        ]))
        .await;
        let mut test_model = model(GoogleApi::GenerativeAi, "gemini-3-pro-preview");
        test_model.base_url = format!("http://{address}/v1beta");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            Some("k".into()),
            reqwest::Client::new(),
            vec![test_model.clone()],
        );

        let (message, _events) = collect(&provider, &test_model).await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert!(matches!(message.content[0], Content::Thinking(_)));
        match &message.content[0] {
            Content::Thinking(thinking) => {
                assert_eq!(thinking.thinking, "pondering");
                assert_eq!(thinking.thinking_signature.as_deref(), Some("c2ln"));
            }
            other => panic!("expected thinking, got {other:?}"),
        }
        match &message.content[1] {
            Content::Text(text) => assert_eq!(text.text, "the answer"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_function_call_becomes_a_tool_call_with_a_stable_id() {
        let (address, _server) = serve(sse(&[json!({"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"read","args":{"path":"a.txt"}}},
            ]},"finishReason":"STOP"}]})]))
        .await;
        let mut test_model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        test_model.base_url = format!("http://{address}/v1beta");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            Some("k".into()),
            reqwest::Client::new(),
            vec![test_model.clone()],
        );

        let (message, events) = collect(&provider, &test_model).await;
        // A response that asked for a tool did not stop.
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        match &message.content[0] {
            Content::ToolCall(call) => {
                assert_eq!(call.name, "read");
                assert_eq!(call.arguments["path"], "a.txt");
                // Gemini 2.x sends no id, so one is derived from the position.
                assert_eq!(call.id, "read_0_0");
            }
            other => panic!("expected a tool call, got {other:?}"),
        }
        assert!(events.iter().any(|kind| kind == "toolcall_end"));
    }

    #[tokio::test]
    async fn a_safety_stop_is_reported_as_an_error_with_the_reason() {
        let (address, _server) = serve(sse(&[
            json!({"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}),
        ]))
        .await;
        let mut test_model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        test_model.base_url = format!("http://{address}/v1beta");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            Some("k".into()),
            reqwest::Client::new(),
            vec![test_model.clone()],
        );

        let (message, _events) = collect(&provider, &test_model).await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider stopped with: SAFETY")
        );
    }

    #[tokio::test]
    async fn a_stream_without_a_finish_reason_is_an_error() {
        let (address, _server) = serve(sse(&[
            json!({"candidates":[{"content":{"parts":[{"text":"half"}]}}]}),
        ]))
        .await;
        let mut test_model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        test_model.base_url = format!("http://{address}/v1beta");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            Some("k".into()),
            reqwest::Client::new(),
            vec![test_model.clone()],
        );

        let (message, _events) = collect(&provider, &test_model).await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(message
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("without a finish reason"));
    }

    #[tokio::test]
    async fn a_missing_credential_fails_before_any_request() {
        let test_model = model(GoogleApi::GenerativeAi, "gemini-2.5-pro");
        let provider = GoogleProvider::new(
            "google",
            GoogleApi::GenerativeAi,
            None,
            reqwest::Client::new(),
            vec![test_model.clone()],
        );
        let (message, _events) = collect(&provider, &test_model).await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(message
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("No Google credential"));
    }
}
