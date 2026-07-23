#!/usr/bin/env bash
# Build a portable Terry release tarball for Linux.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGET="${1:-}"
if [[ -z "${VERSION:-}" ]]; then
  VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
fi
VERSION="${VERSION:-0.0.0}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"

if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

ARCH="$(echo "$TARGET" | cut -d- -f1)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

BUNDLE_NAME="Terry-${VERSION}-linux-${ARCH}"
DEST="${STAGE}/${BUNDLE_NAME}"
mkdir -p "${DEST}/bin" "${DEST}/share/applications" "${DEST}/share/icons/hicolor/512x512/apps"

echo "==> Building terry ($TARGET)…"
export ZED_BUNDLE=true
export RELEASE_VERSION="$VERSION"
cargo build --release --package terry --target "$TARGET"

BIN="${TARGET_DIR}/${TARGET}/release/terry"
if [[ ! -f "$BIN" ]]; then
  BIN="${TARGET_DIR}/release/terry"
fi
cp "$BIN" "${DEST}/bin/terry"
chmod +x "${DEST}/bin/terry"

cp resources/linux/terry.desktop "${DEST}/share/applications/terry.desktop"
cp resources/app-icon.png "${DEST}/share/icons/hicolor/512x512/apps/terry.png"
if [[ -f LICENSE-GPL ]]; then
  cp LICENSE-GPL "${DEST}/LICENSE"
elif [[ -f LICENSE ]]; then
  cp LICENSE "${DEST}/LICENSE"
fi

cat >"${DEST}/README.txt" <<EOF
Terry ${VERSION}

Run:
  ./bin/terry

Optional system install:
  sudo cp bin/terry /usr/local/bin/
  sudo cp share/applications/terry.desktop /usr/local/share/applications/
  sudo cp share/icons/hicolor/512x512/apps/terry.png /usr/local/share/icons/hicolor/512x512/apps/
EOF

OUT_DIR="${TARGET_DIR}/release"
mkdir -p "$OUT_DIR"
ARCHIVE="${OUT_DIR}/${BUNDLE_NAME}.tar.gz"
tar -C "$STAGE" -czf "$ARCHIVE" "$BUNDLE_NAME"
echo "==> Wrote $ARCHIVE"
echo "$ARCHIVE"
