use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

/// Konfigurasi runtime — semua via env var (Pilar 8: provider via config, bukan kode).
#[derive(Clone, Debug)]
pub struct Config {
    pub telegram_bot_token: String,
    pub database_url: String,
    pub anthropic_api_key: String,
    pub anthropic_model: String,
    /// Base URL provider — default api.anthropic.com; bisa diganti ke endpoint
    /// Anthropic-compatible milik provider lain (mis. GLM: open.bigmodel.cn/api/anthropic).
    pub anthropic_base_url: String,
    pub ai_provider: String,
    /// Hard allowlist chat id (Pilar 9 keamanan #1) — pesan dari luar ini di-drop total.
    pub allowed_chat_ids: Vec<i64>,
    /// N pesan terakhir sebagai context (Pilar 2) — jangan kirim full history.
    pub n_context: i64,
    pub soul_path: String,
    /// Root workdir utk read_file/write_file + cwd default run_command (Pilar 9).
    pub work_roots: Vec<PathBuf>,
    /// Timeout eksekusi per shell command (detik).
    pub run_cmd_timeout: u64,
    /// Timeout menunggu approval destructive command (detik).
    pub confirm_timeout: u64,
    /// Backend pencarian web (Pilar 10 — pola sama seperti AI_PROVIDER). Default: tavily.
    pub search_provider: String,
    /// API key Tavily — tanpa ini tool web_search balas pesan konfigurasi.
    pub tavily_api_key: Option<String>,
    /// Timeout per request fetch_url (detik).
    pub fetch_timeout: u64,
    /// Timeout generate_image (detik) — generasi gambar bisa lambat.
    pub image_timeout: u64,
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
            anthropic_base_url: env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".into())
                .trim_end_matches('/')
                .to_string(),
            ai_provider: env::var("AI_PROVIDER").unwrap_or_else(|_| "anthropic".into()),
            allowed_chat_ids,
            n_context: env::var("N_CONTEXT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            soul_path: env::var("SOUL_PATH").unwrap_or_else(|_| "soul.md".into()),
            work_roots: env::var("WORK_ROOTS")
                .ok()
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim())
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    vec![env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
                })
                .into_iter()
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
                .collect(),
            run_cmd_timeout: env::var("RUN_CMD_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
            confirm_timeout: env::var("CONFIRM_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            search_provider: env::var("SEARCH_PROVIDER")
                .unwrap_or_else(|_| "tavily".into())
                .to_ascii_lowercase(),
            tavily_api_key: env::var("TAVILY_API_KEY")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            fetch_timeout: env::var("FETCH_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            image_timeout: env::var("IMAGE_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
        })
    }
}
