<script lang="ts">
  // A widget's pane on the Plugins page: open and close its window, reset its place, and its own settings (the kind's
  // settings component, lib/widgets.ts). A setting changed here is saved and sent to the open window at once; the
  // window's own changes (a view dragged) come back through the app (App.svelte) into the same settings.
  import { onMount } from 'svelte';
  import type { Component } from 'svelte';
  import type { PluginsConfig } from '../lib/types';
  import * as widgets from '../lib/widgets';
  import type { Options, SettingsProps } from '../lib/widgets';

  interface Props {
    kind: string;
    plugins: PluginsConfig;
    open: boolean;
    onplugins: (p: PluginsConfig) => void;
    onsay: (text: string, kind?: 'info' | 'error') => void;
  }
  let { kind, plugins, open, onplugins, onsay }: Props = $props();

  const def = $derived(widgets.widgetDef(kind));
  const config = $derived(widgets.widgetConfig(plugins, kind));
  let Settings = $state<Component<SettingsProps> | null>(null);
  let busy = $state(false);

  $effect(() => {
    const d = def;
    Settings = null;
    if (!d) return;
    let alive = true;
    d.settings()
      .then((m) => {
        if (alive) Settings = m.default;
      })
      .catch((e) => onsay(String(e), 'error'));
    return () => {
      alive = false;
    };
  });

  function change(patch: Options): void {
    const options = { ...config.options, ...patch };
    onplugins({ ...plugins, widgets: { ...plugins.widgets, [kind]: { ...config, options } } });
    if (open) void widgets.setOptions(kind, options).catch(() => {});
  }

  async function openWindow(): Promise<void> {
    if (!def) return;
    busy = true;
    try {
      await widgets.open(kind, config.options);
      onsay(`${def.name} window open (OBS: window capture "${def.title}")`);
    } catch (e) {
      onsay(`${def.name}: ${String(e)}`, 'error');
    } finally {
      busy = false;
    }
  }

  async function closeWindow(): Promise<void> {
    try {
      await widgets.close(kind);
    } catch (e) {
      onsay(String(e), 'error');
    }
  }

  function resetPlace(): void {
    onplugins({ ...plugins, widgets: { ...plugins.widgets, [kind]: { ...config, window: null } } });
    onsay(open ? 'The window goes back to its first place at the next Open' : 'The window opens at its first place next time');
  }

  onMount(() => {
    // nothing to read: the open state comes from the app (widget-state events)
  });
</script>

{#if def}
  <aside class="pane">
    <header>
      <div class="title">
        <h2>{def.name}</h2>
      </div>
      <p class="faint">{def.blurb}. OBS captures it as the window "{def.title}".</p>
    </header>
    <section>
      <div class="row">
        <span class="dot" class:on={open}></span>
        <span>{open ? 'Open' : 'Closed'}</span>
        <div class="grow"></div>
        <button class="small" class:on={open} disabled={busy} onclick={openWindow}>{open ? 'Reopen' : 'Open'}</button>
        <button class="small" disabled={!open} onclick={closeWindow}>Close</button>
        <button class="small ghost" onclick={resetPlace} title="Forget where the window was; it opens near the top right corner again">Reset place</button>
      </div>
      <p class="hint">The window remembers where you put it and how big you made it. Its checkbox on the card opens it with the game.</p>
    </section>
    <section>
      <h3>Settings</h3>
      {#if Settings}
        <Settings options={config.options} onchange={change} />
      {/if}
    </section>
  </aside>
{/if}

<style>
  .pane {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px 18px;
    overflow-y: auto;
  }
  header .title {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 20px;
  }
  .faint {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  section {
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  h3 {
    margin: 0 0 10px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .grow {
    flex: 1;
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
  .small {
    padding: 4px 10px;
    font-size: 12px;
  }
  .small.on {
    border-color: var(--accent);
  }
  .ghost {
    background: transparent;
    color: var(--text-dim);
  }
  .hint {
    margin: 10px 0 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-faint);
  }
</style>
