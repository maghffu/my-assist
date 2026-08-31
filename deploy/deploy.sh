#!/usr/bin/env bash
# Deploy Hermes-Lite ke /opt/hermes-lite + restart systemd service (ROADMAP Fase 8).
# Idempotent: aman dijalankan berulang. Skill & .env di server TIDAK dioverwrite
# (karena ditulis/diedit runtime di sana) — soul.md & binary selalu di-update.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="/opt/hermes-lite"
SERVICE="hermes-lite"

[[ $EUID -eq 0 ]] || { echo "jalankan sebagai root (sudo ./deploy.sh)"; exit 1; }

echo "==> Build release…"
cd "$REPO_DIR"
cargo build --release

echo "==> Siapkan direktori…"
# Mode root (keputusan owner — lihat hermes-lite.service): artifacts milik root.
# chown menutup artifacts warisan deploy lama yang masih milik user `hermes`.
chown -R root:root "$APP_DIR"
install -d -m 0750 "$APP_DIR"

echo "==> Salin artifacts…"
install -m 0755 target/release/hermes-lite "$APP_DIR/hermes-lite"
install -m 0644 soul.md "$APP_DIR/soul.md"
[[ -d "$APP_DIR/skills" ]] || install -d -m 0755 "$APP_DIR/skills"
if [[ ! -f "$APP_DIR/.env" ]]; then
    if [[ -f "$REPO_DIR/.env" ]]; then
        install -m 0600 "$REPO_DIR/.env" "$APP_DIR/.env"
        echo "    .env disalin (0600) — edit langsung di $APP_DIR/.env ke depannya"
    else
        echo "⚠️  .env tidak ditemukan — salin manual ke $APP_DIR/.env lalu restart service"
    fi
fi

echo "==> Install systemd unit…"
install -m 0644 "$REPO_DIR/deploy/hermes-lite.service" /etc/systemd/system/hermes-lite.service
systemctl daemon-reload
systemctl enable "$SERVICE" >/dev/null

echo "==> Restart service…"
systemctl restart "$SERVICE"
sleep 2
systemctl --no-pager -l status "$SERVICE" | head -8
echo "✅ Deploy selesai — log: journalctl -u $SERVICE -f"
