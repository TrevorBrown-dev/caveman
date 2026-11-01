<script lang="ts">
  // The launcher's front page, laid out like ReSkate's own: the brand and the status on the left, what's new under
  // it, and on the right the way in: Settings, Plugins, Studio and Play.
  import type { LauncherStatus, PluginsConfig } from '../lib/types';
  import { LATEST } from '../lib/changelog';
  import { WIDGETS } from '../lib/widgets';

  interface Props {
    status: LauncherStatus | null;
    gameRunning: boolean;
    /** The FX DLL answers in the running game (null: not known yet). */
    injected: boolean | null;
    version: string;
    plugins: PluginsConfig;
    /** A build runs before the launch. */
    playing: boolean;
    onopen: (view: 'plugins' | 'studio' | 'console') => void;
    onplay: () => void;
    onsettings: () => void;
    onchangelogs: () => void;
  }
  let { status, gameRunning, injected, version, plugins, playing, onopen, onplay, onsettings, onchangelogs }: Props = $props();

  type Mark = 'ok' | 'warn' | 'bad';
  interface Line {
    mark: Mark;
    text: string;
  }
  const lines = $derived.by((): Line[] => {
    if (!status) return [{ mark: 'warn', text: 'Reading...' }];
    const out: Line[] = [];
    out.push(status.gameOk ? { mark: 'ok', text: `skate. at ${status.game}` } : { mark: 'bad', text: `No game at ${status.game} (Settings)` });
    if (status.gameOk) {
      out.push(
        status.skateBuild === 'match'
          ? { mark: 'ok', text: 'skate. build 25414733, the one Caveman knows' }
          : status.skateBuild === 'other'
            ? { mark: 'warn', text: 'Another skate. build: the FX DLL stays out, the mods still build' }
            : { mark: 'warn', text: 'skate. build not checked yet' }
      );
      out.push(status.reskateVersion ? { mark: 'ok', text: `ReSkate ${status.reskateVersion}` } : { mark: 'warn', text: 'ReSkate launcher not found' });
    }
    out.push(
      status.modsBuilt === 'up_to_date'
        ? { mark: 'ok', text: `${status.decksOn} deck(s) and ${status.wheelsOn} wheel(s) built into the game` }
        : status.modsBuilt === 'stale'
          ? { mark: 'warn', text: `${status.decksOn} deck(s) and ${status.wheelsOn} wheel(s) to build (Play builds them)` }
          : { mark: 'warn', text: 'No decks yet: Studio shows where to put them' }
    );
    out.push(status.dllInstalled && status.trainer ? { mark: 'ok', text: 'FX DLL and trainer installed' } : { mark: 'warn', text: status.trainer ? 'FX DLL not installed beside the game (the plugins need it)' : 'caveman-trainer.exe not found (the plugins need it)' });
    return out;
  });
  const ready = $derived(!!status && status.gameOk && status.skateBuild !== 'missing');
  const headline = $derived(gameRunning ? (injected ? 'Skating, the FX DLL is in' : injected === false ? 'Skating, without the FX DLL' : 'Skating') : ready ? 'Ready to skate' : 'Not ready');
  const autostarts = $derived([plugins.obs.autostart ? 'OBS link' : '', plugins.bars.autostart ? 'Soundbars' : '', ...WIDGETS.filter((w) => plugins.widgets[w.kind]?.autostart).map((w) => w.name)].filter(Boolean));
</script>

<div class="home">
  <div class="left">
    <div class="brand">
      <span class="logo" aria-hidden="true"></span>
      <div>
        <div class="welcome">Caveman</div>
        <div class="faint small">Launcher {version}</div>
      </div>
    </div>
    <h1 class="wordmark">CAVEMAN</h1>

    <section class="tile status" class:ready class:running={gameRunning}>
      <header>
        <h2>Status</h2>
        <span class="pill" class:ok={ready && !gameRunning} class:live={gameRunning}>{gameRunning ? 'IN GAME' : ready ? 'READY' : 'CHECK'}</span>
      </header>
      <p class="headline"><span class="mark" class:ok={ready} class:bad={!ready}></span>{headline}</p>
      <ul>
        {#each lines as l, i (i)}
          <li><span class="mark {l.mark}"></span>{l.text}</li>
        {/each}
      </ul>
    </section>

    <section class="tile news">
      <header>
        <h2>What's new</h2>
        <span class="faint small">{LATEST ? `${LATEST.version}${LATEST.date ? ` · ${LATEST.date}` : ''}` : ''}</span>
      </header>
      {#if LATEST}
        <ul class="bullets">
          {#each LATEST.bullets.slice(0, 5) as b, i (i)}
            <li>{b}</li>
          {/each}
        </ul>
      {:else}
        <p class="dim">No changelog bundled.</p>
      {/if}
    </section>

    <div class="corner">
      <button class="flat" onclick={onchangelogs}>Changelogs</button>
      <button class="flat" onclick={() => onopen('console')} title="Every caveman-trainer command, with completion">Console</button>
    </div>
  </div>

  <div class="right">
    <button class="nav small-nav" onclick={onsettings}>
      <span class="nav-title">Settings</span>
      <span class="nav-icon" aria-hidden="true">&#9881;</span>
    </button>
    <button class="nav" onclick={() => onopen('plugins')}>
      <span class="nav-text">
        <span class="nav-title">Plugins</span>
        <span class="nav-sub">{autostarts.length > 0 ? `${autostarts.join(', ')} start with the game` : 'OBS link, board cam, soundbars'}</span>
      </span>
      <span class="nav-icon" aria-hidden="true">&#9889;</span>
    </button>
    <button class="nav" onclick={() => onopen('studio')}>
      <span class="nav-text">
        <span class="nav-title">Studio</span>
        <span class="nav-sub">{status ? `${status.decksOn} of ${status.decks} decks, ${status.wheelsOn} of ${status.wheels} wheels in the game` : 'decks, wheels, packs'}</span>
      </span>
      <span class="nav-icon" aria-hidden="true">&#9998;</span>
    </button>
    <button class="play" disabled={playing || gameRunning} onclick={onplay}>
      <span class="play-title">{playing ? 'Building...' : gameRunning ? 'Playing' : 'Play'}</span>
      <span class="play-sub">{gameRunning ? 'skate. is running' : status?.reskateVersion ? `ReSkate ${status.reskateVersion}` : 'ReSkate'}</span>
    </button>
  </div>
</div>

<style>
  .home {
    position: relative;
    display: grid;
    grid-template-columns: minmax(360px, 520px) 1fr;
    height: 100%;
    padding: 24px 36px 72px 44px;
    gap: 24px;
  }
  .left {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-height: 0;
  }
  .tile.news {
    flex: 0 1 auto;
    min-height: 0;
    overflow-y: auto;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 12px;
    width: fit-content;
    padding: 8px 18px 8px 8px;
    background: rgba(14, 12, 18, 0.72);
    border-radius: var(--radius-sm);
  }
  .logo {
    width: 14px;
    height: 40px;
    border-radius: 7px;
    background: linear-gradient(180deg, #ff3b30 0 16%, #ff9500 16% 33%, #ffd60a 33% 50%, #34c759 50% 66%, #0a84ff 66% 83%, #8c50ff 83%);
    transform: rotate(-18deg);
    margin: 0 6px;
  }
  .welcome {
    font-weight: 700;
    font-size: 15px;
  }
  .small {
    font-size: 12px;
  }
  .wordmark {
    margin: 2px 0 0;
    font-size: 58px;
    line-height: 1;
    font-weight: 900;
    letter-spacing: 0.04em;
    font-style: italic;
    color: #fff;
    text-shadow: 0 4px 0 rgba(0, 0, 0, 0.45), 0 10px 30px rgba(0, 0, 0, 0.5);
  }
  .tile {
    background: rgba(16, 14, 20, 0.84);
    border-left: 5px solid var(--accent);
    padding: 14px 20px 16px;
    border-radius: 3px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  }
  .tile.status.ready {
    border-left-color: var(--good);
  }
  .tile.status.running {
    border-left-color: #2ba7ff;
  }
  .tile.news {
    border-left-color: var(--warn);
  }
  .tile header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }
  .tile h2 {
    margin: 0;
    font-size: 24px;
    font-weight: 800;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .pill {
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    padding: 4px 12px;
    border-radius: 999px;
    background: rgba(243, 198, 75, 0.2);
    color: var(--warn);
  }
  .pill.ok {
    background: rgba(79, 209, 139, 0.2);
    color: var(--good);
  }
  .pill.live {
    background: rgba(43, 167, 255, 0.2);
    color: #7cc8ff;
  }
  .headline {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 10px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--line);
    font-size: 19px;
    font-weight: 700;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 7px;
    font-size: 13px;
    color: var(--text-dim);
  }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .mark {
    flex: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--text-faint);
    box-shadow: inset 0 0 0 2px rgba(0, 0, 0, 0.35);
  }
  .mark.ok {
    background: var(--good);
  }
  .mark.warn {
    background: var(--warn);
  }
  .mark.bad {
    background: var(--bad);
  }
  .bullets {
    list-style: disc;
    padding-left: 18px;
    gap: 5px;
  }
  .bullets li {
    display: list-item;
  }
  .corner {
    position: absolute;
    left: 44px;
    bottom: 24px;
    display: flex;
    gap: 8px;
  }
  .flat {
    background: rgba(16, 14, 20, 0.84);
    border: none;
    border-radius: 3px;
    padding: 12px 22px;
    font-weight: 800;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    font-size: 13px;
    color: var(--text);
  }
  .flat:hover {
    background: rgba(40, 36, 48, 0.9);
  }
  .right {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: stretch;
    gap: 12px;
    max-width: 520px;
    margin-left: auto;
    margin-bottom: -44px;
    width: 100%;
  }
  .nav {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 18px 24px;
    background: rgba(16, 14, 20, 0.84);
    border: none;
    border-radius: 3px;
    text-align: left;
    color: var(--text);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.3);
  }
  .nav:hover {
    background: rgba(40, 36, 48, 0.92);
  }
  .nav.small-nav {
    padding: 12px 24px;
  }
  .nav-text {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .nav-title {
    font-size: 26px;
    font-weight: 800;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .small-nav .nav-title {
    font-size: 20px;
  }
  .nav-sub {
    font-size: 13px;
    color: var(--text-dim);
  }
  .nav-icon {
    flex: none;
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    font-size: 24px;
    color: var(--text-dim);
  }
  .small-nav .nav-icon {
    width: 40px;
    height: 40px;
    font-size: 20px;
  }
  .play {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: space-between;
    gap: 44px;
    padding: 22px 26px 18px;
    margin-top: 8px;
    background: var(--accent);
    border: none;
    border-radius: 3px;
    color: var(--accent-ink);
    text-align: left;
    box-shadow: 0 10px 34px rgba(255, 138, 61, 0.28);
  }
  .play:hover:not(:disabled) {
    background: #ff9c58;
  }
  .play:disabled {
    opacity: 0.7;
  }
  .play-title {
    font-size: 48px;
    line-height: 1;
    font-weight: 900;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .play-sub {
    font-size: 14px;
    font-weight: 600;
  }
  @media (max-width: 1240px) {
    .home {
      grid-template-columns: minmax(320px, 440px) 1fr;
      padding: 20px 24px;
    }
    .wordmark {
      font-size: 48px;
    }
    .play-title {
      font-size: 38px;
    }
  }
</style>
