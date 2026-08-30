//! Progress notifier (UX): satu pesan status live per turn yang di-edit seiring
//! agent bekerja — tiap tool call tampil (🔧 mulai → ✅/⚠️ selesai + durasi) —
//! plus typing indicator berkelanjutan (Telegram menghapus status "typing"
//! setelah ±5 detik, jadi tanpa loop turn panjang tampak mati).
//!
//! Desain:
//! - **Lazy**: pesan status hanya dibuat saat `notify()` pertama (biasanya tool
//!   call pertama) — chat santai tanpa tool tidak berisik.
//! - **Fire-and-forget**: `notify()` tidak pernah memblokir agent loop; edit
//!   diserialisasi satu writer task dengan tick antar-edit (aman rate limit).
//! - **`finish()` merapikan**: sukses → hapus pesan status (balasan final yang
//!   mewakili); gagal → edit jadi ringkasan ❌ supaya jejak kendala tetap terlihat.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, MessageId};

/// Tick writer task — juga throttle antar-edit (rate limit Telegram).
const WRITER_TICK: Duration = Duration::from_millis(900);
/// Interval pengulangan typing indicator.
const TYPING_INTERVAL: Duration = Duration::from_secs(4);
/// Batas panjang teks status.
const MAX_STATUS_CHARS: usize = 480;

struct Inner {
    msg_id: Option<MessageId>,
    text: String,
    dirty: bool,
    writer_running: bool,
}

struct Shared {
    bot: Bot,
    chat_id: ChatId,
    inner: Mutex<Inner>,
    stop: AtomicBool,
}

/// Handle progres satu turn. Clone-murah (Arc); typing loop mati saat drop.
#[derive(Clone)]
pub struct Notifier(Arc<Shared>);

impl Notifier {
    /// Mulai notifier: typing loop jalan sampai `finish()` / drop.
    pub fn start(bot: Bot, chat_id: ChatId) -> Notifier {
        let shared = Arc::new(Shared {
            bot,
            chat_id,
            inner: Mutex::new(Inner {
                msg_id: None,
                text: String::new(),
                dirty: false,
                writer_running: false,
            }),
            stop: AtomicBool::new(false),
        });

        // Typing indicator berkelanjutan — tanpa ini "typing…" hilang setelah ±5
        // detik dan turn yang mengerjakan tool tampak seperti tidak hidup.
        {
            let sh = shared.clone();
            tokio::spawn(async move {
                let _ = sh.bot.send_chat_action(sh.chat_id, ChatAction::Typing).await;
                loop {
                    tokio::time::sleep(TYPING_INTERVAL).await;
                    if sh.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = sh.bot.send_chat_action(sh.chat_id, ChatAction::Typing).await;
                }
            });
        }

        Self(shared)
    }

    /// Update status (fire-and-forget, tidak pernah memblokir agent loop).
    pub fn notify(&self, text: impl Into<String>) {
        let text = truncate_status(&text.into());
        let mut inner = self.0.inner.lock().unwrap();
        if !inner.dirty && inner.text == text && inner.msg_id.is_some() {
            return; // tidak ada perubahan — hindari edit "message is not modified"
        }
        inner.text = text;
        inner.dirty = true;
        if !inner.writer_running {
            inner.writer_running = true;
            let sh = self.0.clone();
            tokio::spawn(async move { writer(sh).await });
        }
    }

    /// Akhiri: `Some(text)` → edit pesan status jadi ringkasan;
    /// `None` → hapus pesan status (pekerjaan selesai, balasan final menggantikan).
    pub async fn finish(&self, summary: Option<String>) {
        self.0.stop.store(true, Ordering::Relaxed);
        let (msg_id, last) = {
            let mut inner = self.0.inner.lock().unwrap();
            inner.dirty = false;
            (inner.msg_id, inner.text.clone())
        };
        let Some(id) = msg_id else {
            return; // tidak pernah ada status → tidak ada yang perlu dirapikan
        };
        match summary {
            Some(s) => {
                let s = truncate_status(&s);
                if s != last {
                    let _ = self.0.bot.edit_message_text(self.0.chat_id, id, s).await;
                }
            }
            None => {
                let _ = self.0.bot.delete_message(self.0.chat_id, id).await;
            }
        }
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        // Pastikan typing loop selalu berhenti, termasuk jalur command yang
        // tidak pernah memanggil finish().
        self.0.stop.store(true, Ordering::Relaxed);
    }
}

async fn writer(sh: Arc<Shared>) {
    loop {
        tokio::time::sleep(WRITER_TICK).await;
        if sh.stop.load(Ordering::Relaxed) {
            break;
        }
        let job = {
            let mut inner = sh.inner.lock().unwrap();
            if inner.dirty {
                inner.dirty = false;
                Some(inner.text.clone())
            } else {
                None
            }
        };
        let Some(text) = job else { continue };

        let existing = sh.inner.lock().unwrap().msg_id;
        match existing {
            Some(id) => {
                if let Err(e) = sh.bot.edit_message_text(sh.chat_id, id, text.clone()).await {
                    tracing::debug!("edit status gagal ({e:#}) — kirim pesan status baru");
                    if let Ok(m) = sh.bot.send_message(sh.chat_id, text).await {
                        sh.inner.lock().unwrap().msg_id = Some(m.id);
                    }
                }
            }
            None => {
                if let Ok(m) = sh.bot.send_message(sh.chat_id, text).await {
                    sh.inner.lock().unwrap().msg_id = Some(m.id);
                }
            }
        }
    }
    // Izinkan notify() berikutnya spawn ulang (praktisnya hanya saat sebelum finish).
    sh.inner.lock().unwrap().writer_running = false;
}

fn truncate_status(s: &str) -> String {
    let total = s.chars().count();
    if total <= MAX_STATUS_CHARS {
        return s.to_string();
    }
    let t: String = s.chars().take(MAX_STATUS_CHARS).collect();
    format!("{t}…")
}
