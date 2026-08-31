# Deploy & Ops Manual — Hermes-Lite di VPS

Dokumentasi setup produksi (ROADMAP Fase 8). Target: VPS Linux RHEL-like
(terverifikasi di OpenCloudOS 9.6). Prinsip: satu proses, systemd + journald.
**Mode root (keputusan owner)** — service berjalan sebagai root, tanpa sandbox
systemd; detail risiko & mitigasi lihat bagian Keamanan di bawah.

## Layout

| Path | Isi |
|---|---|
| `/opt/hermes-lite/hermes-lite` | Binary (release) — di-update oleh `deploy.sh` |
| `/opt/hermes-lite/.env` | Konfigurasi + secret (chmod 600, owner `root`) |
| `/opt/hermes-lite/soul.md` | Persona — di-update dari repo oleh `deploy.sh` |
| `/opt/hermes-lite/skills/` | Skill library (Pilar 11) — ditulis runtime, **tidak** dioverwrite deploy |
| `/root/my-assist` | Repo source (dev) |

## Setup Awal (VPS baru)

### 1. Dependensi sistem

```bash
# Build deps (rust + tesseract binding): gcc, clang, pkg-config, tesseract-devel, leptonica-devel
# Runtime: libtesseract + tessdata sudah ikut paket di atas
dnf install -y gcc clang pkg-config tesseract-devel leptonica-devel git

# Bahasa OCR tambahan (default paket cuma eng) — tessdata_fast:
curl -sL -o /usr/share/tesseract/tessdata/ind.traineddata \
  https://github.com/tesseract-ocr/tessdata_fast/raw/main/ind.traineddata

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. PostgreSQL

Sudah jalan via docker (port 5432). Buat DB + user:

```bash
docker exec -it <pg-container> psql -U postgres -c \
  "CREATE USER hermes WITH PASSWORD '...'; CREATE DATABASE hermes OWNER hermes;"
```

Migrasi otomatis saat startup (atau manual: `DATABASE_URL=... ./hermes-lite migrate`).

### 3. Deploy

```bash
cd /root/my-assist
cp .env.example .env   # isi TELEGRAM_BOT_TOKEN, ANTHROPIC_API_KEY/BASE_URL/MODEL,
                       # ALLOWED_CHAT_ID, DATABASE_URL, TAVILY_API_KEY
sudo ./deploy/deploy.sh
```

Script build release, bikin user `hermes`, salin artifacts ke `/opt/hermes-lite`,
install systemd unit (enable + restart). `.env` hanya disalin pertama kali —
setelah itu edit langsung di `/opt/hermes-lite/.env`.

## Operasional

| Aksi | Perintah |
|---|---|
| Log live | `journalctl -u hermes-lite -f` |
| Restart / stop | `systemctl restart|stop hermes-lite` |
| Update versi | `cd /root/my-assist && git pull && sudo ./deploy/deploy.sh` |
| Edit persona | edit `soul.md` di repo → deploy lagi |
| Edit config | edit `/opt/hermes-lite/.env` → `systemctl restart hermes-lite` |
| Migrasi manual | `/opt/hermes-lite/hermes-lite migrate` |

## Keamanan (Pilar 9) — MODE ROOT

**Keputusan owner (revisi desain awal):** service berjalan sebagai **root** supaya
agent mampu mengerjakan workflow admin + coding penuh tanpa sudoers allowlist
per-command. Sandbox unprivileged & `MemoryMax` dinonaktifkan (cargo/rustc via
`run_command` dihitung dalam cgroup dan mudah >512M → OOM-kill; monitoring
resource manual via `/status`). Risiko yang disadari & diterima: indirect prompt
injection via web/OCR → root RCE; kebocoran token bot = root RCE.

Mitigasi yang tetap aktif:
- Allowlist `ALLOWED_CHAT_ID` di-drop level gateway — pesan luar tidak diproses
- Confirmation gate destructive pattern (✅ sekali / 🔁 sesi / ❌ tolak, via inline keyboard)
- Audit `command_logs` + secret masking di output yang dikirim ke Telegram
- `WORK_ROOTS` membatasi `read_file`/`write_file` + cwd awal (`run_command` bebas)
- Timeout per command (`RUN_CMD_TIMEOUT`) + kill process group saat timeout

User sistem `hermes` warisan deploy lama dibiarkan (nologin, tidak dipakai) —
artifacts sudah di-chown root oleh `deploy.sh`.

## Backup

Yang perlu dibackup cuma: `/opt/hermes-lite/.env` + `skills/` + dump Postgres
(`pg_dump hermes`). Semua state lain ada di Postgres.

## Troubleshooting

- **Service restart loop**: `journalctl -u hermes-lite -n 50` — biasanya env var
  wajib kosong / DB belum jalan (docker postgres mati).
- **OCR gagal init**: cek `OCR_LANG` cocok dengan traineddata terpasang
  (`ls /usr/share/tesseract/tessdata/`), atau set `OCR_TESSDATA` ke path tessdata.
- **web_search balas pesan konfigurasi**: `TAVILY_API_KEY` belum diisi.
- **Telegram 409 Conflict**: ada dua proses polling token yang sama
  (`pgrep -f hermes-lite` — pastikan tidak ada sisa `nohup` manual).

## Deploy Otomatis via CI (GitHub Actions) — flow baru

Sejak Agustus 2026 build release **tidak lagi dilakukan di VPS** (makan
resource, lambat). Alur baru:

```
push ke main → GitHub Actions (container rockylinux:9)
             → tesseract/leptonica .oc9 EXACT spt VPS (repo publik OpenCloudOS)
             → cargo build --release → tarball + sha256
             → GitHub Release tag "nightly" (atau tag v* utk stabil)
                 ↓ (poll tiap 5 menit, tanpa token — repo publik)
VPS: hermes-lite-deploy.timer → poll-deploy.sh → install.sh → restart service
```

| Komponen | Lokasi | Fungsi |
|---|---|---|
| `.github/workflows/build-release.yml` | repo | Build CI + publish release |
| `deploy/oc9-appstream.repo` | repo | Repo oc9 publik utk tesseract-devel 5.3.2-8.oc9 |
| `deploy/install-release.sh` | tarball (`install.sh`) | Installer VPS (update binary/soul/unit, verifikasi service) |
| `deploy/poll-deploy.sh` | `/opt/hermes-lite/bin/` | Poll release, cek sha256, panggil installer |
| `deploy/hermes-lite-deploy.{service,timer}` | VPS systemd | Trigger tiap 5 menit |

**Kenapa rockylinux:9 + repo oc9**: runner `ubuntu-latest` (glibc 2.39)
menghasilkan binary yang gagal jalan di VPS (glibc 2.38), dan tesseract EL9
tidak ada di EPEL. Dengan tesseract-devel **exact sama** (`.oc9`), DT_NEEDED
binary CI = `libtesseract.so.5.3.2` persis seperti binary lama — tanpa
patchelf/symlink hack.

**Operasi sehari-hari:**

- Deploy: cukup `git push` (live ≤ 5 menit). Log: `journalctl -u hermes-lite-deploy`
- Cek versi terpasang: `cat /opt/hermes-lite/BUILD_INFO` & `.deployed-build`
- Ganti channel (misal ke stabil): `echo v0.2.1 > /opt/hermes-lite/.deploy-channel`
- Rollback manual: `systemctl stop hermes-lite-deploy.timer`, extract tarball dari
  `/opt/hermes-lite/releases/`, `bash install.sh` (3 tarball terakhir disimpan)
- `.env`, `skills/`, `data/`, `keys/` tidak pernah dioverwrite deploy

Catatan: `deploy/deploy.sh` (build-on-VPS) tetap dipertahankan sebagai jalur
darurat bila CI tidak bisa dipakai.
