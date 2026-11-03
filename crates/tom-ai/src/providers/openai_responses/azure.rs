//! Azure's three deviations from the OpenAI Responses protocol.
//!
//! Azure serves the same `/responses` wire format, so the transcript converter,
//! tool schemas and SSE handling are shared with [`super`] rather than copied.
//! What is genuinely different is small, and all of it lives here:
//!
//! 1. **Endpoint.** An Azure model ships without a usable `baseUrl` (one
//!    resource per user, resolved at request time) and its paths are
//!    `/openai/v1/...`, not `/v1/...`.
//! 2. **Deployment names.** Azure routes by *deployment*, which is usually — but
//!    not always — named after the model. The wire `model` field is the
//!    deployment; `model.id` stays the catalog id so `/model`, cost tables and
//!    session records keep naming what the user chose.
//! 3. **`api-version`.** A query parameter, not a path segment.
//!
//! Mirrors `packages/ai/src/api/azure-openai-config.ts`.

use super::Model;

/// Default `api-version`. `v1` is Azure's stable-endpoint contract; a dated
/// value (`2024-10-21`) still works when a deployment needs one.
pub const DEFAULT_API_VERSION: &str = "v1";

pub const ENV_BASE_URL: &str = "AZURE_OPENAI_BASE_URL";
pub const ENV_RESOURCE_NAME: &str = "AZURE_OPENAI_RESOURCE_NAME";
pub const ENV_API_VERSION: &str = "AZURE_OPENAI_API_VERSION";
pub const ENV_DEPLOYMENT_MAP: &str = "AZURE_OPENAI_DEPLOYMENT_NAME_MAP";

/// Per-request Azure overrides. Carried on the provider (not in
/// `SimpleStreamOptions`) because they are provider configuration, like the
/// base URL, rather than a per-call toggle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AzureOptions {
    pub base_url: Option<String>,
    pub resource_name: Option<String>,
    pub api_version: Option<String>,
    /// Send this deployment name regardless of the model id.
    pub deployment_name: Option<String>,
    /// `modelId=deploymentName,...`, the same shape as the env var.
    pub deployment_map: Option<String>,
}

/// A resolved endpoint plus the version to put in the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureEndpoint {
    pub base_url: String,
    pub api_version: String,
}

impl AzureEndpoint {
    /// The request URL: `<base>/responses?api-version=<version>`.
    pub fn responses_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.api_version.is_empty() {
            format!("{base}/responses")
        } else {
            format!(
                "{base}/responses?api-version={}",
                urlencode(&self.api_version)
            )
        }
    }
}

/// Whether `host` is one of Azure's OpenAI hosts, where a bare path means
/// `/openai/v1` rather than a gateway root the user chose deliberately.
fn is_azure_host(host: &str) -> bool {
    [
        "openai.azure.com",
        "cognitiveservices.azure.com",
        "ai.azure.com",
    ]
    .iter()
    .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Normalize an Azure base URL.
///
/// Trailing slashes are dropped. On a recognized Azure host a bare root — or a
/// path that already names the responses endpoint — becomes `/openai/v1`, and
/// any query is dropped: a `?api-version=` copied from the portal would
/// otherwise be sent twice, contradicting the version we resolve. A non-Azure
/// host (a gateway or proxy) is preserved verbatim, query included.
pub fn normalize_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    let without_slash = trimmed.trim_end_matches('/');
    let mut parsed = reqwest::Url::parse(without_slash)
        .map_err(|_| format!("Invalid Azure OpenAI base URL: {base_url}"))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !is_azure_host(&host) {
        return Ok(without_slash.to_string());
    }
    let path = parsed.path().trim_end_matches('/');
    if path.is_empty() || path == "/openai" || path == "/openai/v1/responses" {
        parsed.set_path("/openai/v1");
    }
    parsed.set_query(None);
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

/// Parse `modelId=deploymentName,...`. A malformed fragment is skipped rather
/// than failing the request: one bad pair must not cost the user their model.
pub fn parse_deployment_map(value: Option<&str>) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let Some(value) = value else {
        return map;
    };
    for entry in value.split(',') {
        let Some((id, deployment)) = entry.split_once('=') else {
            continue;
        };
        let (id, deployment) = (id.trim(), deployment.trim());
        if id.is_empty() || deployment.is_empty() {
            continue;
        }
        map.insert(id.to_string(), deployment.to_string());
    }
    map
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Which deployment serves this model: an explicit name, then the map, then the
/// model id itself.
pub fn resolve_deployment_name(model_id: &str, options: &AzureOptions) -> String {
    if let Some(name) = options
        .deployment_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return name.to_string();
    }
    let map = options
        .deployment_map
        .as_deref()
        .map(|value| parse_deployment_map(Some(value)))
        .filter(|map| !map.is_empty())
        .unwrap_or_else(|| parse_deployment_map(env(ENV_DEPLOYMENT_MAP).as_deref()));
    map.get(model_id)
        .cloned()
        .unwrap_or_else(|| model_id.to_string())
}

/// Resolve the endpoint: the provider's base URL, its resource name, the
/// matching environment variables, then the model's own `baseUrl`.
pub fn resolve_endpoint(
    model_base_url: &str,
    options: &AzureOptions,
) -> Result<AzureEndpoint, String> {
    let explicit = options
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| env(ENV_BASE_URL));
    let resource = options
        .resource_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| env(ENV_RESOURCE_NAME));

    let resolved = explicit
        .or_else(|| resource.map(|name| format!("https://{name}.openai.azure.com/openai/v1")))
        .or_else(|| {
            let base = model_base_url.trim();
            (!base.is_empty()).then(|| base.to_string())
        })
        .ok_or_else(|| {
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or \
             AZURE_OPENAI_RESOURCE_NAME, or configure baseUrl for this model."
                .to_string()
        })?;

    Ok(AzureEndpoint {
        base_url: normalize_base_url(&resolved)?,
        api_version: options
            .api_version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| env(ENV_API_VERSION))
            .unwrap_or_else(|| DEFAULT_API_VERSION.to_string()),
    })
}

/// The model to put on the wire plus the endpoint to send it to.
///
/// The returned model keeps every field of the caller's model except `id`
/// (replaced by the deployment name) and `base_url` (replaced by the resolved
/// endpoint), so the body builder emits the deployment while cost tables, the
/// system prompt and the session keep the catalog id.
pub fn resolve_request(
    model: &Model,
    options: &AzureOptions,
) -> Result<(Model, AzureEndpoint), String> {
    let endpoint = resolve_endpoint(&model.base_url, options)?;
    let mut wire = model.clone();
    wire.id = resolve_deployment_name(&model.id, options);
    wire.base_url = endpoint.base_url.clone();
    Ok((wire, endpoint))
}

/// Percent-encode a query value. Azure versions are plain (`v1`,
/// `2024-10-21`), so this only has to be safe, not clever.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Api;

    /// Environment variables are process-global, so the tests that set them run
    /// under one lock and clear what they set.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_env() {
        for name in [
            ENV_BASE_URL,
            ENV_RESOURCE_NAME,
            ENV_API_VERSION,
            ENV_DEPLOYMENT_MAP,
        ] {
            std::env::remove_var(name);
        }
    }

    fn model(base_url: &str) -> Model {
        Model::new(
            "gpt-4o-mini",
            "GPT-4o mini",
            Api::AzureOpenaiResponses,
            "azure",
            base_url,
        )
    }

    #[test]
    fn normalizes_azure_host_roots_to_the_openai_v1_path() {
        for (input, expected) in [
            (
                "https://my-resource.openai.azure.com",
                "https://my-resource.openai.azure.com/openai/v1",
            ),
            (
                "https://my-resource.cognitiveservices.azure.com/",
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://my-resource.ai.azure.com",
                "https://my-resource.ai.azure.com/openai/v1",
            ),
            (
                "https://my-resource.cognitiveservices.azure.com/openai",
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://my-resource.services.ai.azure.com/openai/v1/responses",
                "https://my-resource.services.ai.azure.com/openai/v1",
            ),
            // An already-correct path is preserved.
            (
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
                "https://my-resource.cognitiveservices.azure.com/openai/v1",
            ),
            // A stale api-version from a copied portal URL is dropped.
            (
                "https://my-resource.openai.azure.com/openai?api-version=2024-12-01",
                "https://my-resource.openai.azure.com/openai/v1",
            ),
            // A gateway is not Azure and keeps its path and query.
            (
                "https://my-proxy.example.com/v1",
                "https://my-proxy.example.com/v1",
            ),
            (
                "https://my-proxy.example.com/v1?custom=true",
                "https://my-proxy.example.com/v1?custom=true",
            ),
        ] {
            assert_eq!(
                normalize_base_url(input).unwrap(),
                expected,
                "input: {input}"
            );
        }
    }

    #[test]
    fn rejects_a_url_that_is_not_a_url() {
        let error = normalize_base_url("not-a-url").unwrap_err();
        assert!(error.contains("Invalid Azure OpenAI base URL"), "{error}");
    }

    #[test]
    fn a_deployment_map_maps_ids_and_skips_malformed_pairs() {
        let map = parse_deployment_map(Some("gpt-4o-mini=prod-mini, ,broken,x=y"));
        assert_eq!(
            map.get("gpt-4o-mini").map(String::as_str),
            Some("prod-mini")
        );
        assert_eq!(map.get("x").map(String::as_str), Some("y"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn the_deployment_defaults_to_the_model_id() {
        let options = AzureOptions::default();
        assert_eq!(
            resolve_deployment_name("gpt-4o-mini", &options),
            "gpt-4o-mini"
        );
    }

    #[test]
    fn an_explicit_deployment_wins_over_the_map() {
        let options = AzureOptions {
            deployment_name: Some("explicit".into()),
            deployment_map: Some("gpt-4o-mini=mapped".into()),
            ..Default::default()
        };
        assert_eq!(resolve_deployment_name("gpt-4o-mini", &options), "explicit");
    }

    #[test]
    fn the_endpoint_comes_from_the_resource_name_then_the_model() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();

        let from_resource = AzureOptions {
            resource_name: Some("my-resource".into()),
            ..Default::default()
        };
        let endpoint = resolve_endpoint("https://ignored.example/v1", &from_resource).unwrap();
        assert_eq!(
            endpoint.base_url,
            "https://my-resource.openai.azure.com/openai/v1"
        );
        assert_eq!(endpoint.api_version, DEFAULT_API_VERSION);

        // No option and no env: the model's own baseUrl is the last resort.
        let endpoint =
            resolve_endpoint("https://model.example/openai/v1", &AzureOptions::default()).unwrap();
        assert_eq!(endpoint.base_url, "https://model.example/openai/v1");
        clear_env();
    }

    #[test]
    fn a_missing_endpoint_is_an_actionable_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        let error = resolve_endpoint("", &AzureOptions::default()).unwrap_err();
        assert!(error.contains("AZURE_OPENAI_BASE_URL"), "{error}");
        assert!(error.contains("baseUrl"), "{error}");
        clear_env();
    }

    #[test]
    fn the_environment_supplies_endpoint_version_and_deployment() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var(ENV_BASE_URL, "https://env-resource.openai.azure.com");
        std::env::set_var(ENV_API_VERSION, "2024-10-21");
        std::env::set_var(ENV_DEPLOYMENT_MAP, "gpt-4o-mini=env-deployment");

        let (wire, endpoint) = resolve_request(&model(""), &AzureOptions::default()).unwrap();
        assert_eq!(
            endpoint.base_url,
            "https://env-resource.openai.azure.com/openai/v1"
        );
        assert_eq!(endpoint.api_version, "2024-10-21");
        assert_eq!(wire.id, "env-deployment");
        clear_env();
    }

    #[test]
    fn the_request_url_appends_the_version_as_a_query() {
        let endpoint = AzureEndpoint {
            base_url: "https://r.openai.azure.com/openai/v1".into(),
            api_version: "2024-10-21".into(),
        };
        assert_eq!(
            endpoint.responses_url(),
            "https://r.openai.azure.com/openai/v1/responses?api-version=2024-10-21"
        );
        let unversioned = AzureEndpoint {
            base_url: "https://r.openai.azure.com/openai/v1/".into(),
            api_version: String::new(),
        };
        assert_eq!(
            unversioned.responses_url(),
            "https://r.openai.azure.com/openai/v1/responses"
        );
    }

    #[test]
    fn the_wire_model_carries_the_deployment_but_keeps_the_catalog_id_elsewhere() {
        let options = AzureOptions {
            base_url: Some("https://r.openai.azure.com".into()),
            deployment_name: Some("prod".into()),
            ..Default::default()
        };
        let original = model("https://ignored.example");
        let (wire, _) = resolve_request(&original, &options).unwrap();
        assert_eq!(wire.id, "prod", "the wire model is the deployment");
        assert_eq!(
            original.id, "gpt-4o-mini",
            "the caller's model is untouched"
        );
        assert_eq!(wire.provider, original.provider);
        assert_eq!(
            wire.base_url, "https://r.openai.azure.com/openai/v1",
            "the endpoint replaces the placeholder base URL"
        );
    }
}
