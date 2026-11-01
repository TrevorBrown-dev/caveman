<script lang="ts">
  // The mod generator: chosen decks and wheels into a Thunderstore-ready package (`caveman pack`, run by the back end):
  // a folder `Owner-Name` and `Owner-Name-1.0.0.zip` with manifest.json, icon.png, README.md and CHANGELOG.md, which
  // needs nothing of Caveman to play. Shader effects (cycle, glow, the rainbow trail, animation) cannot travel in a
  // package: those decks come out plain, and the result says which.
  import * as api from '../lib/api';
  import type { Config, Library, LibraryItem, PackEvent, PackResult } from '../lib/types';
  import { fileUrl } from '../lib/api';
  import { chips } from '../lib/effects';

  interface Props {
    config: Config;
    library: Library | null;
    onsay: (text: string, kind?: 'info' | 'error') => void;
  }
  let { config, library, onsay }: Props = $props();

  const KEY = 'caveman-pack-form';
  interface Form {
    owner: string;
    name: string;
    version: string;
    description: string;
    website: string;
    icon: string;
    out: string;
  }
  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem(KEY) ?? 'null') as Form | null;
    } catch {
      return null;
    }
  })();
  // svelte-ignore state_referenced_locally
  let form = $state<Form>(saved ?? { owner: (config.author || '').replace(/[^A-Za-z0-9_]+/g, '_'), name: '', version: '1.0.0', description: '', website: '', icon: '', out: '' });
  let chosen = $state<Record<string, boolean>>({});
  let tab = $state<'deck' | 'wheel'>('deck');
  let query = $state('');
  let running = $state(false);
  let events = $state<string[]>([]);
  let result = $state<PackResult | null>(null);
  let error = $state<string | null>(null);

  const items = $derived(library ? (tab === 'deck' ? library.decks : library.wheels) : []);
  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((i) => [i.display, i.group, ...chips(i.settings)].join(' ').toLowerCase().includes(q));
  });
  const all = $derived(library ? [...library.decks, ...library.wheels] : []);
  const picked = $derived(all.filter((i) => chosen[i.source]));
  const pickedDecks = $derived(picked.filter((i) => i.kind === 'deck').length);
  const pickedWheels = $derived(picked.filter((i) => i.kind === 'wheel').length);
  const shaderPicked = $derived(picked.filter((i) => i.kind === 'deck' && usesShaders(i)).length);
  const folderName = $derived(`${form.owner}-${form.name}`);
  const problems = $derived.by((): string[] => {
    const out: string[] = [];
    const word = /^[A-Za-z0-9_]+$/;
    if (!form.owner) out.push('Owner (your Thunderstore team name) is empty.');
    else if (!word.test(form.owner)) out.push('Owner: letters, digits and _ only.');
    if (!form.name) out.push('Name is empty.');
    else if (!word.test(form.name)) out.push('Name: letters, digits and _ only (Thunderstore shows _ as a space).');
    if (folderName.length > 64) out.push('Owner-Name is longer than 64 characters.');
    if (!/^\d+\.\d+\.\d+$/.test(form.version)) out.push('Version: three numbers, such as 1.0.0.');
    if (form.description.length > 250) out.push(`Description: ${form.description.length} of 250 characters.`);
    if (form.website && !/^https?:\/\//.test(form.website)) out.push('Website: a link starting with https://, or empty.');
    if (picked.length === 0) out.push('Choose at least one deck or wheel.');
    return out;
  });

  function usesShaders(i: LibraryItem): boolean {
    const s = i.settings;
    return !!s && (s.shader?.effect === 'cycle' || (s.shader?.glow ?? 0) > 0 || s.board?.effect === 'nyan' || /\.(gif|mp4|webm|mov|mkv|avi|m4v)$/i.test(i.source));
  }

  function remember(): void {
    localStorage.setItem(KEY, JSON.stringify(form));
  }

  function setAll(on: boolean, onlyEnabled = false): void {
    const next = { ...chosen };
    for (const i of items) next[i.source] = on && (!onlyEnabled || i.enabled);
    chosen = next;
  }

  async function generate(): Promise<void> {
    if (running || problems.length > 0) return;
    running = true;
    events = [];
    error = null;
    result = null;
    remember();
    try {
      result = await api.packBuild(
        {
          owner: form.owner,
          name: form.name,
          version: form.version,
          description: form.description,
          website: form.website,
          icon: form.icon.trim() || null,
          out: form.out.trim() || null,
          items: picked.map((i) => ({ kind: i.kind, source: i.source })),
        },
        (e: PackEvent) => {
          const line = e.event === 'log' ? String(e.message ?? '') : e.event === 'item' ? `ready  ${e.display}` : e.event === 'skipped' || e.event === 'warning' ? `${e.event.toUpperCase()}  ${e.display ?? e.source}: ${e.reason ?? e.message ?? ''}` : e.message ? String(e.message) : `${e.event}`;
          events = [...events, line];
        }
      );
      onsay(`Packed ${folderName} ${form.version}: ${result.decks} deck(s) and ${result.wheels} wheel(s)`);
    } catch (e) {
      error = String(e);
      onsay(error, 'error');
    } finally {
      running = false;
    }
  }
</script>

<div class="packs">
  <aside class="choose">
    <div class="tabs" role="tablist">
      <button role="tab" class:active={tab === 'deck'} aria-selected={tab === 'deck'} onclick={() => (tab = 'deck')}>Decks <span class="count">{library?.decks.length ?? 0}</span></button>
      <button role="tab" class:active={tab === 'wheel'} aria-selected={tab === 'wheel'} onclick={() => (tab = 'wheel')}>Wheels <span class="count">{library?.wheels.length ?? 0}</span></button>
    </div>
    <div class="tools">
      <input type="search" placeholder="Search" bind:value={query} />
      <button class="small" onclick={() => setAll(true)}>All</button>
      <button class="small" onclick={() => setAll(true, true)} title="The ones switched on in the library">On</button>
      <button class="small" onclick={() => setAll(false)}>None</button>
    </div>
    <ul class="list">
      {#each filtered as item (item.source)}
        <li>
          <label class="card" class:on={!!chosen[item.source]}>
            <input type="checkbox" checked={!!chosen[item.source]} onchange={(e) => (chosen = { ...chosen, [item.source]: e.currentTarget.checked })} />
            <span class="thumb" class:wheel={item.kind === 'wheel'} style:background-color={item.colour ?? undefined}><img src={fileUrl(item.preview ?? item.source, item.modified)} alt="" loading="lazy" /></span>
            <span class="meta">
              <span class="name" title={item.display}>{item.display}</span>
              <span class="chips">
                {#if item.group}<span class="chip">{item.group}</span>{/if}
                {#each chips(item.settings) as c (c)}<span class="chip">{c}</span>{/each}
                {#if item.kind === 'deck' && usesShaders(item)}<span class="chip warn" title="Needs Caveman-Shaders: plain in a package">shader</span>{/if}
              </span>
            </span>
          </label>
        </li>
      {/each}
      {#if filtered.length === 0}<li class="dim empty">Nothing here.</li>{/if}
    </ul>
    <p class="picked-line">{pickedDecks} deck(s), {pickedWheels} wheel(s) chosen{shaderPicked > 0 ? `; ${shaderPicked} with shader effects (plain in the pack)` : ''}</p>
  </aside>

  <section class="form">
    <h3>The package</h3>
    <div class="grid">
      <label><span>Owner</span><input type="text" bind:value={form.owner} onchange={remember} placeholder="Your Thunderstore team" spellcheck="false" /></label>
      <label><span>Name</span><input type="text" bind:value={form.name} onchange={remember} placeholder="Cool_Decks" spellcheck="false" /></label>
      <label><span>Version</span><input type="text" bind:value={form.version} onchange={remember} placeholder="1.0.0" spellcheck="false" /></label>
      <label><span>Website</span><input type="text" bind:value={form.website} onchange={remember} placeholder="https:// (optional)" spellcheck="false" /></label>
      <label class="wide"><span>Description <span class="faint">({form.description.length}/250)</span></span><input type="text" bind:value={form.description} onchange={remember} placeholder="One line about the pack" /></label>
      <label class="wide"><span>Icon</span><input type="text" bind:value={form.icon} onchange={remember} placeholder="A PNG, made 256 x 256 (empty: the first deck's thumbnail)" spellcheck="false" /></label>
      <label class="wide"><span>Output folder</span><input type="text" bind:value={form.out} onchange={remember} placeholder={`${config.game}\\Caveman Packs`} spellcheck="false" /></label>
    </div>
    <p class="preview mono">{folderName}{folderName === '-' ? '' : `-${form.version}.zip`}</p>
    {#if problems.length > 0}
      <ul class="problems">
        {#each problems as p, i (i)}<li>{p}</li>{/each}
      </ul>
    {/if}
    <div class="row">
      <button class="primary" disabled={running || problems.length > 0} onclick={generate}>{running ? 'Generating...' : 'Generate the package'}</button>
      {#if result}
        <button onclick={() => api.reveal(result!.zip)}>Show the zip</button>
        <button class="ghost" onclick={() => api.reveal(result!.folder)}>Show the folder</button>
      {/if}
    </div>
    {#if result}
      <div class="result">
        <p><strong>{result.decks} deck(s) and {result.wheels} wheel(s)</strong> packed ({Math.round(result.zipBytes / 1024)} KB).</p>
        <p class="mono small">{result.zip}</p>
        {#if result.shaderDecks.length > 0}
          <p class="warn-text">Plain in the package (their shader effects need Caveman-Shaders): {result.shaderDecks.join(', ')}.</p>
        {/if}
        <p class="dim small">Upload the zip on thunderstore.io (community ReSkate, category Mods), or drop it on ReSkate's launcher to install it. The README ends with "Generated with Caveman".</p>
      </div>
    {/if}
    {#if error}<p class="err">{error}</p>{/if}
    {#if events.length > 0}
      <details open={!!error}>
        <summary class="dim">The build's log ({events.length} lines)</summary>
        <pre class="mono">{events.join('\n')}</pre>
      </details>
    {/if}
    <p class="dim small hint">A pack is its own mod, apart from your personal Caveman-Decks: its decks get their own ids, so a player can have both. Keep the Owner and Name the same from version to version and raise the version for each upload.</p>
  </section>
</div>

<style>
  .packs {
    display: grid;
    grid-template-columns: minmax(300px, 380px) minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  .choose {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
    background: var(--panel);
  }
  .tabs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    padding: 12px 12px 8px;
  }
  .tabs button {
    background: transparent;
    border-color: var(--line);
    color: var(--text-dim);
  }
  .tabs button.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .count {
    font-size: 11px;
    color: var(--text-faint);
    margin-left: 4px;
  }
  .tools {
    display: flex;
    gap: 6px;
    padding: 0 12px 8px;
  }
  .tools input {
    flex: 1;
    min-width: 0;
  }
  .small {
    font-size: 12px;
    padding: 4px 9px;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    list-style: none;
    margin: 0;
    padding: 0 8px 8px;
  }
  .card {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 6px;
    margin: 2px 0;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .card:hover {
    background: var(--panel-2);
  }
  .card.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .thumb {
    flex: none;
    width: 24px;
    height: 84px;
    border-radius: 12px;
    overflow: hidden;
    background: #0c0b0f;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .thumb.wheel {
    width: 44px;
    height: 44px;
    border-radius: 50%;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 500;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 999px;
    background: var(--raised);
    color: var(--text-dim);
    border: 1px solid var(--line);
  }
  .chip.warn {
    color: var(--warn);
    border-color: rgba(243, 198, 75, 0.4);
  }
  .empty {
    padding: 12px;
  }
  .picked-line {
    margin: 0;
    padding: 8px 12px;
    border-top: 1px solid var(--line);
    font-size: 12px;
    color: var(--text-dim);
  }
  .form {
    padding: 16px 20px;
    overflow-y: auto;
  }
  h3 {
    margin: 0 0 12px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 14px;
    max-width: 720px;
  }
  .grid label {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .grid label.wide {
    grid-column: 1 / -1;
  }
  .grid label > span {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .preview {
    margin: 10px 0 6px;
    color: var(--text-dim);
  }
  .problems {
    margin: 6px 0 10px;
    padding-left: 18px;
    font-size: 12px;
    color: var(--warn);
  }
  .row {
    display: flex;
    gap: 8px;
    margin: 10px 0;
  }
  .result {
    margin: 12px 0;
    padding: 10px 12px;
    border: 1px solid rgba(79, 209, 139, 0.4);
    border-radius: var(--radius-sm);
    background: rgba(79, 209, 139, 0.06);
  }
  .result p {
    margin: 4px 0;
  }
  .warn-text {
    color: var(--warn);
  }
  .err {
    color: var(--bad);
  }
  .small {
    font-size: 12px;
  }
  pre {
    max-height: 240px;
    overflow: auto;
    white-space: pre-wrap;
    color: var(--text-dim);
  }
  .hint {
    max-width: 70ch;
  }
</style>
