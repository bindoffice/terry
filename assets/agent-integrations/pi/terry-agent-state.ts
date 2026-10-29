// installed by Terry
// managed by Terry; reinstalling or updating the integration overwrites this file.
// TERRY_INTEGRATION_ID=pi
// TERRY_INTEGRATION_VERSION=1
// @ts-nocheck

// Reports Pi's lifecycle state to Terry over the local IPC API.
//
// Terry injects TERRY_TERMINAL_TOKEN, TERRY_IPC_URL and TERRY_IPC_TOKEN into
// every terminal, so a report is attributed to the terminal it came from.
//
// State mapping:
//   ui_prompt_start  -> blocked (permission/approval/question dialogs)
//   ui_prompt_end    -> back to working or idle
//   agent_start      -> working
//   agent_settled    -> idle

const token = process.env.TERRY_TERMINAL_TOKEN;
const ipcUrl = process.env.TERRY_IPC_URL;
const ipcToken = process.env.TERRY_IPC_TOKEN;
const source = "terry:pi";

function enabled() {
  return !!token && !!ipcUrl && !!ipcToken;
}

async function report(state, message) {
  if (!enabled()) {
    return;
  }
  try {
    await fetch(`${ipcUrl}/api/report_agent_state`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${ipcToken}`,
      },
      body: JSON.stringify({ token, state, message, agent: "pi", source }),
    });
  } catch {
    // Terry may be gone; state reporting is best-effort.
  }
}

async function reportSession(ctx) {
  if (!enabled()) {
    return;
  }
  let sessionPath;
  try {
    const file = ctx?.sessionManager?.getSessionFile?.();
    if (typeof file === "string" && file.length > 0) {
      sessionPath = file;
    }
  } catch {
    sessionPath = undefined;
  }
  if (!sessionPath) {
    return;
  }
  try {
    await fetch(`${ipcUrl}/api/report_agent_session`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${ipcToken}`,
      },
      body: JSON.stringify({ token, agent: "pi", session_path: sessionPath, source }),
    });
  } catch {
    // Terry may be gone; session reporting is best-effort.
  }
}

export default function (pi) {
  if (!enabled()) {
    return;
  }

  let rootSession = false;
  let agentActive = false;
  let blockedCount = 0;
  let blockedMessage;
  let lastState;
  let lastMessage;

  function desiredState() {
    if (blockedCount > 0) {
      return { state: "blocked", message: blockedMessage };
    }
    if (agentActive) {
      return { state: "working", message: undefined };
    }
    return { state: "idle", message: undefined };
  }

  function publish(force = false) {
    const next = desiredState();
    if (!force && next.state === lastState && next.message === lastMessage) {
      return;
    }
    lastState = next.state;
    lastMessage = next.message;
    void report(next.state, next.message);
  }

  pi.on("session_start", (_event, ctx) => {
    // RPC/JSON/print modes are headless and have no pane to attribute.
    if (ctx?.mode !== "tui") {
      return;
    }
    rootSession = true;
    agentActive = ctx?.isIdle?.() === false;
    void reportSession(ctx);
    publish(true);
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!rootSession) {
      return;
    }
    agentActive = true;
    void reportSession(ctx);
    publish();
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!rootSession || ctx?.isIdle?.() !== true) {
      return;
    }
    agentActive = false;
    publish();
  });

  // Pi fires these whenever it waits on a blocking user-facing prompt, which is
  // exactly the "blocked" state Terry's sidebar shows.
  pi.on("ui_prompt_start", (event) => {
    if (!rootSession) {
      return;
    }
    blockedCount += 1;
    blockedMessage = event?.title || event?.kind;
    publish();
  });

  pi.on("ui_prompt_end", () => {
    if (!rootSession) {
      return;
    }
    blockedCount = Math.max(0, blockedCount - 1);
    if (blockedCount === 0) {
      blockedMessage = undefined;
    }
    publish();
  });

  pi.on("session_shutdown", () => {
    if (!rootSession) {
      return;
    }
    rootSession = false;
    blockedCount = 0;
    agentActive = false;
    publish(true);
  });
}
