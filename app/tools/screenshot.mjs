// Screenshots of the app for review: starts headless Chrome with a DevTools port, opens each URL, waits until the 3D
// preview reports it has drawn the deck (window.cavemanPreview.ready()), and saves a PNG. Uses Node's own fetch and
// WebSocket (no packages).
// Usage: node tools/screenshot.mjs <out dir> <name>=<url>[ :: <script>] ...
//   e.g. node tools/screenshot.mjs shots showcase=http://localhost:5173/ ride=http://localhost:5173/?mode=ride
//   A script after ` :: ` runs once the preview is ready (to click something), and the shot is taken 3 s later.
//   An empty URL keeps the current page. With ATTACH=<port> it uses a running browser's DevTools port instead of
//   starting Chrome, e.g. the Tauri app started with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>.
//   SHOT_DEADLINE_MS (default 100000) bounds the whole run, shorter than a 120 s tool timeout.
//
// Chrome must never outlive this script (2026-10-21: orphans left by an earlier version rendered WebGL in software
// for hours, and a fixed port made later runs talk to an orphan). So: Chrome picks its own port (written to the
// profile's DevToolsActivePort), every exit path kills Chrome's whole process tree and deletes the profile, and a run
// first sweeps away any Chrome or profile an earlier run of this script left behind.

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const CHROME = process.env.CHROME ?? 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
const PROFILE_PREFIX = 'caveman-shots-';
const DEADLINE = Date.now() + Number(process.env.SHOT_DEADLINE_MS ?? 100000);
const [outDir, ...shots] = process.argv.slice(2);
if (!outDir || shots.length === 0) {
  console.error('usage: node tools/screenshot.mjs <out dir> <name>=<url> ...');
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });

const wait = (ms) => new Promise((done) => setTimeout(done, ms));
const left = () => DEADLINE - Date.now();

// Kills a process and every child it started (Windows does not kill children with their parent).
function killTree(pid) {
  if (!pid) return;
  if (process.platform === 'win32') {
    const r = spawnSync('taskkill', ['/PID', `${pid}`, '/T', '/F'], { encoding: 'utf8' });
    if (process.env.SHOT_DEBUG) console.error(`taskkill ${pid}: status ${r.status} ${r.error ?? ''} ${(r.stdout ?? '').trim()} ${(r.stderr ?? '').trim()}`);
  }
  else {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {
      // already gone
    }
  }
}

function removeProfile(dir) {
  for (let i = 0; i < 10 && existsSync(dir); i++) {
    try {
      rmSync(dir, { recursive: true, force: true });
    } catch {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);   // Chrome may still hold files for a moment
    }
  }
}

// Every Chrome process whose command line names `marker` (a profile folder): the browser, its GPU, network, crashpad and
// renderer processes all carry --user-data-dir. On Windows the PID spawn() returns is a launcher that hands over to the
// real browser and exits, so killing by that PID finds nothing: kill by the profile instead. Returns how many.
function killByProfile(marker) {
  if (process.platform !== 'win32') return 0;
  const script = `$p = @(Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" | Where-Object { $_.CommandLine -like '*${marker}*' }); $p | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }; $p.Count`;
  const r = spawnSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8' });
  const n = Number((r.stdout ?? '').trim()) || 0;
  if (process.env.SHOT_DEBUG) console.error(`killed ${n} Chrome process(es) on ${marker}`);
  return n;
}

// Leftovers of earlier runs: Chrome processes on one of our profiles, then the profiles themselves.
function sweep() {
  const n = killByProfile(PROFILE_PREFIX);
  if (n) console.error(`swept ${n} leftover Chrome process(es) from an earlier run`);
  for (const name of readdirSync(tmpdir())) {
    if (name.startsWith(PROFILE_PREFIX)) removeProfile(join(tmpdir(), name));
  }
}

const attach = process.env.ATTACH ? Number(process.env.ATTACH) : null;
const profile = join(tmpdir(), `${PROFILE_PREFIX}${process.pid}-${Date.now()}`);
let chrome = null;
let ws = null;
let cleaned = false;

function cleanup() {
  if (cleaned) return;
  cleaned = true;
  if (process.env.SHOT_DEBUG) console.error(`cleanup: chrome pid ${chrome?.pid ?? 'none'}, profile ${profile}`);
  try {
    ws?.close();
  } catch {
    // closing a dead socket
  }
  if (chrome) {
    killTree(chrome.pid);   // the launcher, if it is still alive
    killByProfile(profile.split(/[\\/]/).pop());   // the real browser tree
    removeProfile(profile);
  }
}

process.on('exit', cleanup);
for (const signal of ['SIGINT', 'SIGTERM', 'SIGBREAK', 'SIGHUP']) {
  process.on(signal, () => {
    cleanup();
    process.exit(130);
  });
}
process.on('uncaughtException', (e) => {
  console.error(e);
  cleanup();
  process.exit(1);
});
process.on('unhandledRejection', (e) => {
  console.error(e);
  cleanup();
  process.exit(1);
});

// The DevTools port: ATTACH's, or the one Chrome picked for itself (--remote-debugging-port=0 writes it to the profile).
async function devtoolsPort() {
  if (attach) return attach;
  const file = join(profile, 'DevToolsActivePort');
  while (left() > 0) {
    if (existsSync(file)) {
      const port = Number(readFileSync(file, 'utf8').split('\n')[0]);
      if (port > 0) return port;
    }
    await wait(100);
  }
  throw new Error('Chrome did not report its DevTools port before the deadline');
}

async function target(port) {
  while (left() > 0) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === 'page');
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // not up yet
    }
    await wait(200);
  }
  throw new Error('Chrome did not open a page before the deadline');
}

try {
  if (!attach) {
    sweep();
    chrome = spawn(CHROME, [
      '--headless=new',
      '--remote-debugging-port=0',
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--use-angle=swiftshader',
      '--enable-unsafe-swiftshader',
      '--hide-scrollbars',
      '--window-size=1440,900',
      'about:blank',
    ], { stdio: 'ignore', detached: process.platform !== 'win32' });
    if (process.platform === 'win32') {
      // A watchdog for the one path no handler sees (node killed hard): it waits for this process to end, however it
      // ends, then kills this run's Chrome and deletes its profile. After a normal exit it finds nothing to do.
      const marker = profile.split(/[\\/]/).pop();
      // (Wait-Process returned at once when started this way, so it polls)
      const watchdog = `while (Get-Process -Id ${process.pid} -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 500 }; Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" | Where-Object { $_.CommandLine -like '*${marker}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }; Start-Sleep -Milliseconds 500; Remove-Item -LiteralPath '${profile}' -Recurse -Force -ErrorAction SilentlyContinue`;
      const dog = spawn('powershell', ['-NoProfile', '-NonInteractive', '-WindowStyle', 'Hidden', '-Command', watchdog], { stdio: 'ignore', detached: true });
      if (process.env.SHOT_DEBUG) {
        console.error(`watchdog pid ${dog.pid}`);
        dog.on('exit', (code) => console.error(`watchdog exited early: ${code}`));
        dog.on('error', (e) => console.error(`watchdog failed: ${e}`));
      }
      dog.unref();
    }
  }
  ws = new WebSocket(await target(await devtoolsPort()));
  await new Promise((ok, fail) => {
    ws.onopen = ok;
    ws.onerror = fail;
  });
  let nextId = 1;
  const pending = new Map();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  const send = (method, params = {}) => {
    const id = nextId++;
    ws.send(JSON.stringify({ id, method, params }));
    return new Promise((done) => pending.set(id, done));
  };

  await send('Page.enable');
  if (chrome) await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  for (const shot of shots) {
    if (left() <= 5000) {
      console.error(`deadline reached: skipped the remaining shots from ${shot.slice(0, shot.indexOf('='))} on`);
      break;
    }
    const at = shot.indexOf('=');
    const name = shot.slice(0, at);
    const [url, script] = shot.slice(at + 1).split(' :: ');
    if (url) await send('Page.navigate', { url });
    const started = Date.now();
    let ready = false;
    // wait for the preview, leaving time for the script, the capture and the clean-up before the deadline
    while (Date.now() - started < 60000 && left() > 5000) {
      await wait(500);
      const r = await send('Runtime.evaluate', {
        expression: '!!(window.cavemanPreview && window.cavemanPreview.ready())',
        returnByValue: true,
      });
      if (r.result?.result?.value === true) {
        ready = true;
        break;
      }
    }
    if (script) {
      await send('Runtime.evaluate', { expression: script });
      await wait(3000);
    }
    await wait(400);
    const shotResult = await send('Page.captureScreenshot', { format: 'png' });
    const file = join(outDir, `${name}.png`);
    writeFileSync(file, Buffer.from(shotResult.result.data, 'base64'));
    console.log(`${ready ? 'ready' : 'TIMED OUT'}  ${file}  (${((Date.now() - started) / 1000).toFixed(1)} s)`);
  }
} finally {
  cleanup();
}
