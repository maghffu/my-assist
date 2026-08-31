# Rebuild & Deploy hermes-lite dari Agent (tanpa SSH manual)

Source: `/root/my-assist` (clone GitHub maghffu/my-assist, Rust). Service: `systemctl restart hermes-lite`. Per 30 Agu 2026 service jalan sebagai **root** (unit diubah owner) — tidak perlu sudo.

## Langkah

1. **Patch source** — pakai script python anchored-replace (contoh: `/root/my-assist/patch_destructive.py`), jangan hand-edit besar. Idempotent: cek dulu `"system prune" in src` → batal kalau sudah.
2. **Build di background** (cargo release butuh 1-2 menit > timeout 120s):
   ```bash
   cd /root/my-assist && nohup cargo build --release > /tmp/build.log 2>&1 &
   ```
   Poll: `tail -3 /tmp/build.log` sampai `Finished` / `error`.
3. **Verifikasi patch masuk binary**: `strings target/release/hermes-lite | grep -E "pattern|baru"`
4. **Deploy** (JANGAN cp langsung — binary sedang jalan & hardlink → ETXTBSY):
   ```bash
   cp target/release/hermes-lite /opt/hermes-lite/hermes-lite.new && mv -f /opt/hermes-lite/hermes-lite.new /opt/hermes-lite/hermes-lite
   ```
   (mv = rename → inode baru, proses lama aman.)
5. **Restart dengan delay** — service menjalankan agent itu sendiri; restart = bunuh diri sendiri. Delay supaya pesan terakhir sempat terkirim:
   ```bash
   nohup bash -c 'sleep 15; systemctl restart hermes-lite' >/tmp/restart.log 2>&1 &
   ```

## Komunikasi saat deploy/restart (WAJIB — insiden 31 Agu 08:15, owner kehilangan kabar)

- **SEBELUM restart (step 5)**: kirim pesan dulu ke owner: deploy selesai, restart ~15 detik, "bakal sekejap hilang, nanti lapor balik". BARU eksekusi restart.
- **Reminder lapor balik (WAJIB)**: sebelum restart, buat reminder one-shot `kind=job` di **+2 menit** dari waktu restart, instruksinya: "verifikasi hermes-lite hidup (systemctl show hermes-lite -p ActiveEnterTimestamp), cek /tmp/build.log & hasil patch, lalu LAPORKAN hasil deploy ke owner secara proaktif". Scheduler reload pending reminders saat startup → service hidup lagi, owner otomatis dapat laporan tanpa harus chat duluan.
- Kalau build GAGAL: jangan restart sama sekali — laporkan error + usulan fix, binary lama tetap jalan.
- Setelah hidup, aturan umum: turn pertama setelah downtime HARUS buka dengan status apa yang terjadi, bukan nunggu owner tanya "kok diem?".

## Gotchas

- **NAMA BINARY: `hermes-lite`** (pakai hyphen, BUKAN `hermes`) — deploy 30 Agu gagal karena salah nama file.
- **JANGAN PERNAH `systemctl stop hermes-lite` dari dalam agent** — INSIDEN 30 Agu 2026 18:25: chain `stop && cp && start` bunuh agent sendiri di step pertama, cp+start tidak pernah jalan, service mati 13 jam (SIGTERM exit bersih → `Restart=on-failure` tidak trigger). SELALU `restart` via detached delayed bash (langkah 5), atau minta owner jalankan manual.
- Skill ini hanya di-inject kalau pesan owner mengandung token nama skill (rebuild/deploy/hermes/lite). Kalau task-mu berakhir dengan perlu mengganti binary service ini dan skill ini belum dimuat di atas → WAJIB `read_file skills/rebuild-deploy-hermes-lite.md` DULU sebelum menyentuh /opt/hermes-lite atau systemctl.
- Patch BISA HILANG kalau owner git reset/pull ulang repo sebelum build (sudah pernah terjadi) — selalu `grep -n "pattern" src/shell.rs` dulu sebelum build.
- Toolchain Rust di `/root/.cargo/bin` (rustup, installed 30 Agu 2026).
- Verifikasi post-restart: `systemctl show hermes-lite -p ActiveEnterTimestamp` harus > waktu deploy.
- `cargo build` incremental satu file ≈ 1m05s; build dari nol jauh lebih lama.
