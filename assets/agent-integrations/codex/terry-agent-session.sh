#!/bin/sh
# installed by Terry
# managed by Terry; reinstalling or updating the integration overwrites this file.
# TERRY_INTEGRATION_ID=codex
# TERRY_INTEGRATION_VERSION=1
#
# Codex hook: reports the resumable session to Terry over the local IPC API.
#
# Codex exposes hooks through `~/.codex/hooks.json` (with `[features] hooks =
# true` in `config.toml`). Its hook payload is passed on stdin and includes
# `session_id` and `transcript_path`.

[ "${1:-}" = "session" ] || exit 0

if [ -z "$TERRY_TERMINAL_TOKEN" ] || [ -z "$TERRY_IPC_URL" ] || [ -z "$TERRY_IPC_TOKEN" ]; then
  exit 0
fi

payload=$(cat)

session_ref=$(printf '%s' "$payload" |
  sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
  head -n 1)
if [ -z "$session_ref" ]; then
  session_ref=$(printf '%s' "$payload" |
    sed -n 's/.*"transcript_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1)
fi
[ -n "$session_ref" ] || exit 0

curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_session" \
  -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"codex\",\"session_id\":\"$session_ref\"}" \
  >/dev/null 2>&1

exit 0
