use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Error;

const VERSION: &str = "2026-10-21";
const BETA: &str = "server-side-fallback-2026-07-01";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const MAX_RETRIES: u32 = 3;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

pub struct ModelRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub effort: Option<&'a str>,
    /// `{name, description, input_schema}` objects.
    pub tools: &'a [Value],
    /// Append-only history.
    pub messages: &'a [Value],
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, rename = "cache_read_input_tokens")]
    pub cache_read_tokens: u64,
    #[serde(default, rename = "cache_creation_input_tokens")]
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelResponse {
    /// Kept as raw JSON so it can be echoed back unchanged (thinking and fallback blocks
    /// included).
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
    pub stop_details: Option<Value>,
    #[serde(default)]
    pub usage: Usage,
}

#[async_trait]
pub trait Model: Send + Sync {
    async fn complete(&self, req: &ModelRequest<'_>) -> Result<ModelResponse, Error>;
}

enum Auth {
    ApiKey(String),
    Bearer(String),
}

/// Raw HTTP client for the Anthropic Messages API.
pub struct HttpModel {
    client: reqwest::Client,
    base_url: String,
    auth: Auth,
}

impl HttpModel {
    /// Reads `ANTHROPIC_API_KEY`, else `ANTHROPIC_AUTH_TOKEN`; `ANTHROPIC_BASE_URL` is optional.
    pub fn from_env() -> Result<Self, Error> {
        let get = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
        let auth = match (get("ANTHROPIC_API_KEY"), get("ANTHROPIC_AUTH_TOKEN")) {
            (Some(k), _) => Auth::ApiKey(k),
            (None, Some(t)) => Auth::Bearer(t),
            (None, None) => {
                return Err(Error::Model(
                    "no credentials: set ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) to run tasks; lint-only needs neither".into(),
                ));
            }
        };
        let base_url =
            get("ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into());
        Ok(Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').into(),
            auth,
        })
    }

    fn body(req: &ModelRequest<'_>) -> Value {
        let mut body = json!({
            "model": req.model,
            "max_tokens": 16000,
            "system": req.system,
            "tools": req.tools,
            "messages": req.messages,
            "fallbacks": "default",
            "cache_control": {"type": "ephemeral"},
        });
        if let Some(effort) = req.effort {
            body["output_config"] = json!({"effort": effort});
        }
        body
    }
}

#[async_trait]
impl Model for HttpModel {
    async fn complete(&self, req: &ModelRequest<'_>) -> Result<ModelResponse, Error> {
        let body = Self::body(req);
        let mut attempt = 0;
        loop {
            let mut rb = self
                .client
                .post(format!("{}/v1/messages", self.base_url))
                .header("content-type", "application/json")
                .header("anthropic-version", VERSION)
                .json(&body);
            rb = match &self.auth {
                Auth::ApiKey(k) => rb.header("x-api-key", k).header("anthropic-beta", BETA),
                Auth::Bearer(t) => rb
                    .header("authorization", format!("Bearer {t}"))
                    .header("anthropic-beta", format!("{BETA},{OAUTH_BETA}")),
            };
            let resp = rb
                .send()
                .await
                .map_err(|e| Error::Model(format!("request failed: {e}")))?;
            let status = resp.status();
            if status.is_success() {
                return resp
                    .json()
                    .await
                    .map_err(|e| Error::Model(format!("bad response body: {e}")));
            }
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let text = resp.text().await.unwrap_or_default();
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if !retryable || attempt >= MAX_RETRIES {
                return Err(Error::Model(format!("HTTP {status}: {text}")));
            }
            let wait = retry_after
                .unwrap_or(Duration::from_secs(1 << attempt))
                .min(MAX_BACKOFF);
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_required_fields_and_no_forbidden_ones() {
        let req = ModelRequest {
            model: "m",
            system: "s",
            effort: Some("medium"),
            tools: &[],
            messages: &[],
        };
        let b = HttpModel::body(&req);
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["cache_control"]["type"], "ephemeral");
        assert_eq!(b["max_tokens"], 16000);
        for k in ["tool_choice", "temperature", "thinking"] {
            assert!(b.get(k).is_none(), "{k}");
        }
        let req = ModelRequest {
            effort: None,
            ..req
        };
        assert!(HttpModel::body(&req).get("output_config").is_none());
    }
}
