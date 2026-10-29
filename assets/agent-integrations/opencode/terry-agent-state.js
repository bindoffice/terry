// installed by Terry
// managed by Terry; reinstalling or updating the integration overwrites this file.
// TERRY_INTEGRATION_ID=opencode
// TERRY_INTEGRATION_VERSION=1

// Reports OpenCode's lifecycle state to Terry over the local IPC API.
//
// Terry injects TERRY_TERMINAL_TOKEN, TERRY_IPC_URL and TERRY_IPC_TOKEN into
// every terminal, so a report is attributed to the terminal it came from.

const token = process.env.TERRY_TERMINAL_TOKEN;
const ipcUrl = process.env.TERRY_IPC_URL;
const ipcToken = process.env.TERRY_IPC_TOKEN;
const AGENT = "opencode";

function enabled() {
  return !!token && !!ipcUrl && !!ipcToken;
}

async function post(path, body) {
  if (!enabled()) {
    return;
  }
  try {
    await fetch(`${ipcUrl}${path}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${ipcToken}`,
      },
      body: JSON.stringify({ token, agent: AGENT, ...body }),
    });
  } catch {
    // Terry may be gone; state reporting is best-effort.
  }
}

function reportState(state, sessionID) {
  return post("/api/report_agent_state", { state, message: sessionID });
}

function reportSession(sessionID) {
  if (!sessionID) {
    return Promise.resolve();
  }
  return post("/api/report_agent_session", { session_id: sessionID });
}

// Only a local TUI owns the pane's lifecycle. `run`, shared servers and
// attached clients either have no pane or belong to a different TUI.
function ownsLocalLifecycle() {
  const args = process.argv.slice(2);
  const separator = args.indexOf("--");
  if (separator !== -1) {
    args.splice(separator);
  }
  if (args.some((arg) => arg === "--attach" || arg.startsWith("--attach="))) {
    return false;
  }
  while (
    args[0] === "--print-logs" ||
    args[0] === "--log-level" ||
    args[0]?.startsWith("--log-level=")
  ) {
    args.splice(0, args[0] === "--log-level" ? 2 : 1);
  }
  return (
    args[0] === "run" ||
    (!["serve", "web", "attach"].includes(args[0]) && args.includes("--mini"))
  );
}

const SESSION_STATE_BY_STATUS = new Map([
  ["idle", "idle"],
  ["active", "working"],
  ["busy", "working"],
  ["pending", "working"],
  ["retry", "working"],
  ["running", "working"],
  ["streaming", "working"],
  ["working", "working"],
]);

function stateFromSessionStatus(status) {
  const kind = typeof status === "string" ? status : status?.type;
  return typeof kind === "string"
    ? SESSION_STATE_BY_STATUS.get(kind.toLowerCase())
    : undefined;
}

function sessionIDFromProperties(properties) {
  return typeof properties?.sessionID === "string" && properties.sessionID
    ? properties.sessionID
    : undefined;
}

// Child (subagent) sessions must not replace the pane's root session.
const CHILD_EVENT_STATES = new Map([
  ["permission.asked", "blocked"],
  ["question.asked", "blocked"],
  ["permission.replied", "working"],
  ["question.replied", "working"],
  ["question.rejected", "working"],
]);

export const TerryAgentStatePlugin = async () => {
  if (!enabled() || !ownsLocalLifecycle()) {
    return {};
  }

  const childSessions = new Map();
  let reportedRootSessionID;

  return {
    "chat.message": async ({ sessionID }) => {
      if (sessionID && childSessions.has(sessionID)) {
        return;
      }
      await reportState("working", sessionID);
    },
    event: async ({ event }) => {
      const type = event?.type;
      const properties = event?.properties ?? {};
      const sessionID = sessionIDFromProperties(properties);

      const info = properties.info;
      if (info?.id && info.parentID) {
        childSessions.set(info.id, info.parentID);
      }
      if (sessionID && childSessions.has(sessionID)) {
        const state = CHILD_EVENT_STATES.get(type);
        if (state) {
          let rootSessionID = sessionID;
          while (childSessions.has(rootSessionID)) {
            rootSessionID = childSessions.get(rootSessionID);
          }
          await reportState(state, rootSessionID);
        }
        return;
      }

      switch (type) {
        case "session.created":
          reportedRootSessionID = sessionID;
          await reportSession(sessionID);
          break;
        case "session.updated":
          if (sessionID && sessionID !== reportedRootSessionID) {
            await reportSession(sessionID);
          }
          break;
        case "session.status": {
          const state = stateFromSessionStatus(properties.status);
          if (state) {
            await reportState(state, sessionID);
          } else {
            await reportSession(sessionID);
          }
          break;
        }
        case "tool.execute.before":
        case "tool.execute.after":
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
        case "session.compacted":
          await reportState("working", sessionID);
          break;
        case "permission.asked":
        case "question.asked":
        case "session.error":
          await reportState("blocked", sessionID);
          break;
        case "session.idle":
          await reportState("idle", sessionID);
          break;
        default:
          break;
      }
    },
  };
};

export default {
  id: "terry.opencode",
  server: TerryAgentStatePlugin,
  setup() {},
};
