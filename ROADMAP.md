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
| 4 | Shell access (`run_command`, file tools, confirmation) | ✅ selesai |
| 5 | Web search, fetch_url chain, image generation | ✅ selesai |
| 6 | Memory depth: background review, dreaming, skills | ✅ selesai |
| 7 | OCR (Tesseract) | ✅ selesai |
| 8 | Hardening & deploy VPS | ✅ selesai |

**Status verifikasi Fase 0–3 (2026-08-29):** `cargo build` hijau ✅ · binary jalan + validasi config graceful ✅ · migrasi belum bisa dites lokal (PostgreSQL portable diblokir endpoint security mesin dev — exception `0xC0000142` pada child process; detail di bawah) — migrasi akan tervalidasi saat dijalankan di VPS.

**Status verifikasi Fase 4 (2026-08-29, mesin Linux/VPS):** `cargo build` hijau ✅ · `cargo test` 4/4 lulus (destructive detection, masking, cwd marker, confirm parsing) ✅ · migrasi tervalidasi via `hermes-lite migrate` (Postgres 17 docker, 4 tabel + `_sqlx_migrations`) ✅ · bot live long polling dengan wiring shell + callback handler confirmation gate ✅ · smoke test mekanik wrapper bash + marker cwd ✅. End-to-end `run_command` via Telegram menyusul diuji owner (perlu tap approve di keyboard konfirmasi).

**Status verifikasi Fase 5 (2026-08-29, mesin Linux/VPS):** `cargo build` hijau ✅ · `cargo test` 14/14 lulus (SSRF IP ranges, URL validation, HTML strip, entity decode, Tavily parsing, formatting caps, urlencoding) + 2 integration test network `cargo test -- --ignored` lulus (SSRF guard menolak `localtest.me`→127.0.0.1 + cloud metadata + IP literal private; fetch chain example.com via markdown.new) ✅ · smoke test `generate_image` (Pollinations → send_photo) & `web_search` live via Telegram menyusul diuji owner — butuh `TAVILY_API_KEY` di .env utk search.

**Status verifikasi Fase 6 (2026-08-29, mesin Linux/VPS):** `cargo build` hijau ✅ · `cargo test` 22/22 lulus (slugify, save/list/delete roundtrip, path traversal guard, keyword injection, JSON parse robust, duplikat detection, dream action shape) ✅ · `generate_image` sudah diverifikasi owner live (gambar kucing nyampe) ✅ · background review & dreaming live via Telegram menyusul diuji owner (`/dream` utk trigger manual).

**Status verifikasi Fase 7 (2026-08-29, mesin Linux/VPS):** tesseract 5.3.2 + leptonica 1.84 + tessdata `eng`+`ind` (tessdata_fast) terpasang ✅ · binding leptess 0.14 (`set_image_from_mem`, unix-only dep — dev Windows tetap buildable) ✅ · `cargo test` 25/25 termasuk integration test OCR native pada aset statis (`testdata/ocr-sample.png`) ✅ · CLI smoke sebagai user `hermes` lulus ✅ · foto via Telegram → OCR → turn menyusul diuji owner.

**Status verifikasi Fase 8 (2026-08-29, mesin Linux/VPS):** release build 6m57s (2 core) ✅ · user `hermes` unprivileged + `/opt/hermes-lite` (`.env` 0600) ✅ · systemd unit aktif (Restart=on-failure, MemoryMax=512M — RSS aktual ±16-23MB) ✅ · `systemd-analyze security` exposure 4.2 OK (ProtectSystem=strict, PrivateTmp, CapabilityBoundingSet kosong, UMask=0027) ✅ · journald logging jalan ✅ · `deploy/deploy.sh` idempotent (skill & .env server tidak dioverwrite) + `deploy/DEPLOY.md` ops manual ✅.

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

- ✅ `SEARCH_PROVIDER` abstraction (trait `SearchProvider`) + Tavily primary; tanpa key → tool balas pesan konfigurasi (bot tetap jalan)
- ✅ `fetch_url` chain 4-tier (`Accept: text/markdown` → markdown.new → r.jina.ai → HTML strip lokal) + SSRF guard dua lapis (pre-check DNS + custom resolver reqwest yang berlaku juga utk redirect) + size cap 2MB/request, context 15K char, versi penuh jadi file attachment
- ✅ `generate_image` via Pollinations → `send_photo` (timeout 60s, konfirmasi teks ke LLM)
- Env baru: `SEARCH_PROVIDER`, `TAVILY_API_KEY`, `FETCH_TIMEOUT=30`, `IMAGE_TIMEOUT=60`
- Integration test network: `cargo test -- --ignored`

## Fase 6 — Memory Depth (Pilar 5/6/11)

- ✅ Background review pasca-turn: `tokio::spawn` fire-and-forget dari gateway (tanpa latency), LLM call tanpa tools + output JSON, fakta `explicit`/`inferred`, anti-duplikat 2 lapis (prompt + normalized containment Rust-side), skip pertukaran pendek (<120 char), token tercatat di `/usage`
- ✅ Dreaming cycle: otomatis mingguan (interval_at +7 hari, BUKAN immediately) + `/dream` manual — konsolidasi memory (drop/rewrite/upgrade inferred→explicit, validasi id anti-halusinasi, max 20 aksi/chat, konservatif: parse gagal → skip) dan review skills (delete/rewrite/merge)
- ✅ Skills: `save_skill` (slugify nama, anti-trivial min 80 char, cap 20KB, path traversal guard), storage file `skills/*.md`, injection ke system prompt (daftar nama selalu + skill cocok keyword dimuat penuh, max 3 file × 4K char; sisanya via `read_file skills/<nama>.md`)
- ✅ `/skills` (list) + `/dream` (trigger manual); env baru: `SKILLS_DIR` (default `skills`)

## Fase 7 — OCR (Pilar 7)

- ✅ Handler foto Telegram (resolusi terbesar, cap 10MB) → Tesseract via **leptess 0.14** native binding (`set_image_from_mem`, tanpa temp file, `spawn_blocking`) → teks jadi prompt biasa → turn agent penuh (tools tersedia)
- ✅ Teks < 12 char → balas langsung tanpa LLM call; caption foto digabung ke prompt; cap 15K char
- ✅ Bahasa via `OCR_LANG` (default `eng+ind`, tessdata_fast `ind` di-download manual); `OCR_TESSDATA` opsional
- ✅ Binding unix-only (`[target.'cfg(unix)']`) — stub error di non-unix, dev Windows tetap bisa build
- Note: leptess 0.13 rusak vs leptonica-sys 0.4 (bindgen opaque) — wajib 0.14+

## Fase 8 — Hardening & Deploy VPS

- ✅ Dedicated unprivileged user `hermes` (nologin, tanpa capability, tanpa sudo/docker — evidence-driven, cara tambah sudoers allowlist didokumentasikan di DEPLOY.md)
- ✅ systemd `hermes-lite.service`: Restart=on-failure, MemoryMax=512M, UMask=0027, sandbox lengkap (ProtectSystem=strict + ReadWritePaths=/opt/hermes-lite /tmp, PrivateTmp, dst.) — exposure 4.2 OK
- ✅ `deploy/deploy.sh` idempotent: build release → user/dir → artifacts → unit → restart; `.env` & `skills/` server tidak dioverwrite
- ✅ `deploy/DEPLOY.md`: setup awal (deps, tessdata ind, postgres docker), operasional (log/update/backup), troubleshooting
- ✅ Journald + RUST_LOG=info via .env
- `/usage` persist: di-skip — in-memory memadai (evidence-driven)

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
