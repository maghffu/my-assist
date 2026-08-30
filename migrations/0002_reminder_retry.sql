-- Reminder retry backoff (Pilar 4 bugfix): tambah fail_count utk menghitung
-- backoff eksponensial saat pengiriman job/reminder gagal — mencegah retry
-- tiap 30 detik membabi buta (retry-forever) yang membakar kuota AI.
ALTER TABLE reminders
    ADD COLUMN IF NOT EXISTS fail_count INT NOT NULL DEFAULT 0;
