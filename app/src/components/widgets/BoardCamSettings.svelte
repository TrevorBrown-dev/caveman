<script lang="ts">
  // The board cam's settings pane in the app: the viewpoint (which a drag in the window also changes; the sliders
  // follow), how the board turns, mirror and nose. The window's size is its own: resize it by its edges.
  import type { SettingsProps } from '../../lib/widgets';

  let { options, onchange }: SettingsProps = $props();
  const elevation = $derived(Number(options.elevation ?? -30));
  const azimuth = $derived(Number(options.azimuth ?? 20));
  const zoom = $derived(Number(options.zoom ?? 1));
</script>

<div class="settings">
  <div class="slider">
    <span class="label">From</span>
    <input type="range" min="-89" max="89" step="1" value={elevation} onchange={(e) => onchange({ elevation: Number(e.currentTarget.value) })} />
    <span class="value mono" title="Below the board shows the art, above the grip">{elevation < 0 ? `${-elevation}° below` : `${elevation}° above`}</span>
  </div>
  <div class="slider">
    <span class="label">Around</span>
    <input type="range" min="-180" max="180" step="1" value={azimuth} onchange={(e) => onchange({ azimuth: Number(e.currentTarget.value) })} />
    <span class="value mono">{azimuth}°</span>
  </div>
  <div class="slider">
    <span class="label">Zoom</span>
    <input type="range" min="0.5" max="2" step="0.05" value={zoom} onchange={(e) => onchange({ zoom: Number(e.currentTarget.value) })} />
    <span class="value mono" title="1: the whole board fits whichever way it turns">{zoom.toFixed(2)}x</span>
  </div>
  <div class="field">
    <span class="label">Turns with</span>
    <div class="segmented">
      <button class:on={options.turn !== 'world'} onclick={() => onchange({ turn: 'camera' })} title="A flip looks as on screen">the camera</button>
      <button class:on={options.turn === 'world'} onclick={() => onchange({ turn: 'world' })}>the world</button>
    </div>
  </div>
  <div class="switches">
    <label class="check" title="Left and right swapped"><input type="checkbox" checked={options.mirror === true} onchange={(e) => onchange({ mirror: e.currentTarget.checked })} /><span>Mirror</span></label>
    <label class="check" title="The nose the other way"><input type="checkbox" checked={options.nose === true} onchange={(e) => onchange({ nose: e.currentTarget.checked })} /><span>Nose</span></label>
  </div>
  <p class="hint">The window shows the picture on the board in the game by itself. In the window: drag to turn the view (the sliders follow), scroll to zoom, move it by the strip at its top, resize it by its edges.</p>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .slider,
  .field {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .slider input {
    flex: 1;
    accent-color: var(--accent);
  }
  .label {
    width: 72px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .value {
    min-width: 86px;
    font-size: 12px;
    text-align: right;
    color: var(--text-dim);
  }
  .mono {
    font-family: Consolas, 'Cascadia Mono', ui-monospace, monospace;
  }
  .segmented {
    display: inline-flex;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .segmented button {
    padding: 5px 11px;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .segmented button + button {
    border-left: 1px solid var(--line-strong);
  }
  .segmented button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .switches {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 18px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 13px;
    cursor: pointer;
  }
  .check input {
    accent-color: var(--accent);
  }
  .hint {
    margin: 4px 0 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-faint);
  }
</style>
