# Deploy & Ops Manual — Hermes-Lite di VPS

Dokumentasi setup produksi (ROADMAP Fase 8). Target: VPS Linux RHEL-like
(terverifikasi di OpenCloudOS 9.6). Prinsip: satu proses, unprivileged user,
systemd + journald.

## Layout

| Path | Isi |
|---|---|
| `/opt/hermes-lite/hermes-lite` | Binary (release) — di-update oleh `deploy.sh` |
| `/opt/hermes-lite/.env` | Konfigurasi + secret (chmod 600, owner `hermes`) |
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
| Migrasi manual | `sudo -u hermes /opt/hermes-lite/hermes-lite migrate` |

## Keamanan (Pilar 9)

- Bot jalan sebagai user `hermes` (nologin shell, tanpa capability, `ProtectSystem=strict`
  — FS read-only kecuali `/opt/hermes-lite` + `/tmp` private)
- Allowlist `ALLOWED_CHAT_ID` di-drop level gateway — pesan luar tidak diproses
- Kebocoran token bot = RCE di user `hermes` — karenanya: `.env` 0600, sandbox, audit
  `command_logs`. Sudoers/docker group TIDAK diberikan (evidence-driven — tambahkan
  allowlist spesifik saja kalau owner terbukti butuh, mis:
  `hermes ALL=(root) NOPASSWD: /usr/bin/systemctl reload nginx`)
- MemoryMax=512M (guard OCR spike di VPS kecil)

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
