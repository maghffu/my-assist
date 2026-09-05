# VPS App Ops — klien remote + stack lokal (versi publik)

> **Mirror publik.** Detail identitas (IP, domain, nama instansi klien, repo,
> port, credential fragment) sengaja dihilangkan dari salinan repo ini.
> Versi lengkap hanya ada di `/opt/hermes-lite/skills/` di server — direktori
> itu tidak pernah dioverwrite deploy dan tidak pernah di-push ke git publik.

Dua target, dua pola akses:
- **app klien** — remote VPS via alias ssh (`ssh <alias>`). Go backend + React/Vite frontend, dilayani nginx.
- **stack lokal** — mesin ini sendiri: PM2 Express apps, Docker Postgres, nginx.

## Standard deploy cycle — app klien

User bilang "pull dan rebuild" → jalankan 4 langkah, lalu verifikasi:

```bash
ssh <alias> "cd /opt/<app> && git pull"                    # 1. deploy branch default
ssh <alias> "cd /opt/<app>/backend && export PATH=\$PATH:/usr/local/go/bin && CGO_ENABLED=0 go build -o server ./cmd/api"  # 2. Go NOT on PATH; entrypoint ./cmd/api
ssh <alias> "systemctl restart <app-svc> && sleep 3 && systemctl is-active <app-svc>"  # 3.
ssh <alias> "cd /opt/<app>/frontend && pnpm install --frozen-lockfile && pnpm build"   # 4. dist/ served langsung oleh nginx — tanpa copy step
```

**Verify**: `curl -s -o /dev/null -w '%{http_code}' https://<domain-klien>` → 200, dan pastikan nginx ambil asset hash baru:
`curl -s https://<domain-klien> | grep -oE 'assets/[a-zA-Z0-9_-]+\.js' | head -1`

Pitfalls:
- Frontend build 1–3 menit di box kecil → jalankan via run_command dengan timeout cukup (atau background + cek belakangan).
- `grep -i error` di journalctl bisa match kolom SQL `last_error` — baca konteks sebelum menyebutnya error.
- API root dan `/health` return 404 by design; route asli di `/api/v1/*` dan auth-protected (curl dari server dapat 401, bukan bukti rusak).
- `backend/server` binary dan `backend/.env` sengaja untracked — jangan pernah commit.

## DB / Redis di server klien
```bash
ssh <alias> "docker exec postgres psql -U postgres -d <db> -c '...'"
ssh <alias> "docker exec redis redis-cli"
```
Ada key cache status storage (~5 menit TTL). Setelah edit tabel `storage_connections`, `DEL` key cache itu supaya UI langsung update.

## Full OAuth disconnect (storage_connections)

`status='disconnected'` SENDIRIAN tidak nempel — app auto-refresh token dan balik ke `connected` di poll status berikutnya. Replikasi semantik `Disconnect()` milik app:

```sql
UPDATE storage_connections SET status='disconnected',
  access_token_encrypted='', refresh_token_encrypted='', expires_at=NULL, updated_at=NOW()
WHERE provider='onedrive' AND deleted_at IS NULL;
```
Lalu DEL key cache status di Redis. Reconnect butuh OAuth login ulang dari halaman storage superadmin.

Aturan umum: saat memutasi state app langsung di DB-nya, baca dulu source operasi itu (mis. service layer `Disconnect()`) dan tiru semantik update-nya — jangan menebak dari schema saja.

## Menghapus secret dari .env di server
1. `sed -i '/^KEY=/d' /path/.env`, buktikan: `grep -c KEY /path/.env` → 0.
2. Audit exposure: grep working tree; `git ls-files | xargs grep -l '<fragment>'`; history: `git log --all --oneline -S '<fragment>'` per nilai secret.
3. Restart service dan pastikan sehat tanpa config itu.
4. Bandingkan nilai yang user paste vs yang benar-benar ada di server — bisa beda. Hapus yang ada, laporkan mismatch-nya.
5. Kalau integrasi masih dibutuhkan: tawarkan systemd `Environment=` drop-in atau env file di luar repo — jangan taruh plaintext creds di tracked file.

## Stack lokal (PM2)
- PM2 kosong (setelah reboot) → `pm2 resurrect`. Verify via curl endpoint masing-masing app → 200.
- Container DB lama yang sudah tidak dipakai: kalau perlu dinyalakan lagi — batasi memory via config DB-nya (`max server memory`), BUKAN `docker update --memory` pada container running (OOM-kill, exit 137). Kalau sampai mati: `docker start <container>`, lalu verifikasi dengan query asli.

## Gaya lapor (preferensi owner)
Balas Bahasa Indonesia, ringkas, dengan bukti verifikasi ✅ (systemctl state, HTTP code, git hash) dan sebutkan side effects (service direstart, downtime sebentar, provider mana yang disentuh).
