pub mod anthropic;

use crate::config::Config;
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Content block universal — format Anthropic-style; provider lain melakukan adaptasi
/// di layer impl masing-masing (Pilar 8: agent core tidak tahu detail provider).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: String },
    /// Blok reasoning model (GLM dan extended-thinking Anthropic) — tidak ditampilkan
    /// ke user; dikirim balik apa adanya (signature wajib utk beberapa provider).
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct ProviderResponse {
    pub blocks: Vec<ContentBlock>,
    pub stop_reason: String,
    pub usage: ProviderUsage,
}

/// Opsi per-call untuk jalur internal (dream/review/summary — Pilar 6):
/// override max_tokens & thinking effort tanpa menyentuh jalur chat utama.
/// Reasoning model (GLM) dengan effort tinggi bisa memakan seluruh budget
/// output untuk thinking hingga TIDAK ada text block — insiden /dream 5 Sep.
#[derive(Clone, Debug, Default)]
pub struct CallOpts {
    /// Override max_tokens (None = default provider, anthropic.rs MAX_TOKENS).
    pub max_tokens: Option<u32>,
    /// Override effort/thinking intensity (None = pakai effort config provider).
    pub effort: Option<String>,
}

/// Trait abstraction lintas AI provider (Pilar 8).
#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn chat(
        &self,
        system: &str,
        messages: &[ApiMessage],
        tools: &[ToolDef],
    ) -> Result<ProviderResponse>;

    /// Varian `chat` dengan opsi per-call. Default: delegasi ke `chat` —
    /// provider yang belum mendukung override mengabaikan opsinya, jadi
    /// caller internal aman memanggil ini di provider manapun (Pilar 8).
    async fn chat_opts(
        &self,
        system: &str,
        messages: &[ApiMessage],
        tools: &[ToolDef],
        opts: &CallOpts,
    ) -> Result<ProviderResponse> {
        let _ = opts; // provider tanpa dukungan: opsi diabaikan, bukan error
        self.chat(system, messages, tools).await
    }
    fn name(&self) -> &'static str;
    fn model_name(&self) -> &str;
}

/// Factory — provider aktif dipilih via `AI_PROVIDER` env, bukan ganti kode.
pub fn build(cfg: &Config) -> Result<Arc<dyn AiProvider>> {
    match cfg.ai_provider.as_str() {
        "anthropic" => Ok(Arc::new(anthropic::AnthropicProvider::new(
            cfg.anthropic_base_url.clone(),
            cfg.anthropic_api_key.clone(),
            cfg.anthropic_model.clone(),
            cfg.anthropic_effort.clone(),
        ))),
        other => bail!(
            "provider tidak dikenal: {:?} (tersedia: anthropic). Set AI_PROVIDER.",
            other
        ),
    }
}
