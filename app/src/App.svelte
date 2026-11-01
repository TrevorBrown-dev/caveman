<script lang="ts">
  // The shell: the launcher's front page (Home) over a background picture, and the pages that slide over it: Plugins,
  // Studio and the Console. It owns the config, the library, the game's state (polled), and the FX DLL's life while the
  // app is open: when the game runs without the DLL and the setting says so, the DLL goes in; once it answers, the
  // plugins marked to start with the game are started, once per game session.
  import { onMount } from 'svelte';
  import Home from './components/Home.svelte';
  import Plugins from './components/Plugins.svelte';
  import Studio from './components/Studio.svelte';
  import Console from './components/Console.svelte';
  import SettingsDialog from './components/SettingsDialog.svelte';
  import * as api from './lib/api';
  import * as live from './lib/live';
  import { ENTRIES } from './lib/changelog';
  import { pluginsOf, type Config, type LauncherStatus, type Library as Lib, type PluginsConfig } from './lib/types';

  type View = 'home' | 'plugins' | 'studio' | 'console';
  let view = $state<View>('home');
  let config = $state<Config | null>(null);
  let library = $state<Lib | null>(null);
  let status = $state<LauncherStatus | null>(null);
  let version = $state('');
  let gameRunning = $state(false);
  let injected = $state<boolean | null>(null);
  let playing = $state(false);
  let showSettings = $state(false);
  let showChangelogs = $state(false);
  let toast: { text: string; kind: 'info' | 'error' } | null = $state(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const plugins: PluginsConfig = $derived(pluginsOf(config));

  function say(text: string, kind: 'info' | 'error' = 'info'): void {
    toast = { text, kind };
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), kind === 'error' ? 7000 : 3500);
  }

  async function refresh(): Promise<void> {
    try {
      library = await api.library();
    } catch (e) {
      say(`Could not read the drop folders: ${String(e)}`, 'error');
    }
  }

  async function readStatus(): Promise<void> {
    try {
      status = await api.launcherStatus();
    } catch (e) {
      say(String(e), 'error');
    }
  }

  async function saveConfig(c: Config): Promise<void> {
    try {
      config = await api.saveConfig(c);
      showSettings = false;
      await refresh();
      await readStatus();
    } catch (e) {
      say(String(e), 'error');
    }
  }

  function savePlugins(p: PluginsConfig): void {
    if (!config) return;
    const before = plugins;
    void saveConfig({ ...config, plugins: p as unknown as Record<string, unknown> });
    // a plugin's checkbox is its switch: ticked while the game runs with the DLL in, it starts now; unticked, it stops
    for (const id of ['bars', 'obs', 'boardcam'] as const) {
      if (p[id].autostart === before[id].autostart) continue;
      if (p[id].autostart) {
        if (injected === true) startPlugin(id, p).catch((e) => say(String(e), 'error'));
      } else {
        stopPlugin(id).catch((e) => say(String(e), 'error'));
      }
    }
  }

  async function stopPlugin(id: 'bars' | 'obs' | 'boardcam'): Promise<void> {
    if (id === 'boardcam') {
      await live.boardcamClose();
    } else if (id === 'obs') {
      if (await live.videoRunning()) await live.videoStop();
    } else if (injected === true) {
      await live.live(['bars', 'off']);
    }
  }

  /** Starts one plugin as its "start with the game" switch would. Returns what was started, or null. */
  async function startPlugin(id: 'bars' | 'obs' | 'boardcam', p: PluginsConfig): Promise<string | null> {
    if (id === 'bars') {
      await live.live(['bars', 'on']);
      return 'the soundbars';
    }
    if (id === 'obs') {
      if (await live.videoRunning()) return null;
      const source = p.obs.source === 'obs' ? 'obs' : p.obs.file.trim();
      if (!source) return null;
      await live.videoStart([source, '--rotate', p.obs.rotate, '--fit', p.obs.fit]);
      return 'the OBS link';
    }
    // (the window reads the picture on the board itself: `boardArt`)
    await live.boardcamOpen({ ...p.boardcam, art: null, rate: 60 });
    return 'the board cam window';
  }

  // ---- the game's state, and what follows from it
  let sessionAttached = false;
  let sessionStarted = false;
  /** The picture last put on the board has been put back in this game session (or there was none to put back). */
  let reapplied = false;
  let wasRunning = false;
  async function poll(): Promise<void> {
    let running = false;
    try {
      running = await api.gameRunning();
    } catch {
      running = false;
    }
    gameRunning = running;
    if (!running) {
      injected = null;
      if (wasRunning) {
        sessionAttached = false;
        sessionStarted = false;
        reapplied = false;
        void readStatus();
      }
      wasRunning = false;
      return;
    }
    wasRunning = true;
    let now: boolean | null = null;
    let brief: live.LiveStatus['brief'] = undefined;
    try {
      const s = await live.status(false);
      now = s.injected;
      brief = s.brief;
    } catch {
      now = null;
    }
    const before = injected;
    injected = now;
    // the picture last put on the board comes back by itself in a new game session (Trevor, 2026-10-30): once the
    // DLL's board module is up with nothing on the board yet (its light status says so; DLLs from that evening on)
    const remembered = plugins.live.lastDeck;
    if (now === true && !reapplied && remembered && brief && typeof brief === 'object' && brief.art === null) {
      reapplied = true;
      try {
        const r = await live.live(remembered.args);
        if (r.ok) say(`Back on the board: ${remembered.label}`);
        else say(`${remembered.label} could not go back on the board: ${r.text}`, 'error');
      } catch (e) {
        say(`${remembered.label} could not go back on the board: ${String(e)}`, 'error');
      }
    }
    if (now === false && config?.attach && !sessionAttached && !api.isMock) {
      sessionAttached = true;
      try {
        await api.attachDll();
        say('The game is running: putting the FX DLL in');
      } catch (e) {
        say(`FX DLL: ${String(e)}`, 'error');
      }
    }
    if (now === true && before !== true && !sessionStarted) {
      sessionStarted = true;
      await autostart();
    }
  }

  /** The plugins marked to start with the game, once the DLL answers. */
  async function autostart(): Promise<void> {
    const p = plugins;
    const started: string[] = [];
    try {
      for (const id of ['bars', 'obs', 'boardcam'] as const) {
        if (!p[id].autostart) continue;
        const what = await startPlugin(id, p);
        if (what) started.push(what);
      }
    } catch (e) {
      say(`Starting the plugins: ${String(e)}`, 'error');
    }
    if (started.length > 0) say(`Started ${started.join(', ')}`);
  }

  async function play(): Promise<void> {
    if (playing || gameRunning) return;
    playing = true;
    try {
      const built = await api.build(false, () => {});
      if (built.outcome === 'built') say(`Built ${built.decks} deck(s) and ${built.wheels} wheel(s)`);
      const attach = await api.launch();
      sessionAttached = !!attach;
      say(attach ? 'Starting ReSkate; the FX DLL goes in once the game is up' : 'Starting ReSkate...');
      await readStatus();
    } catch (e) {
      say(String(e), 'error');
    } finally {
      playing = false;
    }
  }

  function keys(e: KeyboardEvent): void {
    if (e.key === 'Escape' && view !== 'home' && !showSettings && !showChangelogs) view = 'home';
  }

  onMount(() => {
    (async () => {
      config = await api.loadConfig();
      version = await api.appVersion();
      await refresh();
      await readStatus();
      const params = new URLSearchParams(window.location.search);
      const want = params.get('page');
      if (want === 'plugins' || want === 'studio' || want === 'console') view = want;
      await poll();
    })();
    const timer = setInterval(poll, 4000);
    return () => clearInterval(timer);
  });
</script>

<svelte:window onkeydown={keys} />

<div class="app">
  <div class="bg" aria-hidden="true"></div>
  {#if view === 'home'}
    <Home {status} {gameRunning} {injected} {version} {plugins} {playing} onopen={(v) => (view = v)} onplay={play} onsettings={() => (showSettings = true)} onchangelogs={() => (showChangelogs = true)} />
    {#if api.isMock}<span class="mock">mock data</span>{/if}
  {:else}
    <div class="page">
      <header class="page-head">
        <button class="back" onclick={() => (view = 'home')} title="Back to the front page (Escape)">&larr; Back</button>
        <h1>{view === 'plugins' ? 'Plugins' : view === 'studio' ? 'Studio' : 'Console'}</h1>
        <span class="state" class:running={gameRunning}>{gameRunning ? (injected ? 'Game running, FX DLL in' : 'Game running') : 'Game closed'}</span>
        <div class="spacer"></div>
        <button class="ghost" onclick={() => (showSettings = true)}>Settings</button>
      </header>
      <div class="page-body">
        {#if view === 'plugins' && config}
          <Plugins {config} {plugins} {library} {gameRunning} {injected} onplugins={savePlugins} onconfig={saveConfig} onsay={say} onattach={() => api.attachDll().then(() => say('Putting the FX DLL in'), (e) => say(String(e), 'error'))} />
        {:else if view === 'studio' && config}
          <Studio {config} {library} {gameRunning} onrefresh={refresh} onsay={say} onbuilt={readStatus} />
        {:else if view === 'console'}
          <Console onsay={say} />
        {/if}
      </div>
    </div>
  {/if}

  {#if toast}
    <div class="toast" class:error={toast.kind === 'error'} role="status">{toast.text}</div>
  {/if}
  {#if showSettings && config}
    <SettingsDialog {config} onsave={saveConfig} onclose={() => (showSettings = false)} />
  {/if}
  {#if showChangelogs}
    <dialog class="changelogs" open>
      <header>
        <h2>Changelogs</h2>
        <button class="ghost" onclick={() => (showChangelogs = false)}>Close</button>
      </header>
      <div class="entries">
        {#each ENTRIES as e (e.version)}
          <section>
            <h3>{e.version}{e.date ? ` (${e.date})` : ''}</h3>
            <ul>
              {#each e.bullets as b, i (i)}<li>{b}</li>{/each}
            </ul>
          </section>
        {/each}
      </div>
    </dialog>
  {/if}
</div>

<style>
  .app {
    position: relative;
    height: 100%;
    overflow: hidden;
  }
  /* The background: public/background.jpg when there is one (a 12:7 picture, 2880 x 1680 fits every size this
     window takes), over a dark gradient that shows until then. */
  .bg {
    position: absolute;
    inset: 0;
    background:
      url('/background.jpg') center / cover no-repeat,
      radial-gradient(1200px 700px at 70% 30%, #3a2a3f 0%, #1a1520 55%, #0d0b10 100%);
  }
  .bg::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(90deg, rgba(8, 7, 10, 0.55) 0%, rgba(8, 7, 10, 0.15) 45%, rgba(8, 7, 10, 0.35) 100%);
  }
  .mock {
    position: absolute;
    top: 10px;
    right: 14px;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(243, 198, 75, 0.2);
    color: var(--warn);
  }
  .page {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: 52px minmax(0, 1fr);
    background: rgba(18, 16, 22, 0.97);
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .back {
    font-weight: 600;
  }
  .page-head h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 800;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .state {
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 999px;
    background: rgba(79, 209, 139, 0.12);
    color: var(--good);
  }
  .state.running {
    background: rgba(43, 167, 255, 0.14);
    color: #7cc8ff;
  }
  .spacer {
    flex: 1;
  }
  .page-body {
    min-height: 0;
  }
  .toast {
    position: fixed;
    bottom: 56px;
    left: 50%;
    transform: translateX(-50%);
    max-width: min(640px, calc(100vw - 32px));
    padding: 10px 16px;
    border-radius: var(--radius);
    background: var(--raised);
    border: 1px solid var(--line-strong);
    box-shadow: var(--shadow);
    z-index: 5;
  }
  .toast.error {
    border-color: rgba(255, 107, 107, 0.6);
    color: #ffd2d2;
  }
  .changelogs {
    position: fixed;
    inset: auto;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(720px, calc(100vw - 48px));
    max-height: calc(100vh - 80px);
    display: flex;
    flex-direction: column;
    padding: 0;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--text);
    box-shadow: var(--shadow);
    z-index: 6;
  }
  .changelogs header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 18px;
    border-bottom: 1px solid var(--line);
  }
  .changelogs h2 {
    margin: 0;
    font-size: 18px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .entries {
    overflow-y: auto;
    padding: 6px 18px 18px;
  }
  .entries h3 {
    margin: 14px 0 6px;
    font-size: 14px;
  }
  .entries ul {
    margin: 0;
    padding-left: 20px;
    color: var(--text-dim);
  }
</style>
