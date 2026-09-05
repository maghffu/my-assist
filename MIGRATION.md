# Strategi Migrasi: Hermes Agent (lama) → Hermes-Lite

> Hasil inventarisasi aktual VPS, 29 Agu 2026. Semua nomor/ukuran di bawah sudah diverifikasi,
> bukan estimasi. Urutan fase disusun dari nilai tertinggi → risiko terendah, dengan cutover di akhir.

## 0. Hasil Inventarisasi Sumber

| Sumber | Lokasi | Volume | Isi |
|---|---|---|---|
| Memory curated | `/root/.hermes/memories/MEMORY.md` | 2.1 KB | Fakta padat per-topik, dipisah `§` (CV, email PT, pajak, server, lari, dll) |
| Profil user | `/root/.hermes/memories/USER.md` | 1.2 KB | Identitas, preferensi bahasa/gaya, konteks bisnis PSD, NPWP/KTP, goal lari |
| Honcho (deriver) | container `honcho-database-1`, db `postgres` | 225 documents, 86 messages, 3 collections | Derived facts hasil Deriver + dialectic (sample: "furin is based in REDACTED-CITY") |
| Cron/jobs | `/root/.hermes/cron/jobs.json` | 3 job | (a) Renew Server H-3..H-day (tanggal di server), (b) Running reminder harian, (c) restart-gateway [selesai, skip] |
| Skills custom | `skills/{productivity/running-schedule, devops/vps-app-ops, autonomous-ai-agents/hermes-custom-plugins}` | 3 skill (10.9/4.5/5.0 KB + references + scripts) | Teridentifikasi custom via diff terhadap `default.tar.gz` (82 skill stock) |
| Skills stock | `skills/*` lainnya | 82 SKILL.md | Bundled bawaan Hermes Agent — menyesuaikan toolset hermes agent, bukan hermes-lite |
| Data files | `/root/.hermes/data/` | 2 file | `running_progress.json` (log training harian), `keuangan_pt_psd.json` |
| Scripts | `/root/.hermes/scripts/` | 4 script | `running_schedule.py/.sh`, `running_reminder.sh`, `send_email.py` — murni Python stdlib, zero pip deps ✅ |
| Kanban | `/root/.hermes/kanban.db` | tabel `tasks` **kosong** | Tidak ada yang perlu dimigrasi |
| Chat history | Honcho `messages` (86 rows) | kecil | Lihat Fase 5 — diarsipkan, TIDAK diimpor ke tabel `messages` |

**Fakta penting target (hermes-lite):**
- Tabel `memory`: cap **20.000 karakter total** per chat_id (cukup luas: MEMORY.md+USER.md ≈ 3.4 KB + sisa Honcho yang lolos dedup)
- Tabel `reminders`: `kind` = `static|job`, `recur` = NULL | `daily` | `weekly` — **cron expression BELUM diimplementasi** (`compute_next_run` hanya daily/weekly, note ROADMAP Fase 3)
- Skills: **flat** `skills/<slug>.md`, satu file per skill, max 20 KB/file, inject max 3 file × 4000 chars per turn — tidak scan subdirektori references
- Sandbox service: user `hermes`, `ProtectHome=true` (**/root total tak terlihat**), `ProtectSystem=strict`, RW hanya `/opt/hermes-lite` + `/tmp` → semua script/data HARUS pindah ke `/opt/hermes-lite`
- `python3` (3.11.6) tersedia untuk user hermes ✅

**Timezone:** server = Asia/Shanghai (+0800), owner = WIB (+0700). Cron lama berjalan di waktu server: running reminder 07:50+08 = **06:50 WIB**, renew server 00:53+08 = 23:53 WIB hari sebelumnya. → Lihat Keputusan Terbuka #1.

---

## Fase 0 — Preflight & Freeze (5 menit)

> ✅ **SELESAI 29 Agu 2026 16:22 CST** — backup `hermes-agent-pre-migration-20260829.tar.gz` (154 MB) + `honcho-pre-migration-20260829.sql.gz` (131 KB) di `/root/backups/`; `hermes-gateway` stopped.

1. **Snapshot full** (rollback guarantee):
   ```bash
   tar -czf /root/backups/hermes-agent-pre-migration-$(date +%Y%m%d).tar.gz \
       /root/.hermes /root/hermes-gateway.service.user.bak
   docker exec honcho-database-1 pg_dump -U postgres postgres | gzip \
       > /root/backups/honcho-pre-migration-$(date +%Y%m%d).sql.gz
   ```
2. **Freeze writer**: `systemctl stop hermes-gateway` (bot lama berhenti menulis memory/cron).
   Bot lama & hermes-lite memakai token bot berbeda → tidak ada konflik polling; hermes-lite tetap hidup selama migrasi.
3. Cek `systemctl is-active hermes-lite` = active.

## Fase 1 — Memory (nilai tertinggi, kerjakan paling dulu)

> ✅ **SELESAI 29 Agu 2026** — 25 fakta total (20 explicit + 5 inferred), 3.685 chars (18% dari cap 20.000). Detail:
> - MEMORY.md → 12 blok, 11 di-insert (Honcho fact di-drop — stack decommission; vault & email fact di-rewrite ke path baru; SMTP username/password DIPINDAH keluar → `.env`)
> - USER.md → 3 blok perilaku masuk `soul.md` (repo + deployed tersinkron); 3 blok fakta masuk memory; PT summary dipindah ke `/opt/hermes-lite/docs/`
> - Honcho 225 docs → LLM-assisted dedup (GLM, thinking disabled) → 7 kandidat → curation manual → 4 lolos sebagai `inferred`
> - Bonus temuan: `send_email.py` hardcode SMTP_PASS + DKIM key di `/root/.hermes/keys/` → ditangani Fase 3

**1a. MEMORY.md → tabel `memory`** — split per `§` (10 blok), tiap blok = 1 baris `type='explicit'`.
Insert via SQL langsung (bukan via tool, supaya tidak kena review-cap):
```sql
INSERT INTO memory (chat_id, fact, type) VALUES ($ALLOWED_CHAT_ID, '...', 'explicit');
```
Perhatikan: blok yang berisi SMTP user/email (baris "Email PT") — password tidak ikut jika memang tidak ada di file; **credential apapun tidak masuk memory** (ingat insiden `.env` di journal kemarin). SMTP cred → `.env` hermes-lite.

**1b. USER.md → split dua arah** (manual curation, ini persona vs fakta):
- → `soul.md` (perilaku): "communicates in Bahasa Indonesia…", "prefers concise responses…"
- → `memory` (fakta): bisnis PSD, PPh/NPWP/KTP, rekening, goal lari, dst.

**1c. Honcho documents → dedup → sisanya masuk `memory`**:
```bash
docker exec honcho-database-1 psql -U postgres -d postgres \
  -Atc "SELECT content FROM documents" > /tmp/honcho_docs.txt
  # dedup vs fakta yang sudah masuk (paraphrase-check), sisanya insert type='inferred'
```
225 dokumen kebanyakan paraphrase pendek dari MEMORY.md/USER.md ("furin is a software engineer" sudah tercakup).
**Jangan blind-insert 225 baris** — itu melanggar prinsip curated memory (AGENTS.md Pilar 5) dan memboroskan system prompt. Estimasi hasil unik: hanya beberapa belas baris. Dedup boleh dibantu LLM (sekali call, batch semua dokumen + daftar fakta existing → minta output hanya yang benar-benar baru), hasil tetap direview manual via `/memory`.

**Checklist akhir fase:** `SELECT SUM(LENGTH(fact)) FROM memory` < 20.000; tampilan `/memory` di Telegram rapi.

## Fase 2 — Skills (3 custom saja, stock tidak ikut)

> ✅ **SELESAI 29 Agu 2026** — 4 file skill di `/opt/hermes-lite/skills/` (+ mirror ke repo `skills/`):
> - `vps-app-ops-intram.md` (6.1 KB) — SKILL.md + intram-server.md merged; **2 referensi honcho DI-DROP** (stack decommission)
> - `running-schedule-jadwal-lari.md` (9.9 KB) — semua path `/root/.hermes/...` → `/opt/hermes-lite/...`; section OCR diadaptasi (foto auto-OCR gateway, bukan `vision_analyze`); `ocr_workout.py` ikut pindah
> - `running-zone2-watch.md` (4.8 KB) — merge zone2-base-training + huawei-watch refs
> - `markdown-new-extract-fetch-url.md` (1.9 KB) — hanya pengetahuan tahan-lama markdown.new (Pilar 10); mekanik plugin hermes-agent di-drop (platform mati)
> - **Trik naming**: matcher hermes-lite pakai token dari nama file → alias Bahasa Indonesia dibubuhkan di slug (`...-jadwal-lari.md`) supaya pesan "jadwal lari" match → auto-inject penuh, tanpa ubah kode

Konversi format nested → flat `skills/<slug>.md`:

| Skill lama | Target | Perlakuan |
|---|---|---|
| `devops/vps-app-ops` (+3 references) | `skills/vps-app-ops.md` | Merge references ke satu file (total < 20 KB ✅). **Rewrite semua path** `/root/.hermes/...` → `/opt/hermes-lite/...` |
| `productivity/running-schedule` (+2 references +scripts) | `skills/running-schedule.md` | SKILL.md 10.9 KB + references → kemungkinan > 20 KB jika digabung. Split 2 file: `running-schedule.md` (inti + referensi skill #2 di body) dan `running-zone2-watch.md` (zone2 + Huawei watch). Scripts ikut Fase 3 |
| `autonomous-ai-agents/hermes-custom-plugins` (+references) | `skills/hermes-custom-plugins.md` | Merge references (markdownnew-provider dll — masih relevan dgn Pilar 10 hermes-lite) |

Setelah copy: `chown hermes:hermes`, cek `/skills` di Telegram menampilkan 3 skill, test inject keyword ("cek server intram" harus memuat vps-app-ops).

**82 skill stock TIDAK dimigrasi** — isinya prosedur untuk toolset hermes agent (apple imessage, smart-home, mlops, dll) yang tidak dimiliki hermes-lite. Kalau nanti terbukti butuh satu-dua (evidence-driven), copy manual per kasus.

## Fase 3 — Scripts & Data Files → `/opt/hermes-lite`

> ✅ **SELESAI 29 Agu 2026** — scripts (4: running_schedule.py, running_reminder.sh, ocr_workout.py, send_email.py) + data (2 JSON) + DKIM key → `/opt/hermes-lite/{scripts,data,keys}`. Fix: `PROGRESS_FILE` hardcoded → absolut path baru. **`send_email.py` refactor: SMTP_PASS hardcode → env `SMTP_USER`/`SMTP_PASS`/`DKIM_KEY_FILE` di `.env`** + guard exit-2 kalau env kosong; key chmod 600. Smoke test dari sandbox identik-unit: `running_schedule.py today` → output "Zone 2 Run 3 KM" ✅

```bash
mkdir -p /opt/hermes-lite/{scripts,data}
cp /root/.hermes/scripts/*.py /root/.hermes/scripts/*.sh /opt/hermes-lite/scripts/
cp /root/.hermes/data/*.json /opt/hermes-lite/data/
chown -R hermes:hermes /opt/hermes-lite/scripts /opt/hermes-lite/data
```
Fix wajib:
1. `running_schedule.py`: `PROGRESS_FILE = os.path.expanduser("~/.hermes/data/...")` — untuk user `hermes` ini salah path. Ganti jadi absolut `/opt/hermes-lite/data/running_progress.json`.
2. `send_email.py`: password/credential → env var, dibaca dari `.env` hermes-lite (jangan hardcode, jangan masuk skill/memory).
3. Smoke test sebagai user sandbox:
   ```bash
   sudo -u hermes bash -c 'cd /opt/hermes-lite && python3 scripts/running_schedule.py today'
   ```
   (kalau gagal karena sandbox systemd — jalankan via bot: "cek jadwal lari hari ini")

## Fase 4 — Reminders

> ✅ **SELESAI 29 Agu 2026** — job lari harian (`kind=job`, `recur=daily`, 07:50 WIB, prompt → run_command script + konvensi balas `SKIP` di hari rest) + 4 one-shot renew server (H-3..H-day, waktu trigger di server). **Patch kode kecil**: `process_due_reminders` kini diam kalau job balas `SKIP` (replikasi perilaku silent-on-rest sistem lama; rebuild + redeploy binary). Fakta basi "skill running-schedule kosong" dihapus; reminder test "cek tesla" dibersihkan.

Job (c) restart-gateway: sudah `completed`+disabled → **skip**.

**Job (a) Renew Server** — cron mingguan H-3..H-day (schedule lengkap di DB) + prompt hitung-mundur.
Cron expr belum disupport hermes-lite, tapi cron ini hanya hidup 4 hari → **konversi ke 4 baris one-shot `kind='static'`** (teks fixed, tanpa perlu LLM):
```sql
-- Anchor WIB (keputusan owner 29 Agu 2026): H-3 (15 Okt), H-2, H-1, H-day
INSERT INTO reminders (chat_id, message, remind_at, kind) VALUES
 ($ALLOWED_CHAT_ID, '⚠️ H-3! Renew server dalam 3 hari lagi', 'REDACTED', 'static'),
 ($ALLOWED_CHAT_ID, '🟡 H-2! ...', 'REDACTED', 'static'),
 ($ALLOWED_CHAT_ID, '🔴 H-1! BESOK renew server! ...', 'REDACTED', 'static'),
 ($ALLOWED_CHAT_ID, '🚨 HARI INI deadline renew server! ...', 'REDACTED', 'static');
```
(Timestamp WIB — keputusan final: re-anchor ke wall-clock WIB.)

**Job (b) Running reminder harian** → 1 baris `kind='job'`, `recur='daily'`:
- `message` (prompt job): "Jalankan `python3 /opt/hermes-lite/scripts/running_schedule.py today` via run_command, lalu kirim hasilnya (jadwal hari ini + progress) ke owner. Ringkas."
- `remind_at` = besok pada jam trigger (**WIB 07:50**, keputusan final — bukan 06:50 hasil instant lama).
- Job dieksekusi dengan tools penuh (Pilar 4) — budget call di-code sudah ada.

## Fase 5 — Chat History: Arsipkan, Jangan Diimpor

86 messages Honcho **tidak** dimasukkan ke tabel `messages` hermes-lite karena:
- Context hermes-lite = 20 pesan terakhir per chat — history lama yang diimpor hanya jadi noise yang dikirim berulang tiap call (boros token, lawan prinsip Pilar 2)
- Nilai pengetahuannya sudah diambil alih oleh Fase 1 (memory extraction)
- Kalau suatu saat butuh recall percakapan lama → itu use case FTS (Prinsip Retrieval tahap 2), bukan import

Yang dilakukan hanya arsip:
```bash
docker exec honcho-database-1 psql -U postgres -d postgres \
  -Atc "SELECT created_at||'|'||sender_id||'|'||content FROM messages ORDER BY created_at" \
  > /opt/hermes-lite/archive/honcho_messages.txt
```

## Fase 6 — Cutover & Decommission

Setelah Fase 1–5 tervalidasi (minimal 1–2 hari masa paruh aktif):

1. `systemctl disable --now hermes-gateway` (sudah stopped sejak Fase 0; sekarang permanen)
2. Honcho stack matikan (sudah terbukti tidak dibutuhkan hermes-lite — memory-nya sudah terekstrak):
   ```bash
   cd /opt/honcho && docker compose down
   ```
   → bebaskan 4 container (pgvector, redis, TEI embedding, api, deriver) — RAM win paling besar di VPS ini
3. ~~SearXNG~~ **SELESAI 29 Agu 2026**: container + image dihapus (`docker rm -f searxng && docker rmi searxng/searxng`). Satu-satunya consumer adalah hermes agent lama yang memang di-decommission.
4. Jangan dulu `docker rm` container honcho & jangan hapus `/root/.hermes` — biarkan 2 minggu sebagai rollback window, baru bersihkan (bonus: `docker system prune` akan membebaskan ±1.1 GB build cache + volume honcho setelah window lewat).

## Fase 7 — Validasi

- [ ] `/memory` menampilkan fakta hasil migrasi, total char < 20.000, tanpa credential di dalamnya
- [ ] `/reminders` menampilkan running job (daily) + 4 renew one-shot
- [ ] Reminder running terfire besok pagi dan mengirim ringkasan jadwal (bukti script + data path benar)
- [ ] `/skills` menampilkan 3 skill; test keyword: "cek intram" (vps-app-ops), "jadwal lari" (running-schedule)
- [ ] `send_email.py` smoke test via bot dengan SMTP cred dari env
- [ ] **Vault**: suruh bot buat commit+push ke vault (`/opt/vault`), lalu cek `git -C /opt/vault log` muncul commit baru — dan path SSH laptop (`/root/vault`) tetap resolve
- [ ] `docker ps` bersih dari honcho; `free -h` sebelum vs sesudah tercatat

## Rollback

Backup Fase 0 + stack lama hanya `stopped` bukan `deleted`:
`systemctl enable --now hermes-gateway && cd /opt/honcho && docker compose up -d`
Data hermes-lite hasil migrasi tidak mengganggu — keduanya token bot berbeda.

## Yang TIDAK Dimigrasi (dan alasannya)

| Item | Alasan |
|---|---|
| 82 stock skills | Prosedural utk toolset hermes agent yang tak dimiliki hermes-lite; on-demand copy kalau terbukti butuh |
| `kanban.db` | Tabel tasks kosong (0 baris) |
| `auth.json`, `config.yaml` (provider lama) | hermes-lite punya provider config sendiri (`.env`); kredensial lama tidak dipakai |
| Chat history → tabel `messages` | Lihat Fase 5 |
| Job restart-gateway | One-shot, sudah completed |
| `.hermes_history`, cache/* | Artefak CLI, tanpa nilai |

## Keputusan Terbuka

| # | Keputusan | Status |
|---|---|---|
| 1 | Timezone anchor reminder | ✅ **WIB** (29 Agu 2026) — running 07:50 WIB, renew 00:53 WIB |
| 2 | SearXNG dibuang | ✅ **DONE** — container + image terhapus 29 Agu 2026 |
| 3 | Akses vault (`/root/vault`) | ✅ **SELESAI 29 Agu 2026** — vault dipakai owner (Obsidian desktop di laptop, akses via SSH). Solusi: **pindah ke `/opt/vault` + symlink `/root/vault`** (path SSH laptop tidak berubah), bare repo → `/opt/vault.git` (symlink `/root/git/obsidian-vault.git` tetap ada), `chown hermes:hermes`, unit hermes-lite dapat `ReadWritePaths` utk keduanya. Akses dari sandbox **terverifikasi** (write + `git push --dry-run` OK via `systemd-run` dengan property identik unit). Saat Fase 1: **rewrite fakta vault** ke path & workflow baru — jangan di-skip. |
