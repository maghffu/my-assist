use anyhow::Result;
use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn init_pool(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await?;
    Ok(pool)
}

/// Migrasi di-embed di binary dan dijalankan otomatis saat startup (lihat ROADMAP: log keputusan).
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
