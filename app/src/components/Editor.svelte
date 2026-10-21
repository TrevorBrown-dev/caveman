<script lang="ts">
  import type { LibraryItem } from '../lib/types';
  import { BOARD_EFFECTS, FINISHES, HOLO_MODES, glowFactor, needsShaders, type EditModel } from '../lib/effects';
  import { reveal } from '../lib/api';

  interface Props {
    item: LibraryItem | null;
    model: EditModel | null;
    dirty: boolean;
    saving: boolean;
    autoColour: string;
    fitNotes: string[];
    onchange: (m: EditModel) => void;
    onsave: () => void;
    onrevert: () => void;
    onclear: () => void;
  }
  let { item, model, dirty, saving, autoColour, fitNotes, onchange, onsave, onrevert, onclear }: Props = $props();

  function set<K extends keyof EditModel>(key: K, value: EditModel[K]): void {
    if (!model) return;
    onchange({ ...model, [key]: value });
  }

  const factor = $derived(model ? glowFactor(model.glow) : 0);
</script>

<aside class="editor">
  {#if !item}
    <div class="placeholder dim">
      <p>Pick a deck or a wheel from the library.</p>
    </div>
  {:else}
    <header>
      <h2 title={item.display}>{item.display}</h2>
      <p class="path mono faint" title={item.source}>{item.source}</p>
      <div class="row">
        <button class="ghost small" onclick={() => reveal(item.source)}>Show file</button>
        {#if item.sidecar}<span class="badge">settings file</span>{:else if item.kind === 'deck'}<span class="badge muted">suffixes</span>{/if}
      </div>
    </header>

    {#if item.kind === 'wheel'}
      <section>
        <h3>Wheel</h3>
        <p class="dim">
          Colour <span class="swatch" style:background-color={item.colour ?? '#ffffff'}></span>
          <span class="mono">{item.colour}</span>, from the file name (<span class="mono">Name.ff0000.png</span>).
        </p>
        <p class="faint">Wheels have no settings file yet: rename the PNG to change the colour.</p>
      </section>
    {:else if model}
      <div class="scroll">
        <section>
          <h3>Finish</h3>
          <div class="segmented wrap">
            {#each FINISHES as f (f.id)}
              <button class:on={model.finish === f.id} title={f.hint} onclick={() => set('finish', f.id)}>{f.label}</button>
            {/each}
          </div>
          {#if model.finish === 'holo'}
            <p class="label">Holo pattern</p>
            <div class="segmented wrap">
              {#each HOLO_MODES as h (h.id)}
                <button class:on={model.mode === h.id} onclick={() => set('mode', h.id)}>{h.label}</button>
              {/each}
            </div>
          {/if}
          {#if model.finish !== 'none'}
            <label class="check">
              <input type="checkbox" checked={model.spot} onchange={(e) => set('spot', e.currentTarget.checked)} />
              <span>Spot: only on the light parts of the art</span>
            </label>
          {/if}
        </section>

        <section>
          <h3>Raised print</h3>
          <label class="check">
            <input type="checkbox" checked={model.emboss} onchange={(e) => set('emboss', e.currentTarget.checked)} />
            <span>Emboss the light parts of the art</span>
          </label>
          {#if model.emboss}
            <div class="slider">
              <span class="label">Strength</span>
              <input type="range" min="0.5" max="20" step="0.5" value={model.embossStrength} oninput={(e) => set('embossStrength', +e.currentTarget.value)} />
              <span class="value mono">{model.embossStrength.toFixed(1)}</span>
            </div>
          {/if}
        </section>

        <section>
          <h3>Shader effects</h3>
          <label class="check">
            <input type="checkbox" checked={model.cycle} onchange={(e) => set('cycle', e.currentTarget.checked)} />
            <span>Colour cycle: the colours turn through the hues</span>
          </label>
          {#if model.cycle}
            <div class="slider">
              <span class="label">Speed</span>
              <input type="range" min="0.05" max="2" step="0.05" value={model.speed} oninput={(e) => set('speed', +e.currentTarget.value)} />
              <span class="value mono">{model.speed.toFixed(2)}/s</span>
            </div>
          {/if}
          <label class="check">
            <input type="checkbox" checked={model.glowOn} onchange={(e) => set('glowOn', e.currentTarget.checked)} />
            <span>Glow: the print lights up</span>
          </label>
          {#if model.glowOn}
            <div class="slider">
              <span class="label">Strength</span>
              <input type="range" min="0.05" max="1" step="0.01" value={model.glow} oninput={(e) => set('glow', +e.currentTarget.value)} />
              <span class="value mono" title="How much the game multiplies the print by">x{factor < 10 ? factor.toFixed(1) : Math.round(factor)}</span>
            </div>
          {/if}
        </section>

        <section>
          <h3>Board effect</h3>
          <div class="segmented">
            {#each BOARD_EFFECTS as b (b.id)}
              <button class:on={model.board === b.id} title={b.hint} onclick={() => set('board', b.id)}>{b.label}</button>
            {/each}
          </div>
          {#if model.board === 'aura'}
            <div class="colour">
              <label class="check">
                <input type="checkbox" checked={model.auraColour === null} onchange={(e) => set('auraColour', e.currentTarget.checked ? null : autoColour)} />
                <span>Colour from the art</span>
              </label>
              <input type="color" value={model.auraColour ?? autoColour} disabled={model.auraColour === null} oninput={(e) => set('auraColour', e.currentTarget.value)} />
              <span class="mono faint">{model.auraColour ?? autoColour}</span>
            </div>
          {:else if model.board === 'nyan'}
            <p class="faint small">Six solid bands behind the board while you ride (Ride in the preview).</p>
          {/if}
        </section>

        {#if needsShaders(model)}
          <p class="note">Uses Caveman's shaders: the build adds the Caveman-Shaders mod automatically.</p>
        {/if}

        {#if item.notes.length > 0 || fitNotes.length > 0}
          <section>
            <h3>Notes</h3>
            {#each [...item.notes, ...fitNotes] as n (n)}
              <p class="dim small">{n}</p>
            {/each}
          </section>
        {/if}
      </div>

      <footer>
        <button class="primary" disabled={!dirty || saving} onclick={onsave}>{saving ? 'Saving...' : 'Save settings'}</button>
        <button disabled={!dirty || saving} onclick={onrevert}>Revert</button>
        <button class="ghost" disabled={!item.sidecar || saving} title="Remove the settings file; the file name's suffixes apply again" onclick={onclear}>Use suffixes</button>
      </footer>
    {/if}
  {/if}
</aside>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
  }
  .placeholder {
    margin: auto;
    padding: 24px;
    text-align: center;
  }
  header {
    padding: 16px 16px 10px;
    border-bottom: 1px solid var(--line);
  }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .path {
    margin: 4px 0 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .small {
    font-size: 12px;
    padding: 4px 8px;
  }
  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .badge.muted {
    background: var(--raised);
    color: var(--text-dim);
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 4px 16px 16px;
  }
  section {
    padding: 12px 0 14px;
    border-bottom: 1px solid var(--line);
  }
  section:last-child {
    border-bottom: none;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .label {
    margin: 10px 0 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .segmented {
    display: flex;
    gap: 4px;
  }
  .segmented.wrap {
    flex-wrap: wrap;
  }
  .segmented button {
    flex: 1 1 auto;
    padding: 6px 10px;
    font-size: 13px;
    background: var(--panel-2);
    border-color: var(--line);
  }
  .segmented button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 0 4px;
    cursor: pointer;
  }
  .slider {
    display: grid;
    grid-template-columns: 64px 1fr 56px;
    align-items: center;
    gap: 10px;
    margin: 6px 0 2px 24px;
  }
  .slider .label {
    margin: 0;
  }
  .value {
    text-align: right;
    color: var(--text-dim);
  }
  .colour {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }
  .colour .check {
    margin: 0;
    flex: 1;
  }
  .swatch {
    display: inline-block;
    width: 12px;
    height: 12px;
    border-radius: 3px;
    vertical-align: -1px;
    border: 1px solid var(--line-strong);
  }
  .note {
    margin: 12px 0 0;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: var(--panel-2);
    border: 1px solid var(--line);
    font-size: 12px;
    color: var(--text-dim);
  }
  footer {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
  footer .primary {
    flex: 1;
  }
  p {
    margin: 4px 0;
  }
</style>
