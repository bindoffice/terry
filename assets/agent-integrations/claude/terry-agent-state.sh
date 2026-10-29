#!/bin/sh
# installed by Terry
# managed by Terry; reinstalling or updating the integration overwrites this file.
# TERRY_INTEGRATION_ID=claude
# TERRY_INTEGRATION_VERSION=1
#
# Claude Code hook: reports lifecycle state to Terry over the local IPC API.
#
# Usage from ~/.claude/settings.json:
#   "command": "/path/to/terry-agent-state.sh SessionStart"
#
# Claude passes the hook payload as JSON on stdin; Terry extracts the session
# id from it, so this script never parses JSON itself.

event="$1"

if [ -z "$TERRY_TERMINAL_TOKEN" ] || [ -z "$TERRY_IPC_URL" ] || [ -z "$TERRY_IPC_TOKEN" ]; then
  exit 0
fi

payload=$(cat)

case "$event" in
  SessionStart)
    state="idle"
    ;;
  UserPromptSubmit|PreToolUse|PostToolUse)
    state="working"
    ;;
  Stop|SessionEnd)
    state="idle"
    ;;
  Notification)
    # Claude notifies for permission requests and for "waiting for your input".
    case "$payload" in
      *permission*) state="blocked" ;;
      *) state="idle" ;;
    esac
    ;;
  *)
    exit 0
    ;;
esac

curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_state" \
  -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"claude\",\"state\":\"$state\"}" \
  >/dev/null 2>&1

# SessionStart also reports the resumable session so a restart can run
# `claude --resume <id>` instead of starting over.
if [ "$event" = "SessionStart" ]; then
  session_id=$(printf '%s' "$payload" |
    sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1)
  if [ -n "$session_id" ]; then
    curl -s -m 2 -X POST "$TERRY_IPC_URL/api/report_agent_session" \
      -H "Authorization: Bearer $TERRY_IPC_TOKEN" \
      -H "Content-Type: application/json" \
      -d "{\"token\":\"$TERRY_TERMINAL_TOKEN\",\"agent\":\"claude\",\"session_id\":\"$session_id\"}" \
      >/dev/null 2>&1
  fi
fi

exit 0
