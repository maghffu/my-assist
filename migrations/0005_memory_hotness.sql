-- Memory hotness (adopsi pola OpenViking memory_lifecycle).
-- Additive-only: rollback binary lama tetap jalan di atas skema ini.

-- Jejak akses: dipakai hitung hotness score = sigmoid(log1p(n)) * decay(accessed_at).
ALTER TABLE memory ADD COLUMN IF NOT EXISTS access_count INT NOT NULL DEFAULT 0;
ALTER TABLE memory ADD COLUMN IF NOT EXISTS accessed_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Rewrite/di-dream harus reset freshness-nya juga (handled di query UPDATE aplikasi).
