#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-subtitle-fast}"
BUNDLE_ID="${BUNDLE_ID:-com.weidix.subtitle-fast}"
FEATURES="${FEATURES:-gui}"
TARGET_DIR="${ROOT_DIR}/target/release"
DIST_DIR="${ROOT_DIR}/target/bundle/macos"
APP_DIR="${DIST_DIR}/${APP_NAME}.app"
ICON_PATH="${ROOT_DIR}/crates/subtitle-fast/assets/app-icon/logo.icns"

VERSION="$(
  python3 - <<'PY'
import tomllib
from pathlib import Path
path = Path("crates/subtitle-fast/Cargo.toml")
data = tomllib.loads(path.read_text(encoding="utf-8"))
print(data["package"]["version"])
PY
)"

cargo build --release --bin subtitle-fast --features "${FEATURES}"

rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS" "${APP_DIR}/Contents/Resources"
cp "${TARGET_DIR}/subtitle-fast" "${APP_DIR}/Contents/MacOS/subtitle-fast"
chmod +x "${APP_DIR}/Contents/MacOS/subtitle-fast"
cp "${ICON_PATH}" "${APP_DIR}/Contents/Resources/logo.icns"

cat > "${APP_DIR}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>subtitle-fast</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleIconFile</key>
  <string>logo.icns</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.15</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

mkdir -p "${DIST_DIR}"
ZIP_PATH="${DIST_DIR}/${APP_NAME}-${VERSION}-macos.zip"
ditto -c -k --sequesterRsrc --keepParent "${APP_DIR}" "${ZIP_PATH}"
echo "macOS bundle created at ${APP_DIR}"
echo "macOS zip created at ${ZIP_PATH}"
echo "Output directory: ${DIST_DIR}"
