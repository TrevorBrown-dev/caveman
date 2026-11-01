<script lang="ts">
  // A widget window's page (`?view=widget&kind=<kind>`): the one component of its kind filling a see-through window,
  // and the frame around it that every widget shares: the strip at the top that moves the window, the right-click
  // menu (the window's bounds shown while you place it, the widget's own items, Close), Escape, a hint for the first
  // seconds, and a line in the corner while the widget has nothing to show. The options come in the query and change
  // live when the app's settings pane sends new ones (`widget-options-set`).
  import { onMount } from 'svelte';
  import type { Component } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { isMock } from '../../lib/api';
  import { onOptionsSet, optionsFromQuery, widgetDef, type MenuItem, type Options, type WidgetProps } from '../../lib/widgets';

  const params = new URLSearchParams(window.location.search);
  // `?view=boardcam` (the page's name before the widgets) still opens the board cam
  const kind = params.get('kind') ?? (params.get('view') === 'boardcam' ? 'boardcam' : '');
  const def = widgetDef(kind);
  let options = $state<Options>(def ? optionsFromQuery(params, def.defaults) : {});
  let Widget = $state<Component<WidgetProps> | null>(null);
  let status = $state<string | null>(def ? null : `no widget "${kind}"`);
  let items = $state<MenuItem[]>([]);
  let hint = $state(true);
  let menu = $state<{ x: number; y: number } | null>(null);
  /** The window's bounds shown (a frame and a tint), for placing it on the screen; off for the capture. */
  let bounds = $state(false);

  function onContextMenu(e: MouseEvent): void {
    e.preventDefault();
    hint = false;
    const h = 40 + 32 * (items.length + 2);
    menu = { x: Math.max(0, Math.min(e.clientX, window.innerWidth - 200)), y: Math.max(0, Math.min(e.clientY, window.innerHeight - h)) };
  }
  function closeWindow(): void {
    if (isMock) window.close();
    else void getCurrentWindow().close();
  }
  function onKey(event: KeyboardEvent): void {
    if (event.key !== 'Escape') return;
    if (menu) {
      menu = null;
      return;
    }
    closeWindow();
  }
  function onPointerDown(): void {
    menu = null;
    hint = false;
  }

  onMount(() => {
    const hintTimer = setTimeout(() => (hint = false), 6000);
    let alive = true;
    if (def) {
      def
        .component()
        .then((m) => {
          if (alive) Widget = m.default;
        })
        .catch((e) => (status = String(e)));
    }
    const off = onOptionsSet(kind, (o) => {
      options = { ...options, ...o };
    });
    return () => {
      alive = false;
      clearTimeout(hintTimer);
      void off.then((u) => u());
    };
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="widget" role="presentation" oncontextmenu={onContextMenu} onpointerdowncapture={onPointerDown}>
  {#if Widget}
    <Widget {options} onstatus={(s) => (status = s)} onmenu={(m) => (items = m)} />
  {/if}
</div>
<!-- the strip at the top moves the window (Tauri's drag region must be the element under the pointer) -->
<div class="grip" data-tauri-drag-region title="Drag here to move the window"></div>
{#if bounds}
  <div class="bounds"></div>
{/if}
{#if menu}
  <div class="menu" role="menu" style="left: {menu.x}px; top: {menu.y}px">
    <button type="button" role="menuitem" onclick={() => { bounds = !bounds; menu = null; }}>{bounds ? 'Hide window bounds' : 'Show window bounds'}</button>
    {#each items as item (item.label)}
      <button type="button" role="menuitem" onclick={() => { item.run(); menu = null; }}>{item.label}</button>
    {/each}
    <button type="button" role="menuitem" onclick={closeWindow}>Close</button>
  </div>
{/if}
{#if status}
  <div class="note">{status}</div>
{:else if hint && def}
  <div class="note">{def.hint}</div>
{/if}

<style>
  :global(html),
  :global(body),
  :global(#app) {
    background: transparent !important;
    margin: 0;
    overflow: hidden;
  }
  .widget {
    position: fixed;
    inset: 0;
  }
  .grip {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    height: 22px;
    cursor: move;
  }
  .grip:hover {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.18), rgba(255, 255, 255, 0));
  }
  .note {
    position: fixed;
    left: 10px;
    bottom: 8px;
    font: 12px system-ui, sans-serif;
    color: #fff;
    text-shadow: 0 1px 2px #000, 0 0 6px #000;
    opacity: 0.85;
    pointer-events: none;
  }
  .bounds {
    position: fixed;
    inset: 0;
    box-sizing: border-box;
    border: 2px dashed rgba(255, 255, 255, 0.85);
    background: rgba(0, 0, 0, 0.22);
    pointer-events: none;
  }
  .menu {
    position: fixed;
    z-index: 10;
    min-width: 190px;
    padding: 4px;
    background: rgba(24, 24, 28, 0.96);
    border: 1px solid rgba(255, 255, 255, 0.18);
    border-radius: 6px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
    font: 13px system-ui, sans-serif;
  }
  .menu button {
    display: block;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: #fff;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .menu button:hover {
    background: rgba(255, 255, 255, 0.12);
  }
</style>
