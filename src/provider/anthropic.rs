use super::{ApiMessage, AiProvider, ContentBlock, ProviderResponse, ProviderUsage, ToolDef};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
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

        let http = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;

        let status = http.status();
        let raw = http.text().await?;
        if !status.is_success() {
            let snippet: String = raw.chars().take(2000).collect();
            bail!("Anthropic API error ({}): {}", status, snippet);
        }

        let parsed: ResBody = serde_json::from_str(&raw)?;
        let usage = parsed.usage.unwrap_or_default();
        Ok(ProviderResponse {
            blocks: parsed.content,
            stop_reason: parsed.stop_reason.unwrap_or_else(|| "end_turn".into()),
            usage: ProviderUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            },
        })
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
