<script lang="ts">
  // The speed widget's settings pane in the app: units, decimals, what shows besides the number.
  import type { SettingsProps } from '../../lib/widgets';

  let { options, onchange }: SettingsProps = $props();
</script>

<div class="settings">
  <div class="field">
    <span class="label">Units</span>
    <div class="segmented">
      <button class:on={options.units === 'kmh' || !options.units} onclick={() => onchange({ units: 'kmh' })}>km/h</button>
      <button class:on={options.units === 'mph'} onclick={() => onchange({ units: 'mph' })}>mph</button>
      <button class:on={options.units === 'ms'} onclick={() => onchange({ units: 'ms' })}>m/s</button>
    </div>
  </div>
  <div class="field">
    <span class="label">Decimals</span>
    <div class="segmented">
      {#each [0, 1, 2] as d (d)}
        <button class:on={Number(options.decimals ?? 0) === d} onclick={() => onchange({ decimals: d })}>{d}</button>
      {/each}
    </div>
  </div>
  <div class="switches">
    <label class="check"><input type="checkbox" checked={options.label !== false} onchange={(e) => onchange({ label: e.currentTarget.checked })} /><span>Caption</span></label>
    <label class="check"><input type="checkbox" checked={options.max !== false} onchange={(e) => onchange({ max: e.currentTarget.checked })} /><span>Top speed and bar</span></label>
    <label class="check" title="A dark panel behind the number; off for the bare number over the game"><input type="checkbox" checked={options.dim !== false} onchange={(e) => onchange({ dim: e.currentTarget.checked })} /><span>Panel</span></label>
  </div>
  <p class="hint">The speed is the board's own, from its position read 60 times a second outside the game, smoothed over a tenth of a second. The number scales with the window: resize it by its edges.</p>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .label {
    width: 72px;
    font-size: 13px;
    color: var(--text-dim);
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
