-- Memory v3 (adopsi memory-types OpenViking, dipangkas utk single-user):
-- kolom kind + audit trail semua perubahan memory (pola memory_diff.json OpenViking).
-- Additive-only; baris lama default 'general' — diklasifikasi ulang oleh dreaming
-- (action reclassify, bertahap — tanpa one-time LLM migration).
-- Taxonomy 4+general (bukan 9 types OpenViking): identity/soul sudah dipegang soul.md
-- (Pilar 3), cases/trajectories sudah tercakup Skills (Pilar 11).

ALTER TABLE memory ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'general'
    CHECK (kind IN ('profile', 'preference', 'entity', 'event', 'general'));

CREATE TABLE IF NOT EXISTS memory_changes (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT      NOT NULL,
    memory_id  BIGINT,               -- NULL = baris sudah dihapus (snapshot di old_*)
    action     TEXT        NOT NULL CHECK (action IN ('insert','update','delete','retype','reclassify')),
    old_fact   TEXT, new_fact TEXT,
    old_type   TEXT, new_type TEXT,
    old_kind   TEXT, new_kind TEXT,
    source     TEXT        NOT NULL CHECK (source IN ('agent','review','dream','manual')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_memory_changes_chat ON memory_changes (chat_id, id DESC);
