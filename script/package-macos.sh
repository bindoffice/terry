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
ENTITLEMENTS="${ROOT}/resources/terry.entitlements"

if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

ARCH="$(echo "$TARGET" | cut -d- -f1)"
case "$ARCH" in
  aarch64) ARCH_LABEL="aarch64" ;;
  x86_64) ARCH_LABEL="x86_64" ;;
  *) ARCH_LABEL="$ARCH" ;;
esac

resolve_signing_identity() {
  if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
    printf '%s\n' "$MACOS_SIGNING_IDENTITY"
    return
  fi
  local identity
  identity="$(
    security find-identity -v -p codesigning 2>/dev/null \
      | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' \
      | head -1
  )"
  if [[ -n "$identity" ]]; then
    printf '%s\n' "$identity"
    return
  fi
  if [[ "${REQUIRE_SIGNING:-false}" == "true" ]]; then
    echo "error: no Developer ID Application identity found, but REQUIRE_SIGNING=true" >&2
    echo "  - set MACOS_SIGNING_IDENTITY, or" >&2
    echo "  - import the Developer ID certificate into the keychain" >&2
    security find-identity -v -p codesigning >&2 || true
    exit 1
  fi
  # Ad-hoc: avoids the Apple Silicon "damaged" Gatekeeper state for unsigned
  # + quarantined downloads, but is not a real distribution signature.
  echo "==> Warning: no Developer ID certificate found; using AD-HOC signing" >&2
  echo "    (the package will be quarantined/blocked on other machines)" >&2
  printf '%s\n' "-"
}

sign_app() {
  local app_path="$1"
  local identity="$2"
  local binary_path=""
  local candidate

  xattr -cr "${app_path}" || true

  for candidate in \
    "${app_path}/Contents/MacOS/terry" \
    "${app_path}/Contents/MacOS/Terry"
  do
    if [[ -f "$candidate" ]]; then
      binary_path="$candidate"
      break
    fi
  done

  if [[ -z "$binary_path" ]]; then
    echo "error: no executable found under ${app_path}/Contents/MacOS" >&2
    exit 1
  fi

  if [[ "$identity" == "-" ]]; then
    echo "==> Code signing (ad-hoc)…"
    codesign --force --sign "$identity" --timestamp=none \
      --entitlements "$ENTITLEMENTS" "$binary_path"
    codesign --force --deep --sign "$identity" --timestamp=none \
      --entitlements "$ENTITLEMENTS" "$app_path"
  else
    echo "==> Code signing with: $identity"
    # Sign nested executable first, then the bundle (Apple's recommended order).
    codesign --force --options runtime --timestamp \
      --entitlements "$ENTITLEMENTS" --sign "$identity" "$binary_path"
    codesign --force --options runtime --timestamp \
      --entitlements "$ENTITLEMENTS" --sign "$identity" "$app_path"
  fi

  codesign --verify --verbose=2 "$app_path"
  codesign -dv --verbose=2 "$app_path" 2>&1 | sed -n 's/^/    /p' | head -20
}

notarize_zip_if_configured() {
  local zip_path="$1"
  local identity="$2"

  if [[ "$identity" == "-" ]]; then
    return
  fi
  if [[ -z "${APPLE_NOTARIZATION_KEY:-}" || -z "${APPLE_NOTARIZATION_KEY_ID:-}" || -z "${APPLE_NOTARIZATION_ISSUER_ID:-}" ]]; then
    echo "==> Skipping notarization (APPLE_NOTARIZATION_* not set)"
    return
  fi

  echo "==> Notarizing $zip_path…"
  local key_file
  key_file="$(mktemp)"
  printf '%s\n' "$APPLE_NOTARIZATION_KEY" >"$key_file"
  xcrun notarytool submit "$zip_path" --wait \
    --key "$key_file" \
    --key-id "$APPLE_NOTARIZATION_KEY_ID" \
    --issuer "$APPLE_NOTARIZATION_ISSUER_ID"
  rm -f "$key_file"
}

echo "==> Ensuring cargo-bundle…"
if ! command -v cargo-bundle >/dev/null 2>&1 && ! cargo bundle --help >/dev/null 2>&1; then
  cargo install cargo-bundle --locked
fi

echo "==> Building terry ($TARGET)…"
export ZED_BUNDLE=true
export RELEASE_VERSION="$VERSION"
cargo build --release --package terry --target "$TARGET"

echo "==> Creating .app bundle…"
# Do NOT pass --bin: cargo-bundle then looks for [package.metadata.bundle.bin.*]
# and ignores the top-level [package.metadata.bundle] (name/identifier/icon).
BUNDLE_LOG="$(mktemp)"
set +e
cargo bundle --release --target "$TARGET" --package terry | tee "$BUNDLE_LOG"
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
  "${TARGET_DIR}/${TARGET}/release/bundle/osx/terry.app" \
  "${TARGET_DIR}/release/bundle/osx/terry.app" \
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

# Normalize to Terry.app when cargo-bundle emitted a lowercase name.
# On case-insensitive APFS, terry.app and Terry.app are the same path — rename
# via an intermediate name so the zip entry is correctly capitalized.
APP_PARENT="$(dirname "$APP_PATH")"
APP_BASE="$(basename "$APP_PATH")"
if [[ "$APP_BASE" != "Terry.app" ]]; then
  TMP_APP="${APP_PARENT}/.terry-rename-$$.app"
  rm -rf "$TMP_APP" "${APP_PARENT}/Terry.app"
  mv "$APP_PATH" "$TMP_APP"
  mv "$TMP_APP" "${APP_PARENT}/Terry.app"
  APP_PATH="${APP_PARENT}/Terry.app"
fi

if [[ -f resources/AppIcon.icns ]]; then
  mkdir -p "${APP_PATH}/Contents/Resources"
  cp resources/AppIcon.icns "${APP_PATH}/Contents/Resources/AppIcon.icns"
fi

SIGN_IDENTITY="$(resolve_signing_identity)"
sign_app "$APP_PATH" "$SIGN_IDENTITY"

OUT_DIR="${TARGET_DIR}/release"
mkdir -p "$OUT_DIR"
# Resolve to an absolute path before the subshell `cd`s into the .app parent,
# otherwise a relative ZIP_PATH lands under bundle/osx/... instead of target/release/.
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
ZIP_NAME="Terry-${VERSION}-macos-${ARCH_LABEL}.zip"
ZIP_PATH="${OUT_DIR}/${ZIP_NAME}"
rm -f "$ZIP_PATH"
(
  cd "$(dirname "$APP_PATH")"
  ditto -c -k --sequesterRsrc --keepParent "$(basename "$APP_PATH")" "$ZIP_PATH"
)

if [[ ! -f "$ZIP_PATH" ]]; then
  echo "error: zip was not created at $ZIP_PATH" >&2
  exit 1
fi

notarize_zip_if_configured "$ZIP_PATH" "$SIGN_IDENTITY"

echo "==> Wrote $ZIP_PATH"
echo "$ZIP_PATH"
