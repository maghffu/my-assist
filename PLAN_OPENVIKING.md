# Plan Eksekusi — Adopsi Pola OpenViking (Memory v3, Skills L0, Session Summary)

Pendamping `AGENTS.md` + `ROADMAP.md`. Ini plan eksekusi untuk adopsi pola **OpenViking**
(volcengine/OpenViking — context database untuk AI agent) yang dinilai layak diambil
setelah riset 2026-09-04. Yang diadopsi adalah **pola/behavior**-nya, BUKAN infrastrukturnya.

**Yang diadopsi (dipetakan ke pilar):**

| Fase | Pola OpenViking | Pilar Hermes-Lite | DB impact |
|---|---|---|---|
| OV-1 | L0 abstract untuk skills (`.abstract.md` → frontmatter `description:`) | Pilar 11 | — (file) |
| OV-2 | Session archive + summarization (recent N dipertahankan, sisanya jadi ringkasan L0/L1) | Pilar 2 + 6 | tabel baru `session_summaries` |
| OV-3 | Memory types terstruktur (`profile`/`preferences`/`entities`/`events`) + `memory_diff` audit | Pilar 5 + 6 | kolom `memory.kind` + tabel `memory_changes` |
| OV-4 | Observable retrieval (trajectory per query bisa didebug) | lintas pilar | — |

**Yang TIDAK diadopsi (guard rail — melanggar prinsip low-footprint / belum ada bukti butuh):**

- ❌ `viking://` filesystem + vector index + embedding provider — service terpisah; memory di
  Postgres + skills di file sudah proporsional untuk volume single-user
- ❌ LLM intent analyzer per pesan — tambah biaya + latency per turn untuk volume kecil
- ❌ Parser PDF/HTML/video/code-repo ingestion — bukan use case Telegram assistant
- ❌ Freshness bubbling / sampling parent summary — bahkan OpenViking sendiri menandai ini
  write amplification (TODO di docs mereka); terlalu kompleks untuk puluhan skill
- ❌ Hierarchical retrieval / directory recursion — itu eskalasi tahap-3 (pgvector) menurut
  Prinsip Retrieval AGENTS.md; baru dievaluasi kalau FTS terbukti kurang

**Aturan umum semua fase:** additive-only migration (binary lama tetap jalan di schema baru),
tanpa dependency baru (frontmatter di-parse manual, ±20 baris), semua LLM call background
via `tokio::spawn` (pola Pilar 6) — tidak menambah latency respons owner.

---

## Fase OV-1 — Skill L0: description frontmatter

**Masalah:** `section_for_prompt` hanya meng-inject daftar NAMA skill; keyword matching hanya
cocok token dari nama (`renew-ssl-nginx` → `renew`, `ssl`, `nginx`). Skill bernama bagus tapi
topiknya tidak tercakup namanya tidak akan pernah match — dan LLM tidak tahu isi skill yang
belum dimuat. OpenViking menyelesaikan ini dengan L0 `.abstract.md` satu kalimat per entri.

**Desain:** frontmatter YAML minimal di kepala tiap file skill:

```markdown
---
description: Renew sertifikat SSL via certbot di nginx, termasuk urusan port 80 yang dipakai apache
---
## Langkah
1. ...
```

### Deliverable

1. **`src/skills.rs`:**
   - `SkillMeta` tambah field `description: String`
   - `parse_description(content: &str) -> String` — parse manual: kalau content diawali `---`,
     cari `---` penutup, ambil baris `description: ...` (trim). Tidak pakai crate YAML.
   - **Fallback tanpa frontmatter:** baris pertama non-kosong yang BUKAN heading (`# ...`) —
     atau heading itu sendiri minus `#` — dijadikan description. Semua skill lama otomatis
     dapat L0 tanpa backfill manual.
   - `section_for_prompt`: daftar jadi `- renew-ssl-nginx — <description>`; keyword matching
     diperluas: token dari nama **plus** token dari description (tetap ≥3 char,
     `INJECT_MAX_FILES`/`INJECT_MAX_CHARS` tidak berubah). Fallback description ikut dipakai
     matching sehingga skill lama langsung menikmati perluasan ini.
2. **`src/tools/mod.rs` — tool `save_skill`:**
   - Param baru opsional `description: string` (satu kalimat, ≤160 char — divalidasi)
   - Kalau kosong → derive dari baris pertama konten (fallback yang sama)
   - `save_skill()` menulis frontmatter di kepala file; overwrite lama mengganti description
3. **`src/review.rs` — dreaming skills review:**
   - Listing skill ke LLM menyertakan description (sudah otomatis lewat konten)
   - Instruksi prompt: saat `rewrite`, PERTAHANKAN/diperbarui frontmatter description
   - Safety net `rewrite_skill()`: kalau konten hasil rewrite tidak punya frontmatter padahal
     file lama punya → prepend description lama (jangan sampah L0 karena rewrite)
4. **`src/gateway.rs` — `/skills`:** tampilkan `name — description` (bukan nama saja)

### File yang disentuh
`src/skills.rs`, `src/tools/mod.rs`, `src/review.rs`, `src/gateway.rs` — **tanpa migration**.

### Kriteria selesai
- `cargo test`: unit test parse frontmatter (ada/tidak ada/heading fallback), matching via
  token description, rewrite-preserve-frontmatter
- Live: `/skills` menampilkan description; pesan menyebut topik yang HANYA ada di description
  (bukan di nama) → skill ter-load penuh

---

## Fase OV-2 — Session Summary (rolling archive, adopsi session commit OpenViking)

**Masalah:** context = N pesan terakhir (`N_CONTEXT`). Pesan yang jatuh dari window hilang
diam-diam — percakapan panjang kehilangan benang merah tanpa jejak. OpenViking: recent N
rounds dipertahankan, pesan lebih tua di-archive menjadi summary terstruktur; ini sumber
klaim penghematan token terbesar mereka (34–91% di LoCoMo).

**Desain — ROLLING summary per chat (bukan per-segment ala OpenViking):**

Satu baris `session_summaries` per `chat_id` berisi ringkasan konsolidat yang terus diperluas.
Kalau cap tercapai, summary lama + batch baru di-recompress jadi summary baru (self-compacting).
Per-segment archive (satu row per sesi) ditolak untuk sekarang — menambah kompleksitas tanpa
bukti butuh recall per segmen (evidence-driven); rolling cukup untuk single-user.

### Migration `migrations/0006_session_summaries.sql`

```sql
-- Session summary (adopsi pola session-commit OpenViking): pesan yang jatuh dari
-- context window diringkas jadi rolling summary per chat — bukan hilang diam-diam.
-- Additive-only: binary lama tidak menyentuh tabel ini.
CREATE TABLE IF NOT EXISTS session_summaries (
    chat_id     BIGINT PRIMARY KEY,
    summary     TEXT        NOT NULL,
    archived_to BIGINT      NOT NULL DEFAULT 0,  -- id pesan terakhir yang sudah diarsipkan
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

### Algoritma (modul baru `src/summary.rs`)

```
Trigger : post-turn spawn (setelah spawn_post_turn_review, di gateway) — fire-and-forget
Guard   : satu tokio::Mutex global (single-user — tidak perlu per-chat); try_lock gagal →
          skip turn ini (bukan queue, hindari penumpukan)
Batch   : candidates = messages WHERE chat_id=$1 AND id > archived_to
          count_total - N_CONTEXT >= MIN_ARCHIVE_BATCH (10) → ambil
          (count_total - N_CONTEXT) pesan TERLAMAU sebagai batch (window hidup tetap utuh)
Summarize: LLM(existing_summary, batch) → ringkasan gabungan ≤ SUMMARY_MAX_CHARS (3000)
          — instruksikan: pertahankan fakta/keputusan/nama file/nomor, buang basa-basi
Commit  : UPSERT session_summaries (summary, archived_to = max(batch.id), updated_at)
          → DELETE messages batch → release lock
Fail-safe: LLM call gagal → TIDAK ada delete, log warn, batch di-retry turn berikutnya
```

- LLM call internal tanpa tools — reuse helper `call_llm_text` (dipindah/di-expose
  `pub(crate)` dari `review.rs`, atau jadi method `Agent` — pilih saat implementasi)
- Karakteristik prompt ringkasan = turunan L1 overview OpenViking: konteks + keputusan +
  hasil, bukan transkrip

### Injection ke system prompt

- `agent.rs` `build_system_prompt` tambah section **`## Riwayat percakapan sebelumnya (ringkasan)`**
  (cap `SUMMARY_INJECT_CAP` = 2500 char) — diletakkan sebelum section Memory
- Di-load per turn: `SELECT summary FROM session_summaries WHERE chat_id = $1`
- **Termasuk untuk scheduled job** (`include_history = false`): ringkasan = konteks stabil
  hasil distilasi, bukan window hidup — justru ini cara job tahu apa yang terakhir dikerjakan
  (trade-off dicatat: biaya ±2.5k char vs kontinuitas; kalau terbukti boros, config flag menyusul)

### Integrasi lain

- **`/new` (gateway.rs):** `DELETE FROM session_summaries WHERE chat_id = $1` — reset session
  = reset ringkasan juga (konsisten semantik "session baru")
- **`/status`:** tampilkan `summary: X char (arsip s/d msg #N)`

### File yang disentuh
`migrations/0006_session_summaries.sql` (baru), `src/summary.rs` (baru),
`src/agent.rs`, `src/gateway.rs`, `src/review.rs` (expose helper).

### Kriteria selesai
- `cargo test`: unit test matematika batch & cap (trim di boundary char, bukan byte),
  upsert idempoten
- Live: percakapan > N_CONTEXT+10 pesan → log "session archived: N pesan → X char summary",
  pesan terus nyambung konteksnya setelah window ganti; `/new` mereset; LLM gagal →
  pesan TIDAK hilang (verifikasi via log retry turn berikutnya)

---

## Fase OV-3 — Memory v3: kinds + audit trail (adopsi memory types + memory_diff OpenViking)

**Masalah:** memory flat — hanya `explicit|inferred`. Ratusan fakta tanpa kategori sulit
dibaca owner (`/memory`), sulit di-dedupe oleh dreaming, dan perubahan oleh dreaming/review
terjadi tanpa jejak (owner tidak tahu apa yang "diubah sendiri" oleh agent).

**Desain — kind taxonomy 4+general** (OpenViking pakai 9 types; kita pangkas —
`identity`/`soul` sudah dipegang `soul.md` Pilar 3, `cases`/`trajectories`/`experiences`
sudah tercakup Skills Pilar 11):

| kind | Isi | Contoh |
|---|---|---|
| `profile` | Identitas dasar owner | nama, lokasi, pekerjaan, keluarga |
| `preference` | Preferensi & kebiasaan | "deploy suka jam malam", "jawaban singkat" |
| `entity` | Orang/proyek/objek yang berulang | "VPS utama = 2 vCPU di Jakarta", domain, stack |
| `event` | Peristiwa & keputusan bertanggal | "23 Agu ikut race 10K", "pindah ke Postgres 17" |
| `general` | Fallback (default baris lama) | sisanya |

### Migration `migrations/0007_memory_kinds_audit.sql`

```sql
-- Memory v3 (adopsi memory-types OpenViking, dipangkas utk single-user):
-- kolom kind + audit trail semua perubahan memory (pola memory_diff.json OpenViking).
-- Additive-only; baris lama default 'general' — diklasifikasi ulang oleh dreaming.

ALTER TABLE memory ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'general'
    CHECK (kind IN ('profile', 'preference', 'entity', 'event', 'general'));

CREATE TABLE IF NOT EXISTS memory_changes (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT      NOT NULL,
    memory_id  BIGINT,               -- NULL = baris sudah dihapus (snapshot di old_*)
    action     TEXT        NOT NULL CHECK (action IN ('insert','update','delete','retype','reclassify')),
    old_fact   TEXT, new_fact TEXT,
    old_type   TEXT, new_type TEXT,
    old_kind   TEXT, new_kind TEXT,
    source     TEXT        NOT NULL CHECK (source IN ('agent','review','dream','manual')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_memory_changes_chat ON memory_changes (chat_id, id DESC);
```

### Ubah cara agent menyimpan memory (semua jalur tulis)

1. **`src/memory.rs` — semua write fn tambah param `source: &str` + wajib menulis audit:**
   - `save_fact(pool, chat_id, fact, fact_type, kind, source)` → INSERT + audit `insert`
   - `update_fact(...)` → audit `update` (old_fact/new_fact — old dibaca dulu)
   - `delete_fact(...)` → audit `delete` (snapshot old_* sebelum DELETE)
   - `set_fact_type(...)` → audit `retype`
   - **Baru:** `set_kind(pool, chat_id, id, kind, source)` → audit `reclassify`
   - Caller yang tidak lolos cap (`save_fact` return warning) TIDAK menulis audit — bukan perubahan
2. **`src/tools/mod.rs` — tool `save_memory`:**
   - Param baru `kind` (enum 5 nilai, default `general`)
   - Description diperluas: panduan singkat memilih kind + contoh
3. **`src/review.rs` — post-turn review:**
   - Output schema jadi `{"fact","type","kind"}`; prompt mengajarkan taxonomy + "kalau ragu,
     general". Existing block di prompt juga menampilkan kind tiap fakta
4. **`src/review.rs` — dreaming cycle:**
   - Listing memory menyertakan kind
   - **Action baru `"reclassify": {"id", "kind"}`** — sekaligus mekanisme backfill: baris
     `general` lama diklasifikasi ulang bertahap tiap cycle (tidak ada one-time LLM migration)
   - Action `upgrade`/`rewrite`/`patch`/`drop` — semua kini via fn beraudit (source `dream`)
5. **Recall & display:**
   - `recall_facts`: output dikelompokkan per kind dengan sub-header (`### Profil`, `###
     Preferensi`, dst. — hanya header yang isinya non-kosong). Budget/dedupe/hotness logic
     TIDAK berubah
   - `/memory` (gateway): tampilkan tag kind per baris — `[42] (explicit|preference) ...`

### `/memory log` — audit reader (baru)

```
/memory log        → 10 perubahan terakhir: #42 update [dream] "lama…" → "baru…"
/memory log 25     → 25 terakhir
```

`/memory del` → source `manual`; tool `save_memory` → `agent`; post-turn review → `review`.

### File yang disentuh
`migrations/0007_memory_kinds_audit.sql` (baru), `src/memory.rs`, `src/tools/mod.rs`,
`src/review.rs`, `src/gateway.rs`, `src/agent.rs` (system prompt: sebut taxonomy kind).

### Kriteria selesai
- `cargo test`: validasi kind, audit row shape per aksi, recall grouping, reclassify path
- Live: `save_memory` dengan kind baru → `/memory` tampil dengan tag; `/memory del` → muncul
  di `/memory log` sebagai `delete [manual]`; `/dream` menghasilkan ≥1 `reclassify` pada
  baris general lama; sebelum/sesudah `rewrite` terlihat old→new di log

---

## Fase OV-4 — Observability: context trace (adopsi observable retrieval)

**Masalah:** tidak ada jejak KONTEKS APA yang dikirim per turn — memory mana yang masuk,
skill mana yang di-inject, berapa char system prompt. Tuning recall (v2) jadi guesswork.
OpenViking menyimpan trajectory retrieval per query untuk di-debug; versi kita: breakdown
per turn, murah.

### Deliverable

1. **`src/agent.rs`:**
   - Struct `ContextTrace { memory_facts: usize, memory_chars: usize, skills_listed: usize,
     skills_injected: usize, summary_chars: usize, history_msgs: usize, history_chars: usize,
     system_chars: usize }`
   - Diisi di `run_turn` (data sudah ada di titik itu — tinggal dihitung), disimpan ke
     `Arc<Mutex<Option<ContextTrace>>>` (trace TERAKHIR saja, in-memory) +
     `tracing::info!` per turn
2. **`/status` (gateway):** tambah baris "konteks turn terakhir" — angka-angka trace di atas

### File yang disentuh
`src/agent.rs`, `src/gateway.rs` — **tanpa migration**.

### Kriteria selesai
- Log per turn memuat breakdown; `/status` menampilkan trace terakhir setelah satu pertanyaan

---

## Urutan eksekusi & estimasi

| Urut | Fase | Estimasi | Alasan urutan |
|---|---|---|---|
| 1 | OV-1 Skills L0 | ±1.5 jam | Terkecil, zero-risk, langsung terasa di matching |
| 2 | OV-3 Memory kinds + audit | ±3 jam | Menyentuh jalur tulis memory — dikerjakan sebelum OV-2 supaya summary & recall final bentuknya sekalian |
| 3 | OV-2 Session summary | ±4 jam | Butuh loop LLM baru; independen dari OV-3 tapi sistem prompt-nya kena dua-duanya |
| 4 | OV-4 Trace | ±1 jam | Bonus; paling berguna SETELAH OV-2/OV-3 ada (ada yang dilacak) |

Tiap fase ditutup dengan `cargo build` + `cargo test` hijau dan commit terpisah — konsisten
konvensi ROADMAP (tidak ada fase menutup dengan kode setengah rusak).

## Rollback

Kedua migration additive-only: binary lama jalan di atas schema baru (kolom punya default,
tabel baru tidak disentuh binary lama). Rollback manual bila perlu:
`DROP TABLE session_summaries; DROP TABLE memory_changes; ALTER TABLE memory DROP COLUMN kind;`

> **KOREKSI pasca-insiden 5 Sep 00:00 (downtime 2 menit):** klaim "binary lama jalan di atas
> schema baru" hanya berlaku untuk proses yang SUDAH berjalan — binary lama yang di-RESTART
> menolak hidup karena sqlx memvalidasi `_sqlx_migrations` versi lebih baru ("migration N
> previously applied but is missing in the resolved migrations"). Jadi: jangan pernah apply
> migration ke DB prod sebelum binary yang memuatnya ter-deploy (lihat gotcha di skill
> rebuild-deploy-hermes-lite.md). Terjadi karena `cargo run -- migrate` verifikasi manual
> dari repo menunjuk DB prod yang sama.

## Delta AGENTS.md (dilakukan saat eksekusi, bukan sekarang)

- **Pilar 2:** context = window hidup + rolling session summary (tabel `session_summaries`)
- **Pilar 5:** memory bertipe kind (`profile|preference|entity|event|general`) + audit
  `memory_changes`; semua jalur tulis (tool/review/dream/manual) tercatat
- **Pilar 11:** skill file berformat frontmatter `description:` (L0) — matching & injection
  memakai nama + description
- **Skema Database:** tambah `session_summaries`, `memory_changes`, kolom `memory.kind`

## Log Keputusan Singkat

1. **Rolling single summary, bukan per-segment** — volume single-user kecil; segment archive
   = kompleksitas tanpa bukti butuh recall per segmen. Cap 3000 char + self-compaction.
2. **Taxonomy 4+general, bukan 9 types OpenViking** — `identity`/`soul` sudah di `soul.md`,
   `experiences`/`trajectories` sudah di Skills; menambah type = menambah kebisingan prompt.
3. **Audit tabel terpisah, bukan reuse `messages`** — beda lifecycle (append-only, bukan
   context window), murah, dan tidak mencemari history percakapan.
4. **Frontmatter di-parse manual (tanpa crate)** — satu field `description:`; menambah
   dependency YAML untuk ini melanggar prinsip low-footprint.
5. **Summary ikut di-inject untuk scheduled job** — ringkasan = distilasi stabil, bukan
   window hidup; ini satu-satunya cara job harian tahu progres terakhir. Trade-off token
   dicatat, config flag menyusul kalau terbukti boros.
6. **Pesan terarsip DIHAPUS setelah summary commit** — ringkasan adalah arsipnya; tabel
   `messages` tetap ramping (sekaligus guard pertumbuhan yang selama ini tidak ada).
   Fail-safe: hapus hanya setelah UPSERT summary sukses.
7. **Backfill kind via dreaming bertahap, bukan one-time LLM migration** — action
   `reclassify` + listing yang menyertakan kind lama; baris `general` menyusut sendiri
   dari cycle ke cycle.
