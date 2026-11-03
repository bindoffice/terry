//! Shared wire handling for Google's `generateContent` API, used by both
//! [`crate::providers::google_generative_ai`] and
//! [`crate::providers::google_vertex`].
//!
//! The two adapters differ only in authentication and endpoint; the transcript
//! shape (`contents`/`parts`), tool declarations, thinking controls and the
//! streaming event decoder are identical, so they live here once. Mirrors
//! `packages/ai/src/api/google-shared.ts`.
//!
//! Divergences worth knowing about:
//!
//! * **Tool call ids.** Gemini 2.x does not send `functionCall.id`, and the
//!   Responses/Anthropic-shaped harness needs one to pair a result with its
//!   call. Ids are therefore synthesized deterministically from the call's
//!   position, not from the clock, so replaying the same response twice yields
//!   the same id (a wall-clock id made a resumed turn look like a new call).
//! * **Thought signatures.** `thoughtSignature` may appear on any part and
//!   identifies the model's encrypted reasoning. It must be echoed back on the
//!   part it arrived on; an empty text part carrying one is therefore kept
//!   rather than dropped.

use serde_json::{json, Map, Value};

use crate::model::Model;
use crate::types::{
    Api, AssistantMessage, AssistantMessageEvent, Content, Context, ImageContent, InputModality,
    Message, StopReason, ThinkingBudgets, ThinkingLevel, Tool, ToolCall, ToolCallType, Usage,
    UserContent,
};

/// Google's discrete thinking control, used by Gemini 3+ and Gemma 4.
pub const THINKING_LEVELS: &[&str] = &["MINIMAL", "LOW", "MEDIUM", "HIGH"];

/// Whether this model uses `thinkingLevel` instead of `thinkingBudget`.
///
/// Gemini 2.x takes a token budget; 3+ (and Gemma 4) take a discrete level. The
/// id is the only signal available, and a wrong choice is rejected by the API
/// rather than silently ignored, so getting it wrong is loud.
pub fn uses_thinking_level(model: &Model) -> bool {
    let id = model.id.to_ascii_lowercase();
    let is_gemini_new = id.starts_with("gemini-3")
        || id
            .strip_prefix("gemini-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|major| major.split('.').next())
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= 4);
    is_gemini_new
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
        || id.contains("gemma-4")
        || id.contains("gemma4")
}

/// Map a pi thinking level onto Google's discrete level, honoring the model's
/// own `thinkingLevelMap` first.
pub fn thinking_level_for(model: &Model, level: ThinkingLevel) -> Result<&'static str, String> {
    let mapped = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&level))
        .and_then(|value| value.clone());
    let resolved = match mapped.as_deref() {
        Some(value) => value.to_ascii_lowercase(),
        None => match level {
            ThinkingLevel::Minimal => "minimal".into(),
            ThinkingLevel::Low => "low".into(),
            ThinkingLevel::Medium => "medium".into(),
            ThinkingLevel::High => "high".into(),
            other => {
                return Err(format!(
                    "Unsupported Google thinking level for {}/{}: {other:?}",
                    model.provider, model.id
                ))
            }
        },
    };
    THINKING_LEVELS
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(&resolved))
        .ok_or_else(|| {
            format!(
                "Unsupported Google thinking level mapping for {}/{}: {level:?} -> {resolved}",
                model.provider, model.id
            )
        })
}

/// `thinkingConfig` for a model whose thinking is turned off.
///
/// A model that cannot turn thinking off entirely gets its floor level instead,
/// which is what upstream does: sending `budget: 0` to such a model is a 400.
pub fn disabled_thinking_config(model: &Model) -> Result<Value, String> {
    if !uses_thinking_level(model) {
        return Ok(json!({ "thinkingBudget": 0 }));
    }
    let floor = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&ThinkingLevel::Off))
        .and_then(|value| value.clone());
    match floor {
        // Mapped to null: thinking cannot be disabled, so send the floor level.
        Some(level) if !level.eq_ignore_ascii_case("off") => {
            let resolved = THINKING_LEVELS
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(&level))
                .ok_or_else(|| {
                    format!(
                        "Unsupported Google thinking level mapping for {}/{}: off -> {level}",
                        model.provider, model.id
                    )
                })?;
            Ok(json!({ "thinkingLevel": resolved }))
        }
        _ => Ok(json!({ "thinkingBudget": 0 })),
    }
}

/// Token budget for a budget-based model. `-1` means "let the model decide".
pub fn thinking_budget(
    model: &Model,
    level: ThinkingLevel,
    custom: Option<&ThinkingBudgets>,
) -> i64 {
    if let Some(budget) = custom.and_then(|budgets| match level {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        ThinkingLevel::High => budgets.high,
        _ => None,
    }) {
        return budget as i64;
    }
    let table: Option<[i64; 4]> = if model.id.contains("2.5-pro") {
        Some([128, 2048, 8192, 32768])
    } else if model.id.contains("2.5-flash-lite") {
        Some([512, 2048, 8192, 24576])
    } else if model.id.contains("2.5-flash") {
        Some([128, 2048, 8192, 24576])
    } else {
        None
    };
    match (table, level) {
        (Some(table), ThinkingLevel::Minimal) => table[0],
        (Some(table), ThinkingLevel::Low) => table[1],
        (Some(table), ThinkingLevel::Medium) => table[2],
        (Some(table), ThinkingLevel::High) => table[3],
        // Dynamic budget: the model picks, which is also the right answer for
        // every model this table does not name.
        _ => -1,
    }
}

/// Whether this model wants an explicit `functionCall.id` echoed back.
///
/// Gemini 3+, Claude-behind-Google and `gpt-oss` all correlate calls by id;
/// Gemini 2.x rejects an id it did not send.
pub fn requires_tool_call_id(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    if id.starts_with("claude-") || id.starts_with("gpt-oss-") {
        return true;
    }
    gemini_major_version(&id).is_some_and(|major| major >= 3)
}

fn gemini_major_version(id: &str) -> Option<u32> {
    let rest = id
        .strip_prefix("gemini-live-")
        .or_else(|| id.strip_prefix("gemini-"))?;
    rest.split(['-', '.'])
        .next()
        .and_then(|major| major.parse::<u32>().ok())
}

/// Gemini 3+ supports multimodal `functionResponse.parts`; older models need a
/// separate user turn for result images.
fn supports_multimodal_function_response(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    gemini_major_version(&id).is_none_or(|major| major >= 3)
}

/// Only a valid base64 signature from the same provider+model is replayable;
/// anything else must be dropped rather than sent back as garbage.
fn valid_thought_signature(same_source: bool, signature: Option<&str>) -> Option<String> {
    let signature = signature?;
    if !same_source || signature.is_empty() || signature.len() % 4 != 0 {
        return None;
    }
    let valid = signature
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
    valid.then(|| signature.to_string())
}

fn text(value: &str) -> String {
    // A lone surrogate is not valid UTF-8 and the JSON encoder rejects it.
    value.to_string()
}

/// Convert the provider-neutral transcript into Gemini `contents`.
///
/// The leading system message becomes `systemInstruction` (Gemini has no
/// mid-conversation system role), so `system_prompt` is handled by the caller.
pub fn convert_messages(model: &Model, ctx: &Context) -> Vec<Value> {
    let mut contents: Vec<Value> = Vec::new();
    let include_ids = requires_tool_call_id(&model.id);

    for message in &ctx.messages {
        match message {
            Message::User(user) => {
                let parts: Vec<Value> = match &user.content {
                    UserContent::Text(value) => vec![json!({ "text": text(value) })],
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text(value) => Some(json!({ "text": text(&value.text) })),
                            Content::Image(image) => Some(inline_data(image)),
                            _ => None,
                        })
                        .collect(),
                };
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "user", "parts": parts }));
            }
            Message::Assistant(assistant) => {
                if matches!(
                    assistant.stop_reason,
                    StopReason::Error | StopReason::Aborted
                ) {
                    continue;
                }
                // Google ties a thought signature to the model that produced it;
                // replaying one from another model (or provider) is rejected.
                let same_source = assistant.provider == model.provider
                    && assistant.model == model.id
                    && assistant.api == model.api;
                let mut parts: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        Content::Text(value) => {
                            let signature = valid_thought_signature(
                                same_source,
                                value.text_signature.as_deref(),
                            );
                            // An empty text part is dropped unless it carries a
                            // signature: Gemini attaches one to a part whose text
                            // is empty and requires it echoed back, and dropping
                            // it makes the model stop mid-task next turn.
                            if value.text.trim().is_empty() && signature.is_none() {
                                continue;
                            }
                            let mut part = json!({ "text": text(&value.text) });
                            if let Some(signature) = signature {
                                part["thoughtSignature"] = Value::String(signature);
                            }
                            parts.push(part);
                        }
                        Content::Thinking(thinking) => {
                            if same_source {
                                let signature = valid_thought_signature(
                                    same_source,
                                    thinking.thinking_signature.as_deref(),
                                );
                                if thinking.thinking.trim().is_empty() && signature.is_none() {
                                    continue;
                                }
                                let mut part = json!({
                                    "thought": true,
                                    "text": text(&thinking.thinking),
                                });
                                if let Some(signature) = signature {
                                    part["thoughtSignature"] = Value::String(signature);
                                }
                                parts.push(part);
                            } else if !thinking.thinking.trim().is_empty() {
                                // Cross-model: the signature is unusable, so the
                                // reasoning survives only as plain text. No tags,
                                // or the model starts imitating them.
                                parts.push(json!({ "text": text(&thinking.thinking) }));
                            }
                        }
                        Content::ToolCall(call) => {
                            let signature = valid_thought_signature(
                                same_source,
                                call.thought_signature.as_deref(),
                            );
                            let mut function_call = json!({
                                "name": call.name,
                                "args": call.arguments,
                            });
                            if include_ids {
                                function_call["id"] = Value::String(call.id.clone());
                            }
                            let mut part = json!({ "functionCall": function_call });
                            if let Some(signature) = signature {
                                part["thoughtSignature"] = Value::String(signature);
                            }
                            parts.push(part);
                        }
                        // Images never appear on their own in an assistant turn;
                        // a tool's images arrive inside its result message.
                        Content::Image(_) => {}
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult(result) => {
                let text_result = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text(value) => Some(value.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let wants_images = model.input.contains(&InputModality::Image);
                let images: Vec<&ImageContent> = if wants_images {
                    result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Image(image) => Some(image),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let has_text = !text_result.trim().is_empty();
                let has_images = !images.is_empty();
                let response_value = if has_text {
                    text(&text_result)
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };
                let image_parts: Vec<Value> =
                    images.iter().map(|image| inline_data(image)).collect();

                let mut function_response = json!({
                    "name": result.tool_name,
                    "response": if result.is_error {
                        json!({ "error": response_value })
                    } else {
                        json!({ "output": response_value })
                    },
                });
                let multimodal = supports_multimodal_function_response(&model.id);
                if has_images && multimodal {
                    function_response["parts"] = Value::Array(image_parts.clone());
                }
                if include_ids {
                    function_response["id"] = Value::String(result.tool_call_id.clone());
                }
                let part = json!({ "functionResponse": function_response });

                // Cloud Code Assist requires every function response of a batch in
                // one user turn, so append to a trailing response turn when there
                // is one.
                let trailing_is_responses = contents
                    .last()
                    .and_then(|content| content.get("parts"))
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|part| part.get("functionResponse").is_some())
                    });
                if trailing_is_responses {
                    if let Some(parts) = contents
                        .last_mut()
                        .and_then(|content| content.get_mut("parts"))
                        .and_then(Value::as_array_mut)
                    {
                        parts.push(part);
                    }
                } else {
                    contents.push(json!({ "role": "user", "parts": [part] }));
                }

                if has_images && !multimodal {
                    // Older models cannot nest images in a function response, so
                    // they follow as a plain user turn.
                    let mut parts = vec![json!({ "text": "Tool result image:" })];
                    parts.extend(image_parts);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    contents
}

fn inline_data(image: &ImageContent) -> Value {
    json!({
        "inlineData": {
            "mimeType": image.mime_type,
            "data": image.data,
        }
    })
}

/// Schema keys that describe the document rather than the data, so Gemini's
/// OpenAPI-subset converter must not see them.
const SCHEMA_META_KEYS: &[&str] = &[
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions",
];

/// Keywords whose values map names to subschemas, so their keys are data.
const NAME_MAP_KEYS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
    "dependencies",
];

/// Keywords holding a subschema or an array of them.
const SUBSCHEMA_KEYS: &[&str] = &[
    "additionalProperties",
    "unevaluatedProperties",
    "propertyNames",
    "items",
    "prefixItems",
    "additionalItems",
    "unevaluatedItems",
    "contains",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "contentSchema",
];

fn sanitize_for_openapi(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !SCHEMA_META_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), sanitize_for_openapi(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sanitize_for_openapi).collect()),
        other => other.clone(),
    }
}

/// Gemini ignores `const` in `parametersJsonSchema` but honors `enum`, so a
/// single-value enum is the portable encoding.
fn rewrite_const_as_enum(schema: &Value, is_name_map: bool) -> Value {
    match schema {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| rewrite_const_as_enum(item, false))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                let child_is_name_map = NAME_MAP_KEYS.contains(&key.as_str());
                let nested = if is_name_map || SUBSCHEMA_KEYS.contains(&key.as_str()) {
                    rewrite_const_as_enum(value, false)
                } else if child_is_name_map {
                    rewrite_const_as_enum(value, true)
                } else {
                    value.clone()
                };
                out.insert(key.clone(), nested);
            }
            if !is_name_map && out.contains_key("const") && !out.contains_key("enum") {
                let value = out.get("const").cloned().unwrap_or(Value::Null);
                if value.is_string() || value.is_number() {
                    out.remove("const");
                    out.insert("enum".into(), Value::Array(vec![value]));
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// `tools` for the request, or `None` when there are no tools.
pub fn convert_tools(tools: &[Tool], use_legacy_parameters: bool) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    let declarations: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let parameters = tool.parameters.as_value();
            let mut declaration = json!({
                "name": tool.name,
                "description": tool.description,
            });
            if use_legacy_parameters {
                declaration["parameters"] = sanitize_for_openapi(parameters);
            } else {
                declaration["parametersJsonSchema"] = rewrite_const_as_enum(parameters, false);
            }
            declaration
        })
        .collect();
    Some(json!([{ "functionDeclarations": declarations }]))
}

/// Function-calling mode. `VALIDATED` lets Gemini 3+ enforce required
/// parameters, which is the closest analogue of strict tool calling.
pub fn function_calling_mode(
    tools: &[Tool],
    tool_choice: Option<&str>,
    supports_strict: bool,
) -> Option<&'static str> {
    let strict = supports_strict
        && tools.iter().any(|tool| {
            matches!(
                tool.constrained_sampling,
                Some(crate::types::ConstrainedSamplingConfig::JsonSchema { .. })
            )
        });
    match tool_choice {
        Some("none") => Some("NONE"),
        Some("any") => Some("ANY"),
        Some(_) => Some("AUTO"),
        None if strict => Some("VALIDATED"),
        None => None,
    }
}

/// Gemini 3+ validates required function parameters.
pub fn supports_strict_tool_sampling(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    gemini_major_version(&id).is_some_and(|major| major >= 3)
}

/// Map a `finishReason` onto the neutral stop reason. Anything a content filter
/// ends is an error: the model did not finish, and pretending it stopped
/// normally would hand the user a truncated answer with no explanation.
pub fn stop_reason(finish_reason: &str) -> StopReason {
    match finish_reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

/// Read token accounting from `usageMetadata`.
pub fn usage_from(metadata: &Value) -> Option<Usage> {
    let get = |key: &str| metadata.get(key).and_then(Value::as_i64);
    let prompt = get("promptTokenCount")?;
    let cached = get("cachedContentTokenCount").unwrap_or(0);
    let candidates = get("candidatesTokenCount").unwrap_or(0);
    let thoughts = get("thoughtsTokenCount").unwrap_or(0);
    Some(Usage {
        input: (prompt - cached).max(0),
        output: candidates + thoughts,
        cache_read: cached,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: (thoughts > 0).then_some(thoughts),
        total_tokens: get("totalTokenCount").unwrap_or(prompt + candidates),
        cost: Default::default(),
    })
}

/// Emit the terminal event for a finished assistant message.
pub async fn emit_error(
    producer: &mut crate::event_stream::AssistantMessageEventStreamProducer,
    out: &mut AssistantMessage,
    message: impl Into<String>,
    aborted: bool,
) {
    out.stop_reason = if aborted {
        StopReason::Aborted
    } else {
        StopReason::Error
    };
    out.error_message = Some(message.into());
    producer
        .push(AssistantMessageEvent::Error {
            reason: if aborted {
                crate::types::ErrorReason::Aborted
            } else {
                crate::types::ErrorReason::Error
            },
            error: out.clone(),
        })
        .await;
}

/// The empty assistant message a Google stream starts from.
pub fn empty_message(api: Api, model: &Model) -> AssistantMessage {
    AssistantMessage::empty(api, model.provider.clone(), model.id.clone(), now_ms())
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A tool call's id, preserved when Google supplied one and derived from the
/// call position when it did not.
///
/// Deterministic rather than clock-based: a resumed turn re-reads the same
/// response, and a fresh id each time made the harness see a different call.
pub fn tool_call_id(
    provided: Option<&str>,
    name: &str,
    response_index: usize,
    call_index: usize,
    taken: &mut std::collections::HashSet<String>,
) -> String {
    if let Some(id) = provided.filter(|id| !id.is_empty()) {
        if taken.insert(id.to_string()) {
            return id.to_string();
        }
    }
    let mut candidate = format!("{name}_{response_index}_{call_index}");
    let mut suffix = 0usize;
    while !taken.insert(candidate.clone()) {
        suffix += 1;
        candidate = format!("{name}_{response_index}_{call_index}_{suffix}");
    }
    candidate
}

/// Build a `ToolCall` content block.
pub fn tool_call(id: String, name: String, arguments: Value, signature: Option<String>) -> Content {
    Content::ToolCall(ToolCall {
        kind: ToolCallType,
        id,
        name,
        arguments,
        thought_signature: signature,
        namespace: None,
        recorded: None,
    })
}

/// A `ToolCall` inside a response part, if the part carries one.
pub fn part_function_call(part: &Value) -> Option<(&str, &Value, Option<&str>)> {
    let call = part.get("functionCall")?;
    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
    let args = call.get("args").unwrap_or(&Value::Null);
    let id = call.get("id").and_then(Value::as_str);
    Some((name, args, id))
}

/// Thinking vs visible text for a streamed part. `thought: true` is the marker;
/// a bare `thoughtSignature` is not (it can appear on any part).
pub fn is_thinking_part(part: &Value) -> bool {
    part.get("thought").and_then(Value::as_bool) == Some(true)
}

/// Keep a signature that arrived once on an earlier delta of the same block.
pub fn retain_signature(existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => existing.map(str::to_string),
    }
}

/// Text of a `UserContent` for the request, or `None` when it has none.
pub fn user_text(content: &UserContent) -> Option<String> {
    match content {
        UserContent::Text(value) => Some(value.clone()),
        UserContent::Blocks(blocks) => {
            let joined = blocks
                .iter()
                .filter_map(|block| match block {
                    Content::Text(value) => Some(value.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TextContent;

    fn gemini(id: &str, api: Api) -> Model {
        let mut model = Model::new(id, id, api, "google", "https://example.test");
        model.reasoning = true;
        model.input = vec![InputModality::Text, InputModality::Image];
        model
    }

    #[test]
    fn gemini_3_and_later_use_a_thinking_level() {
        for id in [
            "gemini-3-flash-preview",
            "gemini-3.1-pro-preview",
            "gemini-4-argon",
            "gemma-4-9b",
        ] {
            assert!(
                uses_thinking_level(&gemini(id, Api::GoogleGenerativeAi)),
                "{id}"
            );
        }
        for id in ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-2.0-flash"] {
            assert!(
                !uses_thinking_level(&gemini(id, Api::GoogleGenerativeAi)),
                "{id}"
            );
        }
    }

    #[test]
    fn a_thinking_level_maps_through_the_model_table() {
        let mut model = gemini("gemini-3-pro-preview", Api::GoogleGenerativeAi);
        assert_eq!(
            thinking_level_for(&model, ThinkingLevel::High).unwrap(),
            "HIGH"
        );
        model.thinking_level_map = Some(
            [(ThinkingLevel::High, Some("medium".to_string()))]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            thinking_level_for(&model, ThinkingLevel::High).unwrap(),
            "MEDIUM"
        );
    }

    #[test]
    fn only_id_bearing_models_get_tool_call_ids() {
        assert!(requires_tool_call_id("gemini-3-pro-preview"));
        assert!(requires_tool_call_id("claude-sonnet-4"));
        assert!(requires_tool_call_id("gpt-oss-120b"));
        assert!(!requires_tool_call_id("gemini-2.5-pro"));
    }

    #[test]
    fn a_synthesized_tool_call_id_is_deterministic_and_unique() {
        let mut taken = std::collections::HashSet::new();
        // No id from Google: derived from position.
        let first = tool_call_id(None, "read", 0, 0, &mut taken);
        let mut again = std::collections::HashSet::new();
        assert_eq!(tool_call_id(None, "read", 0, 0, &mut again), first);
        // A duplicate id from the wire is not reused.
        let second = tool_call_id(Some(&first), "read", 0, 1, &mut taken);
        assert_ne!(second, first);
        assert_eq!(second, "read_0_1");
        // A fresh id is kept verbatim.
        let mut fresh = std::collections::HashSet::new();
        assert_eq!(tool_call_id(Some("abc"), "read", 0, 0, &mut fresh), "abc");
    }

    #[test]
    fn const_becomes_a_single_value_enum() {
        let schema = json!({
            "type": "object",
            "properties": {"mode": {"const": "fast"}},
        });
        let rewritten = rewrite_const_as_enum(&schema, false);
        assert_eq!(rewritten["properties"]["mode"]["enum"], json!(["fast"]));
        assert!(rewritten["properties"]["mode"].get("const").is_none());
    }

    #[test]
    fn property_names_are_not_treated_as_schema_keywords() {
        // A property literally named `const` must survive as a name.
        let schema = json!({
            "type": "object",
            "properties": {"const": {"type": "string"}},
        });
        let rewritten = rewrite_const_as_enum(&schema, false);
        assert!(rewritten["properties"]["const"].is_object());
        assert!(rewritten["properties"]["const"].get("enum").is_none());
    }

    #[test]
    fn openapi_sanitizing_drops_meta_keys_recursively() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$defs": {"x": {"type": "string"}},
            "type": "object",
            "properties": {"a": {"$comment": "no", "type": "string"}},
        });
        let clean = sanitize_for_openapi(&schema);
        assert!(clean.get("$schema").is_none());
        assert!(clean.get("$defs").is_none());
        assert!(clean["properties"]["a"].get("$comment").is_none());
        assert_eq!(clean["properties"]["a"]["type"], "string");
    }

    #[test]
    fn tools_use_parameters_json_schema_by_default() {
        let mut tool = Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: crate::types::Schema::new(json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
            })),
            constrained_sampling: None,
        };
        let converted = convert_tools(&[tool.clone()], false).expect("tools");
        let declaration = &converted[0]["functionDeclarations"][0];
        assert_eq!(declaration["name"], "read");
        assert!(declaration.get("parametersJsonSchema").is_some());
        assert!(declaration.get("parameters").is_none());

        // The legacy shape is opt-in (Cloud Code Assist with Claude models).
        let legacy = convert_tools(&[tool.clone()], true).expect("tools");
        assert!(legacy[0]["functionDeclarations"][0]
            .get("parameters")
            .is_some());

        tool.parameters = crate::types::Schema::empty_object();
        assert!(convert_tools(&[], false).is_none());
        let _ = tool;
    }

    #[test]
    fn stop_reasons_are_mapped_and_filters_are_errors() {
        assert_eq!(stop_reason("STOP"), StopReason::Stop);
        assert_eq!(stop_reason("MAX_TOKENS"), StopReason::Length);
        for reason in ["SAFETY", "RECITATION", "MALFORMED_FUNCTION_CALL", "OTHER"] {
            assert_eq!(stop_reason(reason), StopReason::Error, "{reason}");
        }
    }

    #[test]
    fn usage_separates_cached_and_reasoning_tokens() {
        let usage = usage_from(&json!({
            "promptTokenCount": 1000,
            "cachedContentTokenCount": 400,
            "candidatesTokenCount": 200,
            "thoughtsTokenCount": 50,
            "totalTokenCount": 1250,
        }))
        .unwrap();
        assert_eq!(usage.input, 600);
        assert_eq!(usage.cache_read, 400);
        assert_eq!(usage.output, 250);
        assert_eq!(usage.reasoning, Some(50));
        assert_eq!(usage.total_tokens, 1250);
    }

    #[test]
    fn a_text_block_without_a_signature_and_no_text_is_dropped() {
        let model = gemini("gemini-2.5-pro", Api::GoogleGenerativeAi);
        let mut assistant =
            AssistantMessage::empty(Api::GoogleGenerativeAi, "google", "gemini-2.5-pro", 1);
        assistant.content = vec![Content::Text(TextContent {
            kind: crate::types::TextContentType,
            text: "   ".into(),
            text_signature: None,
        })];
        let ctx = Context {
            messages: vec![Message::Assistant(Box::new(assistant))],
            ..Default::default()
        };
        assert!(convert_messages(&model, &ctx).is_empty());
    }

    #[test]
    fn an_empty_text_block_with_a_signature_is_kept() {
        // Gemini attaches the signature to a part whose text is empty and
        // requires it echoed back; dropping it ends the next turn mid-task.
        let model = gemini("gemini-3-pro-preview", Api::GoogleGenerativeAi);
        let mut assistant =
            AssistantMessage::empty(Api::GoogleGenerativeAi, "google", "gemini-3-pro-preview", 1);
        assistant.content = vec![Content::Text(TextContent {
            kind: crate::types::TextContentType,
            text: String::new(),
            text_signature: Some("c2lnbmF0dXJl".into()),
        })];
        let ctx = Context {
            messages: vec![Message::Assistant(Box::new(assistant))],
            ..Default::default()
        };
        let contents = convert_messages(&model, &ctx);
        assert_eq!(contents.len(), 1);
        assert_eq!(
            contents[0]["parts"][0]["thoughtSignature"], "c2lnbmF0dXJl",
            "the signature must survive"
        );
    }

    #[test]
    fn a_foreign_signature_is_not_replayed() {
        let model = gemini("gemini-3-pro-preview", Api::GoogleGenerativeAi);
        let mut assistant =
            AssistantMessage::empty(Api::AnthropicMessages, "anthropic", "claude-sonnet-4", 1);
        assistant.content = vec![Content::Text(TextContent {
            kind: crate::types::TextContentType,
            text: "hi".into(),
            text_signature: Some("c2lnbmF0dXJl".into()),
        })];
        let ctx = Context {
            messages: vec![Message::Assistant(Box::new(assistant))],
            ..Default::default()
        };
        let contents = convert_messages(&model, &ctx);
        assert!(contents[0]["parts"][0].get("thoughtSignature").is_none());
    }
}
