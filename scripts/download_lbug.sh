#!/bin/sh
# Download a COMPLETE prebuilt static liblbug (liblbug.a + bundled dep archives)
# from the cartographer fork's GitHub release, into the Rust crate cache.
#
# Upstream's prebuilt ships only liblbug.a without the bundled third-party
# static archives (libyyjson.a, etc.) that liblbug.a references, so its final
# link fails ("Undefined symbols: _yyjson_*"). This release asset bundles
# liblbug.a together with all of its bundled dep archives so build.rs can link
# them with whole-archive (see link_prebuilt_bundled_deps in build.rs).
#
# Only platforms we publish an asset for are handled; every other platform
# exits nonzero so build.rs falls back to a CMake source build.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ENV_FILE="${1:-$PROJECT_DIR/.cache/lbug-prebuilt.env}"
CACHE_LIB_DIR="${LBUG_TARGET_DIR:-$PROJECT_DIR/.cache/lbug-prebuilt/lib}"

RELEASE_REPO="${LBUG_PREBUILT_REPO:-skylence-be/ladybug-rust}"
RELEASE_TAG="${LBUG_PREBUILT_TAG:-lbug-prebuilt-0.16.1}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS:$ARCH" in
  Darwin:arm64)
    ASSET="liblbug-static-osx-arm64.tar.gz"
    ;;
  Darwin:x86_64)
    ASSET="liblbug-static-osx-x86_64.tar.gz"
    ;;
  Linux:x86_64)
    ASSET="liblbug-static-linux-x86_64-compat.tar.gz"
    ;;
  Linux:aarch64)
    ASSET="liblbug-static-linux-aarch64-compat.tar.gz"
    ;;
  *)
    echo "No complete lbug prebuilt published for $OS/$ARCH; falling back to source build." >&2
    exit 1
    ;;
esac

mkdir -p "$CACHE_LIB_DIR"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
TARBALL="$TMP_DIR/$ASSET"

echo "Downloading $ASSET from $RELEASE_REPO ($RELEASE_TAG) ..."
if command -v gh >/dev/null 2>&1; then
  gh release download "$RELEASE_TAG" --repo "$RELEASE_REPO" -p "$ASSET" \
    --dir "$TMP_DIR" --clobber
else
  ASSET_URL="https://github.com/$RELEASE_REPO/releases/download/$RELEASE_TAG/$ASSET"
  curl -fsSL "$ASSET_URL" -o "$TARBALL"
fi

if [ ! -f "$TARBALL" ]; then
  echo "Failed to download $ASSET" >&2
  exit 1
fi

echo "Extracting into $CACHE_LIB_DIR ..."
tar -xzf "$TARBALL" -C "$CACHE_LIB_DIR"

LIB_PATH="$CACHE_LIB_DIR/liblbug.a"
if [ ! -f "$LIB_PATH" ]; then
  echo "Expected precompiled library not found at $LIB_PATH after extraction" >&2
  exit 1
fi

mkdir -p "$(dirname "$ENV_FILE")"
cat > "$ENV_FILE" <<EOF
LBUG_LIBRARY_DIR=$CACHE_LIB_DIR
LBUG_INCLUDE_DIR=$CACHE_LIB_DIR
EOF

echo "Wrote $ENV_FILE"
echo "Resolved precompiled library: $LIB_PATH"
