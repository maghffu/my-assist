# Hermes-Lite — Personal AI Agent (Telegram + Rust)

## Project Overview

Personal AI assistant terinspirasi dari Hermes Agent (NousResearch), tapi versi **minimal** yang fokus pada low resource footprint. Berjalan sebagai satu proses Rust di VPS kecil, terhubung ke Telegram sebagai frontend dan AI provider (Anthropic Claude) sebagai otak.

**Prinsip desain utama:**
- Memory footprint sekecil mungkin — hindari dependency berat (no vector DB, no Redis, no multi-container stack)
- Single-user personal assistant, bukan multi-tenant SaaS
- Semua state di satu Postgres instance — jangan nambah service terpisah kecuali benar-benar perlu
- Prioritaskan simplicity dulu, upgrade kompleksitas cuma kalau ada bukti nyata dibutuhkan (evidence-driven, bukan speculative)

---

## Tech Stack

| Layer | Pilihan | Alasan |
|---|---|---|
| Bahasa | Rust | Memory footprint kecil, long-running process yang efisien |
| Async runtime | Tokio | Standar de-facto untuk async Rust |
| Telegram client | `teloxide` | Library Telegram bot paling matang di ekosistem Rust |
| HTTP client | `reqwest` | Untuk call ke Anthropic API |
| Database | PostgreSQL | Satu-satunya storage — dipakai untuk history, reminder, dan memory |
| DB driver | `sqlx` | Async, compile-time query checking |
| Serialization | `serde` / `serde_json` | Untuk request/response API dan tool calling |
| AI Provider | Anthropic API (`api.anthropic.com/v1/messages`) | Model: `claude-sonnet-4-6` (sesuaikan sesuai kebutuhan biaya/kualitas) |
| Koneksi Telegram | **Long polling** (`getUpdates`) | Tidak butuh domain/HTTPS publik/webhook — cocok untuk VPS kecil tanpa reverse proxy |
| Scheduler | `tokio::time::interval` (polling loop sederhana) | Tidak perlu library cron eksternal untuk MVP |
| Process execution | `tokio::process` | Bagian dari Tokio, tanpa dependency baru — eksekusi shell command via tool `run_command` (lihat Pilar 9) |
| OCR | Tesseract (via `leptess` / `tesseract-rs` binding) | Native dependency, perlu `libtesseract` terinstall di VPS — dipilih karena lokal/tanpa API call tambahan, konsisten dengan prinsip low-footprint |
| AI Provider (arsitektur) | Trait-based abstraction, provider dipilih via config (`AI_PROVIDER` env var) | Agent utama yang dipakai owner saat ini belum vision-capable — OCR jadi "penyama rata" input teks lintas provider |

**Yang sengaja TIDAK dipakai di MVP (dan kenapa):**
- ❌ Vector DB / pgvector — overkill untuk volume single-user; FTS Postgres cukup untuk kebutuhan saat ini
- ❌ Redis — tidak perlu caching layer terpisah di skala ini
- ❌ Honcho / memory backend eksternal — bagus tapi butuh multi-container stack (API+Deriver+Postgres+Redis) yang kontradiktif dengan tujuan low-footprint
- ❌ Webhook Telegram — butuh domain + HTTPS, polling lebih simpel untuk VPS personal
- ❌ Fine-tuning / actual model training — tidak feasible tanpa GPU, dan tidak perlu untuk use case ini (lihat bagian "self-improvement" di bawah)
- ❌ Vision LLM sebagai satu-satunya jalur baca gambar — agent utama yang dipakai owner saat ini **belum vision-capable**, jadi OCR dipilih sebagai preprocessing universal yang bekerja di semua provider (lihat Pilar 7)
- ❌ OCR via API eksternal (Google Vision, Azure) — dipilih OCR lokal (Tesseract) supaya tidak menambah provider/biaya per-call, walau trade-off-nya kualitas lebih bergantung pada kualitas gambar input

---

## Arsitektur Konseptual — 12 Pilar

```
Telegram User → Telegram Bot API (polling) → Rust Backend → AI Provider (Anthropic API)
                                                    ↓            ↘              ↘
                                              PostgreSQL      Shell VPS       Web
                                    (messages, reminders,   (run_command,   (web_search,
                                     memory, command_logs)   read_file,     fetch_url,
                                                            write_file)    generate_image)
```

### 1. Gateway
Proses utama yang polling Telegram (`getUpdates`), terima pesan masuk, forward ke AI provider, kirim balasan balik. Single process, tidak perlu public URL.

**Slash commands** — diproses langsung di gateway TANPA lewat LLM (murah & deterministik): `/status` (uptime, provider aktif, resource usage), `/memory` (list + hapus fakta), `/reminders` (list + hapus), `/skills` (list skill), `/provider` (ganti provider aktif — tetap manual, sesuai non-goal), `/usage` (estimasi spend token). Biar owner bisa kelola agent langsung dari Telegram tanpa SSH ke VPS.

### 2. Context (short-term history)
History percakapan per `chat_id`, disimpan di tabel `messages`. Kirim N pesan terakhir sebagai context ke tiap API call — jangan kirim seluruh history (boros token).

### 3. Soul (persona / system prompt)
File statis (`soul.md` atau semacamnya) berisi kepribadian, tone, dan aturan main agent. Di-load dan digabung dengan curated memory saat membangun system prompt tiap sesi. Diedit manual oleh user, bukan auto-generated.

### 4. Reminder / Cron & Scheduled Jobs
Fitur proaktif — bukan cuma reaktif jawab pertanyaan. Reminder disimpan di tabel `reminders` dengan `remind_at` timestamp, di-trigger via polling loop periodik (misal tiap 30 detik). Ekstraksi waktu dari bahasa natural ("besok jam 3 sore") menggunakan **native tool calling** Anthropic API (bukan minta LLM mengembalikan JSON manual via prompt biasa) — LLM sendiri yang memutuskan kapan memanggil tool `create_reminder` dengan parameter terstruktur.

**Scheduled jobs berulang** (diadopsi dari Hermes Agent Nous Research, versi minimal): baris reminder bisa berupa *job* — prompt yang **dieksekusi agent sendiri** saat trigger tiba, dengan context segar dan akses tools penuh (`web_search`, `fetch_url`, `run_command`), bukan sekadar mengirim teks statis. Contoh: daily briefing pagi (search berita → rangkum → kirim), cek backup mingguan, laporan disk space. Dua field tambahan di `reminders`: `kind` (`static` | `job`) dan `recur` (NULL = one-shot, atau `daily`/`weekly`/cron expression). Job dieksekusi dengan **budget call terbatas** supaya tidak runaway.

### 5. Memory (curated, cross-session)
Fakta stabil tentang user yang bertahan lintas sesi, disimpan di tabel `memory` dengan **cap karakter ketat** (mencegah system prompt membengkak). Dua sumber pengisian:
- **Real-time:** LLM memanggil tool `save_memory(fact)` saat mendeteksi info penting secara langsung dalam percakapan
- **Background review** (lihat pilar 6) — proses pasif yang menangkap fakta yang terlewat

### 6. Self-Improvement Loop (background review, bukan fine-tuning)

**Klarifikasi penting:** ini BUKAN model fine-tuning / training. Model (Claude) tetap statis. "Self-improvement" di sini berarti proses ekstraksi & kurasi fakta otomatis yang berjalan async setelah tiap turn percakapan, terinspirasi dari pola "Deriver" milik Honcho (Plastic Labs) — tapi diimplementasikan sendiri secara ringan tanpa infra tambahan.

**Karakteristik:**
- Berjalan **async/fire-and-forget** (`tokio::spawn`) — TIDAK menambah latency respons ke user
- Menganalisis pertukaran pesan terbaru dan membedakan **2 level fakta**:
  - `[FACT]` — fakta eksplisit, disebut langsung oleh user
  - `[INFERRED]` — kesimpulan deduktif, disimpulkan LLM dari konteks (harus ditandai jelas sebagai dugaan, bukan fakta pasti, agar tidak menyesatkan sesi berikutnya)
- Prompt review harus eksplisit membedakan kedua kategori ini dan menghindari duplikasi dengan memory yang sudah ada
- **"Dreaming" (konsolidasi periodik):** proses terpisah, berjalan berkala (misal mingguan via cron), yang me-review seluruh memori tersimpan — gabungkan yang tumpang tindih, buang yang tidak relevan lagi, upgrade `[INFERRED]` jadi `[FACT]` jika terkonfirmasi, atau hapus jika kontradiksi dengan info baru

### 7. OCR Pipeline
Menangani input gambar dari Telegram (foto dokumen, screenshot, notes) supaya bisa dipahami oleh AI agent yang **belum vision-capable**. Alur: gambar diterima → diproses via Tesseract (lokal, native binding) → teks hasil ekstraksi dikirim sebagai prompt biasa ke AI provider yang sedang aktif.

**Catatan desain:**
- Dipilih OCR lokal (bukan panggil API vision eksternal) supaya tidak menambah provider/biaya per-call dan tetap sejalan dengan prinsip low-footprint
- Kualitas OCR sangat bergantung pada kualitas gambar input (foto miring/gelap/tulisan tangan hasilnya jauh lebih jelek dibanding dokumen/screenshot rapi) — ini trade-off yang disadari, bukan bug
- Karena outputnya berupa teks polos, hasil OCR otomatis kompatibel dengan provider manapun — vision-capable atau tidak (lihat Pilar 8)

### 8. Provider Abstraction Layer (Switchable AI Agent)
Agent tidak boleh terikat ke satu AI provider secara hardcoded. Owner memakai lebih dari satu AI agent/provider dan ingin bisa berpindah tanpa mengubah kode inti.

**Pendekatan:** trait-based abstraction di Rust — definisikan satu interface umum (kirim messages, terima balasan, tool calling jika didukung), lalu tiap provider (Anthropic, OpenAI-compatible, model lokal via Ollama, dst) mengimplementasikan interface tersebut secara terpisah. Provider aktif dipilih lewat config (misal env var `AI_PROVIDER`), bukan diganti manual di kode.

**Hal yang perlu diperhatikan (bukan blocker, tapi berpengaruh ke desain):**
- Format request/response berbeda tiap provider (Anthropic, OpenAI, model lokal masing-masing punya struktur API sendiri)
- Format tool calling juga berbeda tiap provider — kalau provider aktif tidak mendukung tool calling, fitur yang bergantung padanya (reminder, save_memory) butuh jalur fallback (misal prompting manual minta output JSON)
- Vision support berbeda-beda antar provider — inilah salah satu alasan utama kenapa OCR pipeline (Pilar 7) penting: dengan OCR, gambar selalu masuk sebagai teks universal, terlepas dari provider yang sedang aktif mendukung vision atau tidak

### 9. Shell Access (Command Execution)

Kemampuan owner menyuruh agent mengeksekusi shell command di VPS — coding, setting server, instalasi package, debugging, dsb. Diimplementasikan sebagai **native tool di agent loop** (`run_command`), sejajar dengan `create_reminder` dan `save_memory`.

**Keputusan desain — native tool, bukan wrap agent eksternal:**
- Alternatif yang sudah dievaluasi: goose (Rust, CLI + API, proyek Linux Foundation), Open Interpreter (fork Codex), pi CLI headless (`pi -p`) — semuanya ditolak untuk MVP karena masing-masing membawa agent loop + config + provider stack sendiri yang tumpang tindih dengan Hermes-Lite
- Native tool = ±150 baris Rust di atas infrastruktur yang sudah ada — konsisten dengan prinsip low-footprint dan evidence-driven
- Dispatch ke coding agent CLI eksternal dievaluasi ulang sebagai **phase 2**, hanya jika terbukti butuh sesi coding multi-file kompleks via Telegram

**Mekanik inti:**
- `tokio::process::Command` spawn `bash -lc "cd $cwd && <cmd>"` — one-shot per command (bukan PTY persisten; `cwd` dilacak sebagai state tool biar `cd` tetap efektif antar panggilan, meniru perilaku bash tool Claude Code)
- **Timeout per command** (default 120 detik) + kill process group supaya tidak meninggalkan proses yatim
- **Output handling:** truncate-only — tail output masuk context LLM (cap ±2000 karakter); **TIDAK ada file attachment** (`send_document` untuk output panjang dihapus — keputusan owner: file `.txt` yang terkirim otomatis lebih mengganggu daripada membantu). Mitigasi info hilang: deskripsi tool `run_command` mengarahkan agent pipe/limit dari awal (`| head`, `grep`, `LIMIT` di SQL) alih-alih dump penuh lalu filter. Batas 4096 karakter/pesan Telegram tetap di-handle gateway lewat chunking balasan agent
- **Tool pendamping `read_file` / `write_file`** dengan batasan path di workdir yang diizinkan — biar agent bisa baca config / tulis kode tanpa heredoc atau echo via shell (lebih aman, lebih hemat token)

**Keamanan — shell access artinya kebocoran token bot = remote code execution, jadi lapisan pertahanan wajib:**
1. **Hard allowlist `ALLOWED_CHAT_ID`** — pesan dari chat_id di luar allowlist di-drop total, tidak diproses, tidak masuk history
2. ~~Proses bot berjalan sebagai dedicated unprivileged user~~ — **REVISI KEPUTUSAN OWNER: service dijalankan sebagai root**, agar workflow admin + coding berjalan penuh tanpa friction (systemctl, dnf, docker, cargo build, edit /etc). Risiko indirect prompt injection via web/OCR → root RCE disadari & diterima secara eksplisit. Mitigasi yang tetap aktif: allowlist `ALLOWED_CHAT_ID`, confirmation gate destructive pattern, audit `command_logs`, secret masking, dan `WORK_ROOTS` yang membatasi `read_file`/`write_file` + cwd awal (`run_command` tetap bebas). Jalur downgrade nanti kalau mau lebih aman: sudoers allowlist per-command spesifik
3. **Confirmation gate untuk destructive pattern** (`rm -rf`, `dd`, `mkfs`, `fdisk`, `chmod -R`, `chown -R`, `reboot`, `curl | sh`, dsb.) — bot kirim inline keyboard konfirmasi ("Approve command?") dulu, eksekusi hanya setelah owner tap approve. Command non-destruktif jalan langsung tanpa konfirmasi (single-user, jangan ganggu flow)
4. **Audit trail** — semua command tercatat di tabel `command_logs` (chat_id, command, exit_code, duration, created_at)
5. **Secret masking** — nilai env var / credential tidak boleh bocor ke output yang dikirim ke Telegram (mask `API_KEY=...` style output)
6. stdin interaktif tidak didukung di MVP — command harus one-shot non-interaktif (yang butuh interaksi seperti `passwd` dijalankan lewat flag non-interaktif atau sudoers script spesifik)

### 10. Web Search & URL Fetching

Menutup celah **knowledge cutoff** LLM — tanpa web access, agent buta terhadap info terkini (versi library terbaru, berita, harga, error message baru). Diimplementasikan sebagai **dua tool client-side di agent loop** yang dieksekusi proses Rust sendiri — hasilnya kompatibel dengan semua provider (konsisten dengan Pilar 8).

**Tool:**
- **`web_search(query)`** → list hasil terstruktur {title, url, snippet}
- **`fetch_url(url)`** → konten halaman sebagai markdown bersih, siap masuk context

**Kenapa tool dedicated, bukan via `run_command` + curl:** hasil terstruktur dari API jauh lebih hemat token dibanding HTML mentah; search adalah operasi read-only yang aman secara default (tidak perlu confirmation gate); dan pemisahan concern memudahkan rate-limit/audit per capability.

**Search backend — swappable via `SEARCH_PROVIDER` (pola sama seperti `AI_PROVIDER`):**
- **Default: `jina` (s.jina.ai keyless)** — tier-1: nol setup, SERP dirender di sisi Jina, nol resource VPS; keyless ±20 RPM — naikkan via `JINA_API_KEY` (gratis, daftar jina.ai); request pakai header `X-Respond-With: no-content` (snippet-only, hemat token)
- **Alternatif: Tavily** — dibangun khusus untuk LLM agent, return konten LLM-ready (sering tidak perlu fetch URL terpisah = hemat token), free tier ±1000 credit/bulan — cukup jauh untuk single-user
- Alternatif yang disupport: `brave` (JSON stabil, free tier ±2000 query/bulan), `google_cse` (100/hari), `ddg_scrape` (zero-key, hanya untuk darurat — datacenter IP mudah kena CAPTCHA/block, terverifikasi saat riset)
- Bing Search API sudah pensiun — tidak dipertimbangkan

**fetch_url — fallback chain 4 tingkat (semua client-side via `reqwest`, nol biaya):**
1. **Direct fetch + header `Accept: text/markdown`** — content negotiation native Cloudflare "Markdown for Agents"; situs di balik Cloudflare yang opt-in langsung membalas markdown — tercepat, tanpa pihak ketiga, gratis selamanya
2. **markdown.new** — `GET https://markdown.new/<url>`; dibangun di atas infra Cloudflare (Workers AI `toMarkdown()` + Browser Rendering untuk halaman JS-heavy); dinyatakan free "always", rate limit eksplisit 500 req/hari/IP + header `x-rate-limit-remaining` untuk tracking
3. **r.jina.ai** (keyless) — cadangan kalau markdown.new down/limit; rate limit keyless ketat
4. **Plain HTML + crate `readability`** — last resort parsing lokal (dependency opsional, baru ditambahkan kalau tier 1-3 terbukti sering gagal)

**Hygiene & batasan (berlaku semua tier):**
1. **SSRF protection** — resolve DNS dulu, cek IP, baru fetch; block private/reserved range (`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, link-local `169.254.169.254` metadata, `::1`)
2. **Size cap** — konten > ~15-20KB dipotong untuk context (truncate-only, tanpa file attachment — konsisten dengan output handling Pilar 9)
3. **Timeout per request** (default 30 detik)
4. **Hanya URL publik** — jangan pernah fetch URL yang butuh auth/berisi data sensitif melalui chain ini (tier 2-3 melewati infra pihak ketiga; identitas operator markdown.new pun tidak jelas)
5. **No caching di MVP** — volume single-user kecil; tambahkan kalau terbukti quota jadi masalah (evidence-driven)

**Yang dievaluasi dan ditolak:**
- **SearXNG self-host** — service terpisah, melanggar prinsip satu proses low-footprint
- **Google/Bing scraping sebagai jalur utama** — fragile (CAPTCHA), rate-limit agresif
- **Perplexity / Exa** — berbayar di depan, tidak proporsional untuk kebutuhan MVP
- **Anthropic server-side `web_search` tool** — nol infra dan kualitas bagus (~$10/1000 search), tapi provider-specific sehingga bentrok dengan Pilar 8; dievaluasi ulang sebagai optimasi hanya kalau provider aktif = Anthropic dan volume search tinggi

### 11. Skills (Autonomous Skill Library)

Diadopsi dari fitur *autonomous skill creation* Hermes Agent (Nous Research), versi minimal. Komplementer dengan Memory (Pilar 5): **memory menyimpan fakta tentang user** ("owner deploy suka jam malam"), **skills menyimpan pengetahuan prosedural tentang cara mengerjakan sesuatu** ("cara renew sertifikat SSL di VPS ini: langkah 1-2-3, gotcha: port 80 dipakai apache").

**Mekanisme:**
- Tool `save_skill(name, content)` — saat agent berhasil menyelesaikan masalah non-trivial (setup, troubleshooting, konfigurasi, dsb.), ia menulis sendiri file skill berisi langkah, command yang terbukti jalan, dan gotchas
- Storage: file markdown di direktori `skills/`, satu file per skill — human-readable, bisa diedit manual seperti `soul.md`, git-able, tanpa schema DB baru
- Injection: daftar nama skill selalu dimuat ke system prompt; skill relevan dimuat penuh (matching sederhana by nama/keyword dulu — jangan buru-buru ke semantic matching, lihat Prinsip Retrieval)
- **Dreaming cycle (Pilar 6) juga me-review skills** — merge duplikat, update yang outdated, hapus yang terbukti salah; edit/hapus memakai `write_file` dan pola Pilar 9
- Aturan hemat: JANGAN auto-save untuk hal trivial (jawaban FAQ, one-liner) — hanya masalah non-trivial yang mungkin dihadapi lagi (ditegaskan di deskripsi tool)

**Kenapa file, bukan tabel:** jumlah skill single-user kecil (puluhan, bukan ribuan); file = zero-migration, mudah direview manual, konsisten dengan pola `soul.md`. Migrasi ke tabel hanya kalau terbukti butuh query/relasi (evidence-driven).

### 12. Image Generation (Text-to-Image)

Kemampuan agent membuat gambar dari deskripsi teks dan mengirimnya ke Telegram sebagai foto. Diadopsi karena terbukti murah dan **tidak butuh model vision-capable**.

**Kenapa kompatibel dengan agent non-vision:** model tidak pernah menyentuh data gambar — dia hanya memanggil tool `generate_image(prompt)` (input teks, output konfirmasi teks), sisanya dikerjakan backend. Polanya simetris dengan OCR (Pilar 7): image **input** butuh vision → diatasi OCR sebagai teks universal; image **output** tidak butuh vision → cukup HTTP call + `send_photo`.

**Implementasi:**
- **Pollinations.ai** — gratis, tanpa API key: `GET https://image.pollinations.ai/prompt/<url-encoded-prompt>` → bytes gambar → `send_photo` via teloxide. ±30 baris Rust dengan dependency yang sudah ada (reqwest + teloxide)
- Tool `generate_image(prompt)` — tool result berupa konfirmasi teks ("✅ gambar terkirim, 1024x1024")
- Timeout per request (default 60 detik — generasi bisa lambat), hasil ditampung in-memory/temp file, tidak perlu persist
- Sama seperti tools lain: ikut jalur fallback JSON-prompt kalau provider aktif tidak support native tool calling (Pilar 8)

**Batasan yang disadari (bukan bug):**
- Model tidak bisa "melihat" hasilnya — iterasi visual ("birunya kurang") hanya berupa re-generate dengan prompt yang dimodifikasi; verifikasi akhir tetap oleh owner via Telegram
- Kualitas bergantung model default Pollinations; opsi model/parameter ditambah belakangan kalau perlu

**Yang dihindari:** API image generation berbayar (DALL-E, Stability, dsb.) sebagai primary — tidak proporsional untuk volume single-user; hosting model diffusion lokal di VPS — footprint besar, kontradiktif dengan prinsip utama.

---

## Skema Database (konseptual)

- **`messages`** — short-term chat history per `chat_id` (role, content, timestamp)
- **`reminders`** — chat_id, message, remind_at, sent (boolean), kind (`static` | `job`), recur (NULL = one-shot, atau `daily`/`weekly`/cron expression)
- **`memory`** — chat_id, fact, type (`explicit` | `inferred`), created_at — dengan cap karakter total per chat_id
- **`command_logs`** — chat_id, command, exit_code, duration_ms, created_at — audit trail untuk semua eksekusi shell (Pilar 9)

Semua di satu Postgres instance yang sama; tidak perlu database terpisah.

---

## Prinsip Retrieval — Kapan Upgrade Kompleksitas

Urutan eskalasi yang disepakati (jangan lompat tahap tanpa bukti kebutuhan nyata):

1. **Curated memory saja** (MVP sekarang) — load semua fakta, karena volume masih kecil
2. **Full-Text Search (Postgres `tsvector`)** — kalau history mulai panjang dan butuh recall spesifik atas percakapan lama. Catatan: stemmer default Postgres kurang optimal untuk Bahasa Indonesia, mungkin perlu konfigurasi dictionary tambahan; untuk istilah teknis/kode, keyword matching literal justru sering lebih presisi daripada semantic search
3. **Vector search (pgvector + embedding API)** — HANYA jika FTS terbukti tidak cukup menangkap kemiripan makna/paraphrase. Ini menambah dependency provider embedding eksternal, cost per operasi, dan kompleksitas index — jangan mulai dari sini

---

## Catatan Bahasa & Komunikasi

- Owner berkomunikasi campur Bahasa Indonesia dan Inggris secara natural dalam percakapan sehari-hari maupun saat vibe coding
- Istilah teknis (nama tabel, library, konsep pemrograman) sebaiknya tetap dalam Bahasa Inggris/istilah aslinya, jangan diterjemahkan paksa
- Prioritaskan kejelasan teknis dibanding konsistensi bahasa — tidak masalah kalau satu respons/dokumen campur dua bahasa selama maksudnya jelas

---

## Non-Goals (eksplisit di luar scope MVP)

- Multi-platform gateway (Discord, Slack, WhatsApp, dll) — fokus Telegram saja
- Voice transcription (Whisper) — butuh compute yang tidak proporsional untuk VPS kecil
- Subagent delegation / container isolation per task
- Model fine-tuning / RL training pipeline
- IDE integration (VS Code, JetBrains, dll)
- Multi-user / multi-tenant support
- OCR via API vision eksternal (Google Vision, Azure Cognitive Services) — cukup Tesseract lokal untuk MVP
- Auto-switch provider berdasarkan konteks/biaya secara otomatis — switching tetap manual via config, bukan logic pemilihan otomatis
- Wrap coding agent eksternal (goose, Open Interpreter, pi CLI headless) untuk eksekusi shell/coding — implementasi native tool (`run_command`) dipilih untuk MVP; dispatch ke agent eksternal dievaluasi ulang hanya jika ada bukti nyata butuh sesi coding multi-file kompleks via Telegram
- Shell interaktif / PTY persisten di MVP — command berjalan one-shot non-interaktif
- Metasearch engine self-host (SearXNG dsb.) — service terpisah, langgar prinsip single-process low-footprint
- Scraping search engine (Google/Bing/DDG) sebagai jalur utama — fragile dan mudah kena CAPTCHA; hanya `ddg_scrape` sebagai fallback darurat
- Browser automation lokal (headless Chrome di VPS) untuk fetching — resource berat untuk VPS kecil; halaman JS-heavy sudah tertangani tier Browser Rendering milik markdown.new
- Search provider berbayar (Perplexity, Exa, Serper) sebagai primary — tidak proporsional untuk volume single-user
- Text-to-speech / voice note output — opsi lokal (Piper TTS) menambah footprint tidak proporsional untuk VPS kecil; opsi cloud gratis (Edge TTS) menambah ketergantungan eksternal tanpa SLA — defer sampai ada permintaan nyata. (Image generation sudah diadopsi di Pilar 12 karena murah dan tidak butuh vision)
