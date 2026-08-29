/// Soul — persona statis, diedit manual oleh owner (Pilar 3).
const DEFAULT_SOUL: &str = "Kamu adalah Hermes, asisten pribadi owner di Telegram. \
Hangat, santai, to the point. Jawab ringkas dulu, detail kalau diminta. \
Jujur kalau tidak yakin — jangan mengarang fakta.";

pub fn load(path: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => {
            tracing::warn!(path, "soul file kosong — pakai default");
            DEFAULT_SOUL.into()
        }
        Err(e) => {
            tracing::warn!(path, error = %e, "gagal baca soul file — pakai default");
            DEFAULT_SOUL.into()
        }
    }
}
