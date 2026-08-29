-- Hermes-Lite initial schema (Pilar 2, 4, 5, 9)
-- Semua di satu Postgres instance (AGENTS.md: Skema Database)

CREATE TABLE IF NOT EXISTS messages (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT      NOT NULL,
    role       TEXT        NOT NULL CHECK (role IN ('user', 'assistant')),
    content    TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_messages_chat_recent
    ON messages (chat_id, created_at DESC);

CREATE TABLE IF NOT EXISTS reminders (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT      NOT NULL,
    message    TEXT        NOT NULL,
    remind_at  TIMESTAMPTZ NOT NULL,
    sent       BOOLEAN     NOT NULL DEFAULT FALSE,
    kind       TEXT        NOT NULL DEFAULT 'static' CHECK (kind IN ('static', 'job')),
    recur      TEXT,               -- NULL = one-shot; 'daily' | 'weekly' (cron expr menyusul)
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_reminders_due
    ON reminders (sent, remind_at);

CREATE TABLE IF NOT EXISTS memory (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT      NOT NULL,
    fact       TEXT        NOT NULL,
    type       TEXT        NOT NULL DEFAULT 'explicit' CHECK (type IN ('explicit', 'inferred')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Cap karakter total per chat_id ditegakkan di layer aplikasi (Pilar 5)
CREATE INDEX IF NOT EXISTS idx_memory_chat
    ON memory (chat_id, id DESC);

CREATE TABLE IF NOT EXISTS command_logs (
    id          BIGSERIAL PRIMARY KEY,
    chat_id     BIGINT      NOT NULL,
    command     TEXT        NOT NULL,
    exit_code   INTEGER,
    duration_ms BIGINT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_command_logs_chat
    ON command_logs (chat_id, created_at DESC);
