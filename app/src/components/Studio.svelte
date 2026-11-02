<script lang="ts">
  // The Studio: where decks and wheels are made and managed. The library (with a switch per item for what goes into
  // the game), the editor, the 3D preview and the build, as before; buttons to open the art folders and to get the
  // templates; and the Packs tab, the mod generator for creators (Packs.svelte).
  import Library from './Library.svelte';
  import Editor from './Editor.svelte';
  import PreviewPane from './PreviewPane.svelte';
  import BuildLog from './BuildLog.svelte';
  import Packs from './Packs.svelte';
  import * as api from '../lib/api';
  import { sameModel, toModel, toSettings, type EditModel } from '../lib/effects';
  import type { BuildEvent, BuildResult, Config, Library as Lib, LibraryItem } from '../lib/types';

  interface Props {
    config: Config;
    library: Lib | null;
    gameRunning: boolean;
    onrefresh: () => Promise<void>;
    onsay: (text: string, kind?: 'info' | 'error') => void;
    /** A build ran (the front page's status reads again). */
    onbuilt?: () => void;
  }
  let { config, library, gameRunning, onrefresh, onsay, onbuilt }: Props = $props();

  let tab = $state<'library' | 'packs'>('library');
  let selected = $state<LibraryItem | null>(null);
  let deck = $state<LibraryItem | null>(null);
  let wheel = $state<LibraryItem | null>(null);
  let libraryTab = $state<'deck' | 'wheel'>('deck');
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
  let templates = $state<string | null>(null);

  const model: EditModel | null = $derived(deck ? drafts[deck.source] ?? toModel(deck.settings) : null);
  const dirty = $derived(!!deck && !!drafts[deck.source] && !sameModel(drafts[deck.source], toModel(deck.settings)));
  const liveSettings = $derived(deck && model ? toSettings(model, deck.settings) : null);
  const unsaved = $derived(Object.keys(drafts).length);

  // keep the selection across refreshes (the entries are replaced)
  $effect(() => {
    const lib = library;
    const find = (item: LibraryItem | null): LibraryItem | null => (item && lib ? ([...lib.decks, ...lib.wheels].find((i) => i.source === item.source) ?? null) : null);
    selected = find(selected);
    deck = find(deck);
    wheel = find(wheel);
    if (!deck && lib && lib.decks.length > 0 && !selected) void select(lib.decks.find((d) => /\.png$/i.test(d.source)) ?? lib.decks[0]);
  });

  async function select(item: LibraryItem): Promise<void> {
    selected = item;
    if (item.kind === 'deck') {
      if (deck?.source === item.source) return;
      deck = item;
      if (!/\.png$/i.test(item.source)) {
        // an animated deck (a GIF or a video): the preview reads PNG only; the game shows its first frame as the art
        art = null;
        fitNotes = ['An animated deck: the preview shows the deck plain; in game its frames play (the first frame is the still).'];
        artLoading = false;
        return;
      }
      artLoading = true;
      try {
        const fitted = await api.fittedArt(item.source);
        if (deck?.source !== item.source) return;
        fitNotes = fitted.notes;
        art = await api.imageBitmap(fitted.path);
      } catch (e) {
        art = null;
        onsay(`Could not load ${item.display}: ${String(e)}`, 'error');
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
      await onrefresh();
      onsay(`Saved ${deck?.display ?? 'the deck'}'s settings`);
    } catch (e) {
      onsay(`Not saved: ${String(e)}`, 'error');
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
      await onrefresh();
      onsay('Removed the settings file: the file name decides again');
    } catch (e) {
      onsay(String(e), 'error');
    }
  }

  // one item's switch, or a folder's or the whole tab's at once (all of one kind: the library shows one tab)
  async function toggle(items: LibraryItem[], on: boolean): Promise<void> {
    const change = items.filter((i) => i.enabled !== on);
    if (change.length === 0) return;
    try {
      await api.setItemsOff(change[0].kind, change.map((i) => i.source), !on);
      await onrefresh();
      onbuilt?.();
      if (change.length > 1) onsay(`${change.length} ${change[0].kind}s ${on ? 'switched on' : 'switched off'}`);
    } catch (e) {
      onsay(String(e), 'error');
    }
  }

  async function runBuild(): Promise<void> {
    if (building) return;
    if (unsaved > 0) onsay(`${unsaved} deck(s) have unsaved changes: the build uses their saved settings`);
    building = true;
    buildEvents = [];
    buildError = null;
    buildResult = null;
    logOpen = true;
    try {
      buildResult = await api.build(false, (e) => (buildEvents = [...buildEvents, e]));
      await onrefresh();
      onbuilt?.();
      if (buildResult.outcome === 'built') onsay(`Built ${buildResult.decks} deck(s) in ${buildResult.seconds.toFixed(1)} s`);
      else if (buildResult.outcome === 'up_to_date') onsay('The mods are up to date');
    } catch (e) {
      buildError = String(e);
      onsay(buildError, 'error');
    } finally {
      building = false;
    }
  }

  async function getTemplates(): Promise<void> {
    try {
      templates = await api.templatesSave();
      await api.reveal(templates);
      onsay(`Templates written to ${templates}`);
    } catch (e) {
      onsay(String(e), 'error');
    }
  }

  function keys(e: KeyboardEvent): void {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      if (dirty) void save();
    }
  }
</script>

<svelte:window onkeydown={keys} />

<div class="studio">
  <div class="bar">
    <div class="tabs" role="tablist">
      <button role="tab" class:on={tab === 'library'} aria-selected={tab === 'library'} onclick={() => (tab = 'library')}>Library</button>
      <button role="tab" class:on={tab === 'packs'} aria-selected={tab === 'packs'} onclick={() => (tab = 'packs')} title="Build chosen decks into a Thunderstore-ready package">Packs</button>
    </div>
    <div class="folders">
      <button class="ghost" onclick={() => library && api.reveal(library.decksDir)} title={library?.decksDir ?? ''}>Deck folder</button>
      <button class="ghost" onclick={() => library && api.reveal(library.wheelsDir)} title={library?.wheelsDir ?? ''}>Wheel folder</button>
      <button class="ghost" onclick={getTemplates} title="The deck and wheel templates as PNGs, in the Caveman folder's Templates beside the game">Get the templates</button>
    </div>
    <div class="spacer"></div>
    <button class="ghost" onclick={onrefresh} title="Read the drop folders again">Refresh</button>
    <button class="primary" disabled={building || gameRunning} title={gameRunning ? 'Close the game to build' : 'Build what is switched on into the mods (Play does this too)'} onclick={runBuild}>
      {building ? 'Building...' : 'Build now'}
    </button>
  </div>

  {#if tab === 'library'}
    <main>
      <Library {library} selected={selected?.source ?? null} onselect={select} bind:tab={libraryTab} ontoggle={toggle} />
      <div class="centre">
        <PreviewPane
          {art}
          settings={liveSettings}
          wheelColour={wheel?.colour ?? '#f2efe8'}
          {wheelFace}
          loading={artLoading}
          title={deck ? `${deck.display}${dirty ? '  (unsaved)' : ''}` : 'No deck selected'}
          onautocolour={(c) => (autoColour = c)}
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
  {:else}
    <Packs {config} {library} {onsay} />
  {/if}
</div>

<style>
  .studio {
    display: grid;
    grid-template-rows: 48px minmax(0, 1fr);
    height: 100%;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 2px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  .tabs button {
    border: none;
    background: transparent;
    padding: 4px 14px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .tabs button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .folders {
    display: flex;
    gap: 2px;
    margin-left: 8px;
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
  @media (max-width: 1180px) {
    main {
      grid-template-columns: 270px minmax(0, 1fr) 320px;
    }
  }
</style>
