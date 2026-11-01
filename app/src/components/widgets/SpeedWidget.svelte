<script lang="ts">
  // The speed widget: the board's speed from the pose stream (lib/telemetry.svelte.ts), big, with the session's top
  // speed under it as a bar. It scales with its window: the digits take the window's height.
  import { onMount } from 'svelte';
  import { resetMax, speedIn, start, stop, telemetry, unitLabel, type Units } from '../../lib/telemetry.svelte';
  import type { WidgetProps } from '../../lib/widgets';

  let { options, onstatus, onmenu }: WidgetProps = $props();

  const units = $derived((['kmh', 'mph', 'ms'].includes(String(options.units)) ? options.units : 'kmh') as Units);
  const decimals = $derived(Math.max(0, Math.min(2, Number(options.decimals ?? 0) || 0)));
  const value = $derived(speedIn(telemetry.speed, units));
  const top = $derived(speedIn(telemetry.speedMax, units));
  const fraction = $derived(top > 0 ? Math.min(1, value / top) : 0);

  onMount(() => {
    start(Number(options.rate ?? 60));
    onmenu?.([{ label: 'Reset top speed', run: resetMax }]);
    return () => stop();
  });
  $effect(() => {
    onstatus(telemetry.live ? null : telemetry.status);
  });
</script>

<!-- nothing in this widget is clicked, so the whole panel moves the window (Tauri's drag region must be the element under
     the pointer: the panel's children let the pointer through) -->
<div class="speed" class:dim={options.dim !== false} data-tauri-drag-region>
  <div class="inner" data-tauri-drag-region>
    {#if options.label !== false}
      <div class="caption">Speed</div>
    {/if}
    <div class="reading">
      <span class="digits">{telemetry.live ? value.toFixed(decimals) : '--'}</span>
      <span class="unit">{unitLabel(units)}</span>
    </div>
    {#if options.max !== false}
      <div class="bar" aria-hidden="true"><div class="fill" style="width: {fraction * 100}%"></div></div>
      <div class="top">top {telemetry.speedMax > 0 ? top.toFixed(decimals) : '--'} {unitLabel(units)}</div>
    {/if}
  </div>
</div>

<style>
  .speed {
    position: fixed;
    inset: 0;
    container-type: size;
    font-family: 'Segoe UI Variable Display', 'Segoe UI', system-ui, sans-serif;
    color: #fff;
  }
  .inner {
    position: absolute;
    inset: 6cqh 4cqw;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2cqh;
    padding: 4cqh 6cqw;
    border-radius: 14px;
    overflow: hidden;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8), 0 0 8px rgba(0, 0, 0, 0.6);
  }
  /* the rows keep their size; a window too small clips rather than squeezing the bar to nothing */
  .caption,
  .reading,
  .bar,
  .top {
    flex: none;
    pointer-events: none;
  }
  .speed,
  .inner {
    cursor: move;
  }
  .dim .inner {
    background: rgba(10, 8, 12, 0.58);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
  }
  .caption {
    font-size: min(12cqh, 7cqw);
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: rgba(255, 255, 255, 0.7);
  }
  .reading {
    display: flex;
    align-items: baseline;
    gap: 0.25em;
    line-height: 1;
    white-space: nowrap;
  }
  .digits {
    font-size: min(40cqh, 24cqw);
    font-weight: 800;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.02em;
  }
  .unit {
    font-size: min(14cqh, 8cqw);
    font-weight: 600;
    color: rgba(255, 255, 255, 0.75);
  }
  .bar {
    height: max(3px, 2.5cqh);
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.16);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: #ff8a3d;
    transition: width 0.12s linear;
  }
  .top {
    font-size: min(10cqh, 6cqw);
    font-variant-numeric: tabular-nums;
    color: rgba(255, 255, 255, 0.65);
  }
</style>
