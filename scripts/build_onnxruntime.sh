#!/usr/bin/env bash
set -euo pipefail

# Download and build ONNX Runtime v1.22.0 into target/onnxruntime.
# Optional operator trim config can be provided with --ops-config or OPS_CONFIG.

usage() {
  cat <<'EOF'
Usage: scripts/build_onnxruntime.sh [--ops-config path]

Environment overrides:
  OUT_DIR     Where to place downloads and sources (default: <repo>/target/onnxruntime)
  ORT_VERSION ONNX Runtime tag to fetch (default: 1.22.0)
  OPS_CONFIG  Operator config path passed to --include_ops_by_config
EOF
}

OPS_CONFIG="${OPS_CONFIG:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ops-config)
      OPS_CONFIG="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-"$ROOT_DIR/target/onnxruntime"}"
ORT_VERSION="${ORT_VERSION:-1.22.0}"
SRC_DIR="$OUT_DIR/onnxruntime-$ORT_VERSION"
ARCHIVE="$OUT_DIR/onnxruntime-$ORT_VERSION.tar.gz"

if [ -n "$OPS_CONFIG" ]; then
  # Convert to absolute path if relative
  if [[ ! "$OPS_CONFIG" = /* ]]; then
    OPS_CONFIG="$ROOT_DIR/$OPS_CONFIG"
  fi
  if [ ! -f "$OPS_CONFIG" ]; then
    echo "Specified ops config not found: $OPS_CONFIG" >&2
    exit 1
  fi
fi

mkdir -p "$OUT_DIR"

if [ ! -d "$SRC_DIR" ]; then
  echo "Downloading ONNX Runtime v$ORT_VERSION sources..."
  curl -L "https://github.com/microsoft/onnxruntime/archive/refs/tags/v${ORT_VERSION}.tar.gz" -o "$ARCHIVE"
  tar -xzf "$ARCHIVE" -C "$OUT_DIR"
fi

# Fix Eigen SHA1 hash mismatch in deps.txt
DEPS_FILE="$SRC_DIR/cmake/deps.txt"
if [ -f "$DEPS_FILE" ]; then
  echo "Fixing Eigen SHA1 hash in deps.txt..."
  sed -i.bak 's/5ea4d05e62d7f954a46b3213f9b2535bdd866803/51982be81bbe52572b54180454df11a3ece9a934/' "$DEPS_FILE"
fi

# Download and apply patches from ort-artifacts
PATCHES_REPO="$OUT_DIR/ort-artifacts"
PATCHES_DIR="$PATCHES_REPO/src/patches/all"
PATCHES_COMMIT="77ec493e3495901a361469951ab992181e52fd05"

if [ ! -d "$PATCHES_REPO" ]; then
  echo "Cloning ort-artifacts repository..."
  git clone https://github.com/pykeio/ort-artifacts.git "$PATCHES_REPO"
  pushd "$PATCHES_REPO" >/dev/null
  git checkout "$PATCHES_COMMIT"
  popd >/dev/null
else
  echo "ort-artifacts repository already exists, using existing patches..."
fi

echo "Applying patches to ONNX Runtime sources..."
pushd "$SRC_DIR" >/dev/null
for patch in "$PATCHES_DIR"/*.patch; do
  if [ -f "$patch" ]; then
    echo "  Applying $(basename "$patch")..."
    if patch -p1 -N -r - < "$patch" 2>/dev/null; then
      echo "    ✓ Applied successfully"
    else
      echo "    ⚠ Skipped (already applied or not applicable)"
    fi
  fi
done
popd >/dev/null

pushd "$SRC_DIR" >/dev/null

# Set build directory to platform-independent location
LIB_DIR="$OUT_DIR/build"

BUILD_ARGS=(
  --config MinSizeRel
  --parallel 
  --skip_tests 
  --disable_ml_ops
  --disable_rtti
  --build_dir "$LIB_DIR"
)

if [ -n "$OPS_CONFIG" ]; then
  echo "Using operator config at $OPS_CONFIG"
  BUILD_ARGS+=(--include_ops_by_config "$OPS_CONFIG")
fi

./build.sh "${BUILD_ARGS[@]}"
popd >/dev/null

echo "ONNX Runtime build finished under $SRC_DIR"
echo "Build artifacts available at $LIB_DIR"
