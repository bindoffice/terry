#!/bin/sh
# installed by Terry
# managed by Terry; reinstalling or updating the integration overwrites this file.
# TERRY_INTEGRATION_ID=kimi
# TERRY_INTEGRATION_VERSION=1
#
# Kimi Code hook: reports lifecycle state to Terry over the local IPC API.
#
# Usage from ~/.kimi-code/config.toml:
#   [[hooks]]
#   event = "SessionStart"
#   command = "/path/to/terry-agent-state.sh session"
#   timeout = 10

action="${1:-}"
case "$action" in
  session|working|blocked|idle) ;;
  *) exit 0 ;;
esac

if [ -z "$TERRY_TERMINAL_TOKEN" ] || [ -z "$TERRY_IPC_URL" ] || [ -z "$TERRY_IPC_TOKEN" ]; then
  exit 0
fi

payload=$(cat)

if [ "$action" = "session" ]; then
  session_id=$(printf '%s' "$payload" |
    sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1)
  [ -n "$session_id" ] || exit 0
  curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_session" \
    -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"kimi\",\"session_id\":\"$session_id\"}" \
    >/dev/null 2>&1
  exit 0
fi

curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_state" \
  -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"kimi\",\"state\":\"$action\"}" \
  >/dev/null 2>&1

exit 0
