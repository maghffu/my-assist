#!/usr/bin/env bash
# Rollback darurat Hermes-Lite ke tarball build lama (escape hatch saat deploy
# baru bikin bot stuck — dipanggil command /rollback di Telegram).
#
# DIJALANKAN VIA systemd-run oleh gateway (unit transient "hermes-rollback-*")
# supaya tetap hidup di luar cgroup hermes-lite.service saat install.sh
# me-restart service-nya sendiri. Bisa juga manual dari shell:
#   /opt/hermes-lite/bin/rollback.sh hermes-lite-<sha8>-linux-x86_64.tar.gz
# Log: journalctl -u 'hermes-rollback-*'
set -euo pipefail
APP_DIR="/opt/hermes-lite"
STAMP="$APP_DIR/.deployed-build"
SERVICE="hermes-lite"
TIMER="hermes-lite-deploy.timer"

[[ $# -eq 1 ]] || { echo "pakai: rollback.sh <hermes-lite-<sha8>-linux-x86_64.tar.gz>"; exit 64; }
asset="$1"
tarball="$APP_DIR/releases/$asset"
[[ "$asset" == */* ]] && { echo "❌ nama asset tidak valid (harus nama file saja)"; exit 1; }
[[ -f "$tarball" ]] || { echo "❌ tarball tidak ada: $tarball"; exit 1; }

# Beri waktu bot mengirim balasan /rollback sebelum install.sh restart service.
sleep 5

cd "$APP_DIR/releases"
if [[ -f "$asset.sha256" ]] && ! sha256sum --check --status "$asset.sha256"; then
  echo "❌ CHECKSUM MISMATCH — rollback dibatalkan"
  exit 1
fi

# Stop deploy timer DULU — poll 5-menit akan lihat stamp != nightly dan
# menimpa balik build rusak begitu stamp diganti. Nyalakan lagi setelah
# build fix rilis: systemctl start $TIMER
systemctl stop "$TIMER"

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
tar -xzf "$tarball" -C "$tmp"
bash "$tmp/install.sh"   # restart service + health check (sleep 3 + is-active)
echo "$asset" > "$STAMP"
echo "✅ rollback ke $asset selesai; $TIMER di-stop"
