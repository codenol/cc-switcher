#!/usr/bin/env bash
# Собрать распространяемый .dmg из готового cc-switcher.app.
#
# Штатный dmg-бандлер Tauri (bundle_dmg.sh) раскладывает окно через Finder/
# AppleScript и часто падает в headless/фоновом окружении. Этот скрипт делает
# то же надёжнее — через hdiutil, с ярлыком на /Applications.
#
# Использование:
#   npm run tauri build      # соберёт .app
#   ./scripts/package-dmg.sh # соберёт .dmg рядом
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/src-tauri/target/release/bundle/macos/cc-switcher.app"
OUT_DIR="$ROOT/src-tauri/target/release/bundle/dmg"
VERSION="$(/usr/bin/plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist" 2>/dev/null || echo 0.0.0)"
ARCH="$(uname -m)"
OUT="$OUT_DIR/cc-switcher_${VERSION}_${ARCH}.dmg"

if [[ ! -d "$APP" ]]; then
  echo "Не найден $APP — сначала: npm run tauri build" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

rm -f "$OUT"
hdiutil create -volname "cc-switcher" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null

echo "Готово: $OUT"
