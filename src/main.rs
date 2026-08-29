mod agent;
mod config;
mod context;
mod db;
mod gateway;
mod memory;
mod provider;
mod reminders;
mod shell;
mod soul;
mod tools;

use anyhow::Result;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Mode khusus: `hermes-lite migrate` -> jalankan migrasi DB saja lalu keluar
    // (cukup DATABASE_URL; tanpa token Telegram — untuk setup VPS/dev)
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL wajib di-set untuk mode migrate");
        let pool = db::init_pool(&url).await?;
        db::run_migrations(&pool).await?;
        tracing::info!("migrasi selesai ✅");
        return Ok(());
    }

    let cfg = config::Config::from_env()?;
    let pool = db::init_pool(&cfg.database_url).await?;
    db::run_migrations(&pool).await?;

    let ai = provider::build(&cfg)?;
    // Bot & ShellCtx dibuat sebelum Agent: ShellCtx memegang bot (utk kirim keyboard
    // konfirmasi & file output) dan Agent memegang ShellCtx (Pilar 9).
    let bot = teloxide::Bot::new(cfg.telegram_bot_token.clone());
    let shell = shell::ShellCtx::new(&cfg, bot.clone(), pool.clone());
    let agent = Arc::new(agent::Agent::new(pool, ai, cfg, shell));

    tracing::info!(
        provider = agent.provider.name(),
        model = agent.provider.model_name(),
        "Hermes-Lite jalan — long polling Telegram"
    );
    gateway::run(bot, agent).await?;
    Ok(())
}
