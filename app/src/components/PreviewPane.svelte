<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { Preview, type Mode } from '../lib/preview/scene';
  import { readBytes, studioBoardModel } from '../lib/api';
  import type { Settings } from '../lib/types';

  interface Props {
    art: ImageBitmap | null;
    settings: Settings | null;
    wheelColour: string;
    wheelFace: ImageBitmap | null;
    /** The wheel being looked at (its PNG), put on the game's board; none: the natural wheels. */
    wheelSource?: string | null;
    loading: boolean;
    title: string;
    onautocolour: (c: string) => void;
    /** The mode to start in (the buttons change it after). */
    mode?: Mode;
  }
  let { art, settings, wheelColour, wheelFace, wheelSource = null, loading, title, onautocolour, mode: startMode = 'showcase' }: Props = $props();

  let host: HTMLDivElement;
  let preview: Preview | null = null;
  let mounted = $state(false);
  let mode = $state<Mode>('showcase');
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** The game's board: what is being fetched, and whether it is shown (else the drawn board, and why). */
  let modelNote = $state<string | null>(null);
  let gameBoard = $state(false);
  let modelProblem = $state<string | null>(null);
  let modelSeq = 0;

  onMount(() => {
    preview = new Preview(host);
    (window as unknown as { cavemanPreview: Preview }).cavemanPreview = preview;
    mounted = true;
  });
  onDestroy(() => {
    modelSeq++;
    preview?.dispose();
  });

  // The game's board, with the wheel being looked at: built by the back end (seconds the first time, cached after);
  // the board shown stays until the new one is in, and only the newest request counts.
  $effect(() => {
    const source = wheelSource;
    const colour = wheelColour;
    if (mounted) void loadBoard(source, colour);
  });

  async function loadBoard(source: string | null, colour: string): Promise<void> {
    const seq = ++modelSeq;
    const wait = setTimeout(() => {
      if (seq === modelSeq) modelNote = source ? 'Putting the wheel on the board...' : 'Building the board from the game...';
    }, 400);
    preview?.expectModel(true);
    try {
      const path = await studioBoardModel(source, source ? colour : null);
      if (seq !== modelSeq) return;
      const bytes = await readBytes(path);
      if (seq !== modelSeq || !preview) return;
      await preview.setGameModel(bytes);
      if (seq !== modelSeq) return;
      gameBoard = true;
      modelProblem = null;
    } catch (e) {
      if (seq === modelSeq) modelProblem = String(e);
    } finally {
      clearTimeout(wait);
      if (seq === modelSeq) {
        modelNote = null;
        preview?.expectModel(false);
      }
    }
  }

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
  {#if loading || modelNote}
    <div class="loading">{loading ? 'Fitting the art...' : modelNote}</div>
  {/if}
  <div class="overlay bottom">
    {#if gameBoard}
      <span class="note">The game's own board, grip, trucks and wheels; its shaders and lighting are approximated. Drag to orbit, scroll to zoom.</span>
    {:else}
      <span class="note" title={modelProblem ?? ''}>Approximate preview: a drawn board{modelProblem ? ` (the game's could not be read)` : ''}. Drag to orbit, scroll to zoom.</span>
    {/if}
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
