#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-subtitle-fast}"
FEATURES="${FEATURES:-gui}"
TARGET_DIR="${ROOT_DIR}/target/release"
DIST_DIR="${ROOT_DIR}/target/bundle/linux"
WORK_DIR="${ROOT_DIR}/target/work/linux"
APPDIR="${WORK_DIR}/AppDir"
ICON_SRC="${ROOT_DIR}/crates/subtitle-fast/assets/app-icon/logo.png"

VERSION="$(
  python3 - <<'PY'
import tomllib
from pathlib import Path
path = Path("crates/subtitle-fast/Cargo.toml")
data = tomllib.loads(path.read_text(encoding="utf-8"))
print(data["package"]["version"])
PY
)"
ARCH_LABEL="${ARCH_LABEL:-$(uname -m)}"
if [ "${ARCH_LABEL}" = "aarch64" ]; then
  ARCH_LABEL="arm64"
fi
PACKAGE_KIND="${PACKAGE_KIND:-gui}"
ARTIFACT_BASENAME="${ARTIFACT_BASENAME:-${APP_NAME}-${PACKAGE_KIND}-${VERSION}-linux-${ARCH_LABEL}}"

cargo build --release --bin subtitle-fast --features "${FEATURES}"

rm -rf "${APPDIR}"
mkdir -p "${APPDIR}/usr/bin" \
  "${APPDIR}/usr/share/applications" \
  "${APPDIR}/usr/share/icons/hicolor/512x512/apps"

cp "${TARGET_DIR}/subtitle-fast" "${APPDIR}/usr/bin/subtitle-fast"
chmod +x "${APPDIR}/usr/bin/subtitle-fast"
cp "${ICON_SRC}" "${APPDIR}/usr/share/icons/hicolor/512x512/apps/subtitle-fast.png"
cp "${ICON_SRC}" "${APPDIR}/subtitle-fast.png"

cat > "${APPDIR}/usr/share/applications/subtitle-fast.desktop" <<EOF
[Desktop Entry]
Name=subtitle-fast
Exec=subtitle-fast
Icon=subtitle-fast
Type=Application
Categories=Video;AudioVideo;
Terminal=false
EOF
cp "${APPDIR}/usr/share/applications/subtitle-fast.desktop" "${APPDIR}/subtitle-fast.desktop"

cat > "${APPDIR}/AppRun" <<'EOF'
#!/usr/bin/env bash
HERE="$(cd "$(dirname "$0")" && pwd)"
exec "${HERE}/usr/bin/subtitle-fast" "$@"
EOF
chmod +x "${APPDIR}/AppRun"

mkdir -p "${DIST_DIR}"
if command -v appimagetool >/dev/null 2>&1; then
  APPIMAGE_PATH="${DIST_DIR}/${ARTIFACT_BASENAME}.AppImage"
  appimagetool "${APPDIR}" "${APPIMAGE_PATH}"
  echo "AppImage created at ${APPIMAGE_PATH}"
else
  ARCHIVE_PATH="${DIST_DIR}/${ARTIFACT_BASENAME}.tar.gz"
  tar -czf "${ARCHIVE_PATH}" -C "${WORK_DIR}" "AppDir"
  echo "AppImage tool not found; tarball created at ${ARCHIVE_PATH}"
fi
echo "Output directory: ${DIST_DIR}"
