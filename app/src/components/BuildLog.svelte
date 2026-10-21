<script lang="ts">
  import type { BuildEvent, BuildResult } from '../lib/types';

  interface Props {
    events: BuildEvent[];
    result: BuildResult | null;
    error: string | null;
    building: boolean;
    open: boolean;
    ontoggle: () => void;
  }
  let { events, result, error, building, open, ontoggle }: Props = $props();

  const warnings = $derived(events.filter((e) => e.event === 'warning' || e.event === 'skipped'));
  const items = $derived(events.filter((e) => e.event === 'item').length);

  function line(e: BuildEvent): string {
    switch (e.event) {
      case 'item':
        return `ready  ${e.display}${e.notes && e.notes.length ? `  (${e.notes.join('; ')})` : ''}`;
      case 'warning':
        return `WARNING  ${e.display}: ${String(e.message ?? '')}`;
      case 'skipped':
        return `skipped  ${String(e.source ?? '').split(/[\\/]/).pop()}: ${e.reason}`;
      case 'scanned':
        return `${e.count} ${e.kind} image(s) in ${String(e.folder ?? '')}`;
      case 'built':
        return `built ${e.decks} deck(s) and ${e.wheels} wheel(s) into ${String(e.mod_dir ?? '')}`;
      case 'shaders_built':
        return `built the shader effects into ${String(e.mod_dir ?? '')}`;
      case 'up_to_date':
        return 'decks are up to date';
      case 'log':
        return String(e.message ?? '');
      default:
        return `${e.event} ${JSON.stringify(e)}`;
    }
  }

  const summary = $derived.by(() => {
    if (building) return `Building... ${items} item(s) read`;
    if (error) return `Build failed: ${error}`;
    if (!result) return 'No build yet this session';
    if (result.outcome === 'up_to_date') return `Up to date (${result.seconds.toFixed(2)} s)`;
    if (result.outcome === 'nothing') return 'Nothing to build';
    const shaders = result.shaders === 'built' ? ', shaders' : result.shaders === 'removed' ? ', shaders removed' : '';
    return `Built ${result.decks} deck(s) and ${result.wheels} wheel(s)${shaders} in ${result.seconds.toFixed(1)} s`;
  });
</script>

<div class="log" class:open>
  <button class="bar ghost" onclick={ontoggle} aria-expanded={open}>
    <span class="dot" class:busy={building} class:bad={!!error} class:good={!!result && !error && !building}></span>
    <span class="summary">{summary}</span>
    {#if warnings.length > 0}<span class="warn">{warnings.length} warning(s)</span>{/if}
    <span class="chev">{open ? 'Hide log' : 'Show log'}</span>
  </button>
  {#if open}
    <pre class="lines mono">{#each events as e, i (i)}{line(e)}
{/each}{#if events.length === 0}The build's progress shows here.{/if}</pre>
  {/if}
</div>

<style>
  .log {
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 14px;
    border-radius: 0;
    text-align: left;
  }
  .summary {
    flex: 1;
    color: var(--text);
  }
  .warn {
    color: var(--warn);
    font-size: 12px;
  }
  .chev {
    font-size: 12px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .dot.good {
    background: var(--good);
  }
  .dot.bad {
    background: var(--bad);
  }
  .dot.busy {
    background: var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .lines {
    margin: 0;
    height: 190px;
    overflow: auto;
    padding: 8px 14px 12px;
    color: var(--text-dim);
    white-space: pre-wrap;
    border-top: 1px solid var(--line);
  }
</style>
