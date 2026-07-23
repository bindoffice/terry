#!/usr/bin/env bash
# Build a Terry.app bundle (and zip) for macOS.
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
case "$ARCH" in
  aarch64) ARCH_LABEL="aarch64" ;;
  x86_64) ARCH_LABEL="x86_64" ;;
  *) ARCH_LABEL="$ARCH" ;;
esac

echo "==> Ensuring cargo-bundle…"
if ! command -v cargo-bundle >/dev/null 2>&1 && ! cargo bundle --help >/dev/null 2>&1; then
  cargo install cargo-bundle --locked
fi

echo "==> Building terry ($TARGET)…"
export ZED_BUNDLE=true
export RELEASE_VERSION="$VERSION"
cargo build --release --package terry --target "$TARGET"

echo "==> Creating .app bundle…"
BUNDLE_LOG="$(mktemp)"
set +e
cargo bundle --release --target "$TARGET" --bin terry | tee "$BUNDLE_LOG"
BUNDLE_STATUS=${PIPESTATUS[0]}
set -e
if [[ "$BUNDLE_STATUS" -ne 0 ]]; then
  echo "error: cargo bundle failed" >&2
  exit "$BUNDLE_STATUS"
fi

APP_PATH=""
for candidate in \
  "$(tail -n 1 "$BUNDLE_LOG")" \
  "${TARGET_DIR}/${TARGET}/release/bundle/osx/Terry.app" \
  "${TARGET_DIR}/release/bundle/osx/Terry.app" \
  "${TARGET_DIR}/${TARGET}/release/bundle/osx/"*.app \
  "${TARGET_DIR}/release/bundle/osx/"*.app
do
  if [[ -d "$candidate" ]]; then
    APP_PATH="$candidate"
    break
  fi
done
rm -f "$BUNDLE_LOG"

if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  echo "error: Terry.app not found after cargo bundle" >&2
  find "${TARGET_DIR}" -name '*.app' -type d 2>/dev/null | head -20 >&2 || true
  exit 1
fi

if [[ -f resources/AppIcon.icns ]]; then
  mkdir -p "${APP_PATH}/Contents/Resources"
  cp resources/AppIcon.icns "${APP_PATH}/Contents/Resources/AppIcon.icns"
fi

OUT_DIR="${TARGET_DIR}/release"
mkdir -p "$OUT_DIR"
ZIP_NAME="Terry-${VERSION}-macos-${ARCH_LABEL}.zip"
ZIP_PATH="${OUT_DIR}/${ZIP_NAME}"
rm -f "$ZIP_PATH"
(
  cd "$(dirname "$APP_PATH")"
  ditto -c -k --sequesterRsrc --keepParent "$(basename "$APP_PATH")" "$ZIP_PATH"
)

echo "==> Wrote $ZIP_PATH"
echo "$ZIP_PATH"
