<script lang="ts">
  // The Plugins page: the features that need the FX DLL in the running game (the OBS link, the soundbars) and the
  // widgets, the see-through windows OBS captures one by one (the board cam, the speed meter; lib/widgets.ts is the
  // registry), one card each with a switch to start it with the game, and the DLL's own settings (put in at launch,
  // the staging ring). A plugin card's settings are the Live panel's sections for it (Live.svelte, `show`); a widget
  // card's are its own pane (Widgets.svelte).
  import type { Config, Library, LibraryItem, PluginsConfig } from '../lib/types';
  import { WIDGETS, widgetConfig } from '../lib/widgets';
  import Library_ from './Library.svelte';
  import Live from './Live.svelte';
  import Widgets from './Widgets.svelte';

  interface Props {
    config: Config;
    plugins: PluginsConfig;
    library: Library | null;
    gameRunning: boolean;
    injected: boolean | null;
    /** Which widget windows are open, by kind. */
    openWidgets: Record<string, boolean>;
    onplugins: (p: PluginsConfig) => void;
    onconfig: (c: Config) => void;
    onsay: (text: string, kind?: 'info' | 'error') => void;
    onattach: () => void;
  }
  let { config, plugins, library, gameRunning, injected, openWidgets, onplugins, onconfig, onsay, onattach }: Props = $props();

  type Which = 'obs' | 'bars';
  const CARDS: { id: Which; title: string; what: string }[] = [
    { id: 'obs', title: 'OBS link', what: "OBS's virtual camera, a video or any picture on the ridden board, live" },
    { id: 'bars', title: 'Soundbars', what: "Equaliser bars on the deck, moving to the PC's sound" },
  ];
  /** A plugin's id, or `widget:<kind>`. */
  let selected = $state<string>('obs');
  const selectedWidget = $derived(selected.startsWith('widget:') ? selected.slice('widget:'.length) : null);
  let deck = $state<LibraryItem | null>(null);
  let libraryTab = $state<'deck' | 'wheel'>('deck');
  let boardArt = $state<string | null>(null);

  const RINGS = [
    { mb: 0, label: 'off' },
    { mb: 4096, label: '4 GB' },
    { mb: 8192, label: '8 GB' },
    { mb: 12288, label: '12 GB' },
    { mb: 16384, label: '16 GB' },
  ];

  function autostart(id: Which, on: boolean): void {
    onplugins({ ...plugins, [id]: { ...plugins[id], autostart: on } });
  }
  function widgetAutostart(kind: string, on: boolean): void {
    onplugins({ ...plugins, widgets: { ...plugins.widgets, [kind]: { ...widgetConfig(plugins, kind), autostart: on } } });
  }
</script>

<div class="plugins" class:widget={selectedWidget !== null}>
  <aside class="cards">
    <section class="dll">
      <h3>FX DLL</h3>
      <p class="state">
        <span class="dot" class:on={injected === true} class:half={gameRunning && !injected}></span>
        {injected ? 'In the game' : gameRunning ? 'Game running, the DLL is not in it' : 'The game is not running'}
      </p>
      <label class="check">
        <input type="checkbox" checked={config.attach} onchange={(e) => onconfig({ ...config, attach: e.currentTarget.checked })} />
        <span>Put it in when the game starts</span>
      </label>
      <label class="ring">
        <span>Staging ring</span>
        <select value={config.ringMb} onchange={(e) => onconfig({ ...config, ringMb: Number(e.currentTarget.value) })}>
          {#each RINGS as r (r.mb)}<option value={r.mb}>{r.label}</option>{/each}
        </select>
      </label>
      <p class="faint small">The ring stands in for the game's staging buffers at a skater build (it holds its size in memory the whole game). The two plugins below need the DLL; with the switch on, Caveman puts it in whichever way the game was started while Caveman is open. Click a card for its controls; its checkbox switches it on, now and at every game start.</p>
      {#if gameRunning && !injected}
        <button class="small" onclick={onattach}>Put it in now</button>
      {/if}
    </section>
    {#each CARDS as c (c.id)}
      <div class="card" class:on={selected === c.id}>
        <button class="pick" onclick={() => (selected = c.id)}>
          <span class="title">{c.title}</span>
          <span class="what">{c.what}</span>
        </button>
        <input class="auto" type="checkbox" checked={plugins[c.id].autostart} onchange={(e) => autostart(c.id, e.currentTarget.checked)} title="On: starts with the game (and now, when the FX DLL is in); off: stops it" aria-label={`${c.title} on`} />
      </div>
    {/each}
    <h3 class="group" title="See-through windows over the game, read from outside it (no DLL needed); OBS captures each as a window">Widgets</h3>
    {#each WIDGETS as w (w.kind)}
      <div class="card" class:on={selected === `widget:${w.kind}`}>
        <button class="pick" onclick={() => (selected = `widget:${w.kind}`)}>
          <span class="title">{w.name} <span class="open" class:on={openWidgets[w.kind] === true} title={openWidgets[w.kind] ? 'The window is open' : 'The window is closed'}></span></span>
          <span class="what">{w.blurb}</span>
        </button>
        <input class="auto" type="checkbox" checked={widgetConfig(plugins, w.kind).autostart} onchange={(e) => widgetAutostart(w.kind, e.currentTarget.checked)} title="On: opens with the game (and now); off: closes it" aria-label={`${w.name} on`} />
      </div>
    {/each}
    <p class="faint small parked">Particle effects are parked for now: the effect test decks are out of the build until their tools are ready.</p>
  </aside>

  {#if selectedWidget !== null}
    <Widgets kind={selectedWidget} {plugins} open={openWidgets[selectedWidget] === true} {onplugins} {onsay} />
  {:else}
    {#if selected === 'obs'}
      <Library_ {library} selected={deck?.source ?? null} onselect={(i) => (deck = i)} bind:tab={libraryTab} live={true} {boardArt} />
    {/if}
    <Live show={selected as Which} {deck} {plugins} {onplugins} {onsay} {library} onboardart={(a) => (boardArt = a)} />
  {/if}
</div>

<style>
  .plugins {
    display: grid;
    grid-template-columns: 300px auto minmax(360px, 1fr);
    height: 100%;
    min-height: 0;
  }
  .plugins.widget {
    grid-template-columns: 300px minmax(360px, 1fr);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
    border-right: 1px solid var(--line);
    background: var(--panel);
    overflow-y: auto;
  }
  .dll {
    padding: 10px 12px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  h3 {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  h3.group {
    margin: 8px 0 0;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .dot.on {
    background: var(--good);
  }
  .dot.half {
    background: var(--warn);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
    cursor: pointer;
  }
  .ring {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 8px 0;
    font-size: 13px;
  }
  .small {
    font-size: 12px;
  }
  button.small {
    padding: 4px 9px;
  }
  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 2px;
    padding: 6px 8px 8px;
    background: var(--panel-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
  }
  .card.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .pick {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    padding: 6px 34px 6px 6px;
    text-align: left;
    background: transparent;
    border-color: transparent;
  }
  .title {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 17px;
    font-weight: 800;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }
  .open {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .open.on {
    background: var(--good);
    box-shadow: 0 0 6px var(--good);
  }
  .auto {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 18px;
    height: 18px;
    cursor: pointer;
  }
  .what {
    font-size: 12px;
    color: var(--text-dim);
  }
  .parked {
    margin-top: auto;
  }
</style>
