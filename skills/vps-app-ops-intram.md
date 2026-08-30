# VPS App Ops — intram + PSD (migrasi dari hermes-agent 29 Agu 2026)

Dua target, dua pola akses:
- **intram** — remote VPS `ssh intram` (REDACTED-IP). App: REDACTED-APP, Go backend + React/Vite frontend (klien: REDACTED-ORG document management, REDACTED-DOMAIN).
- **PSD** — mesin ini sendiri (REDACTED-IP): PM2 Express apps, Docker Postgres, nginx.

## Standard deploy cycle — intram

User bilang "pull dan rebuild" → jalankan 4 langkah, lalu verifikasi:

```bash
ssh intram "cd /opt/REDACTED-APP && git pull"                    # 1. branch master
ssh intram "cd /opt/REDACTED-APP/backend && export PATH=\$PATH:/usr/local/go/bin && CGO_ENABLED=0 go build -o server ./cmd/api"  # 2. Go NOT on PATH; entrypoint ./cmd/api
ssh intram "systemctl restart REDACTED-APP && sleep 3 && systemctl is-active REDACTED-APP"  # 3.
ssh intram "cd /opt/REDACTED-APP/frontend && pnpm install --frozen-lockfile && pnpm build"   # 4. dist/ served langsung oleh nginx — tanpa copy step
```

**Verify**: `curl -s -o /dev/null -w '%{http_code}' https://REDACTED-DOMAIN` → 200, dan pastikan nginx ambil asset hash baru:
`curl -s https://REDACTED-DOMAIN | grep -oE 'assets/[a-zA-Z0-9_-]+\.js' | head -1`

Pitfalls:
- Frontend build 1–3 menit di box 2-core → jalankan via run_command dengan timeout cukup (atau background + cek belakangan).
- `grep -i error` di journalctl bisa match kolom SQL `last_error` — baca konteks sebelum menyebutnya error.
- API root dan `/health` return 404 by design; route asli di `/api/v1/*` dan auth-protected (curl dari server dapat 401, bukan bukti rusak).
- `backend/server` binary dan `backend/.env` sengaja untracked — jangan pernah commit.

## DB / Redis di intram
```bash
ssh intram "docker exec postgres psql -U postgres -d document_intram -c '...'"
ssh intram "docker exec redis redis-cli"
```
Redis key `storage:status` cache storage state (~5 menit TTL). Setelah edit `storage_connections`, `DEL storage:status` supaya UI langsung update.

## Full OAuth disconnect (storage_connections)

`status='disconnected'` SENDIRIAN tidak nempel — app auto-refresh token dan balik ke `connected` di poll /storage/status berikutnya. Replikasi semantik `Disconnect()` milik app:

```sql
UPDATE storage_connections SET status='disconnected',
  access_token_encrypted='', refresh_token_encrypted='', expires_at=NULL, updated_at=NOW()
WHERE provider='onedrive' AND deleted_at IS NULL;
```
Lalu `DEL storage:status` di Redis. Reconnect butuh OAuth login ulang dari halaman storage superadmin.

Aturan umum: saat memutasi state app langsung di DB-nya, baca dulu source operasi itu (mis. `onedrive_service.go` `Disconnect()`) dan tiru semantik update-nya — jangan menebak dari schema saja.

## Menghapus secret dari .env di server
1. `sed -i '/^KEY=/d' /path/.env`, buktikan: `grep -c KEY /path/.env` → 0.
2. Audit exposure: grep working tree; `git ls-files | xargs grep -l '<fragment>'`; history: `git log --all --oneline -S '<fragment>'` per nilai secret.
3. Restart service dan pastikan sehat tanpa config itu.
4. Bandingkan nilai yang user paste vs yang benar-benar ada di server — bisa beda (pernah: user paste SharePoint tenant creds, server pegang consumer-app creds lain). Hapus yang ada, laporkan mismatch-nya.
5. Kalau integrasi masih dibutuhkan: tawarkan systemd `Environment=` drop-in atau env file di luar repo — jangan taruh plaintext creds di tracked file.

## PSD local stack
- PM2 kosong (setelah reboot) → `pm2 resurrect` (REDACTED-APP :3090, push-demo :3091, di /var/www/REDACTED-DOMAIN). Verify: `curl https://REDACTED-DOMAIN/keuangan` → 200.
- MSSQL (container lama, sudah stopped sejak migrasi ke Postgres 29 Agu 2026): kalau perlu dinyalakan lagi — `max server memory` 768MB + docker limit 1GB. Turunkan memory via `sp_configure 'max server memory (MB)', N; RECONFIGURE;` — BUKAN `docker update --memory` pada container running (OOM-kill, exit 137). Kalau sampai mati: `docker start mssql`, lalu verifikasi dengan query asli ke REDACTED-APP.

## Gaya lapor (preferensi owner)
Balas Bahasa Indonesia, ringkas, dengan bukti verifikasi ✅ (systemctl state, HTTP code, git hash) dan sebutkan side effects (service direstart, downtime sebentar, provider mana yang disentuh).

---

## Inventori server intram (2026-08-21)

### Akses
`ssh intram` → root@REDACTED-IP (SSH config di mesin ini; ControlPersist 10m)

### App: REDACTED-APP
- Path: `/opt/REDACTED-APP` — git `git@github.com:maghffu/REDACTED-APP.git`, deploy branch **master**
- Backend: Go (module `github.com/REDACTED-ORG/document-intram`), entry `backend/cmd/api/main.go`, binary `backend/server`
- Go binary di `/usr/local/go/bin/go` (v1.24.4) — TIDAK di default PATH
- systemd: `REDACTED-APP.service`, WorkingDirectory `/opt/REDACTED-APP/backend`, PORT=8080
- Frontend: Vite + React (pnpm), nginx serve `/opt/REDACTED-APP/frontend/dist` langsung
- nginx vhosts: `REDACTED-DOMAIN` → static dist + /assets/; `api.REDACTED-DOMAIN` → proxy 127.0.0.1:8080
- Env: `backend/.env` (untracked). `backend/.env.example` placeholder saja. `render.yaml` keys tanpa nilai.

### Infra (docker-compose di repo root)
- postgres:16-alpine, db `document_intram`, user postgres, 127.0.0.1:5432
- redis:7-alpine, 127.0.0.1:6379

### Storage providers (per 2026-08-21)
- Tabel `storage_connections`: id=1 onedrive, id=2 google_drive
- 21 Agu 2026: OneDrive full disconnect (tokens dikosongkan) atas permintaan user; MS_GRAPH_* dihapus dari .env demi keamanan
- Commit `c5dd2d0` "change to sharepoint" migrasi storage ke SharePoint — butuh env config baru (user punya creds app-registration SharePoint: client REDACTED…, tenant REDACTED…, site REDACTED-DOMAIN) — BELUM ditaruh di server. Perkiraan pekerjaan OAuth di deploy berikutnya.
- API surface: `/api/v1/storage/{status,oauth/url,oauth/callback,disconnect,test-connection}` (superadmin, JWT-protected)

### Quirks yang diketahui
- `backend/server`, `frontend/.env.production`, beberapa upload icon = untracked files — biarkan
- API tidak punya route `/health` atau `/` (404 by design)
- Frontend build ~18s tapi `pnpm install` bisa menit-menit; jalankan di background
