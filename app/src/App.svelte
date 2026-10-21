<script lang="ts">
  import { onMount } from 'svelte';
  import Library from './components/Library.svelte';
  import Editor from './components/Editor.svelte';
  import PreviewPane from './components/PreviewPane.svelte';
  import BuildLog from './components/BuildLog.svelte';
  import SettingsDialog from './components/SettingsDialog.svelte';
  import * as api from './lib/api';
  import { sameModel, toModel, toSettings, type EditModel } from './lib/effects';
  import type { BuildEvent, BuildResult, Config, Library as Lib, LibraryItem } from './lib/types';

  let config = $state<Config | null>(null);
  let library = $state<Lib | null>(null);
  let selected = $state<LibraryItem | null>(null);
  let deck = $state<LibraryItem | null>(null);
  let wheel = $state<LibraryItem | null>(null);
  let libraryTab = $state<'deck' | 'wheel'>('deck');
  /** Unsaved edits per deck (by PNG path), kept while moving between decks. */
  let drafts = $state<Record<string, EditModel>>({});
  let art = $state<ImageBitmap | null>(null);
  let artLoading = $state(false);
  let fitNotes = $state<string[]>([]);
  let wheelFace = $state<ImageBitmap | null>(null);
  let autoColour = $state('#ffffff');
  let saving = $state(false);
  let building = $state(false);
  let buildEvents = $state<BuildEvent[]>([]);
  let buildResult = $state<BuildResult | null>(null);
  let buildError = $state<string | null>(null);
  let logOpen = $state(false);
  let gameRunning = $state(false);
  let showSettings = $state(false);
  let initialMode = $state<'showcase' | 'ride'>('showcase');
  let toast: { text: string; kind: 'info' | 'error' } | null = $state(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const model: EditModel | null = $derived(deck ? drafts[deck.source] ?? toModel(deck.settings) : null);
  const dirty = $derived(!!deck && !!drafts[deck.source] && !sameModel(drafts[deck.source], toModel(deck.settings)));
  const liveSettings = $derived(deck && model ? toSettings(model, deck.settings) : null);
  const unsaved = $derived(Object.keys(drafts).length);

  function say(text: string, kind: 'info' | 'error' = 'info'): void {
    toast = { text, kind };
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), kind === 'error' ? 7000 : 3500);
  }

  async function refresh(): Promise<void> {
    try {
      library = await api.library();
      // keep the selection (and its updated entry) across refreshes
      const find = (item: LibraryItem | null): LibraryItem | null =>
        item ? ([...(library?.decks ?? []), ...(library?.wheels ?? [])].find((i) => i.source === item.source) ?? null) : null;
      selected = find(selected);
      deck = find(deck);
      wheel = find(wheel);
      if (!deck && library.decks.length > 0 && !selected) select(library.decks[0]);
    } catch (e) {
      say(`Could not read the drop folders: ${String(e)}`, 'error');
    }
  }

  async function select(item: LibraryItem): Promise<void> {
    selected = item;
    if (item.kind === 'deck') {
      if (deck?.source === item.source) return;
      deck = item;
      artLoading = true;
      try {
        const fitted = await api.fittedArt(item.source);
        if (deck?.source !== item.source) return;
        fitNotes = fitted.notes;
        art = await api.imageBitmap(fitted.path);
      } catch (e) {
        art = null;
        say(`Could not load ${item.display}: ${String(e)}`, 'error');
      } finally {
        artLoading = false;
      }
    } else {
      wheel = item;
      try {
        wheelFace = await api.imageBitmap(item.source);
      } catch {
        wheelFace = null;
      }
    }
  }

  function edit(m: EditModel): void {
    if (!deck) return;
    if (sameModel(m, toModel(deck.settings))) {
      const { [deck.source]: _, ...rest } = drafts;
      drafts = rest;
    } else {
      drafts = { ...drafts, [deck.source]: m };
    }
  }

  async function save(): Promise<void> {
    if (!deck || !model) return;
    saving = true;
    try {
      await api.saveSettings(deck.source, toSettings(model, deck.settings));
      const { [deck.source]: _, ...rest } = drafts;
      drafts = rest;
      await refresh();
      say(`Saved ${deck?.display ?? 'the deck'}'s settings`);
    } catch (e) {
      say(`Not saved: ${String(e)}`, 'error');
    } finally {
      saving = false;
    }
  }

  function revert(): void {
    if (!deck) return;
    const { [deck.source]: _, ...rest } = drafts;
    drafts = rest;
  }

  async function clear(): Promise<void> {
    if (!deck) return;
    try {
      await api.clearSettings(deck.source);
      const { [deck.source]: _, ...rest } = drafts;
      drafts = rest;
      await refresh();
      say('Removed the settings file: the file name decides again');
    } catch (e) {
      say(String(e), 'error');
    }
  }

  async function runBuild(): Promise<void> {
    if (building) return;
    if (unsaved > 0) say(`${unsaved} deck(s) have unsaved changes: the build uses their saved settings`);
    building = true;
    buildEvents = [];
    buildError = null;
    buildResult = null;
    logOpen = true;
    try {
      buildResult = await api.build(false, (e) => (buildEvents = [...buildEvents, e]));
      await refresh();
      if (buildResult.outcome === 'built') say(`Built ${buildResult.decks} deck(s) in ${buildResult.seconds.toFixed(1)} s`);
    } catch (e) {
      buildError = String(e);
      say(buildError, 'error');
    } finally {
      building = false;
    }
  }

  async function play(): Promise<void> {
    try {
      await api.launch();
      say('Starting ReSkate...');
    } catch (e) {
      say(String(e), 'error');
    }
  }

  async function saveConfig(c: Config): Promise<void> {
    try {
      config = await api.saveConfig(c);
      showSettings = false;
      deck = null;
      wheel = null;
      selected = null;
      art = null;
      await refresh();
    } catch (e) {
      say(String(e), 'error');
    }
  }

  async function poll(): Promise<void> {
    try {
      gameRunning = await api.gameRunning();
    } catch {
      gameRunning = false;
    }
  }

  function keys(e: KeyboardEvent): void {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      if (dirty) save();
    }
  }

  onMount(() => {
    (async () => {
      config = await api.loadConfig();
      await refresh();
      // ?deck=<part of a name> selects that deck, ?wheel=<part> that wheel, ?mode=ride starts riding (screenshots, links)
      const params = new URLSearchParams(window.location.search);
      const want = params.get('deck')?.toLowerCase();
      const match = want ? library?.decks.find((d) => d.display.toLowerCase().includes(want)) : undefined;
      if (match) {
        deck = null;
        await select(match);
      }
      const wantWheel = params.get('wheel')?.toLowerCase();
      const wheelMatch = wantWheel ? library?.wheels.find((w) => w.display.toLowerCase().includes(wantWheel)) : undefined;
      if (wheelMatch) {
        libraryTab = 'wheel';
        await select(wheelMatch);
      }
      if (params.get('mode') === 'ride') initialMode = 'ride';
      if (params.get('log') === '1') logOpen = true;
      await poll();
    })();
    const timer = setInterval(poll, 4000);
    return () => clearInterval(timer);
  });
</script>

<svelte:window onkeydown={keys} />

<div class="app">
  <header class="top">
    <div class="brand">
      <span class="logo" aria-hidden="true"></span>
      <span>Caveman</span>
      {#if api.isMock}<span class="mock">mock data</span>{/if}
    </div>
    <button class="ghost folder mono" title="Change folders" onclick={() => (showSettings = true)}>{config?.game ?? ''}</button>
    <span class="status" class:running={gameRunning}>{gameRunning ? 'Game running' : 'Game closed'}</span>
    <div class="spacer"></div>
    <button class="ghost" onclick={refresh} title="Read the drop folders again">Refresh</button>
    <button class="primary" disabled={building || gameRunning} title={gameRunning ? 'Close the game to build' : 'Build the decks and wheels into the mods'} onclick={runBuild}>
      {building ? 'Building...' : 'Build'}
    </button>
    <button disabled={gameRunning || building} onclick={play} title="Start ReSkate">Play</button>
    <button class="ghost" onclick={() => (showSettings = true)} aria-label="Settings" title="Settings">Settings</button>
  </header>

  <main>
    <Library {library} selected={selected?.source ?? null} onselect={select} bind:tab={libraryTab} />
    <div class="centre">
      <PreviewPane
        {art}
        settings={liveSettings}
        wheelColour={wheel?.colour ?? '#f2efe8'}
        {wheelFace}
        loading={artLoading}
        title={deck ? `${deck.display}${dirty ? '  (unsaved)' : ''}` : 'No deck selected'}
        onautocolour={(c) => (autoColour = c)}
        mode={initialMode}
      />
      <BuildLog events={buildEvents} result={buildResult} error={buildError} {building} open={logOpen} ontoggle={() => (logOpen = !logOpen)} />
    </div>
    <Editor
      item={selected?.kind === 'wheel' ? selected : deck}
      model={selected?.kind === 'wheel' ? null : model}
      {dirty}
      {saving}
      {autoColour}
      {fitNotes}
      onchange={edit}
      onsave={save}
      onrevert={revert}
      onclear={clear}
    />
  </main>

  {#if toast}
    <div class="toast" class:error={toast.kind === 'error'} role="status">{toast.text}</div>
  {/if}
  {#if showSettings && config}
    <SettingsDialog {config} onsave={saveConfig} onclose={() => (showSettings = false)} />
  {/if}
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: 54px 1fr;
    height: 100%;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    font-weight: 700;
    font-size: 16px;
    letter-spacing: 0.01em;
  }
  .logo {
    width: 14px;
    height: 34px;
    border-radius: 7px;
    background: linear-gradient(180deg, #ff3b30 0 16%, #ff9500 16% 33%, #ffd60a 33% 50%, #34c759 50% 66%, #0a84ff 66% 83%, #8c50ff 83%);
    transform: rotate(-18deg);
  }
  .mock {
    font-size: 11px;
    font-weight: 500;
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(243, 198, 75, 0.15);
    color: var(--warn);
  }
  .folder {
    max-width: 340px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status {
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 999px;
    background: rgba(79, 209, 139, 0.12);
    color: var(--good);
  }
  .status.running {
    background: rgba(255, 107, 107, 0.14);
    color: var(--bad);
  }
  .spacer {
    flex: 1;
  }
  main {
    display: grid;
    grid-template-columns: 320px minmax(0, 1fr) 360px;
    min-height: 0;
  }
  .centre {
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
    min-width: 0;
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
  }
  .toast.error {
    border-color: rgba(255, 107, 107, 0.6);
    color: #ffd2d2;
  }
  @media (max-width: 1180px) {
    main {
      grid-template-columns: 270px minmax(0, 1fr) 320px;
    }
  }
</style>
