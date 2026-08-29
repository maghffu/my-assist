use anyhow::{Context, Result};
use std::env;

/// Konfigurasi runtime — semua via env var (Pilar 8: provider via config, bukan kode).
#[derive(Clone, Debug)]
pub struct Config {
    pub telegram_bot_token: String,
    pub database_url: String,
    pub anthropic_api_key: String,
    pub anthropic_model: String,
    pub ai_provider: String,
    /// Hard allowlist chat id (Pilar 9 keamanan #1) — pesan dari luar ini di-drop total.
    pub allowed_chat_ids: Vec<i64>,
    /// N pesan terakhir sebagai context (Pilar 2) — jangan kirim full history.
    pub n_context: i64,
    pub soul_path: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let telegram_bot_token = env::var("TELEGRAM_BOT_TOKEN")
            .context("env TELEGRAM_BOT_TOKEN belum di-set (dari @BotFather)")?;
        let database_url =
            env::var("DATABASE_URL").context("env DATABASE_URL belum di-set")?;
        let anthropic_api_key = env::var("ANTHROPIC_API_KEY")
            .context("env ANTHROPIC_API_KEY belum di-set")?;

        let allowed_raw = env::var("ALLOWED_CHAT_ID")
            .context("env ALLOWED_CHAT_ID belum di-set (chat id Telegram owner)")?;
        let allowed_chat_ids = allowed_raw
            .split(',')
            .map(|s| s.trim().parse::<i64>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("ALLOWED_CHAT_ID harus angka (atau daftar angka dipisah koma)")?;
        if allowed_chat_ids.is_empty() {
            anyhow::bail!("ALLOWED_CHAT_ID tidak boleh kosong");
        }

        Ok(Self {
            telegram_bot_token,
            database_url,
            anthropic_api_key,
            anthropic_model: env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-6".into()),
            ai_provider: env::var("AI_PROVIDER").unwrap_or_else(|_| "anthropic".into()),
            allowed_chat_ids,
            n_context: env::var("N_CONTEXT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            soul_path: env::var("SOUL_PATH").unwrap_or_else(|_| "soul.md".into()),
        })
    }
}
