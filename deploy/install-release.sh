#!/usr/bin/env bash
# Installer VPS Hermes-Lite — dijalankan dari dalam direktori hasil extract
# tarball release (dipanggil otomatis oleh poll-deploy.sh; bisa juga manual:
#   tar -xzf hermes-lite-XXXX-linux-x86_64.tar.gz -C /tmp/d && bash /tmp/d/install.sh
# ).
#
# Paritas perilaku deploy/deploy.sh lama: binary + soul.md + systemd unit
# selalu di-update; .env, skills/, data/, keys/ TIDAK disentuh (milik server).
set -euo pipefail
SRC="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
APP_DIR="/opt/hermes-lite"
SERVICE="hermes-lite"

install -d -m 0750 "$APP_DIR"
install -d -m 0755 "$APP_DIR/bin"
install -m 0755 "$SRC/rollback.sh"        "$APP_DIR/bin/rollback.sh"
# Soname libtesseract.so.5 (build upstream) — di VPS file-nya polos tanpa symlink
# (paket .oc9 tanpa soname); buat symlink idempotent ke file yang ada.
tess_real=$(ls /lib64/libtesseract.so.5.* 2>/dev/null | head -n1)
if [[ -n "$tess_real" && ! -e /lib64/libtesseract.so.5 ]]; then
    ln -s "$(basename "$tess_real")" /lib64/libtesseract.so.5
fi
install -m 0755 "$SRC/hermes-lite"         "$APP_DIR/hermes-lite.new"
install -m 0644 "$SRC/soul.md"             "$APP_DIR/soul.md"
install -m 0644 "$SRC/BUILD_INFO"          "$APP_DIR/BUILD_INFO"
install -m 0644 "$SRC/hermes-lite.service" "/etc/systemd/system/${SERVICE}.service"
# rename terakhir supaya window downtime sempit
mv -f "$APP_DIR/hermes-lite.new" "$APP_DIR/hermes-lite"

systemctl daemon-reload
systemctl enable "$SERVICE" >/dev/null 2>&1 || true
systemctl restart "$SERVICE"

sleep 3
if ! systemctl is-active --quiet "$SERVICE"; then
    echo "❌ service mati setelah deploy — 20 log terakhir:"
    journalctl -u "$SERVICE" -n 20 --no-pager || true
    exit 1
fi
echo "✅ $SERVICE aktif ($(cat "$APP_DIR/BUILD_INFO" | tr '\n' ' '))"
