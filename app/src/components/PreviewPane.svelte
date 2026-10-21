<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { Preview, type Mode } from '../lib/preview/scene';
  import type { Settings } from '../lib/types';

  interface Props {
    art: ImageBitmap | null;
    settings: Settings | null;
    wheelColour: string;
    wheelFace: ImageBitmap | null;
    loading: boolean;
    title: string;
    onautocolour: (c: string) => void;
    /** The mode to start in (the buttons change it after). */
    mode?: Mode;
  }
  let { art, settings, wheelColour, wheelFace, loading, title, onautocolour, mode: startMode = 'showcase' }: Props = $props();

  let host: HTMLDivElement;
  let preview: Preview | null = null;
  let mode = $state<Mode>('showcase');
  let timer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    preview = new Preview(host);
    (window as unknown as { cavemanPreview: Preview }).cavemanPreview = preview;
  });
  onDestroy(() => preview?.dispose());

  // Rebuild the deck's textures when the art or its settings change (a little after a slider stops moving).
  $effect(() => {
    const a = art;
    const s = settings;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      if (!preview) return;
      preview.setDeck(a, s);
      onautocolour(preview.resolvedAuraColour());
    }, 90);
  });

  $effect(() => {
    preview?.setWheel(wheelColour, wheelFace);
  });

  // only the prop is tracked: the buttons change the mode after
  $effect(() => {
    const m = startMode;
    untrack(() => {
      if (m !== mode) setMode(m);
    });
  });

  function setMode(m: Mode): void {
    mode = m;
    preview?.setMode(m);
  }
</script>

<div class="pane">
  <div class="viewport" bind:this={host}></div>
  <div class="overlay top">
    <div class="title">{title}</div>
    <div class="modes" role="group" aria-label="Preview mode">
      <button class:on={mode === 'showcase'} onclick={() => setMode('showcase')}>Showcase</button>
      <button class:on={mode === 'ride'} onclick={() => setMode('ride')}>Ride</button>
      <button class="ghost" title="Reset the view" onclick={() => preview?.resetView()}>Reset view</button>
    </div>
  </div>
  {#if loading}
    <div class="loading">Fitting the art...</div>
  {/if}
  <div class="overlay bottom">
    <span class="note">Approximate preview: the game's shaders and lighting differ. Drag to orbit, scroll to zoom.</span>
  </div>
</div>

<style>
  .pane {
    position: relative;
    min-width: 0;
    min-height: 0;
    background: var(--bg);
  }
  .viewport {
    position: absolute;
    inset: 0;
  }
  .viewport :global(canvas) {
    display: block;
    width: 100%;
    height: 100%;
  }
  .overlay {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    pointer-events: none;
  }
  .overlay.top {
    top: 0;
  }
  .overlay.bottom {
    bottom: 0;
    justify-content: center;
    font-size: 12px;
  }
  .note {
    padding: 4px 12px;
    border-radius: 999px;
    background: rgba(18, 16, 22, 0.72);
    color: var(--text-faint);
  }
  .title {
    font-weight: 600;
    font-size: 15px;
    text-shadow: 0 1px 6px rgba(0, 0, 0, 0.6);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .modes {
    display: flex;
    gap: 4px;
    pointer-events: auto;
    background: rgba(18, 16, 22, 0.75);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 4px;
  }
  .modes button {
    padding: 5px 12px;
    font-size: 13px;
    background: transparent;
    border-color: transparent;
  }
  .modes button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .loading {
    position: absolute;
    top: 56px;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 12px;
    border-radius: 999px;
    background: var(--raised);
    color: var(--text-dim);
    font-size: 12px;
  }
</style>
