#!/usr/bin/env python3
"""Patch /new (reset session) — context.rs + gateway.rs. Idempotent."""
import sys
from pathlib import Path

ROOT = Path(__file__).parent

# Rust line-continuation dalam string literal HELP_TEXT: backslash n backslash
RS = "\\n\\"
NL = "\n"

def patch(path: str, anchor: str, replacement: str, already: str) -> bool:
    p = ROOT / path
    src = p.read_text()
    if already in src:
        print(f"SKIP {path} — sudah dipatch")
        return True
    if anchor not in src:
        print(f"FAIL {path} — anchor tidak ketemu!")
        return False
    p.write_text(src.replace(anchor, replacement, 1))
    print(f"OK   {path}")
    return True

ok = True

# 1) context.rs — tambah clear_messages()
ok &= patch(
    "src/context.rs",
    """    Ok(rows
        .into_iter()
        .map(|(role, content)| ChatMessage { role, content })
        .collect())
}""",
    """    Ok(rows
        .into_iter()
        .map(|(role, content)| ChatMessage { role, content })
        .collect())
}

/// Hapus seluruh riwayat percakapan satu chat (command /new — reset session).
/// Memory & skills = tabel berbeda, tidak tersentuh (identitas agent tetap).
pub async fn clear_messages(pool: &PgPool, chat_id: i64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM messages WHERE chat_id = $1")
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}""",
    "pub async fn clear_messages",
)

# 2) gateway.rs — import context
ok &= patch(
    "src/gateway.rs",
    "use crate::{memory, ocr, reminders, review, shell, skills};",
    "use crate::{context, memory, ocr, reminders, review, shell, skills};",
    "use crate::{context, memory",
)

# 3) HELP_TEXT — daftar perintah
ok &= patch(
    "src/gateway.rs",
    "**Perintah:**" + RS + NL + "/status — uptime, provider, model" + RS + NL,
    "**Perintah:**" + RS + NL + "/new — reset session (hapus riwayat chat ini)" + RS + NL
    + "/status — uptime, provider, model" + RS + NL,
    "/new — reset session",
)

# 4) Menu "/" Telegram
ok &= patch(
    "src/gateway.rs",
    '        BotCommand::new("help", "bantuan / daftar perintah"),',
    '        BotCommand::new("new", "reset session — hapus riwayat chat ini"),\n'
    '        BotCommand::new("help", "bantuan / daftar perintah"),',
    'BotCommand::new("new"',
)

# 5) Match arm /new — sebelum fallback "tidak dikenal"
ok &= patch(
    "src/gateway.rs",
    """        other => Ok(format!(
            "Perintah {} tidak dikenal — ketik /help.",
            other
        )),""",
    """        "/new" => {
            // Reset session: hapus riwayat chat ini — context window langsung
            // kosong tanpa restart. Memory & skills tidak terpengaruh.
            let deleted = context::clear_messages(&agent.pool, chat_id).await?;
            Ok(format!(
                "🆕 Session baru dimulai — {deleted} pesan lama dibuang. Memory & skills tetap aman."
            ))
        }

        other => Ok(format!(
            "Perintah {} tidak dikenal — ketik /help.",
            other
        )),""",
    '"/new" =>',
)

sys.exit(0 if ok else 1)
