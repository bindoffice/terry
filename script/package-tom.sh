#!/usr/bin/env bash
# Build a prebuilt `tom` release for one target and emit the release manifest.
#
# The CLI can update itself by downloading a prebuilt binary instead of compiling
# from source (`tom update --prebuilt`). That path expects a specific shape on the
# release page, which this script produces:
#
#   <repo>/releases/download/v<version>/tom-release.json
#   <repo>/releases/download/v<version>/tom-<target>.tar.gz     (unix)
#   <repo>/releases/download/v<version>/tom-<target>.zip        (windows)
#
# `crates/tom-cli/src/prebuilt_release.rs` is deliberately strict about the
# manifest — schema version, package name, repository URL, tag, 40-char source
# commit, SHA-256s, and a flat binary named exactly `tom` (`tom.exe` on Windows).
# Getting any of those wrong is a 404 or a validation error at update time, on a
# user's machine, so this script checks the same invariants before writing.
#
# Usage:
#   script/package-tom.sh [TARGET]
#
# Environment:
#   VERSION    release version without the `v` prefix (default: from git tag)
#   OUT_DIR    where to write artifacts (default: target/prebuilt)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPOSITORY="https://github.com/solos/tom"
PACKAGE="tom-cli"

TARGET="${1:-}"
if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

# The version is taken from the tag when one is checked out, so a release build
# cannot disagree with the tag that names it. A dirty tree is refused rather than
# silently shipping a binary whose commit is not the one recorded.
if [[ -z "${VERSION:-}" ]]; then
  TAG="$(git describe --tags --exact-match 2>/dev/null || true)"
  if [[ -n "$TAG" ]]; then
    VERSION="${TAG#v}"
  else
    echo "error: no VERSION given and HEAD is not tagged" >&2
    echo "       pass VERSION=x.y.z or check out a release tag" >&2
    exit 1
  fi
fi
VERSION="${VERSION#v}"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree is dirty; a release must match its recorded commit" >&2
  exit 1
fi

SOURCE_COMMIT="$(git rev-parse HEAD)"
if [[ ${#SOURCE_COMMIT} -ne 40 ]]; then
  echo "error: could not resolve a full 40-character source commit" >&2
  exit 1
fi

TAG="v${VERSION}"
OUT_DIR="${OUT_DIR:-target/prebuilt}"
mkdir -p "$OUT_DIR"

IS_WINDOWS=0
[[ "$TARGET" == *windows* ]] && IS_WINDOWS=1

if [[ $IS_WINDOWS -eq 1 ]]; then
  FORMAT="zip"
  BINARY_NAME="tom.exe"
  ARCHIVE_NAME="tom-${TARGET}.zip"
else
  FORMAT="tar.gz"
  BINARY_NAME="tom"
  ARCHIVE_NAME="tom-${TARGET}.tar.gz"
fi

echo "==> Building ${PACKAGE} ${VERSION} for ${TARGET}"
# The workspace release profile keeps `debug = "limited"`, which is right for a
# development build but produces a 161 MB binary here — over the 128 MB
# MAX_LIBRARY limit the updater enforces when extracting, so the update would
# fail on every machine with "extracted library exceeds size limit". Release
# artifacts are stripped instead: 25 MB, and no user-facing behaviour changes.
CARGO_PROFILE_RELEASE_DEBUG=false \
  CARGO_PROFILE_RELEASE_STRIP=symbols \
  cargo build --release --package "$PACKAGE" --bin tom --target "$TARGET"

BIN="${CARGO_TARGET_DIR:-target}/${TARGET}/release/${BINARY_NAME}"
if [[ ! -f "$BIN" ]]; then
  echo "error: expected binary at $BIN" >&2
  exit 1
fi

# The archive must hold the binary at its root: the updater matches the declared
# path exactly, so a leading directory such as `tom-v0.16.0/tom` would not be
# found.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$BIN" "${STAGE}/${BINARY_NAME}"
chmod +x "${STAGE}/${BINARY_NAME}"

# Fail here rather than shipping an artifact the updater will reject.
BIN_SIZE="$(wc -c <"$BIN" | tr -d ' ')"
MAX_LIBRARY=$((128 * 1024 * 1024))
if (( BIN_SIZE > MAX_LIBRARY )); then
  echo "error: $BIN is $((BIN_SIZE / 1048576)) MB, over the ${MAX_LIBRARY} byte limit" >&2
  echo "       the updater extracts with MAX_LIBRARY; strip debug info" >&2
  exit 1
fi

ARCHIVE="${OUT_DIR}/${ARCHIVE_NAME}"
rm -f "$ARCHIVE"
if [[ $IS_WINDOWS -eq 1 ]]; then
  ( cd "$STAGE" && zip -q -X "$ARCHIVE" "$BINARY_NAME" )
else
  # --no-same-owner avoids embedding the building user's uid/gid.
  tar -C "$STAGE" --no-same-owner -czf "$ARCHIVE" "$BINARY_NAME"
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

ARCHIVE_SIZE="$(wc -c <"$ARCHIVE" | tr -d ' ')"
ARCHIVE_SHA="$(sha256_of "$ARCHIVE")"
BINARY_SHA="$(sha256_of "$BIN")"

# Append this target to an existing manifest when one is present, so a release
# assembled across several machines accumulates targets instead of the last
# build overwriting the others.
MANIFEST="${OUT_DIR}/tom-release.json"
TARGETS_JSON="$(python3 - "$MANIFEST" "$TARGET" "$REPOSITORY" "$TAG" "$ARCHIVE_NAME" "$FORMAT" "$ARCHIVE_SIZE" "$ARCHIVE_SHA" "$BINARY_NAME" "$BINARY_SHA" <<'PY'
import json, sys, pathlib

(manifest_path, target, repository, tag, archive_name, fmt,
 size, archive_sha, binary_name, binary_sha) = sys.argv[1:11]

entry = {
    "url": f"{repository}/releases/download/{tag}/{archive_name}",
    "format": fmt,
    "size": int(size),
    "sha256": archive_sha,
    "binary": {"path": binary_name, "sha256": binary_sha},
}

targets = {}
path = pathlib.Path(manifest_path)
if path.is_file():
    try:
        targets = json.loads(path.read_text()).get("targets", {})
    except json.JSONDecodeError:
        # A corrupt leftover must not silently become the new manifest.
        sys.exit(f"error: {manifest_path} is not valid JSON")
targets[target] = entry
print(json.dumps(dict(sorted(targets.items())), indent=2))
PY
)"

python3 - "$MANIFEST" "$VERSION" "$REPOSITORY" "$TAG" "$SOURCE_COMMIT" "$TARGETS_JSON" <<'PY'
import json, sys, pathlib

manifest_path, version, repository, tag, source_commit, targets_json = sys.argv[1:7]

manifest = {
    "schema_version": 1,
    "name": "tom-cli",
    "version": version,
    "repository": repository,
    "tag": tag,
    "source_commit": source_commit,
    "targets": json.loads(targets_json),
}

# Mirror the checks `prebuilt_release::validate` performs, so a malformed
# manifest fails here instead of on a user's machine.
assert manifest["schema_version"] == 1
assert len(source_commit) == 40 and all(c in "0123456789abcdef" for c in source_commit)
for name, dist in manifest["targets"].items():
    assert dist["format"] in ("zip", "tar.gz"), name
    assert 0 < dist["size"] <= 256 * 1024 * 1024, name
    assert len(dist["sha256"]) == 64, name
    assert len(dist["binary"]["sha256"]) == 64, name
    assert dist["binary"]["path"] == ("tom.exe" if "windows" in name else "tom"), name

pathlib.Path(manifest_path).write_text(json.dumps(manifest, indent=2) + "\n")
print(f"wrote {manifest_path}")
PY

echo "==> Artifacts in ${OUT_DIR}"
ls -1 "${OUT_DIR}"
echo
echo "Upload these to the ${TAG} release:"
echo "  ${ARCHIVE_NAME}"
echo "  tom-release.json"
