<script lang="ts">
  import { onMount } from 'svelte';
  import { DEFAULT_PLUGINS, type Library, type LibraryItem, type PluginsConfig } from '../lib/types';
  import { fileUrl, fittedArt } from '../lib/api';
  import * as live from '../lib/live';

  interface Props {
    deck: LibraryItem | null;
    onsay: (text: string, kind?: 'info' | 'error') => void;
    /** The picture on the board now (its file name, as the trainer labels it), for the library to mark. */
    onboardart?: (label: string | null) => void;
    /** Which plugin's sections show: the OBS link (the board, pictures, finish, holo, tint, video), the board cam
     *  window, the bars; or all of them. */
    show?: 'obs' | 'bars' | 'all';
    /** The plugins' saved settings (the board cam's view, the OBS link's source), and how to save them. */
    plugins?: PluginsConfig;
    onplugins?: (p: PluginsConfig) => void;
    /** The library, to find the deck whose picture is on the board (for the board cam window). */
    library?: Library | null;
  }
  let { deck, onsay, onboardart, show = 'all', plugins = DEFAULT_PLUGINS, onplugins, library = null }: Props = $props();
  const has = (part: 'obs' | 'bars'): boolean => show === 'all' || show === part;

  /** The DLL's state (`status --json`, about 30 KB) is read this often. */
  const POLL_MS = 2500;
  /**
   * The Caveman's (`board status --json`) only every this many reads (30 s), at open and after a change: the DLL builds
   * that answer (about 9 MB) under the locks its render-side hooks take, so each read can hold the game up a moment.
   */
  const BOARD_EVERY = 12;

  type Finish = 'none' | 'gloss' | 'matte' | 'foil' | 'chrome' | 'glitter' | 'holo';
  const FINISHES: { id: Finish; label: string }[] = [
    { id: 'none', label: 'Print' },
    { id: 'gloss', label: 'Gloss' },
    { id: 'matte', label: 'Matte' },
    { id: 'foil', label: 'Foil' },
    { id: 'chrome', label: 'Chrome' },
    { id: 'glitter', label: 'Glitter' },
    { id: 'holo', label: 'Holo' }
  ];
  const PATTERNS = ['pearl', 'cellular', 'frost', 'glitter', 'vertical'] as const;
  type Pattern = (typeof PATTERNS)[number];

  interface Entry {
    id: number;
    at: string;
    label: string;
    line: string;
    command: string;
    ok: boolean;
    dry: boolean;
  }
  /** A picture put on the board from here (spot and emboss read it). */
  interface Art {
    path: string;
    label: string;
  }

  let st = $state<live.LiveStatus | null>(null);
  let busy = $state<string | null>(null);
  let last = $state<Entry | null>(null);
  let log = $state<Entry[]>([]);
  let entryId = 0;
  // svelte-ignore state_referenced_locally
  let lastArt = $state<Art | null>(plugins.live.lastDeck ? { path: plugins.live.lastDeck.args[2] ?? '', label: plugins.live.lastDeck.label } : null);
  /** What was put on the board, kept with the plugins' settings (as the trainer command that put it there) so the
   *  app puts it back by itself in the next game session; null for plain white. */
  function remember(art: Art | null, args: string[] = []): void {
    lastArt = art;
    onplugins?.({ ...plugins, live: { lastDeck: art ? { label: art.label, args } : null } });
  }

  let finish = $state<Finish | null>(null);
  let spot = $state(false);
  let emboss = $state(0);
  let strength = $state(1);
  let pattern = $state<Pattern | null>(null);
  let holoColour = $state('#a3ffff');
  let iridescence = $state(1);
  let tint = $state('#ffffff');

  let picturePath = $state('');
  let useColour = $state(false);
  let pictureColour = $state('#000000');
  let picker: HTMLInputElement | undefined = $state();
  let dragging = $state(false);
  let dragPath = $state('');

  // svelte-ignore state_referenced_locally
  let videoSource = $state<'obs' | 'file'>(plugins.obs.source);
  // svelte-ignore state_referenced_locally
  let videoFile = $state(plugins.obs.file);
  // svelte-ignore state_referenced_locally
  let rotate = $state<'cw' | 'ccw' | 'none'>(plugins.obs.rotate);
  // svelte-ignore state_referenced_locally
  let fit = $state<'cover' | 'contain'>(plugins.obs.fit);
  /** The OBS link's source, kept for the next time and for starting it with the game. */
  function saveObs(): void {
    onplugins?.({ ...plugins, obs: { ...plugins.obs, source: videoSource, file: videoFile, rotate, fit } });
  }
  let videoRunning = $state(false);
  let stopping = $state(false);
  let videoLine = $state('');
  let videoErrors = $state<string[]>([]);

  /** The overlay's settings (the bars' and the board cam's), as the DLL has them; edited here, then sent. */
  let cfg = $state<Record<string, any>>({
    enabled: false,
    source: 'audio',
    track: 'band',
    window: 42,
    floor: 0,
    release: 8,
    rise: 35,
    fall: 120,
    gain: 1,
    colour: '#00e5ff',
    count: 8,
    pip: false,
    pip_size: 0.24,
    pip_corner: 'tr',
    pip_view: 'camera',
    pip_art: true,
    pip_mirror: false,
    pip_flip: false
  });
  /** The settings as last read: a value read again unchanged does not overwrite an edit in progress. */
  let remote: Record<string, unknown> | null = null;

  const injected = $derived(st?.injected === true);
  const board = $derived(st?.board ?? null);
  const equipped = $derived(board ? String(board.equipped ?? '').split(':')[0].trim() : '');
  // a dry run runs no write, so its controls work without the DLL too (trying the panel without the game)
  const locked = $derived(!!busy || (!injected && st?.dry !== true));
  // spot and emboss read the picture last put on from here, else the selected deck's PNG (the trainer reads PNG only)
  const artForFinish = $derived<Art | null>(lastArt ?? (deck && live.extOf(deck.source) === 'png' ? { path: deck.source, label: deck.display } : null));
  const deckOnBoard = $derived(!!deck && !!board?.art && board.art === live.baseName(deck.source));
  /** An animated deck (a GIF or a video, not a PNG): it plays on the board as video. */
  const deckClip = $derived(!deck || live.extOf(deck.source) === 'png' ? null : live.extOf(deck.source) === 'gif' ? 'gif' : 'video');
  const cavemanText = $derived.by(() => {
    if (!board) return st?.board_error ? 'The Caveman: not read' : 'The Caveman: reading...';
    if (!board.found) {
      if (board.finding) return 'The Caveman: looking for its texture...';
      return `The Caveman not found${board.not_usable ? ` (${board.not_usable})` : ''}: equip it once`;
    }
    const ridden = equipped === 'yes' ? 'ridden now' : equipped === 'not now' ? 'not ridden now (another deck?)' : 'not seen ridden yet';
    return `The Caveman found, ${ridden}${board.ridden ? '; in-world copy found' : ''}`;
  });
  const dragKind = $derived.by(() => {
    const ext = live.extOf(dragPath);
    return live.PICTURES.includes(ext) ? 'picture' : live.VIDEOS.includes(ext) ? 'video' : 'other';
  });

  $effect(() => {
    onboardart?.(board?.art ?? null);
  });

  // the board cam widget follows the picture on the board (it reads it itself): told to look when the label changes
  let shownOnBoard: string | null | undefined;
  $effect(() => {
    const label = board?.art ?? null;
    if (!st?.injected || label === shownOnBoard) return;
    shownOnBoard = label;
    void live.nudgeBoardcam();
  });

  // ---- running trainer commands
  function lastLine(text: string): string {
    const lines = text
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l);
    return lines[lines.length - 1] ?? '';
  }

  function note(e: Omit<Entry, 'id' | 'at'>): void {
    const entry: Entry = { ...e, id: ++entryId, at: new Date().toLocaleTimeString([], { hour12: false }) };
    last = entry;
    log = [entry, ...log].slice(0, 60);
  }

  const num = (v: unknown): string => String(Math.round(Number(v) * 1000) / 1000);

  /**
   * Runs one trainer command (its arguments, or a step that makes them, such as making a JPG a PNG) with every
   * control locked meanwhile; its last line becomes the result, an error a toast too. Then the state is read again.
   */
  async function run(label: string, args: string[] | (() => Promise<string[]>), done?: (args: string[]) => void): Promise<boolean> {
    if (busy) {
      onsay(`Wait for "${busy}" to finish.`, 'error');
      return false;
    }
    busy = label;
    let list: string[] = [];
    let ok = false;
    try {
      list = typeof args === 'function' ? await args() : args;
      const r = await live.live(list);
      const line = r.dry ? 'dry run: not run' : lastLine(r.text) || (r.ok ? 'done' : `failed (exit code ${r.code ?? 'none'})`);
      note({ label, line, command: r.command, ok: r.ok, dry: r.dry });
      if (r.ok) {
        ok = true;
        done?.(list);
      } else {
        onsay(`${label}: ${line}`, 'error');
        remote = null;
      }
    } catch (e) {
      note({ label, line: String(e), command: '', ok: false, dry: false });
      onsay(`${label}: ${String(e)}`, 'error');
      remote = null;
    } finally {
      busy = null;
      void refresh(['board', 'inject', 'unload'].includes(list[0] ?? ''));
    }
    return ok;
  }

  // ---- the state, polled without piling up: one read at a time, a wanted read runs after the one in flight
  let polls = 0;
  let inflight = false;
  let again = false;
  let boardDue = true;
  let alive = true;

  async function refresh(withBoard = false): Promise<void> {
    if (withBoard) boardDue = true;
    if (inflight) {
      again = true;
      return;
    }
    inflight = true;
    try {
      const wantBoard = boardDue || polls % BOARD_EVERY === 0;
      boardDue = false;
      polls += 1;
      const s = await live.status(wantBoard);
      if (!alive) return;
      // the polls that do not read The Caveman keep the last reading
      if (!wantBoard && !s.board && s.injected && st?.board) s.board = st.board;
      st = s;
      sync(s.bars);
    } catch (e) {
      if (alive) st = { trainer: null, trainer_error: String(e), dry: false, game: false, injected: false, error: String(e) };
    } finally {
      inflight = false;
      if (again && alive) {
        again = false;
        void refresh();
      }
    }
  }

  function sync(bars: Record<string, unknown> | undefined): void {
    if (!bars) return;
    for (const [key, value] of Object.entries(bars)) {
      if (remote === null || remote[key] !== value) cfg[key] = value;
    }
    remote = { ...bars };
  }

  // ---- the board
  const inject = () => run('Inject the FX DLL', ['inject', '--yes']);
  const unload = () => run('Unload the FX DLL', ['unload']);

  function rideDeck(): void {
    if (!deck) return;
    const d = deck;
    void run(`Deck ${d.display}`, ['board', 'deck', d.source], (args) => remember({ path: d.source, label: d.display }, args));
  }
  const plain = () => run('Plain white', ['board', 'reset'], () => remember(null));

  /** An animated deck on the board: its clip as video, upright (deck art is portrait), cropped to the board. */
  function playDeck(): void {
    if (!deck) return;
    videoSource = 'file';
    videoFile = deck.source;
    rotate = 'none';
    fit = 'cover';
    void startVideo();
  }

  /** A GIF deck's first frame as a still (the trainer reads PNG: made one in the app's cache). */
  function deckFrame(): void {
    if (!deck) return;
    const d = deck;
    let png = '';
    void run(
      `First frame of ${d.display}`,
      async () => {
        const stem = live.baseName(d.source).replace(/\.[^.]+$/, '');
        png = await live.stagePng(`${stem} frame 1.png`, await live.pngOf(new Blob([await live.readPicture(d.source)])));
        return ['board', 'art', png];
      },
      (args) => remember({ path: png, label: d.display }, args)
    );
  }

  const colourArgs = (): string[] => (useColour ? ['--colour', pictureColour.slice(1)] : []);

  /** A picture file on the board: a PNG as it is, any other picture made a PNG in the app's cache first. */
  function putPicture(path: string): void {
    const name = live.baseName(path);
    let png = path;
    void run(
      `Picture ${name}`,
      async () => {
        if (live.extOf(path) !== 'png') png = await live.stagePng(name, await live.pngOf(new Blob([await live.readPicture(path)])));
        return ['board', 'art', png, ...colourArgs()];
      },
      (args) => remember({ path: png, label: name }, args)
    );
  }

  function putPath(): void {
    const path = picturePath.trim().replace(/^"(.*)"$/, '$1');
    if (!path) return;
    if (!live.PICTURES.includes(live.extOf(path))) {
      onsay(`Not a picture the panel takes (PNG, JPG, WebP, BMP): ${live.baseName(path)}`, 'error');
      return;
    }
    putPicture(path);
  }

  /** A file from the picker: the webview has its bytes, not its path, so it goes through the app's cache. */
  function picked(e: Event): void {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    let png = '';
    void run(
      `Picture ${file.name}`,
      async () => {
        const bytes = live.extOf(file.name) === 'png' ? new Uint8Array(await file.arrayBuffer()) : await live.pngOf(file);
        png = await live.stagePng(file.name, bytes);
        picturePath = png;
        return ['board', 'art', png, ...colourArgs()];
      },
      (args) => remember({ path: png, label: file.name }, args)
    );
  }

  function onDrop(e: live.DropEvent): void {
    if (e.type === 'enter') {
      dragPath = e.paths[0] ?? '';
      dragging = true;
    } else if (e.type === 'over') {
      dragging = true;
    } else if (e.type === 'leave') {
      dragging = false;
    } else {
      dragging = false;
      dropped(e.paths[0] ?? '');
    }
  }

  function dropped(path: string): void {
    if (!path) return;
    const name = live.baseName(path);
    const ext = live.extOf(path);
    if (live.VIDEOS.includes(ext)) {
      if (videoRunning) {
        onsay('A video is playing: stop it first.', 'error');
        return;
      }
      videoSource = 'file';
      videoFile = path;
      rotate = 'cw';
      onsay(`Video file set: ${name}. Play on the board when ready.`);
      return;
    }
    if (!live.PICTURES.includes(ext)) {
      onsay(`Not a picture (PNG, JPG, WebP, BMP) or a video: ${name}`, 'error');
      return;
    }
    picturePath = path;
    if (!injected) onsay('The FX DLL is not in the game: the picture waits in the path field.', 'error');
    else if (busy) onsay(`Busy with "${busy}": the picture waits in the path field.`, 'error');
    else putPicture(path);
  }

  // ---- the finish and the material
  function applyFinish(f: Finish): void {
    const args = ['board', 'finish', f];
    const art = artForFinish;
    if (art && (spot || emboss > 0)) {
      if (spot) args.push('--spot');
      if (emboss > 0) args.push('--emboss', num(emboss));
      args.push('--art', art.path);
    }
    // the holo's pattern is a live parameter (board foil), not part of the finish's textures
    if (f === 'holo') args.push('--strength', strength.toFixed(2));
    void run(`Finish ${FINISHES.find((x) => x.id === f)?.label ?? f}`, args, () => (finish = f));
  }
  const setPattern = (p: Pattern) => run(`Holo pattern ${p}`, ['board', 'foil', p], () => (pattern = p));
  const setHoloColour = () => run('Holo colour', ['board', 'holotint', holoColour.slice(1)]);
  const setIridescence = () => run('Holo amount', ['board', 'param', 'iridescence', iridescence.toFixed(2)]);
  const setTint = () => run('Tint', ['board', 'tint', tint.slice(1)]);
  const neutral = () => run('Tint neutral', ['board', 'tint', 'neutral'], () => (tint = '#ffffff'));

  // ---- video
  function pickSource(s: 'obs' | 'file'): void {
    videoSource = s;
    rotate = s === 'obs' ? 'none' : 'cw';
    saveObs();
  }

  async function startVideo(): Promise<void> {
    if (busy || videoRunning) return;
    const file = videoFile.trim().replace(/^"(.*)"$/, '$1');
    if (videoSource === 'file' && !file) {
      onsay('Give the path of a video file.', 'error');
      return;
    }
    const args = [videoSource === 'obs' ? 'obs' : file, '--rotate', rotate, '--fit', fit];
    const what = videoSource === 'obs' ? 'the OBS virtual camera' : live.baseName(file);
    saveObs();
    busy = 'Starting the video';
    try {
      videoErrors = [];
      videoLine = 'starting';
      const command = await live.videoStart(args);
      // a producer that failed at once has ended already (its done event may come before or after this)
      videoRunning = await live.videoRunning();
      note({ label: 'Video', line: st?.dry ? 'dry run: not run' : `playing ${what}`, command, ok: true, dry: st?.dry ?? false });
    } catch (e) {
      videoRunning = false;
      note({ label: 'Video', line: String(e), command: '', ok: false, dry: false });
      onsay(`Video: ${String(e)}`, 'error');
    } finally {
      busy = null;
    }
  }

  async function stopVideo(): Promise<void> {
    stopping = true;
    try {
      const said = await live.videoStop();
      videoRunning = false;
      note({ label: 'Video stop', line: lastLine(said) || 'stopped; the kept art returns', command: 'caveman-trainer board video off', ok: true, dry: st?.dry ?? false });
    } catch (e) {
      note({ label: 'Video stop', line: String(e), command: '', ok: false, dry: false });
      onsay(`Video: ${String(e)}`, 'error');
    } finally {
      stopping = false;
      void refresh(true);
    }
  }

  function onVideoEvent(e: live.VideoEvent): void {
    if (e.done) {
      videoRunning = false;
    } else if (e.line) {
      if (e.stream === 'err') videoErrors = [...videoErrors.slice(-3), e.line];
      else videoLine = e.line;
    }
  }

  // ---- the bars (the overlay's settings)
  function toggleBars(): void {
    const on = !cfg.enabled;
    cfg.enabled = on;
    void run(`Bars ${on ? 'on' : 'off'}`, ['bars', on ? 'on' : 'off']);
  }
  function barsWord(key: 'source' | 'track', value: string): void {
    cfg[key] = value;
    void run(`Bars ${key} ${value}`, key === 'source' ? ['bars', 'source', value] : ['bars', 'set', 'track', value]);
  }
  const barsSet = (key: string) => run(`Bars ${key} ${num(cfg[key])}`, ['bars', 'set', key, num(cfg[key])]);
  const barsColour = () => run(`Bars colour ${cfg.colour}`, ['bars', 'colour', String(cfg.colour).replace('#', '')]);
  const barsCount = () => run(`Bars count ${Math.round(Number(cfg.count))}`, ['bars', 'count', String(Math.max(1, Math.round(Number(cfg.count))))]);

  onMount(() => {
    alive = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let offVideo: (() => void) | null = null;
    let offDrop: (() => void) | null = null;
    live
      .videoRunning()
      .then((r) => (videoRunning = r))
      .catch(() => {});
    live
      .onVideo(onVideoEvent)
      .then((u) => (alive ? (offVideo = u) : u()))
      .catch(() => {});
    live
      .onFileDrop(onDrop)
      .then((u) => (alive ? (offDrop = u) : u()))
      .catch(() => {});
    void refresh(true);
    // the next read is planned when this one is done, and skipped while a command runs or the window is hidden
    const tick = async (): Promise<void> => {
      if (!busy && !document.hidden) await refresh();
      if (alive) timer = setTimeout(tick, POLL_MS);
    };
    timer = setTimeout(tick, POLL_MS);
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
      offVideo?.();
      offDrop?.();
    };
  });
</script>

<aside class="live">
  <header>
    <div class="title">
      <h2>{show === 'obs' ? 'OBS link' : show === 'bars' ? 'Soundbars' : 'The Caveman, live'}</h2>
      {#if st?.dry}<span class="badge" title="CAVEMAN_LIVE_DRY is set: only the read-only trainer commands run, the others are logged">dry run</span>{/if}
    </div>
    <p class="faint">
      {#if show === 'obs'}OBS's virtual camera, a video or any picture on the ridden board, live: no build, no re-equip. Equip The Caveman deck (Caveman-FX) in the game.{:else if show === 'bars'}Equaliser bars on the deck, moving to the PC's sound.{:else}What is set here goes onto the board in the running game at once: no build, no re-equip.{/if}
    </p>
    <div class="result" class:bad={!busy && last && !last.ok} aria-live="polite">
      {#if busy}
        <span class="spin" aria-hidden="true"></span>
        <span class="line">{busy}...</span>
      {:else if last}
        <span class="dot" class:on={last.ok} class:bad={!last.ok}></span>
        <span class="line" title={last.command}>{last.label}: {last.line}</span>
      {:else}
        <span class="line faint">Each action's result shows here.</span>
      {/if}
    </div>
  </header>

  <div class="scroll">
    <section>
      <h3>Game</h3>
      {#if !st}
        <p class="dim">Reading the game's state...</p>
      {:else if st.trainer_error}
        <p class="err">{st.trainer_error}</p>
      {:else}
        <div class="row">
          <span class="dot" class:on={injected}></span>
          <span>{injected ? `FX DLL in the game (pid ${st.dll?.pid ?? '?'}, ${st.dll?.phase ?? '?'})` : st.game ? 'Game running; the FX DLL is not in it' : 'The game is not running'}</span>
          <div class="grow"></div>
          {#if injected}
            <button class="ghost small" disabled={!!busy} onclick={unload} title="Put the game's getter back and free the DLL">Unload</button>
          {:else if st.game}
            <button class="small" disabled={!!busy} onclick={inject} title="Load caveman_fx.dll into Skate.exe (after its checks)">Inject</button>
          {/if}
        </div>
        {#if !injected && st.game && st.error}
          <p class="hint faint mono">{st.error}</p>
        {/if}
        {#if injected}
          <div class="row">
            <span class="dot" class:on={board?.found === true && equipped === 'yes'} class:half={board?.found === true && equipped !== 'yes'}></span>
            <span title={board?.equipped ?? ''}>{cavemanText}</span>
          </div>
          <div class="row">
            <span class="label">On the board</span>
            <span class="mono on-board" title={board?.art ?? ''}>{board ? (board.art ?? "The Caveman's own") : '...'}</span>
          </div>
          {#if board?.last_error}<p class="err">{board.last_error}</p>{/if}
          {#if st.board_error}<p class="err">{st.board_error}</p>{/if}
        {/if}
      {/if}
    </section>

    {#if has('obs')}
    <section>
      <h3>Deck from the library</h3>
      {#if deck}
        <div class="deck-card">
          <span class="thumb">
            <img src={fileUrl(deck.preview ?? deck.source, deck.modified)} alt="" />
          </span>
          <div class="deck-side">
            <span class="name" title={deck.source}>{deck.display}</span>
            <span class="faint small-text">{deck.group === '' ? 'Top folder' : deck.group}{deckOnBoard ? '; on the board now' : ''}</span>
            {#if deckClip}
              <span class="faint small-text">An animated deck: it plays on the board as video (Stop is under Video).</span>
              <button class="primary" disabled={locked || videoRunning} onclick={playDeck} title={deck.source}>Play it on the board</button>
              {#if deckClip === 'gif'}
                <button class="ghost small" disabled={locked} onclick={deckFrame} title="The GIF's first frame as a still picture">First frame only</button>
              {/if}
            {:else}
              <button class="primary" disabled={locked} onclick={rideDeck} title={deck.source}>Put on the board</button>
            {/if}
            <button class="ghost small" disabled={locked} onclick={plain} title="The Caveman's own plain white, nothing kept">Plain white</button>
          </div>
        </div>
      {:else}
        <p class="dim">Click a deck in the library on the left, then put it on the board here.</p>
      {/if}
    </section>

    <section>
      <h3>Any picture</h3>
      <div class="dropzone" class:over={dragging}>
        <span>Drop a PNG or JPG anywhere on this window</span>
        <span class="faint small-text">or</span>
        <button class="small" disabled={locked} onclick={() => picker?.click()}>Choose a file...</button>
      </div>
      <input
        class="path mono"
        type="text"
        bind:value={picturePath}
        placeholder="C:\path\to\picture.png"
        spellcheck="false"
        aria-label="A picture's path"
        onkeydown={(e) => e.key === 'Enter' && !locked && putPath()}
      />
      <div class="row">
        <button class="primary" disabled={locked || !picturePath.trim()} onclick={putPath}>Put on the board</button>
      </div>
      <div class="colour">
        <label class="check">
          <input type="checkbox" bind:checked={useColour} />
          <span>A colour behind see-through parts</span>
        </label>
        <input type="color" bind:value={pictureColour} disabled={!useColour} aria-label="The colour behind see-through parts" />
      </div>
      <p class="hint faint">Fitted as Caveman fits deck art (a wide picture is turned); without a colour the natural wood shows through. JPG, WebP and BMP are made PNGs in the app's cache first.</p>
      <input type="file" accept=".png,.jpg,.jpeg,.webp,.bmp" hidden bind:this={picker} onchange={picked} />
    </section>

    <section>
      <h3>Finish</h3>
      <div class="segmented wrap">
        {#each FINISHES as f (f.id)}
          <button class:on={finish === f.id} disabled={locked} onclick={() => applyFinish(f.id)}>{f.label}</button>
        {/each}
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={spot} disabled={!artForFinish} />
        <span>Only on the light parts of the art (spot)</span>
      </label>
      <div class="slider">
        <span class="label">Emboss</span>
        <input type="range" min="0" max="20" step="0.5" bind:value={emboss} disabled={!artForFinish} />
        <span class="value mono">{emboss > 0 ? emboss.toFixed(1) : 'off'}</span>
      </div>
      <p class="hint faint">
        Spot and emboss read {artForFinish ? `"${artForFinish.label}"` : 'the picture on the board (put one first)'}; apply a finish again after changing them.
      </p>
    </section>

    <section>
      <h3>Holo</h3>
      <div class="segmented wrap">
        {#each PATTERNS as p (p)}
          <button class:on={pattern === p} disabled={locked} onclick={() => setPattern(p)}>{p}</button>
        {/each}
      </div>
      <div class="colour">
        <label class="check">
          <input type="color" bind:value={holoColour} onchange={setHoloColour} disabled={locked} />
          <span>Holo colour</span>
        </label>
        <button class="ghost small" disabled={locked} onclick={setHoloColour}>Apply</button>
      </div>
      <div class="slider">
        <span class="label">Amount</span>
        <input type="range" min="0" max="1" step="0.05" bind:value={iridescence} onchange={setIridescence} disabled={locked} />
        <span class="value mono">{iridescence.toFixed(2)}</span>
      </div>
      <div class="slider">
        <span class="label">Strength</span>
        <input type="range" min="0" max="1" step="0.05" bind:value={strength} />
        <span class="value mono">{strength.toFixed(2)}</span>
      </div>
      <p class="hint faint">The pattern, colour and amount are live material values. Strength scales the holo map and goes in with the next Holo finish.</p>
    </section>

    <section>
      <h3>Tint</h3>
      <div class="colour">
        <label class="check">
          <input type="color" bind:value={tint} onchange={setTint} disabled={locked} />
          <span>Graphic tint</span>
        </label>
        <button class="ghost small" disabled={locked} onclick={setTint}>Apply</button>
        <button class="ghost small" disabled={locked} onclick={neutral}>Neutral</button>
      </div>
    </section>

    <section>
      <h3>Video</h3>
      <div class="segmented">
        <button class:on={videoSource === 'obs'} disabled={videoRunning} onclick={() => pickSource('obs')}>OBS virtual camera</button>
        <button class:on={videoSource === 'file'} disabled={videoRunning} onclick={() => pickSource('file')}>Video file</button>
      </div>
      {#if videoSource === 'file'}
        <input class="path mono" type="text" bind:value={videoFile} onchange={saveObs} placeholder="C:\path\to\clip.mp4 (or drop one on the window)" spellcheck="false" disabled={videoRunning} aria-label="A video file's path" />
      {/if}
      <div class="row options">
        <label>
          <span class="label">Rotate</span>
          <select bind:value={rotate} onchange={saveObs} disabled={videoRunning}>
            <option value="none">as is</option>
            <option value="cw">90° clockwise</option>
            <option value="ccw">90° counter-clockwise</option>
          </select>
        </label>
        <label>
          <span class="label">Fit</span>
          <select bind:value={fit} onchange={saveObs} disabled={videoRunning}>
            <option value="cover">cover (crop)</option>
            <option value="contain">contain (bars)</option>
          </select>
        </label>
      </div>
      <div class="row">
        {#if videoRunning}
          <button class="primary" disabled={stopping} onclick={stopVideo}>{stopping ? 'Stopping...' : 'Stop'}</button>
        {:else}
          <button class="primary" disabled={locked} onclick={startVideo}>Play on the board</button>
        {/if}
        <span class="video-line mono faint" title={videoLine}>{videoLine}</span>
      </div>
      {#each videoErrors as line, i (i)}
        <p class="err mono">{line}</p>
      {/each}
      <p class="hint faint">OBS: set the profile's output to 512 x 2048, start its virtual camera, and the scene plays on the board at OBS's frame rate. Needs ffmpeg on PATH. Stop puts the kept art back.</p>
    </section>
    {/if}


    {#if has('bars')}
    <section>
      <h3>Bars</h3>
      <div class="row">
        <span class="dot" class:on={cfg.enabled === true}></span>
        <span>Equaliser bars on the deck</span>
        <div class="grow"></div>
        <button class="small" class:on={cfg.enabled === true} disabled={locked} onclick={toggleBars}>{cfg.enabled ? 'On' : 'Off'}</button>
      </div>
      <div class="field">
        <span class="label">Source</span>
        <div class="segmented">
          <button class:on={cfg.source === 'audio'} disabled={locked} onclick={() => barsWord('source', 'audio')} title="The PC's sound">audio</button>
          <button class:on={cfg.source === 'random'} disabled={locked} onclick={() => barsWord('source', 'random')}>random</button>
        </div>
      </div>
      <div class="field">
        <span class="label">Track</span>
        <div class="segmented">
          <button class:on={cfg.track === 'band'} disabled={locked} onclick={() => barsWord('track', 'band')} title="Each band's own recent peak: every bar shows its own dynamics">each band</button>
          <button class:on={cfg.track === 'all'} disabled={locked} onclick={() => barsWord('track', 'all')} title="The loudest band">the loudest</button>
        </div>
      </div>
      <div class="slider" title="The bands' window under their reference: narrower spreads the bars out">
        <span class="label">Window</span>
        <input type="range" min="6" max="60" step="1" bind:value={cfg.window} disabled={locked} onchange={() => barsSet('window')} />
        <span class="value mono">{num(cfg.window)} dB</span>
      </div>
      <div class="slider" title="Levels under it show as nothing, the rest stretches up: the bars fall between beats">
        <span class="label">Floor</span>
        <input type="range" min="0" max="0.95" step="0.01" bind:value={cfg.floor} disabled={locked} onchange={() => barsSet('floor')} />
        <span class="value mono">{Number(cfg.floor).toFixed(2)}</span>
      </div>
      <div class="slider" title="How fast a reference falls after a peak">
        <span class="label">Release</span>
        <input type="range" min="1" max="40" step="0.5" bind:value={cfg.release} disabled={locked} onchange={() => barsSet('release')} />
        <span class="value mono">{num(cfg.release)} dB/s</span>
      </div>
      <details>
        <summary class="dim">More</summary>
        <div class="slider" title="The meter's attack">
          <span class="label">Rise</span>
          <input type="range" min="5" max="300" step="5" bind:value={cfg.rise} disabled={locked} onchange={() => barsSet('rise')} />
          <span class="value mono">{num(cfg.rise)} ms</span>
        </div>
        <div class="slider" title="The meter's decay">
          <span class="label">Fall</span>
          <input type="range" min="20" max="600" step="10" bind:value={cfg.fall} disabled={locked} onchange={() => barsSet('fall')} />
          <span class="value mono">{num(cfg.fall)} ms</span>
        </div>
        <div class="slider" title="How far the levels push the bars">
          <span class="label">Gain</span>
          <input type="range" min="0" max="4" step="0.05" bind:value={cfg.gain} disabled={locked} onchange={() => barsSet('gain')} />
          <span class="value mono">{Number(cfg.gain).toFixed(2)}</span>
        </div>
        <div class="colour">
          <label class="check">
            <input type="color" bind:value={cfg.colour} onchange={barsColour} disabled={locked} />
            <span>Colour</span>
          </label>
          <label class="count">
            <span class="label">Count</span>
            <input class="mono" type="number" min="1" max="512" step="1" bind:value={cfg.count} onchange={barsCount} disabled={locked} />
          </label>
        </div>
      </details>
      {#if st?.audio?.status}<p class="hint faint">Audio: {st.audio.status}</p>{/if}
    </section>
    {/if}

    {#if log.length > 0}
      <section>
        <h3>Log</h3>
        <ul class="log">
          {#each log as e (e.id)}
            <li class:bad={!e.ok}>
              <span><span class="faint mono">{e.at}</span> {e.label}: {e.line}</span>
              {#if e.command}<code class="mono faint">{e.command}</code>{/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>

  {#if dragging}
    <div class="drop-overlay" aria-hidden="true">
      <div>
        <strong>
          {dragKind === 'picture'
            ? locked
              ? 'Drop to fill in the picture path'
              : 'Drop to put it on the board'
            : dragKind === 'video'
              ? 'Drop to set it as the video file'
              : 'Not a picture or a video'}
        </strong>
        <span class="mono faint">{live.baseName(dragPath)}</span>
      </div>
    </div>
  {/if}
</aside>

<style>
  .live {
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
  }
  header {
    padding: 14px 16px 10px;
    border-bottom: 1px solid var(--line);
  }
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 4px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  .badge {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 999px;
    background: rgba(243, 198, 75, 0.15);
    color: var(--warn);
  }
  header p {
    margin: 0;
    font-size: 12px;
  }
  .result {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
    padding: 6px 10px;
    min-height: 32px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
    font-size: 12px;
  }
  .result.bad {
    border-color: rgba(255, 107, 107, 0.5);
  }
  .result .line {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spin {
    flex: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
      border-color: var(--accent);
    }
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
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
  }
  .grow {
    flex: 1;
  }
  .small {
    font-size: 12px;
    padding: 4px 8px;
  }
  .small-text {
    font-size: 12px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--text-faint);
    flex: none;
  }
  .dot.on {
    background: var(--good);
  }
  .dot.half {
    background: var(--warn);
  }
  .dot.bad {
    background: var(--bad);
  }
  .label {
    font-size: 12px;
    color: var(--text-dim);
  }
  .on-board {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .segmented button.on,
  button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .field {
    display: grid;
    grid-template-columns: 64px 1fr;
    align-items: center;
    gap: 10px;
    margin: 8px 0;
  }
  .field .segmented button {
    padding: 4px 8px;
    font-size: 12px;
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
    grid-template-columns: 64px 1fr 64px;
    align-items: center;
    gap: 10px;
    margin: 6px 0 2px;
  }
  .slider .label {
    margin: 0;
  }
  .value {
    text-align: right;
    color: var(--text-dim);
    white-space: nowrap;
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
  input[type='color'] {
    flex: none;
    width: 34px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--raised);
  }
  input[type='color']:disabled {
    opacity: 0.45;
  }
  .count {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .count input {
    width: 70px;
    padding: 4px 6px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  details {
    margin-top: 8px;
  }
  summary {
    cursor: pointer;
    font-size: 12px;
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12px;
  }
  .deck-card {
    display: flex;
    gap: 12px;
    align-items: stretch;
  }
  .thumb {
    flex: none;
    width: 44px;
    height: 152px;
    border-radius: 22px;
    overflow: hidden;
    background: #0c0b0f;
    border: 1px solid var(--line);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .deck-side {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    min-width: 0;
  }
  .deck-side .name {
    max-width: 100%;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .deck-side .primary {
    margin-top: auto;
  }
  .dropzone {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 6px 8px;
    padding: 12px;
    border: 1px dashed var(--line-strong);
    border-radius: var(--radius-sm);
    text-align: center;
    font-size: 12px;
    color: var(--text-dim);
  }
  .dropzone.over {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--text);
  }
  .drop-overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 24px;
    background: rgba(18, 16, 22, 0.94);
    border: 2px dashed var(--accent);
    pointer-events: none;
    z-index: 5;
  }
  .drop-overlay div {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    padding: 16px 20px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--raised);
    box-shadow: var(--shadow);
    text-align: center;
    overflow-wrap: anywhere;
  }
  .path {
    width: 100%;
    margin: 8px 0 0;
    padding: 6px 8px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  .options label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    min-width: 0;
  }
  select {
    padding: 5px 8px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--panel-2);
  }
  .video-line {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .err {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--warn);
    overflow-wrap: anywhere;
  }
  .err.mono {
    font-size: 11px;
  }
  .log {
    font-size: 12px;
    max-height: 240px;
    overflow-y: auto;
  }
  .log li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 0;
    border-bottom: 1px solid var(--line);
    overflow-wrap: anywhere;
  }
  .log li.bad > span {
    color: var(--warn);
  }
  .log code {
    font-size: 11px;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
