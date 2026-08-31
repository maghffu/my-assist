#!/usr/bin/env bash
# Auto-deploy Hermes-Lite dari GitHub Releases (repo publik — tanpa token).
# Dipasang di VPS sebagai systemd timer: deploy/hermes-lite-deploy.timer
# Log: journalctl -u hermes-lite-deploy.service
#
# Channel: default "nightly" (build terbaru main). Ganti via:
#   echo v0.2.1 > /opt/hermes-lite/.deploy-channel
# Rollback: echo <tag-lama> > .deploy-channel (atau extract tarball lama dari
# /opt/hermes-lite/releases/ lalu jalankan install.sh — matikan timer dulu).
set -uo pipefail
REPO="maghffu/my-assist"
APP_DIR="/opt/hermes-lite"
CHANNEL="$(cat "$APP_DIR/.deploy-channel" 2>/dev/null || echo nightly)"
STAMP="$APP_DIR/.deployed-build"
KEEP=3   # jumlah tarball lama yang disimpan utk rollback manual

releases_json=$(curl -fsSL --max-time 30 \
  "https://api.github.com/repos/$REPO/releases?per_page=30") || exit 0

asset=$(jq -r --arg ch "$CHANNEL" \
  '.[] | select(.tag_name==$ch)
     | .assets[] | select(.name|test("^hermes-lite-.+-linux-x86_64\\.tar\\.gz$"))
     | .name' <<<"$releases_json" | head -n1)
[[ -n "${asset:-}" ]] || exit 0
[[ "$(cat "$STAMP" 2>/dev/null || true)" == "$asset" ]] && exit 0

echo "build baru terdeteksi: $asset (channel: $CHANNEL)"
mkdir -p "$APP_DIR/releases"
url="https://github.com/$REPO/releases/download/$CHANNEL/$asset"
curl -fsSL --max-time 300 -o "$APP_DIR/releases/$asset" "$url" \
  || { echo "❌ download gagal"; exit 1; }
curl -fsSL --max-time 30 -o "$APP_DIR/releases/$asset.sha256" "$url.sha256" || true

cd "$APP_DIR/releases"
if [[ -f "$asset.sha256" ]] && ! sha256sum --check --status "$asset.sha256"; then
  echo "❌ CHECKSUM MISMATCH — deploy dibatalkan"
  rm -f "$asset" "$asset.sha256"
  exit 1
fi

tmp=$(mktemp -d)
tar -xzf "$APP_DIR/releases/$asset" -C "$tmp"
bash "$tmp/install.sh" || { echo "❌ install.sh gagal"; rm -rf "$tmp"; exit 1; }
echo "$asset" > "$STAMP"
rm -rf "$tmp"

ls -t "$APP_DIR/releases"/hermes-lite-*-linux-x86_64.tar.gz 2>/dev/null \
  | tail -n +"$((KEEP + 1))" | xargs -r rm -f
echo "✅ deploy selesai: $asset"
