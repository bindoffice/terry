#!/usr/bin/env bash
# Re-vendor the tom crates from a local tom checkout.
#
# Terry embeds tom rather than depending on published crates, so picking up an
# upstream change means copying `crates/` across and re-applying Terry's local
# adaptations. This script does the copying and then tells you exactly which
# files still need a human:
#
#   script/vendor-tom.sh [--tom-dir DIR] [--check]
#
#   --tom-dir DIR  path to the tom checkout (default: ../tom)
#   --check        copy nothing; report what differs from the checkout
#
# It is a copy, not a merge: files Terry has not modified are overwritten, and
# files Terry *has* modified are listed for manual review. That keeps the
# adaptations explicit instead of silently reverting them.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOM_DIR="$(cd "$ROOT/.." && pwd)/tom"
CHECK_ONLY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tom-dir) TOM_DIR="$2"; shift 2 ;;
    --check) CHECK_ONLY=1; shift ;;
    -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! -d "$TOM_DIR/crates" ]]; then
  echo "error: $TOM_DIR is not a tom checkout" >&2
  exit 1
fi

# Crates Terry vendors, in dependency order.
CRATES=(
  tom-telemetry
  tom-ai
  tom-agent
  tom-tools
  tom-harness
  tom-plugin-sdk
  tom-runtime
  tom-tui
  tom-extensions
  tom-cli
)

# Files that intentionally differ from upstream. Anything else that differs is
# either a local adaptation that needs porting, or drift worth looking at.
MANIFEST_FILES=(Cargo.toml VENDORED.md LICENSE)

# Formats a Rust file down to its semantic content: no trailing whitespace, no
# blank lines, no `use` statements. Import moves are the bulk of the noise after
# a `cargo fmt` with a different toolchain, and they never change behaviour.
normalize() {
  sed 's/[[:space:]]*$//' "$1" | grep -vE '^[[:space:]]*use ' | grep -vE '^[[:space:]]*$' || true
}

# Whether a Rust file differs in more than formatting.
differs_semantically() {
  ! diff -q <(normalize "$1") <(normalize "$2") >/dev/null 2>&1
}

is_ignored() {
  local rel="$1"
  for skip in "${MANIFEST_FILES[@]}"; do
    [[ "$(basename "$rel")" == "$skip" ]] && return 0
  done
  [[ "$rel" == */target/* || "$rel" == */.git/* ]] && return 0
  return 1
}

upstream_commit="$(git -C "$TOM_DIR" rev-parse --short HEAD 2>/dev/null || echo unknown)"
upstream_subject="$(git -C "$TOM_DIR" log --format=%s -1 2>/dev/null || echo unknown)"
echo "tom: $upstream_commit — $upstream_subject"
echo

modified=()
copied=0

for crate in "${CRATES[@]}"; do
  src="$TOM_DIR/crates/$crate"
  dst="$ROOT/crates/$crate"
  [[ -d "$src" ]] || { echo "skip $crate (not in the checkout)"; continue; }

  while IFS= read -r rel; do
    rel="${rel#./}"
    is_ignored "$rel" && continue
    if [[ -f "$dst/$rel" ]] && cmp -s "$src/$rel" "$dst/$rel"; then
      continue
    fi
    if [[ -f "$dst/$rel" ]]; then
      # Present on both sides. Report it only when the difference is more than
      # formatting, so the list stays short enough to actually review.
      if [[ "$rel" == *.rs ]] && ! differs_semantically "$src/$rel" "$dst/$rel"; then
        continue
      fi
      modified+=("crates/$crate/$rel")
      continue
    fi
    if [[ $CHECK_ONLY -eq 0 ]]; then
      mkdir -p "$(dirname "$dst/$rel")"
      cp "$src/$rel" "$dst/$rel"
    fi
    copied=$((copied + 1))
  done < <(cd "$src" && find . -type f)
done

if [[ $CHECK_ONLY -eq 1 ]]; then
  echo "new files upstream would add: $copied"
else
  echo "copied new files: $copied"
fi

if [[ ${#modified[@]} -gt 0 ]]; then
  echo
  echo "These files differ from upstream and were NOT touched."
  echo "Review each: port the upstream change by hand, or keep the local edit."
  printf '  %s\n' "${modified[@]}"
  echo
  echo "After porting, confirm the tree matches again with --check."
fi

echo
echo "Next: cargo check -p tom-cli -p tom-tui, then cargo test -p tom-cli -p tom-tui"
