//! Automatic model discovery for providers that declare no explicit model list.
//!
//! A provider configured with only `api`/`baseUrl`/`apiKey` cannot be used until
//! its model ids are known. Instead of requiring a hand-written `models` array,
//! this module asks the endpoint for its own catalog:
//!
//! * OpenAI-compatible protocols (`openai-completions`, `openai-responses`):
//!   `GET {baseUrl}/models`, reading the `data` (or `models`) array.
//! * Anthropic Messages: `GET {baseUrl}/v1/models`, following `has_more` /
//!   `last_id` pagination.
//!
//! Results are cached in `~/.tom/agent/models.discovered.json`, which
//! [`crate::config::load_models_config`] merges into providers whose `models`
//! list is empty. The cache lets offline runs and failed refreshes keep working,
//! and it is never written back to `models.json`. Startup reuses a cached list
//! for [`REFRESH_INTERVAL_SECS`] (failed attempts included, so an unreachable
//! endpoint cannot stall every launch); `--list-models` always refreshes.
//! Discovery is best-effort: failures only produce warnings and never block
//! startup.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use tom_extensions::ProviderManifest;
use tom_runtime::credentials::{self, CredentialSource};
use tom_runtime::values::resolve_headers;
use tom_runtime::ValueSource;

use crate::config::{
    self, provider_is_anthropic_compatible, provider_is_openai_completions,
    provider_is_openai_responses, DiscoveredProvider, ModelDefinition, ProviderConfig,
};

/// Bound one discovery request so a slow endpoint cannot stall startup.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a cached catalog is reused before startup refreshes it. Failures are
/// cached too, so an unreachable endpoint costs one timeout per window instead
/// of one per launch. `--list-models` bypasses this window.
const REFRESH_INTERVAL_SECS: i64 = 15 * 60;
/// Anthropic requires a version header on every request.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Safety limit for Anthropic pagination (1000 models per page).
const MAX_PAGES: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wire {
    Anthropic,
    OpenAi,
}

fn wire_for(config: &ProviderConfig) -> Option<Wire> {
    if provider_is_anthropic_compatible(config) {
        Some(Wire::Anthropic)
    } else if provider_is_openai_completions(config) || provider_is_openai_responses(config) {
        Some(Wire::OpenAi)
    } else {
        None
    }
}

fn default_base_url(wire: Wire) -> &'static str {
    match wire {
        Wire::Anthropic => "https://api.anthropic.com",
        Wire::OpenAi => "https://api.openai.com",
    }
}

/// Whether a provider is eligible for discovery: a supported protocol and no
/// explicit `models` list.
fn needs_discovery(config: &ProviderConfig) -> bool {
    config.models.is_empty() && wire_for(config).is_some()
}

/// Refresh the discovery cache for every configured provider whose `models`
/// list is empty. Returns human-readable warnings for the caller to print.
///
/// `offline` skips the network entirely (the existing cache still applies).
/// `force_refresh` ignores [`REFRESH_INTERVAL_SECS`] so an explicit
/// `--list-models` always shows the endpoint's current catalog.
pub async fn auto_discover(
    offline: bool,
    manifests: &[ProviderManifest],
    force_refresh: bool,
) -> Vec<String> {
    if offline {
        return Vec::new();
    }

    let cfg = match config::load_models_config_raw() {
        Ok(cfg) => cfg,
        Err(error) => {
            return vec![format!(
                "model discovery skipped: could not read models.json: {error}"
            )]
        }
    };

    let now = unix_now();
    let mut discovered = config::read_discovered_models();
    // Drop entries for providers that now declare models explicitly (or were
    // removed), so a stale cache cannot resurrect an abandoned catalog.
    let cached = discovered.providers.len();
    discovered.providers.retain(|id, _| {
        cfg.providers
            .get(id)
            .is_some_and(|provider| provider.models.is_empty())
    });
    let mut dirty = discovered.providers.len() != cached;

    let targets: Vec<(String, ProviderConfig)> = cfg
        .providers
        .iter()
        .filter(|(id, provider)| {
            needs_discovery(provider) && (force_refresh || !cache_is_fresh(&discovered, id, now))
        })
        .map(|(id, provider)| (id.clone(), provider.clone()))
        .collect();

    let mut warnings = Vec::new();
    if !targets.is_empty() {
        warnings = refresh(&targets, manifests, now, &mut discovered).await;
        dirty = true;
    }

    if dirty {
        if let Err(error) = config::write_discovered_models(&discovered) {
            warnings.push(format!(
                "could not write the model discovery cache: {error}"
            ));
        }
    }
    warnings
}

/// Whether the cached catalog for `id` is recent enough to keep using.
fn cache_is_fresh(discovered: &config::DiscoveredModels, id: &str, now: i64) -> bool {
    discovered.providers.get(id).is_some_and(|entry| {
        entry.fetched_at > 0 && now.saturating_sub(entry.fetched_at) < REFRESH_INTERVAL_SECS
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Fetch every target concurrently and record the results. A failed fetch keeps
/// the last known catalog and only updates the timestamp, so the attempt is not
/// repeated on the next launch within the refresh window.
async fn refresh(
    targets: &[(String, ProviderConfig)],
    manifests: &[ProviderManifest],
    now: i64,
    discovered: &mut config::DiscoveredModels,
) -> Vec<String> {
    let auth = config::read_auth().unwrap_or_default();
    let values = crate::provider::CliValues;
    let idle_timeout = crate::settings::load_settings()
        .unwrap_or_default()
        .http_idle_timeout_ms();
    let declarations: BTreeMap<String, tom_runtime::ProviderDeclaration> = manifests
        .iter()
        .map(|manifest| (manifest.id.clone(), crate::provider::declaration(manifest)))
        .collect();

    let fetches = targets.iter().map(|(id, provider)| {
        let auth = &auth;
        let values = &values;
        let declarations = &declarations;
        async move {
            let manifest = declarations.get(id);
            let result = fetch_models(id, provider, manifest, auth, values, idle_timeout).await;
            (id.clone(), result)
        }
    });
    let results = futures::future::join_all(fetches).await;

    let mut warnings = Vec::new();
    for (id, result) in results {
        match result {
            // `None` means discovery was skipped; keep any cached entry.
            Ok(Some(models)) => {
                discovered.providers.insert(
                    id,
                    DiscoveredProvider {
                        fetched_at: now,
                        models,
                    },
                );
            }
            Ok(None) => {}
            Err(error) => {
                warnings.push(format!(
                    "model discovery for provider \"{id}\" failed: {error}"
                ));
                // Keep the last known catalog usable. Updating the timestamp
                // alone records the attempt so the next launch within the
                // window does not retry a failing endpoint.
                match discovered.providers.get_mut(&id) {
                    Some(entry) => entry.fetched_at = now,
                    None => {
                        discovered.providers.insert(
                            id,
                            DiscoveredProvider {
                                fetched_at: now,
                                models: Vec::new(),
                            },
                        );
                    }
                }
            }
        }
    }
    warnings
}

async fn fetch_models(
    provider_id: &str,
    config: &ProviderConfig,
    manifest: Option<&tom_runtime::ProviderDeclaration>,
    auth: &config::AuthStore,
    values: &dyn ValueSource,
    idle_timeout_ms: Option<i64>,
) -> Result<Option<Vec<ModelDefinition>>, String> {
    let Some(wire) = wire_for(config) else {
        return Ok(None);
    };

    let credential = credentials::resolve(provider_id, Some(config), auth, manifest, values);
    // Never probe a vendor's public endpoint without credentials. A custom base
    // URL may be an unauthenticated local server, so it is always attempted.
    if credential.key.is_none()
        && config.base_url.is_none()
        && credential.source == CredentialSource::Missing
    {
        return Ok(None);
    }

    let base = config
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url(wire).to_string());
    let url = models_url(wire, &base);
    let headers = request_headers(wire, config, &credential, values);
    let client = tom_ai::http::build_client(&url, idle_timeout_ms, None)
        .map_err(|error| format!("could not build HTTP client: {error}"))?;

    let models = match wire {
        Wire::Anthropic => fetch_anthropic(&client, &url, &headers).await?,
        Wire::OpenAi => {
            let body = get_json(&client, &url, &headers).await?;
            parse_models(&body)
        }
    };
    Ok(Some(models))
}

fn request_headers(
    wire: Wire,
    config: &ProviderConfig,
    credential: &credentials::ResolvedCredential,
    values: &dyn ValueSource,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(configured) = &config.headers {
        headers.extend(resolve_headers(configured, None, values));
    }
    if let Some(key) = &credential.key {
        match wire {
            Wire::Anthropic if credential.bearer => {
                remove_header(&mut headers, "x-api-key");
                headers.insert("authorization".to_string(), format!("Bearer {key}"));
            }
            Wire::Anthropic => {
                remove_header(&mut headers, "x-api-key");
                headers.insert("x-api-key".to_string(), key.clone());
            }
            Wire::OpenAi => {
                remove_header(&mut headers, "authorization");
                headers.insert("authorization".to_string(), format!("Bearer {key}"));
            }
        }
    }
    if wire == Wire::Anthropic {
        headers
            .entry("anthropic-version".to_string())
            .or_insert_with(|| ANTHROPIC_VERSION.to_string());
    }
    if !headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("user-agent"))
    {
        headers.insert("user-agent".to_string(), tom_ai::http::user_agent());
    }
    headers
}

fn remove_header(headers: &mut BTreeMap<String, String>, name: &str) {
    headers.retain(|existing, _| !existing.eq_ignore_ascii_case(name));
}

/// Build the catalog endpoint for a provider's base URL.
///
/// * Anthropic appends `/v1/models` to the host root, matching `/v1/messages`.
/// * OpenAI-compatible bases either already name a version segment (`/v1`,
///   `/v3`, `/v1beta`) or are a host root that needs `/v1` added.
fn models_url(wire: Wire, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match wire {
        Wire::Anthropic => format!("{base}/v1/models"),
        Wire::OpenAi => {
            if let Some(root) = base.strip_suffix("/chat/completions") {
                format!("{root}/models")
            } else if base.rsplit('/').next().is_some_and(is_version_segment) {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            }
        }
    }
}

/// Mirrors the OpenAI provider's `is_version_segment`: `v1`, `v3`, `v1beta`.
fn is_version_segment(segment: &str) -> bool {
    let Some(digits) = segment.strip_prefix('v') else {
        return false;
    };
    let digit_count = digits.chars().take_while(|ch| ch.is_ascii_digit()).count();
    digit_count > 0
        && digits
            .chars()
            .skip(digit_count)
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

async fn fetch_anthropic(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<Vec<ModelDefinition>, String> {
    let mut models = Vec::new();
    let mut after_id: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page_url = with_query(url, after_id.as_deref());
        let body = get_json(client, &page_url, headers).await?;
        models.extend(parse_models(&body));

        let has_more = body
            .get("has_more")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let last_id = body
            .get("last_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        match last_id {
            Some(last_id) if has_more => after_id = Some(last_id),
            _ => break,
        }
    }
    Ok(models)
}

fn with_query(url: &str, after_id: Option<&str>) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    {
        let mut query = parsed.query_pairs_mut();
        query.append_pair("limit", "1000");
        if let Some(after) = after_id {
            query.append_pair("after_id", after);
        }
    }
    parsed.to_string()
}

async fn get_json(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    let mut request = client.get(url).timeout(DISCOVERY_TIMEOUT);
    for (name, value) in headers {
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid header name {name:?}"))?;
        let header_value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header {name:?}"))?;
        request = request.header(header_name, header_value);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let snippet = truncate(body.trim(), 200);
        return Err(if snippet.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {snippet}")
        });
    }
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("invalid JSON response: {error}"))
}

/// Extract model ids from a catalog response. Accepts the shapes used by
/// OpenAI-compatible gateways (`data` or `models` arrays, or a bare array) and
/// Anthropic (`data` with `display_name`).
fn parse_models(body: &serde_json::Value) -> Vec<ModelDefinition> {
    let Some(items) = body
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| body.get("models").and_then(serde_json::Value::as_array))
        .or_else(|| body.as_array())
    else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for item in items {
        let (id, name) = match item {
            serde_json::Value::String(id) => (id.as_str(), None),
            serde_json::Value::Object(fields) => {
                let Some(id) = fields.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let name = fields
                    .get("display_name")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| fields.get("name").and_then(serde_json::Value::as_str));
                (id, name)
            }
            _ => continue,
        };
        let id = id.trim();
        if id.is_empty() || !seen.insert(id.to_ascii_lowercase()) {
            continue;
        }
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != id)
            .map(str::to_string);
        models.push(ModelDefinition {
            id: id.to_string(),
            name,
            base_url: None,
            reasoning: None,
            thinking_level_map: None,
            context_window: None,
            max_tokens: None,
            cost: None,
            input: None,
            headers: None,
            compat: None,
            truncation_policy: None,
        });
    }
    models.sort_by(|a, b| {
        a.id.to_ascii_lowercase()
            .cmp(&b.id.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    models
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_models_url_handles_versioned_and_host_roots() {
        assert_eq!(
            models_url(Wire::OpenAi, "https://api.openai.com"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_url(Wire::OpenAi, "https://gateway.test/v1/"),
            "https://gateway.test/v1/models"
        );
        assert_eq!(
            models_url(Wire::OpenAi, "https://gateway.test/api/coding/v3"),
            "https://gateway.test/api/coding/v3/models"
        );
        assert_eq!(
            models_url(Wire::OpenAi, "https://gateway.test/custom/chat/completions"),
            "https://gateway.test/custom/models"
        );
    }

    #[test]
    fn anthropic_models_url_appends_v1() {
        assert_eq!(
            models_url(Wire::Anthropic, "https://api.anthropic.com"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_url(Wire::Anthropic, "https://gateway.test/"),
            "https://gateway.test/v1/models"
        );
    }

    #[test]
    fn parse_models_accepts_common_shapes_and_dedupes() {
        let body = serde_json::json!({
            "data": [
                { "id": "gpt-a", "name": "GPT A" },
                { "id": "GPT-A" },
                { "id": "gpt-b", "display_name": "Display B" },
                { "name": "ignored" }
            ]
        });
        let models = parse_models(&body);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gpt-a", "gpt-b"]
        );
        assert_eq!(models[0].name.as_deref(), Some("GPT A"));
        assert_eq!(models[1].name.as_deref(), Some("Display B"));
    }

    #[test]
    fn parse_models_accepts_bare_and_models_arrays() {
        let bare = serde_json::json!(["m1", "m2"]);
        assert_eq!(parse_models(&bare).len(), 2);

        let nested = serde_json::json!({ "models": [{ "id": "m3" }] });
        assert_eq!(parse_models(&nested)[0].id, "m3");
    }

    #[test]
    fn only_providers_without_models_are_discovered() {
        let mut provider = ProviderConfig {
            name: None,
            base_url: Some("https://gateway.test/v1".into()),
            api_key: None,
            api: Some("openai-completions".into()),
            headers: None,
            auth_header: None,
            models: Vec::new(),
        };
        assert!(needs_discovery(&provider));

        provider.models.push(ModelDefinition {
            id: "manual".into(),
            name: None,
            base_url: None,
            reasoning: None,
            thinking_level_map: None,
            context_window: None,
            max_tokens: None,
            cost: None,
            input: None,
            headers: None,
            compat: None,
            truncation_policy: None,
        });
        assert!(!needs_discovery(&provider));
    }

    #[test]
    fn cached_catalogs_are_reused_only_within_the_refresh_window() {
        let now = 1_000_000;
        let mut discovered = config::DiscoveredModels::default();
        assert!(!cache_is_fresh(&discovered, "missing", now));

        discovered.providers.insert(
            "gateway".into(),
            DiscoveredProvider {
                fetched_at: now - 60,
                models: Vec::new(),
            },
        );
        // Failures are cached too: an empty entry still counts as fresh.
        assert!(cache_is_fresh(&discovered, "gateway", now));

        discovered.providers.get_mut("gateway").unwrap().fetched_at = now - REFRESH_INTERVAL_SECS;
        assert!(!cache_is_fresh(&discovered, "gateway", now));
    }

    async fn serve_once(
        body: &'static str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let count = socket.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
            String::from_utf8_lossy(&bytes).to_string()
        });
        (address, server)
    }

    #[tokio::test]
    async fn fetch_models_reads_openai_catalog_with_bearer_auth() {
        let (address, server) =
            serve_once(r#"{"data":[{"id":"gpt-x","name":"GPT X"},{"id":"gpt-y"}]}"#).await;
        let provider = ProviderConfig {
            name: None,
            base_url: Some(format!("http://{address}/v1")),
            api_key: Some("secret".into()),
            api: Some("openai-completions".into()),
            headers: None,
            auth_header: None,
            models: Vec::new(),
        };
        let values: BTreeMap<String, String> = BTreeMap::new();
        let models = fetch_models(
            "gateway",
            &provider,
            None,
            &config::AuthStore::default(),
            &values,
            None,
        )
        .await
        .unwrap()
        .expect("discovery should run");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gpt-x", "gpt-y"]
        );

        let request = server.await.unwrap();
        assert!(
            request.starts_with("GET /v1/models "),
            "unexpected request: {request}"
        );
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer secret"));
    }

    #[tokio::test]
    async fn fetch_models_uses_anthropic_headers_and_pagination_path() {
        let (address, server) = serve_once(
            r#"{"data":[{"id":"claude-x","display_name":"Claude X"}],"has_more":false}"#,
        )
        .await;
        let provider = ProviderConfig {
            name: None,
            base_url: Some(format!("http://{address}")),
            api_key: Some("secret".into()),
            api: Some("anthropic-messages".into()),
            headers: None,
            auth_header: None,
            models: Vec::new(),
        };
        let values: BTreeMap<String, String> = BTreeMap::new();
        let models = fetch_models(
            "gateway",
            &provider,
            None,
            &config::AuthStore::default(),
            &values,
            None,
        )
        .await
        .unwrap()
        .expect("discovery should run");
        assert_eq!(models[0].id, "claude-x");
        assert_eq!(models[0].name.as_deref(), Some("Claude X"));

        let request = server.await.unwrap();
        assert!(
            request.starts_with("GET /v1/models?limit=1000 "),
            "unexpected request: {request}"
        );
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("x-api-key: secret"));
        assert!(lower.contains("anthropic-version: 2023-06-01"));
    }
}
