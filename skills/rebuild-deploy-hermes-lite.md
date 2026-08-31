# Deploy hermes-lite — CI (GitHub Actions) + fallback build-VPS

Source: `/root/my-assist` (GitHub maghffu/my-assist, Rust). Service: `hermes-lite` (root, tanpa sudo).
**Sejak 31 Agu 2026 build release TIDAK lagi di VPS** — pindah ke GitHub Actions (VPS 2-core tidak layak jadi build farm Rust).

## Alur utama: CI auto-deploy (git push → live ≤ 5 menit)

1. **Patch source** di `/root/my-assist` — python anchored-replace / edit kecil. Selalu `grep -n "pattern" src/...` dulu sebelum commit (patch bisa hilang kalau ada git pull/reset dari sisi lain).
2. **Commit + push ke main**:
   ```bash
   cd /root/my-assist && git add -A && git commit -m "..." && git push
   ```
3. **CI build** (`.github/workflows/build-release.yml`): container `rockylinux:9` (glibc match VPS 2.38; runner ubuntu glibc 2.39 → binary gagal jalan di VPS), tesseract 5.3.2 + leptonica 1.84.0 build dari source, di-cache. Hasil: tarball + sha256 → GitHub Release tag `nightly` (tag `v*` = stabil). Build CI gagal = tidak ada release baru = VPS tetap jalan versi lama (aman, tidak pernah broken deploy).
4. **VPS auto-poll**: `hermes-lite-deploy.timer` (enabled) jalan tiap 5 menit → `/opt/hermes-lite/bin/poll-deploy.sh` (tanpa token, repo publik) → bandingkan `.deployed-build` vs asset → download + verifikasi sha256 → `install.sh` → replace binary/soul/unit → restart service. Log: `journalctl -u hermes-lite-deploy`.
5. **Verifikasi**: `cat /opt/hermes-lite/BUILD_INFO`, `systemctl show hermes-lite -p ActiveEnterTimestamp` ≥ waktu push, `journalctl -u hermes-lite -n 10` startup bersih.

File yang **TIDAK pernah dioverwrite** deploy: `.env`, `skills/`, `data/`, `keys/`.

## Komunikasi saat deploy (WAJIB — insiden 31 Agu 08:15)

- Deploy via timer = restart BISA terjadi kapan saja ≤5 menit setelah push. **SEBELUM push**: kabari owner dulu ("deploy jalan ≤5 menit, sebentar hilang ya").
- Setelah push: buat reminder one-shot `kind=job` **+7 menit**: "verifikasi hermes-lite hidup (systemctl show hermes-lite -p ActiveEnterTimestamp), cek BUILD_INFO + journalctl -u hermes-lite-deploy, LAPORKAN hasil deploy ke owner secara proaktif".
- Build CI gagal: jangan apa-apa di VPS — laporkan error CI + usulan fix; binary lama tetap jalan.

## Channel & rollback

- Channel default `nightly`. Ganti stabil: `echo v0.2.1 > /opt/hermes-lite/.deploy-channel` (di-pick poll berikutnya).
- Rollback manual: `systemctl stop hermes-lite-deploy.timer` → extract tarball lama dari `/opt/hermes-lite/releases/` (3 terakhir disimpan) → `bash install.sh` → hidupkan timer lagi. Atau cukup `echo <tag-lama> > .deploy-channel`.

## Fallback darurat: build on-VPS

`cd /root/my-assist && sudo ./deploy/deploy.sh` — hanya kalau CI tidak bisa dipakai. Detail mode lama: build background `nohup cargo build --release > /tmp/build.log 2>&1 &` (incremental ~1 menit), verifikasi `strings target/release/hermes-lite | grep pattern`, install cp→`mv -f` (ETXTBSY), restart via delayed bash (lihat gotcha di bawah). Toolchain: `/root/.cargo/bin`.

## Gotchas (tetap berlaku)

- **NAMA BINARY: `hermes-lite`** (hyphen, BUKAN `hermes`) — deploy 30 Agu gagal karena salah nama.
- **JANGAN PERNAH `systemctl stop hermes-lite` dari dalam agent** — INSIDEN 30 Agu 18:25: chain `stop && cp && start` bunuh agent sendiri di step pertama, service mati 13 jam (SIGTERM bersih → `Restart=on-failure` tidak trigger). SELALU `restart` via detached delayed bash: `nohup bash -c 'sleep 15; systemctl restart hermes-lite' >/tmp/restart.log 2>&1 &`.
- Push ke main = deploy otomatis ≤5 menit. JANGAN push eksperimen setengah jadi ke main — pakai branch.
- Skill ini hanya di-inject kalau pesan owner mengandung token nama skill (rebuild/deploy/hermes/lite). Kalau task-mu berujung mengganti binary service ini dan skill belum dimuat → WAJIB `read_file skills/rebuild-deploy-hermes-lite.md` DULU sebelum menyentuh /opt/hermes-lite atau systemctl.
- Verifikasi post-deploy: `systemctl show hermes-lite -p ActiveEnterTimestamp` harus > waktu push/poll.
