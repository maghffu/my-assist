# ROADMAP Eksekusi — Hermes-Lite

Pendamping `AGENTS.md` (desain). File ini menjawab **urutan build, deliverable, dan kriteria selesai** per fase.

**Prinsip:** walking skeleton dulu — bot end-to-end paling sederhana hidup di Telegram secepat mungkin, lalu capability di-layer satu per satu. Tiap fase diakhiri dengan kode yang bisa jalan (commit-able), tidak ada fase yang menutup dengan kode setengah rusak.

---

## Status

| Fase | Isi | Status |
|---|---|---|
| 0 | Fondasi: scaffold, config, DB, migrasi | ✅ selesai |
| 1 | Provider layer: trait + Anthropic | ✅ selesai |
| 2 | Agent loop + tools inti (reminder, memory) | ✅ selesai |
| 3 | Telegram gateway: polling, allowlist, slash commands | ✅ selesai |
| 4 | Shell access (`run_command`, file tools, confirmation) | ⬜ |
| 5 | Web search, fetch_url chain, image generation | ⬜ |
| 6 | Memory depth: background review, dreaming, skills | ⬜ |
| 7 | OCR (Tesseract) | ⬜ |
| 8 | Hardening & deploy VPS | ⬜ |

**Status verifikasi Fase 0–3 (2026-08-29):** `cargo build` hijau ✅ · binary jalan + validasi config graceful ✅ · migrasi belum bisa dites lokal (PostgreSQL portable diblokir endpoint security mesin dev — exception `0xC0000142` pada child process; detail di bawah) — migrasi akan tervalidasi saat dijalankan di VPS.

### Terverifikasi berjalan di mesin dev

```bash
# Env build (GNU toolchain + w64devkit) — WAJIB sebelum cargo build di mesin ini:
export PATH="/c/tools/w64devkit/bin:$USERPROFILE/.cargo/bin:$PATH"
export LIBRARY_PATH="C:\\Users\\REDACTED-USER\\.rustup\\toolchains\\stable-x86_64-pc-windows-gnu\\lib\\rustlib\\x86_64-pc-windows-gnu\\lib\\self-contained"

cd /c/xampp/htdocs/my-hermes-agent
cargo build          # ✅ hijau (2 m 21s first build)
./target/debug/hermes-lite.exe   # ✅ graceful exit dengan pesan config yang jelas
```

---

## Fase 0 — Fondasi

**Deliverable:**
- Struktur modul per pilar (`src/`): `gateway`, `context`, `soul`, `reminders`, `memory`, `provider/`, `agent`, `tools/`
- `Cargo.toml` dengan stack terkunci (AGENTS.md): tokio, teloxide, reqwest, sqlx, serde, dotenvy, tracing, chrono
- `migrations/0001_init.sql` — 4 tabel: `messages`, `reminders`, `memory`, `command_logs`
- `.env.example`, `soul.md` (persona starter), `.gitignore`

**Keputusan teknis:**
- Migrasi **di-embed via `sqlx::migrate!()` dan dijalankan otomatis saat startup** + subcommand `hermes-lite migrate` (cukup `DATABASE_URL`) — tidak perlu sqlx-cli terpisah di VPS
- Query pakai **runtime API** (`sqlx::query`/`query_as`), bukan macro `query!` — build tidak butuh live DB / offline metadata. Trade-off: tanpa compile-time checking; upgrade ke macro nanti via `cargo sqlx prepare` kalau schema mulai ramai
- `tls-native-tls` untuk sqlx & reqwest (Windows: schannel, tanpa openssl; VPS Linux: libssl standar)

**Kriteria selesai:** `cargo build` hijau; struktur modul sesuai pilar.

## Fase 1 — Provider Layer

**Deliverable:**
- `src/provider/mod.rs` — trait `AiProvider` (`chat(system, messages, tools) -> blocks/stop_reason/usage`), tipe `ApiMessage`, `ContentBlock` (text/tool_use/tool_result), `ToolDef`
- `src/provider/anthropic.rs` — impl via `POST api.anthropic.com/v1/messages`, header `x-api-key` + `anthropic-version: 2023-06-01`
- Factory `provider::build(cfg)` dipilih `AI_PROVIDER` (Pilar 8)

**Kriteria selesai:** build hijau; pemanggilan nyata diuji menyusul via agent loop (butuh API key).

## Fase 2 — Agent Loop + Tools Inti

**Deliverable:**
- `src/agent.rs` — `Agent::run_turn(chat_id, text, include_history)`: system prompt (soul + memory + waktu), N-history, tool-calling loop (maks 8 iterasi), akumulasi usage, simpan hasil ke history
- `src/tools/mod.rs` — registry + executor: `create_reminder` (RFC3339 + fallback "YYYY-MM-DD HH:MM" dianggap UTC+7), `save_memory` (cap 20.000 char total per chat, Pilar 5)
- `src/soul.rs` — loader `soul.md` dengan fallback default

**Kriteria selesai:** percakapan via CLI test yang bisa membuat row reminder & memory.

## Fase 3 — Telegram Gateway (walking skeleton selesai)

**Deliverable:**
- `src/gateway.rs` — `teloxide::repl` long polling; **hard allowlist `ALLOWED_CHAT_ID`** (drop diam-diam); typing indicator; reply di-chunk < 4096 char
- Reminder loop `tokio::spawn` tiap 30 detik: kirim static reminder / eksekusi job (`kind='job'` → `run_turn` dengan context segar, Pilar 4) / reschedule `recur` (daily/weekly)
- Slash commands (tanpa lewat LLM): `/help` `/status` `/memory` (+ `/memory del <id>`) `/reminders` (+ `/reminders del <id>`) `/provider` `/usage` `/skills` (stub, Fase 6)

**Kriteria selesai:** bot hidup di Telegram, chat + reminder end-to-end.

---

## Fase 4 — Shell Access (Pilar 9)

- `run_command` (`bash -lc`, cwd tracking, timeout 120s + kill process group), `read_file`/`write_file` (path guard)
- Confirmation gate inline keyboard untuk destructive pattern; output > ~4000 char → `send_document`
- Audit `command_logs`; secret masking
- **Catatan dev:** eksekusi shell di Windows dev beda dari VPS Linux — implement target Linux (`bash`), test penuh saat deploy

## Fase 5 — Web & Image (Pilar 10 + 12)

- `SEARCH_PROVIDER` abstraction + Tavily primary; `fetch_url` chain 4-tier (`Accept: text/markdown` → markdown.new → r.jina.ai → readability) + SSRF guard
- `generate_image` via Pollinations → `send_photo`

## Fase 6 — Memory Depth (Pilar 5/6/11)

- Background review pasca-turn (`tokio::spawn`, prompt FACT/INFERRED, anti-duplikat)
- Dreaming cycle mingguan (merge/upgrade/hapus) untuk memory **dan** skills
- `skills/` dir + `save_skill` + injection ke system prompt

## Fase 7 — OCR (Pilar 7)

- Tesseract binding (leptess/tesseract-rs) + handler foto Telegram → teks → context
- **Butuh libtesseract — test di VPS Linux; binding di Windows menyulitkan, jadi sengaja di fase akhir**

## Fase 8 — Hardening & Deploy VPS

- Unprivileged user + sudoers allowlist (bila perlu), systemd unit, `RUST_LOG=info`, journald
- Dokumentasi setup VPS (libtesseract, postgres), `/usage` persist kalau terbukti perlu (sekarang in-memory per proses)

---

## Catatan Dev Environment (Windows)

- Toolchain: `stable-x86_64-pc-windows-gnu` (self-contained, tanpa MSVC Build Tools). Semua deps pure-Rust → aman. Kalau nanti ada crate yang butuh C compiler, opsinya: VS Build Tools atau defer ke VPS
- **w64devkit** di `C:\tools\w64devkit` menyediakan binutils lengkap (`dlltool`, `as`, `gcc`) — rustup GNU sendiri tidak lengkap; `LIBRARY_PATH` wajib menunjuk sysroot self-contained (lihat blok di atas)
- PostgreSQL lokal belum terpasang (machine hanya punya XAMPP/MySQL). **Percobaan portable PG 17 gagal**: postmaster sempat start (`ready to accept connections`) lalu child process dibunuh endpoint security dengan `0xC0000142` (DLL init failed) — dicoba via Git Bash & PowerShell Start-Process, hasil sama; WSL ada tapi tanpa distro. **Kesimpulan: test integrasi DB dilakukan di VPS** — app auto-migrate saat startup, jadi dev lokal bisa lanjut tanpa DB
- Artifacts yang tersisa di `C:\tools` (pg.zip, pgdata/, pgsql/) aman diabaikan; data dir PG bisa dihapus kapan saja
- Migration test: `DATABASE_URL=... cargo run -- migrate`

## Log Keputusan Singkat

| Keputusan | Alasan |
|---|---|
| Runtime `sqlx::query` (bukan `query!`) | Build DB-free; upgrade ke macro ketika schema kompleks |
| Auto-migrate saat startup + subcommand `migrate` | Nol alat tambahan di VPS |
| Slash command diparse manual (bukan macro teloxide) | Kontrol penuh, deterministik, tanpa dep ekstra |
| Usage tracking in-memory dulu | Single-user; persist kalau terbukti perlu (evidence-driven) |
| teloxide 0.13 | API `repl` stabil, cocok pattern long polling single-user |
| Chunk 3800 char (bukan 4096) | Margin aman + reserve prefix emoji/format |

## Cara Menjalankan (Fase 0–3)

**Mesin dev (Windows) — pakai env build di atas dulu, lalu:**

```bash
cp .env.example .env   # isi TELEGRAM_BOT_TOKEN, ANTHROPIC_API_KEY, ALLOWED_CHAT_ID, DATABASE_URL
cargo run              # auto-migrate + long polling
# atau migrasi saja:
DATABASE_URL=postgres://... cargo run -- migrate
```

**VPS Linux (target produksi):**

```bash
# butuh: rustup, libpq-dev (sqlx), postgres
export DATABASE_URL=postgres://hermes:***@localhost:5432/hermes
cargo run -- migrate    # atau langsung cargo run (auto-migrate)
```
