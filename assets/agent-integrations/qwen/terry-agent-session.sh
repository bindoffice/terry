#!/bin/sh
# installed by Terry
# managed by Terry; reinstalling or updating the integration overwrites this file.
# TERRY_INTEGRATION_ID=qwen
# TERRY_INTEGRATION_VERSION=1
#
# Qwen Code hook: reports the resumable session to Terry over the local IPC API.
#
# Usage from ~/.qwen/settings.json:
#   "hooks": {
#     "SessionStart": [{
#       "matcher": "*",
#       "hooks": [{ "type": "command", "command": "/path/to/terry-agent-session.sh session", "timeout": 10000 }]
#     }]
#   }

[ "${1:-}" = "session" ] || exit 0

if [ -z "$TERRY_TERMINAL_TOKEN" ] || [ -z "$TERRY_IPC_URL" ] || [ -z "$TERRY_IPC_TOKEN" ]; then
  exit 0
fi

payload=$(cat)
session_id=$(printf '%s' "$payload" |
  sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
  head -n 1)
[ -n "$session_id" ] || exit 0

curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_session" \
  -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"qwen\",\"session_id\":\"$session_id\"}" \
  >/dev/null 2>&1

exit 0
