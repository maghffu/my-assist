use super::{ApiMessage, AiProvider, ContentBlock, ProviderResponse, ProviderUsage, ToolDef};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const ANTHROPIC_VERSION: &str = "2023-06-01";
/// 4096 terlalu kecil — jawaban teknis panjang sering terpotong (stop_reason
/// max_tokens) dan owner hanya melihat sebagian balasan.
const MAX_TOKENS: u32 = 8192;
/// Timeout per call HTTP — TANPA ini API yang hang membuat turn diam selamanya
/// ("ngomong sama tembok"): tidak ada error, tidak ada balasan.
const HTTP_TIMEOUT: Duration = Duration::from_secs(240);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Retry utk error sementara (429 rate-limit / 5xx / timeout) — backoff kuadrat 2s/8s.
/// Total worst-case hang tetap terikat (3×240s + 10s) — gateway punya turn timeout
/// sendiri sebagai pagar terakhir.
const MAX_ATTEMPTS: u32 = 3;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("reqwest client valid"),
            api_url: format!("{}/v1/messages", base_url.trim_end_matches('/')),
            api_key,
            model,
        }
    }
}

#[derive(Serialize)]
struct ReqBody<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [ApiMessage],
    #[serde(skip_serializing_if = "<[ToolDef]>::is_empty")]
    tools: &'a [ToolDef],
}

#[derive(Deserialize)]
struct ResBody {
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageDto>,
}

#[derive(Deserialize, Default)]
struct UsageDto {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn chat(
        &self,
        system: &str,
        messages: &[ApiMessage],
        tools: &[ToolDef],
    ) -> Result<ProviderResponse> {
        let body = ReqBody {
            model: &self.model,
            max_tokens: MAX_TOKENS,
            system,
            messages,
            tools,
        };

        // Retry loop: error sementara (429/5xx/timeout koneksi) jangan langsung
        // dibanting ke owner sebagai "⚠️ Error" — coba lagi dulu dengan backoff.
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.call_once(&body).await {
                Ok((status, raw)) if status.is_success() => {
                    let parsed: ResBody = serde_json::from_str(&raw)?;
                    let usage = parsed.usage.unwrap_or_default();
                    return Ok(ProviderResponse {
                        blocks: parsed.content,
                        stop_reason: parsed.stop_reason.unwrap_or_else(|| "end_turn".into()),
                        usage: ProviderUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        },
                    });
                }
                Ok((status, raw)) => {
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if retryable && attempt < MAX_ATTEMPTS {
                        let wait = Duration::from_secs(2 * u64::from(attempt) * u64::from(attempt));
                        tracing::warn!(
                            attempt,
                            status = %status,
                            "Anthropic API error sementara — retry dalam {:?}",
                            wait
                        );
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let snippet: String = raw.chars().take(2000).collect();
                    bail!("Anthropic API error ({}): {}", status, snippet);
                }
                Err(e) => {
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| re.is_timeout() || re.is_connect())
                        .unwrap_or(false);
                    if retryable && attempt < MAX_ATTEMPTS {
                        let wait = Duration::from_secs(2 * u64::from(attempt) * u64::from(attempt));
                        tracing::warn!(attempt, "HTTP call gagal ({e:#}) — retry dalam {:?}", wait);
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl AnthropicProvider {
    /// Satu HTTP call tanpa retry — pemanggilnya yang mengelola attempt/backoff.
    async fn call_once(&self, body: &ReqBody<'_>) -> Result<(reqwest::StatusCode, String)> {
        let http = self
            .client
            .post(&self.api_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(body)
            .send()
            .await?;
        let status = http.status();
        let raw = http.text().await?;
        Ok((status, raw))
    }
}
