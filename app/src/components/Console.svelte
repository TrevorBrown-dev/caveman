<script lang="ts">
  // The console: any `caveman-trainer` command typed with completion (lib/console.ts, the trainer's own usage), run
  // through the back end (`live`, which runs the trainer as a hidden child and returns what it printed), with the
  // output kept in a scrollback and the history on Up and Down. For messing about and debugging; the plugin pages
  // drive the same commands with buttons.
  import { onMount, tick } from 'svelte';
  import * as live from '../lib/live';
  import { complete, split, type Completion } from '../lib/console';

  interface Props {
    onsay: (text: string, kind?: 'info' | 'error') => void;
  }
  let { onsay }: Props = $props();

  interface Entry {
    id: number;
    command: string;
    text: string;
    ok: boolean;
    dry: boolean;
    ms: number;
    at: string;
  }

  let line = $state('');
  let entries = $state<Entry[]>([]);
  let history = $state<string[]>([]);
  let cursor = $state(-1);
  let pendingLine = '';
  let running = $state<string | null>(null);
  let picked = $state(0);
  let input: HTMLInputElement | undefined = $state();
  let scroll: HTMLDivElement | undefined = $state();
  let nextId = 1;

  const HISTORY_KEY = 'caveman-console-history';

  const tokens = $derived(split(line));
  const trailing = $derived(line.endsWith(' ') || line === '');
  const completion: Completion = $derived(complete(tokens, trailing));
  const shown = $derived(completion.options.filter((o) => !o.word.startsWith('<')).slice(0, 12));
  const placeholders = $derived(completion.options.filter((o) => o.word.startsWith('<')));

  $effect(() => {
    // the pick stays within the list as it changes
    if (picked >= shown.length) picked = 0;
  });

  function accept(word: string): void {
    const parts = split(line);
    if (trailing) parts.push(word);
    else parts[parts.length - 1] = word;
    line = `${parts.map(quote).join(' ')} `;
    picked = 0;
    input?.focus();
  }

  function quote(word: string): string {
    return word.includes(' ') ? `"${word}"` : word;
  }

  async function run(): Promise<void> {
    const command = line.trim();
    if (!command || running) return;
    const args = split(command);
    if (args[0]?.toLowerCase() === 'caveman-trainer') args.shift();
    if (args.length === 0) return;
    if (args[0] === 'clear' || args[0] === 'cls') {
      entries = [];
      line = '';
      return;
    }
    history = [command, ...history.filter((h) => h !== command)].slice(0, 100);
    localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
    cursor = -1;
    line = '';
    running = command;
    const start = performance.now();
    const at = new Date().toLocaleTimeString([], { hour12: false });
    try {
      const r = await live.live(args);
      const text = r.json ? JSON.stringify(r.json, null, 2) : r.text.trim() || (r.ok ? '(nothing printed)' : `(exit code ${r.code ?? 'none'})`);
      entries = [...entries, { id: nextId++, command: r.command, text, ok: r.ok, dry: r.dry, ms: performance.now() - start, at }];
    } catch (e) {
      entries = [...entries, { id: nextId++, command: `caveman-trainer ${args.map(quote).join(' ')}`, text: String(e), ok: false, dry: false, ms: performance.now() - start, at }];
      onsay(String(e), 'error');
    } finally {
      running = null;
      await tick();
      if (scroll) scroll.scrollTop = scroll.scrollHeight;
      input?.focus();
    }
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      e.preventDefault();
      void run();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      if (shown.length > 0) accept(shown[picked]?.word ?? shown[0].word);
    } else if (e.key === 'ArrowDown' && shown.length > 1 && !e.altKey) {
      e.preventDefault();
      picked = (picked + 1) % shown.length;
    } else if (e.key === 'ArrowUp' && shown.length > 1 && !e.altKey && line.trim() !== '') {
      e.preventDefault();
      picked = (picked - 1 + shown.length) % shown.length;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (history.length === 0) return;
      if (cursor === -1) pendingLine = line;
      cursor = Math.min(cursor + 1, history.length - 1);
      line = history[cursor];
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (cursor <= 0) {
        cursor = -1;
        line = pendingLine;
      } else {
        cursor -= 1;
        line = history[cursor];
      }
    } else if (e.key === 'l' && e.ctrlKey) {
      e.preventDefault();
      entries = [];
    } else if (e.key === 'Escape') {
      line = '';
      cursor = -1;
    }
  }

  function copy(text: string): void {
    void navigator.clipboard?.writeText(text);
  }

  onMount(() => {
    try {
      history = JSON.parse(localStorage.getItem(HISTORY_KEY) ?? '[]');
    } catch {
      history = [];
    }
    input?.focus();
  });
</script>

<div class="console">
  <div class="scrollback" bind:this={scroll}>
    {#if entries.length === 0}
      <p class="faint intro">
        Type a <span class="mono">caveman-trainer</span> command. Tab completes a word, Enter runs it, Up and Down recall earlier ones, Ctrl+L clears. Start with <span class="mono">status</span>, <span class="mono">board</span>, <span class="mono">bars</span>, <span class="mono">mem ring</span> or <span class="mono">draw status</span>.
      </p>
    {/if}
    {#each entries as e (e.id)}
      <article class:bad={!e.ok}>
        <header>
          <span class="prompt">&gt;</span>
          <span class="mono cmd">{e.command}</span>
          <span class="meta faint">{e.at} · {e.ms < 1000 ? `${Math.round(e.ms)} ms` : `${(e.ms / 1000).toFixed(1)} s`}{e.dry ? ' · dry run' : ''}{e.ok ? '' : ' · failed'}</span>
          <button class="ghost tiny" title="Copy the output" onclick={() => copy(e.text)}>Copy</button>
        </header>
        <pre class="mono">{e.text}</pre>
      </article>
    {/each}
    {#if running}
      <article class="running">
        <header><span class="prompt">&gt;</span><span class="mono cmd">{running}</span><span class="meta faint">running (a command is cut off after 90 s)</span></header>
      </article>
    {/if}
  </div>

  <div class="entry">
    <div class="hintline">
      <span class="faint">{completion.hint}</span>
      {#if completion.long}<span class="warn">runs until stopped or for the time given: the console cuts it off after 90 s</span>{/if}
    </div>
    {#if shown.length > 0 || placeholders.length > 0}
      <div class="options">
        {#each shown as o, i (o.word)}
          <button class="opt" class:on={i === picked} class:flag={o.flag} onclick={() => accept(o.word)} title={o.hint ?? ''}>
            <span class="mono">{o.word}</span>
            {#if o.hint}<span class="faint small">{o.hint}</span>{/if}
          </button>
        {/each}
        {#each placeholders as p (p.word)}
          <span class="opt ph"><span class="mono">{p.word}</span>{#if p.hint}<span class="faint small">{p.hint}</span>{/if}</span>
        {/each}
      </div>
    {/if}
    <div class="row">
      <span class="prompt mono">caveman-trainer</span>
      <input class="mono" type="text" bind:this={input} bind:value={line} onkeydown={onKey} oninput={() => (picked = 0)} placeholder="status" spellcheck="false" autocomplete="off" disabled={!!running} aria-label="Command" />
      <button class="primary" disabled={!!running || !line.trim()} onclick={run}>Run</button>
    </div>
  </div>
</div>

<style>
  .console {
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
    height: 100%;
    min-height: 0;
  }
  .scrollback {
    overflow-y: auto;
    padding: 12px 16px;
  }
  .intro {
    margin: 8px 0;
    max-width: 70ch;
  }
  article {
    margin: 0 0 10px;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--line);
  }
  article.bad {
    border-color: rgba(255, 107, 107, 0.45);
  }
  article.running {
    border-style: dashed;
  }
  article header {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .cmd {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    margin-left: auto;
    white-space: nowrap;
    font-size: 11px;
  }
  .tiny {
    font-size: 11px;
    padding: 2px 7px;
  }
  pre {
    margin: 6px 0 0;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text-dim);
    max-height: 420px;
    overflow: auto;
  }
  .prompt {
    color: var(--accent);
    font-weight: 700;
  }
  .entry {
    border-top: 1px solid var(--line);
    padding: 8px 16px 12px;
    background: var(--panel);
  }
  .hintline {
    display: flex;
    gap: 12px;
    font-size: 12px;
    min-height: 18px;
    margin-bottom: 6px;
  }
  .warn {
    color: var(--warn);
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 8px;
  }
  .opt {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 3px 8px;
    font-size: 12px;
    background: var(--panel-2);
    border-color: var(--line);
  }
  .opt.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .opt.flag .mono {
    color: var(--text-dim);
  }
  .opt.ph {
    border: 1px dashed var(--line);
    border-radius: var(--radius-sm);
    color: var(--text-faint);
  }
  .small {
    font-size: 11px;
    max-width: 36ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .row input {
    flex: 1;
    font-size: 13px;
  }
</style>
