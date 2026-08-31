-- Memory v2: recall-based injection (adopsi pola hermes-agent NousResearch).
-- Additive-only: rollback binary lama tetap jalan di atas skema ini.

-- FTS index untuk recall selectif (config 'simple' — isi memory campur ID/EN).
ALTER TABLE memory ADD COLUMN IF NOT EXISTS search_vector tsvector
  GENERATED ALWAYS AS (to_tsvector('simple', fact)) STORED;
CREATE INDEX IF NOT EXISTS idx_memory_fts ON memory USING GIN (search_vector);

-- KV state buat persist last_run timer dreaming (kebal restart).
CREATE TABLE IF NOT EXISTS agent_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
