/* Terry mobile console.
 *
 * A thin client for the terminal daemon's HTTP endpoint. It renders plain text
 * (`/api/read_screen`) rather than a styled grid: the point is answering an
 * agent, and a `<pre>` of the screen tail is enough for that. Adding a canvas
 * renderer later does not change any of the code below.
 *
 * The two hard parts are not rendering:
 *
 * 1. **Staying connected.** A phone changes networks, sleeps, and loses
 *    signal. Every request can fail, so all of them go through `api()`, and the
 *    poll loop always reschedules itself in `finally`.
 * 2. **Not wasting the battery.** Polling a busy agent forever drains a phone.
 *    The interval adapts: fast while something is working or the user is
 *    watching, slow otherwise, and paused while the tab is hidden.
 */

'use strict';

// ---------------------------------------------------------------------------
// Stored connection
// ---------------------------------------------------------------------------

const STORE_KEY = 'terry.connection';

/** @returns {{url: string, token: string} | null} */
function loadConnection() {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!parsed.url || !parsed.token) return null;
    return { url: parsed.url.replace(/\/$/, ''), token: parsed.token };
  } catch {
    return null;
  }
}

// localStorage, not a cookie: this is a single-origin app loaded from the
// daemon itself, so there is no cross-site context to protect, and the token
// must survive a restart. The pairing screen says plainly what it grants.
function saveConnection(url, token) {
  localStorage.setItem(STORE_KEY, JSON.stringify({ url: url.replace(/\/$/, ''), token }));
}

function clearConnection() {
  localStorage.removeItem(STORE_KEY);
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/** Raised when the server refuses the token, so the UI can offer re-pairing. */
class Unauthorized extends Error {}

/**
 * Calls one endpoint.
 *
 * Every failure mode is surfaced as an exception with a message a user can act
 * on; the callers decide whether to retry.
 */
async function api(path, body = {}) {
  const conn = loadConnection();
  if (!conn) throw new Unauthorized('未配对');

  let response;
  try {
    response = await fetch(conn.url + path, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: 'Bearer ' + conn.token,
      },
      body: JSON.stringify(body),
      // A hung request must not block the poll loop forever.
      signal: AbortSignal.timeout(15000),
    });
  } catch (error) {
    throw new Error(error.name === 'TimeoutError' ? '请求超时' : '无法连接');
  }

  if (response.status === 401) throw new Unauthorized('令牌无效');
  if (response.status === 404) {
    const payload = await response.json().catch(() => ({}));
    throw new Error(payload.error || '会话不存在');
  }
  if (!response.ok) {
    const payload = await response.json().catch(() => ({}));
    throw new Error(payload.hint || payload.error || 'HTTP ' + response.status);
  }
  return response.json();
}

// ---------------------------------------------------------------------------
// UI plumbing
// ---------------------------------------------------------------------------

const $ = (id) => document.getElementById(id);

function show(screen) {
  for (const id of ['pairing', 'list', 'detail']) {
    $(id).hidden = id !== screen;
  }
}

function setConn(state, label) {
  const element = $('conn');
  element.dataset.state = state;
  element.textContent = label;
}

/** Escapes text before it is inserted as HTML. */
function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (char) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[char]);
}

/** Agent state as a dot. Unknown sessions get the idle dot. */
function stateDot(state) {
  const known = ['working', 'blocked', 'idle'].includes(state) ? state : 'idle';
  return `<span class="dot" data-state="${known}" title="${escapeHtml(state || '未知')}"></span>`;
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

const state = {
  /** The session shown in the detail screen, or null on the list. */
  openSession: null,
  /** Agent terminals from the last successful poll. */
  agents: [],
  /** Non-agent terminals, shown in the collapsed section. */
  shells: [],
  /** True while the tab is visible; polling pauses otherwise. */
  visible: true,
  /** Set by a failure so the next poll retries sooner. */
  failures: 0,
};

/** How long to wait before the next poll. */
function pollDelay() {
  if (state.failures > 0) {
    // Back off on errors but cap it: a phone that lost signal for a minute
    // should reconnect quickly once it is back, not wait out a long timer.
    return Math.min(1000 * 2 ** (state.failures - 1), 15000);
  }
  if (state.openSession) {
    // Watching one session: the screen changes as the agent works, so poll at
    // a rate that feels live without hammering the daemon.
    return 700;
  }
  // On the list, only state changes matter; they arrive far less often.
  return state.agents.some((agent) => agent.state === 'working') ? 2000 : 5000;
}

// ---------------------------------------------------------------------------
// List screen
// ---------------------------------------------------------------------------

function renderList() {
  const agents = state.agents;
  // "Empty" means no terminals at all, not "no agents": a shell-only screen is
  // a normal state and must not be announced as empty.
  $('list-empty').hidden = agents.length + state.shells.length > 0;

  $('agents').innerHTML = agents
    .map(
      (agent) => `
      <li>
        <button class="row" data-session="${escapeHtml(agent.session)}">
          ${stateDot(agent.state)}
          <span class="name">${escapeHtml(agent.title || agent.label || agent.session)}</span>
          <span class="cwd">${escapeHtml(agent.agent || '')}</span>
        </button>
        ${
          agent.cwd
            ? `<div class="cwd">${escapeHtml(agent.cwd)}</div>`
            : ''
        }
        ${agent.preview ? `<div class="preview">${escapeHtml(agent.preview)}</div>` : ''}
      </li>`,
    )
    .join('');

  $('shell-count').textContent = state.shells.length ? `(${state.shells.length})` : '';
  $('shells-wrap').hidden = state.shells.length === 0;
  $('shells').innerHTML = state.shells
    .map(
      (shell) => `
      <li>
        <button class="row" data-session="${escapeHtml(shell.session)}">
          ${stateDot('idle')}
          <span class="name">${escapeHtml(shell.title || shell.label || shell.session)}</span>
        </button>
        ${shell.cwd ? `<div class="cwd">${escapeHtml(shell.cwd)}</div>` : ''}
        ${shell.preview ? `<div class="preview">${escapeHtml(shell.preview)}</div>` : ''}
      </li>`,
    )
    .join('');
}

async function pollList() {
  const [agentPayload, sessionPayload] = await Promise.all([
    api('/api/list_agent_states'),
    api('/api/list_sessions'),
  ]);

  // Agents first: those are the ones that need a decision from the user.
  state.agents = agentPayload.terminals || [];

  // Everything without a recognised agent is a plain shell. A shell is not an
  // "idle agent" -- but on a phone it is still the thing the user is most often
  // looking at (a build, a test run), so it is listed plainly rather than being
  // hidden behind a disclosure. Only the *label* differs.
  const agentSessions = new Set(state.agents.map((agent) => agent.session));
  state.shells = (sessionPayload.sessions || []).filter(
    (session) => !agentSessions.has(session.session),
  );

  // Pull one preview line per session so the list answers "what is it doing?"
  // without needing to open every one. This also supplies a fallback label:
  // a plain shell never sets an OSC title, so `title` is empty for the most
  // common case and showing a session id instead would be useless.
  const annotate = async (item) => {
    try {
      const payload = await api('/api/read_screen', { session: item.session, lines: 4 });
      const lines = (payload.lines || []).filter((line) => line.trim());
      item.preview = lines.length ? lines[lines.length - 1].trim().slice(0, 120) : '';
      item.label = lines.length ? lines[lines.length - 1].trim().slice(0, 48) : '';
    } catch {
      item.preview = '';
      item.label = '';
    }
  };
  await Promise.all([
    ...state.agents.slice(0, 12).map(annotate),
    ...state.shells.slice(0, 8).map(annotate),
  ]);

  renderList();
}

// ---------------------------------------------------------------------------
// Detail screen
// ---------------------------------------------------------------------------

async function openSession(session) {
  state.openSession = session;
  state.failures = 0;
  $('detail-title').textContent = session;
  $('detail-state').dataset.state = 'idle';
  $('output').textContent = '';
  show('detail');
  // Focus the input only after the screen is visible, otherwise iOS ignores it.
  $('input').value = '';
  await pollDetail();
  // Hand the loop over to the detail poller. Without this the screen would be
  // fetched exactly once and then freeze, which looks like a dead session.
  poll();
}

function closeDetail() {
  state.openSession = null;
  show('list');
  poll(); // refresh the list immediately rather than waiting out the timer
}

async function pollDetail() {
  const session = state.openSession;
  if (!session) return;

  const payload = await api('/api/read_screen', { session, lines: 200 });
  if (state.openSession !== session) return; // user navigated away mid-request

  const output = $('output');
  const atBottom = output.scrollHeight - output.scrollTop - output.clientHeight < 40;
  output.textContent = payload.content || '';
  // Only follow the tail when the user has not scrolled up to read something.
  if (atBottom) output.scrollTop = output.scrollHeight;

  // Keep the header in step with the agent state from the list poll.
  const known = state.agents.find((agent) => agent.session === session);
  if (known) {
    $('detail-title').textContent = known.title || known.label || session;
    $('detail-state').dataset.state = known.state || 'idle';
  }
}

// ---------------------------------------------------------------------------
// Poll loop
// ---------------------------------------------------------------------------

let pollTimer = null;

/**
 * Runs one poll, then schedules the next.
 *
 * The reschedule lives in `finally` so a thrown error can never stop the loop:
 * that is the single most common way a mobile client dies silently after a
 * network change.
 */
function poll() {
  clearTimeout(pollTimer);
  if (!state.visible || !loadConnection()) return;

  const run = async () => {
    try {
      if (state.openSession) {
        // Refresh the list too, so the header state and previews stay honest.
        await pollList().catch(() => {});
        await pollDetail();
      } else {
        await pollList();
      }
      state.failures = 0;
      setConn('live', '在线');
    } catch (error) {
      if (error instanceof Unauthorized) {
        setConn('off', '未授权');
        unpair('令牌已失效，请重新配对');
        return;
      }
      state.failures += 1;
      setConn('retry', '重连中');
    } finally {
      if (loadConnection() && state.visible) {
        pollTimer = setTimeout(run, pollDelay());
      }
    }
  };

  // Delay the first run slightly so a quick burst of user actions does not
  // queue several immediate polls.
  pollTimer = setTimeout(run, 0);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function sendText(text) {
  const session = state.openSession;
  if (!session || !text) return;
  try {
    await api('/api/send_text', { session, text });
  } catch (error) {
    setConn('retry', error.message);
  }
}

async function sendKey(key) {
  const session = state.openSession;
  if (!session) return;
  try {
    // `keys` is a list so a future "send chord" button can reuse this.
    await api('/api/send_keys', { session, keys: [key] });
  } catch (error) {
    setConn('retry', error.message);
  }
}

/** Prompts for a directory and an agent, then starts a session. */
async function newSession() {
  let catalogue;
  try {
    catalogue = await api('/api/list_agents');
  } catch (error) {
    alert('无法读取 agent 列表：' + error.message);
    return;
  }

  // Only offer what this machine can actually launch. The daemon reports it,
  // so the list is honest instead of failing after the user picks.
  const usable = (catalogue.agents || []).filter((agent) => agent.installed);
  if (usable.length === 0) {
    alert('这台机器上没有安装任何 agent。\n先安装一个，例如 claude 或 codex。');
    return;
  }

  const names = usable.map((agent) => agent.id).join(' / ');
  const chosen = prompt(`启动哪个 agent？\n可选：${names}`, usable[0].id);
  if (!chosen) return;
  if (!usable.some((agent) => agent.id === chosen)) {
    alert('没有这个 agent：' + chosen);
    return;
  }

  const cwd = prompt('在哪个目录启动？（留空则用默认）', '');
  const body = { agent: chosen };
  if (cwd) body.cwd = cwd;

  try {
    const payload = await api('/api/new_tab', body);
    await openSession(payload.session);
  } catch (error) {
    alert('启动失败：' + error.message);
  }
}

function unpair(message) {
  clearConnection();
  state.openSession = null;
  state.agents = [];
  state.shells = [];
  clearTimeout(pollTimer);
  show('pairing');
  $('pair-error').hidden = !message;
  $('pair-error').textContent = message || '';
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

async function tryPair() {
  const url = $('pair-url').value.trim();
  const token = $('pair-token').value.trim();
  const error = $('pair-error');

  if (!url || !token) {
    error.hidden = false;
    error.textContent = '地址和令牌都必须填写。';
    return;
  }
  // A bare host is a common mistake; default to http since that is what the
  // daemon speaks.
  const normalized = /^https?:\/\//.test(url) ? url : 'http://' + url;
  saveConnection(normalized, token);

  const button = $('pair-submit');
  button.disabled = true;
  button.textContent = '连接中…';
  try {
    await api('/api/list_sessions');
    error.hidden = true;
    show('list');
    poll();
  } catch (failure) {
    clearConnection();
    error.hidden = false;
    error.textContent =
      failure instanceof Unauthorized
        ? '令牌不对。检查电脑上的 remote-token 文件。'
        : '连不上 ' + normalized + '：' + failure.message;
  } finally {
    button.disabled = false;
    button.textContent = '连接';
  }
}

/** Prefills the form from the page's own origin, which is the common case. */
function prefillFromOrigin() {
  if (location.protocol === 'http:' || location.protocol === 'https:') {
    if (location.host && $('pair-url').value === '') {
      $('pair-url').value = location.origin;
    }
  }
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

$('pair-submit').addEventListener('click', tryPair);
$('pair-token').addEventListener('keydown', (event) => {
  if (event.key === 'Enter') tryPair();
});

$('refresh').addEventListener('click', () => poll());
$('new').addEventListener('click', newSession);
$('unpair').addEventListener('click', () => {
  if (confirm('断开配对？之后需要重新输入令牌。')) unpair('');
});
$('back').addEventListener('click', closeDetail);

// One delegated listener for both lists: the rows are re-rendered on every
// poll, so per-row listeners would have to be reattached each time.
for (const listId of ['agents', 'shells']) {
  $(listId).addEventListener('click', (event) => {
    const row = event.target.closest('[data-session]');
    if (row) openSession(row.dataset.session);
  });
}

$('send').addEventListener('submit', (event) => {
  event.preventDefault();
  const input = $('input');
  const text = input.value;
  if (!text) return;
  input.value = '';
  // A trailing newline is what "press enter" means; send the text as typed and
  // let the daemon's newline handling turn it into a CR.
  sendText(text + '\n');
});

document.querySelector('.keys').addEventListener('click', (event) => {
  const button = event.target.closest('[data-key]');
  if (button) sendKey(button.dataset.key);
});

// Pause polling while the tab is hidden. A phone that keeps polling in the
// background drains its battery and cannot show anything anyway.
document.addEventListener('visibilitychange', () => {
  state.visible = !document.hidden;
  if (state.visible) poll();
  else clearTimeout(pollTimer);
});

// Coming back to a suspended page does not fire visibilitychange on every
// platform, so also react to being restored.
window.addEventListener('pageshow', () => {
  state.visible = true;
  poll();
});

// iOS hides the bottom bar when the keyboard opens; recomputing on resize keeps
// the key bar reachable.
window.addEventListener('resize', () => {
  const output = $('output');
  if (output) output.scrollTop = output.scrollHeight;
});

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

if (loadConnection()) {
  show('list');
  poll();
} else {
  prefillFromOrigin();
  show('pairing');
}
