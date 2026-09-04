-- Session summary (adopsi pola session-commit OpenViking): pesan yang jatuh dari
-- context window diringkas jadi rolling summary per chat — bukan hilang diam-diam.
-- Satu baris per chat_id (bukan per-segment): ringkasan konsolidat yang terus
-- diperluas; kalau cap tercapai, summary lama + batch baru di-recompress
-- (self-compacting). Additive-only: binary lama tidak menyentuh tabel ini.

CREATE TABLE IF NOT EXISTS session_summaries (
    chat_id     BIGINT PRIMARY KEY,
    summary     TEXT        NOT NULL,
    archived_to BIGINT      NOT NULL DEFAULT 0,  -- id pesan terakhir yang sudah diarsipkan
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
