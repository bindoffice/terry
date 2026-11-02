import { JSDOM } from 'jsdom';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

// Resolved relative to this file so the test runs from a fresh checkout.
const WEBAPP = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const html = fs.readFileSync(path.join(WEBAPP, 'index.html'), 'utf8');
const js = fs.readFileSync(path.join(WEBAPP, 'app.js'), 'utf8');

const BASE = process.env.BASE || 'http://127.0.0.1:47823';
const TOKEN = process.env.TOKEN || 'test-token';

let failures = 0;
const check = (name, ok, detail = '') => {
  console.log(`  ${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ' — ' + detail : ''}`);
  if (!ok) failures++;
};

// ---- fake backend: records every request the app makes -------------------
const calls = [];
const responses = {
  '/api/list_sessions': { sessions: [
    { session: 't-1', title: '', cwd: '/home/x', cols: 80, rows: 24, agent: 'claude', state: 'blocked', application_cursor: false },
    { session: 't-2', title: 'shell', cwd: '/tmp', cols: 80, rows: 24, agent: null, state: null, application_cursor: false },
  ]},
  '/api/list_agent_states': { terminals: [
    { session: 't-1', title: '', cwd: '/home/x', agent: 'claude', state: 'blocked' },
  ]},
  '/api/read_screen': { session: 't-1', content: 'Deploy to prod? (y/n)\n', lines: ['Deploy to prod? (y/n)'] },
  '/api/send_keys': { status: 'ok' },
  '/api/send_text': { status: 'ok' },
  '/api/list_agents': { agents: [
    { id: 'claude', launch: 'claude', installed: true, can_resume: true },
    { id: 'codex', launch: 'codex', installed: false, can_resume: true },
  ]},
  '/api/new_tab': { session: 't-3' },
};

const dom = new JSDOM(html, { url: 'http://localhost/', runScripts: 'outside-only', pretendToBeVisual: true });
const { window } = dom;

window.fetch = async (url, options) => {
  const path = new URL(url).pathname;
  calls.push({ path, body: JSON.parse(options.body || '{}'), auth: options.headers.Authorization });
  const body = responses[path];
  if (!body) return { ok: false, status: 404, json: async () => ({ error: 'no route' }) };
  return { ok: true, status: 200, json: async () => body };
};
window.AbortSignal = globalThis.AbortSignal;
window.localStorage.clear();
window.alert = (m) => calls.push({ path: 'alert', body: m });
window.confirm = () => true;
window.prompt = () => null;

window.eval(js);

const $ = (id) => window.document.getElementById(id);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

console.log('\n[1] 未配对时显示配对屏');
check('pairing 可见', !$('pairing').hidden);
check('list 隐藏', $('list').hidden);

console.log('\n[2] 配对成功后进入列表');
$('pair-url').value = BASE;
$('pair-token').value = TOKEN;
$('pair-submit').dispatchEvent(new window.Event('click'));
await wait(300);
check('pairing 隐藏', $('pairing').hidden);
check('list 可见', !$('list').hidden);
check('发出了带 Bearer 的请求', calls.some(c => c.auth === `Bearer ${TOKEN}`));

console.log('\n[3] 列表渲染 agent 与 shell');
await wait(500);
const agentRows = [...$('agents').querySelectorAll('[data-session]')].map(b => b.dataset.session);
const shellRows = [...$('shells').querySelectorAll('[data-session]')].map(b => b.dataset.session);
check('agent 行渲染', agentRows.includes('t-1'), JSON.stringify(agentRows));
check('shell 归入折叠区', shellRows.includes('t-2'), JSON.stringify(shellRows));
check('blocked 状态点', $('agents').innerHTML.includes('data-state="blocked"'));
// title 为空时必须回退到屏幕文本，而不是显示 UUID
check('空 title 回退到屏幕文本', $('agents').innerHTML.includes('Deploy to prod?') , $('agents').innerHTML.slice(0, 0));

console.log('\n[4] 打开详情并读取屏幕');
$('agents').querySelector('[data-session="t-1"]').dispatchEvent(new window.Event('click', { bubbles: true }));
await wait(400);
check('detail 可见', !$('detail').hidden);
check('屏幕内容渲染', $('output').textContent.includes('Deploy to prod?'), JSON.stringify($('output').textContent));

console.log('\n[5] 按键条发送 send_keys');
calls.length = 0;
$('detail').querySelector('[data-key="ctrl-c"]').dispatchEvent(new window.Event('click', { bubbles: true }));
await wait(200);
const keyCall = calls.find(c => c.path === '/api/send_keys');
check('send_keys 被调用', !!keyCall, JSON.stringify(calls.map(c=>c.path)));
check('发送的是 ctrl-c', keyCall && keyCall.body.keys[0] === 'ctrl-c', JSON.stringify(keyCall?.body));

console.log('\n[6] 输入框发送 send_text');
calls.length = 0;
$('input').value = 'y';
$('send').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }));
await wait(200);
const textCall = calls.find(c => c.path === '/api/send_text');
check('send_text 被调用', !!textCall, JSON.stringify(calls.map(c=>c.path)));
check('文本带换行', textCall && textCall.body.text === 'y\n', JSON.stringify(textCall?.body));

console.log('\n[7] 返回列表');
$('back').dispatchEvent(new window.Event('click', { bubbles: true }));
await wait(200);
check('回到 list', !$('list').hidden && $('detail').hidden);

console.log(`\n${failures === 0 ? 'ALL PASS' : failures + ' FAILURE(S)'}`);
process.exit(failures === 0 ? 0 : 1);
