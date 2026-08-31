-- Reminder anchor (Pilar 4 bugfix): simpan jam asli pembuatan recurring reminder.
-- Reschedule memakai anchor (bukan remind_at yang bisa tergeser backoff) supaya
-- HH:MM harian/mingguan tetap stabil — cegah drift 07:50 → 08:56 dst.
ALTER TABLE reminders
    ADD COLUMN IF NOT EXISTS anchor_at TIMESTAMPTZ;
UPDATE reminders SET anchor_at = remind_at WHERE anchor_at IS NULL;
