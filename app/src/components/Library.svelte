<script lang="ts">
  import type { Library, LibraryItem } from '../lib/types';
  import { chips } from '../lib/effects';
  import { fileUrl } from '../lib/api';

  interface Props {
    library: Library | null;
    selected: string | null;
    onselect: (item: LibraryItem) => void;
    tab?: 'deck' | 'wheel';
    /** The Live panel is open: the library is where a deck for the board is picked. */
    live?: boolean;
    /** The picture on the live board (its file name), marked on its card. */
    boardArt?: string | null;
    /** Switches for what goes into the game (the Studio): one per item, one per folder and one for the whole list,
     * each passing the items it covers; none in the live library. */
    ontoggle?: (items: LibraryItem[], on: boolean) => void;
  }
  let { library, selected, onselect, tab = $bindable('deck'), live = false, boardArt = null, ontoggle }: Props = $props();

  let query = $state('');
  let collapsed = $state<Record<string, boolean>>({});

  const items = $derived(library ? (tab === 'deck' ? library.decks : library.wheels) : []);
  const noun = $derived(tab === 'deck' ? 'deck' : 'wheel');
  const on = $derived(items.filter((i) => i.enabled).length);
  const searching = $derived(query.trim() !== '');
  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((i) => {
      const hay = [i.display, i.stem, i.group, ...chips(i.settings)].join(' ').toLowerCase();
      return q.split(/\s+/).every((word) => hay.includes(word));
    });
  });
  const groups = $derived.by(() => {
    const map = new Map<string, LibraryItem[]>();
    for (const item of filtered) {
      const key = item.group;
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(item);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  });
  // the "all" switch covers what the list shows (everything, or what the search matched)
  const shownOn = $derived(filtered.filter((i) => i.enabled).length);

  function thumb(item: LibraryItem): string {
    return fileUrl(item.preview ?? item.source, item.modified);
  }

  function onBoard(item: LibraryItem): boolean {
    return live && item.kind === 'deck' && !!boardArt && item.source.split(/[\\/]/).pop() === boardArt;
  }
</script>

<aside class="library">
  <div class="tabs" role="tablist">
    <button role="tab" class:active={tab === 'deck'} aria-selected={tab === 'deck'} onclick={() => (tab = 'deck')}>
      Decks <span class="count">{library?.decks.length ?? 0}</span>
    </button>
    <button role="tab" class:active={tab === 'wheel'} aria-selected={tab === 'wheel'} onclick={() => (tab = 'wheel')}>
      Wheels <span class="count">{library?.wheels.length ?? 0}</span>
    </button>
  </div>
  <div class="search">
    <input type="search" placeholder={`Search ${tab === 'deck' ? 'decks' : 'wheels'}, folders, effects`} bind:value={query} />
  </div>
  {#if ontoggle && items.length > 0}
    <div class="all-row" title={`Switched off, a ${noun} stays out of the build; its files stay where they are`}>
      <label class="all">
        <input
          class="switch"
          type="checkbox"
          checked={filtered.length > 0 && shownOn === filtered.length}
          indeterminate={shownOn > 0 && shownOn < filtered.length}
          disabled={filtered.length === 0}
          onchange={(e) => ontoggle(filtered, e.currentTarget.checked)}
        />
        <span>{searching ? `All ${filtered.length.toLocaleString()} shown` : `All ${noun}s`}</span>
      </label>
      <span class="faint">{on.toLocaleString()} of {items.length.toLocaleString()} in the game</span>
    </div>
  {/if}
  {#if live}
    <p class="live-hint">
      {#if tab === 'deck'}
        <span class="hint-step"><span class="step">1</span> Click a deck</span>
        <span class="hint-step"><span class="step">2</span> Put it on the board, in the panel on the right</span>
      {:else}
        Wheels are not on the live board: the Live panel works with decks.
      {/if}
    </p>
  {/if}

  <div class="list">
    {#if !library}
      <p class="empty dim">Loading the drop folders...</p>
    {:else if items.length === 0}
      <p class="empty dim">
        No {tab === 'deck' ? 'deck' : 'wheel'} PNGs yet. Drop them in
        <span class="mono">{tab === 'deck' ? library.decksDir : library.wheelsDir}</span>.
      </p>
    {:else if filtered.length === 0}
      <p class="empty dim">Nothing matches "{query}".</p>
    {/if}

    {#each groups as [group, list] (group)}
      {@const name = group === '' ? 'Top folder' : group}
      {@const groupOn = list.filter((i) => i.enabled).length}
      <section class="group">
        <div class="group-head">
          {#if ontoggle}
            <input
              class="switch"
              type="checkbox"
              checked={groupOn === list.length}
              indeterminate={groupOn > 0 && groupOn < list.length}
              title={searching ? `The ${list.length} shown in this folder: on or off` : `Every ${noun} in this folder: on or off`}
              onchange={(e) => ontoggle(list, e.currentTarget.checked)}
              aria-label={`${name}: all in the game`}
            />
          {/if}
          <button class="group-toggle ghost" class:off={ontoggle && groupOn === 0} onclick={() => (collapsed[group] = !collapsed[group])} aria-expanded={!collapsed[group]}>
            <span class="chev" class:shut={collapsed[group]}>&#9662;</span>
            <span class="group-name">{name}</span>
            <span class="count">{ontoggle && groupOn < list.length ? `${groupOn} of ${list.length}` : list.length}</span>
          </button>
        </div>
        {#if !collapsed[group]}
          <ul>
            {#each list as item (item.source)}
              <li class="row-item" class:off={ontoggle && !item.enabled}>
                {#if ontoggle}
                  <input class="switch" type="checkbox" checked={item.enabled} title={item.enabled ? 'In the game: switch off to leave it out of the build' : 'Out of the build: switch on'} onchange={(e) => ontoggle([item], e.currentTarget.checked)} aria-label={`${item.display} in the game`} />
                {/if}
                <button class="card" class:selected={selected === item.source} onclick={() => onselect(item)}>
                  <span class="thumb" class:wheel={item.kind === 'wheel'} style:background-color={item.colour ?? undefined}>
                    <img src={thumb(item)} alt="" loading="lazy" />
                  </span>
                  <span class="meta">
                    <span class="name" title={item.display}>{item.display}</span>
                    <span class="chips">
                      {#each chips(item.settings) as c (c)}
                        <span class="chip">{c}</span>
                      {/each}
                      {#if item.kind === 'wheel' && item.colour}
                        <span class="chip mono">{item.colour}</span>
                      {/if}
                      {#if item.sidecar}
                        <span class="chip saved" title="Has a settings file">JSON</span>
                      {/if}
                      {#if onBoard(item)}
                        <span class="chip live" title="This deck's picture is on the board in the game">on the board</span>
                      {/if}
                    </span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}

    {#if library && library.problems.length > 0}
      <section class="problems">
        <h3>Left out of the build</h3>
        {#each library.problems as p (p.source)}
          <p><span class="mono">{p.source.split(/[\\/]/).pop()}</span>: {p.reason}</p>
        {/each}
      </section>
    {/if}
  </div>
</aside>

<style>
  .library {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--panel);
    border-right: 1px solid var(--line);
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
  .search {
    padding: 0 12px 10px;
  }
  .search input {
    width: 100%;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 0 8px 16px;
  }
  .empty {
    padding: 12px;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .group-toggle {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 6px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--text-dim);
  }
  .group-toggle.off {
    opacity: 0.55;
  }
  .group-name {
    flex: 1;
    min-width: 0;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group-toggle .count {
    flex: none;
    text-transform: none;
  }
  .chev {
    display: inline-block;
    transition: transform 0.12s ease;
  }
  .chev.shut {
    transform: rotate(-90deg);
  }
  ul {
    list-style: none;
    margin: 0 0 8px;
    padding: 0;
  }
  .row-item {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .row-item .card {
    flex: 1;
    min-width: 0;
  }
  .row-item.off .card {
    opacity: 0.5;
  }
  .switch {
    flex: none;
    margin: 0 0 0 4px;
  }
  /* the switch lines up with the folder and card switches below (8px list padding + the switch's 4px) */
  .all-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0 8px;
    padding: 0 6px 8px 0;
    border-bottom: 1px solid var(--line);
    font-size: 12px;
  }
  .all {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    cursor: pointer;
  }
  .all-row .faint {
    white-space: nowrap;
  }
  .card {
    display: flex;
    gap: 10px;
    align-items: center;
    width: 100%;
    padding: 6px;
    margin: 2px 0;
    background: transparent;
    border: 1px solid transparent;
    text-align: left;
  }
  .card:hover:not(:disabled) {
    background: var(--panel-2);
    border-color: var(--line);
  }
  .card.selected {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .thumb {
    flex: none;
    width: 30px;
    height: 104px;
    border-radius: 15px;
    overflow: hidden;
    background: #0c0b0f;
    display: grid;
    place-items: center;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .thumb.wheel {
    width: 52px;
    height: 52px;
    border-radius: 50%;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 5px;
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
  .chip.saved {
    color: var(--accent);
    border-color: rgba(255, 138, 61, 0.4);
  }
  .chip.live {
    color: var(--good);
    border-color: rgba(79, 209, 139, 0.4);
  }
  .live-hint {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 0 12px 10px;
    padding: 7px 10px;
    border: 1px solid rgba(255, 138, 61, 0.35);
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    font-size: 12px;
  }
  .hint-step {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .step {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-ink);
    font-size: 11px;
    font-weight: 700;
  }
  .problems {
    margin: 12px 4px 0;
    padding: 10px 12px;
    border: 1px solid rgba(243, 198, 75, 0.35);
    border-radius: var(--radius-sm);
    background: rgba(243, 198, 75, 0.06);
    font-size: 12px;
  }
  .problems h3 {
    margin: 0 0 6px;
    font-size: 12px;
    color: var(--warn);
  }
  .problems p {
    margin: 4px 0;
    color: var(--text-dim);
  }
</style>
