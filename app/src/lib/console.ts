// The console's command tree: every `caveman-trainer` command (its USAGE texts in crates/caveman-trainer/src), for
// completion and hints. A word in angle brackets is a value the user types (a file, a number, a colour); `flags` are
// options a command takes anywhere after its words.

export interface Node {
  /** The word, or `<VALUE>` for something typed. */
  word: string;
  hint?: string;
  children?: Node[];
  flags?: string[];
  /** Runs until stopped or for a time: the console caps a command at 90 s. */
  long?: boolean;
}

const onOff: Node[] = [{ word: 'on' }, { word: 'off' }];
const value = (word: string, hint?: string, children?: Node[]): Node => ({ word, hint, children });

const BARS_KEYS = [
  'window', 'track', 'floor', 'release', 'rise', 'fall', 'length', 'base', 'width', 'glow', 'span', 'edge', 'height', 'shift', 'axis', 'facing', 'mirror',
  'land', 'land_ms', 'brightness', 'hdr', 'tempo', 'style', 'outline', 'timing', 'depth_bias', 'layout', 'tip_colour', 'accent_colour', 'soft', 'deck_width',
  'deck_length', 'deck_end', 'rim_out', 'rim_lift', 'count', 'gain', 'colour', 'source',
];

export const TREE: Node[] = [
  { word: 'status', hint: "the DLL's state: phase, hook, scan, skater, states, fx states, log", flags: ['--json'] },
  { word: 'inject', hint: 'load caveman_fx.dll into the running game (without --yes: only the checks)', flags: ['--yes', '--dll', '--pid'] },
  { word: 'unload', hint: "put the game's getter back, stop the DLL's threads and free it" },
  { word: 'watch', hint: 'print skater-state and fx-state changes as they happen (SECONDS, default 60)', children: [value('<SECONDS>')], long: true },
  { word: 'fire', hint: 'force an fx state on: fire fx.test 5000', children: [value('<FX>', 'fx.test, fx.kickflip, fx.cave.aura ...', [value('<MS>')])] },
  { word: 'stop', hint: 'end an override (all without FX)', children: [value('<FX>')] },
  {
    word: 'rules',
    hint: "the DLL's rules (which states fire which fx)",
    children: [value('load', 'load rules from a file', [value('<FILE>')]), value('get', 'print the loaded rules'), value('check', 'check a file offline', [value('<FILE>')]), value('default', 'print the defaults')],
  },
  { word: 'rescan', hint: 'look for the assets again' },
  { word: 'probe', hint: "read-only, no injection: the DLL's checks, scan and skater reads from outside, then watch for SECONDS", children: [value('<SECONDS>')], flags: ['--pid'], long: true },
  { word: 'probe-scene', hint: 'read-only: the camera and the board from outside, four times a second for SECONDS', children: [value('<SECONDS>')], flags: ['--pid'], long: true },
  { word: 'pose', hint: 'read-only: the camera and the board as JSON lines (the board cam window feeds on it); give --seconds here', flags: ['--rate', '--seconds', '--pid'], long: true },
  { word: 'attach', hint: 'wait for the game, inject the DLL, switch the staging ring on (what Caveman does at launch)', flags: ['--ring', '--timeout'], long: true },
  { word: 'probe-timing', hint: "read-only: the renderer's per-frame transforms against the presents", children: [value('<SECONDS>')], flags: ['--wait-moving', '--copies', '--tol', '--wc', '--pid'], long: true },
  { word: 'probe-projection', hint: 'read-only: the projection matrices', flags: ['--pid'], long: true },
  {
    word: 'draw',
    hint: 'the overlay',
    children: [
      value('test', "the overlay's test rectangle", onOff),
      value('status', "the overlay's state"),
      value('reset', 're-arm drawing after failures'),
      value('queue', 'force a queue strategy', [value('auto'), value('swapchain'), value('scan'), value('busiest')]),
      value('streamline', "also hook Streamline's swap chain wrapper", onOff),
      value('depth', 'the depth test (hiding the bars behind the scene)', [...onOff, value('near', 'the near plane in metres', [value('<M>')]), value('status'), value('copies', 'which pass ends are copied', [value('picture'), value('window')])]),
      value('log', "the depth test's record", [value('dump', 'as JSON to a file', [value('<FILE>')]), value('key', 'the key that saves the moments around a blink', [value('<KEY>')]), value('auto', 'save around each blink by itself', onOff), value('status')]),
    ],
  },
  {
    word: 'bars',
    hint: 'the equaliser bars on the deck (no words: the settings and the state)',
    children: [
      ...onOff,
      value('colour', 'RRGGBB', [value('<RRGGBB>')]),
      value('count', 'bars per side', [value('<N>')]),
      value('gain', 'how far the levels push them', [value('<X>')]),
      value('source', "the levels' source", [value('random'), value('audio')]),
      value('land', 'a landing spike by hand'),
      value('set', 'any setting: bars set KEY VALUE', BARS_KEYS.map((k) => value(k, undefined, [value('<VALUE>')]))),
      value('reset', 'every setting back to its default'),
      value('meter', "no game needed: capture the PC's audio and print the bands for SECONDS", [value('<SECONDS>')]),
      value('rim', 'the deck rim the DLL has', [value('status'), value('load', 'read the stock deck edge from the install and send it'), value('show', 'the rim as read from the install'), value('clear')]),
    ],
  },
  {
    word: 'board',
    hint: 'The Caveman deck in the game (no words: its status)',
    children: [
      value('status', 'found? equipped? the art on it'),
      value('art', 'a picture on The Caveman: board art FILE.png', [value('<FILE.png>')]),
      value('deck', 'a deck of the library: a path or part of a file name', [value('<PATH|NAME>')]),
      value('reset', 'plain white again, nothing kept'),
      value('finish', "the deck's finish, live", [value('gloss'), value('matte'), value('foil'), value('chrome'), value('glitter'), value('holo'), value('holo-cellular'), value('holo-frost'), value('holo-vertical'), value('holo-glitter'), value('none')]),
      value('video', 'video on the in-world board (the OBS link plugin page runs this as a managed child)', [value('obs'), value('<FILE>'), value('stop'), value('off'), value('status')]),
      value('params', "the ridden board's live material parameters"),
      value('param', 'write one parameter: board param KEY VALUE', [value('<KEY>', undefined, [value('<VALUE>')])]),
      value('tint', "the graphic's tint", [value('<RRGGBB>'), value('neutral')]),
      value('holotint', "the holo's colour", [value('<RRGGBB>')]),
      value('foil', 'the holo pattern, live', [value('cellular'), value('frost'), value('pearl'), value('glitter'), value('vertical')]),
      value('fx', 'its effects', [value('list'), value('<NAME>', 'fx.cave.<name> without the prefix', [...onOff, value('pulse', 'on for MS', [value('<MS>')])])]),
      value('state', "the texture's resource state by hand", [value('auto'), value('common'), value('srv'), value('ps'), value('nps'), value('generic'), value('<0xNN>')]),
      value('composite-common', undefined, onOff),
      value('find', "look for The Caveman's texture again now"),
      value('siblings', undefined, onOff),
      value('finals', undefined, onOff),
      value('worldcopy', undefined, onOff),
      value('ridden', 'also write the ridden copy (the texture the in-world board samples)', onOff),
      value('fill', 'a plain colour on every target', [value('<RRGGBB>')]),
      value('peek', 'read a block of texels back'),
      value('target', 'a target by hand, or clear', [value('<0xADDR>'), value('clear')]),
      value('hunt', 'find a colour among the textures', [value('<RRGGBB>')]),
      value('slices', 'the tile arrays', [value('match')]),
      value('tiles', undefined, [value('clear')]),
      value('uploads', 'the uploads ring'),
      value('copies', 'the copies the game recorded lately'),
      value('verify', 'read a block back and compare it with what was written'),
      value('probe', "read-only: find The Caveman's texture from outside"),
    ],
    flags: ['--json', '--colour', '--no-keep', '--game', '--spot', '--emboss', '--art', '--strength', '--rotate', '--fit', '--fps', '--size', '--once', '--alpha', '--premultiply', '--resource', '--kind', '--force', '--state', '--at', '--mip', '--n', '--int', '--raw', '--no-publish', '--gain', '--pid', '--deck', '--all', '--owners'],
  },
  {
    word: 'pip',
    hint: 'the old in-game board cam (drawn by the DLL; the board cam window is the way now)',
    children: [
      ...onOff,
      value('load', 'read the stock meshes from the install and send them'),
      value('status'),
      value('size', 'fraction of the screen height', [value('<F>')]),
      value('margin', undefined, [value('<F>')]),
      value('corner', undefined, [value('tl'), value('tr'), value('bl'), value('br')]),
      value('view', undefined, [value('camera'), value('world')]),
      value('elevation', 'degrees', [value('<DEG>')]),
      value('azimuth', 'degrees', [value('<DEG>')]),
      value('fov', 'degrees', [value('<DEG>')]),
      value('zoom', undefined, [value('<X>')]),
      value('art', undefined, onOff),
      value('mirror', undefined, onOff),
      value('flip', undefined, onOff),
      value('backdrop', '0 none .. 1', [value('<A>')]),
    ],
    flags: ['--game'],
  },
  {
    word: 'mem',
    hint: 'GPU memory: the allocation trace and the staging ring',
    children: [
      value('trace', "record every GPU allocation of the game's for SECONDS (30), then the report (blocks meanwhile)", [value('<SECONDS>')]),
      value('status', 'what the trace is doing'),
      value('stop', 'end a trace early'),
      value('report', 'the report of the last trace'),
      value('ring', "the staging ring (the skater build's memory spike): on, off, or no word for its counters", [value('on'), value('off')]),
    ],
    flags: ['--min', '--mb'],
  },
];

/** Whether a typed token is taken by a placeholder word. */
function matches(node: Node, token: string): boolean {
  return node.word.startsWith('<') || node.word.toLowerCase() === token.toLowerCase();
}

export interface Completion {
  /** The words that fit the token being typed. */
  options: { word: string; hint?: string; flag?: boolean }[];
  /** The hint of the deepest command matched so far. */
  hint: string;
  /** The command's words so far are a complete command. */
  complete: boolean;
  long: boolean;
}

/** What could come next for a command line (`tokens`: the words typed, the last one possibly unfinished). */
export function complete(tokens: string[], trailingSpace: boolean): Completion {
  let level: Node[] = TREE;
  let flags: string[] = [];
  let hint = 'caveman-trainer <command>: Tab completes, Enter runs, Up and Down recall';
  let long = false;
  const done = trailingSpace ? tokens : tokens.slice(0, -1);
  const typing = trailingSpace ? '' : (tokens[tokens.length - 1] ?? '');
  for (const token of done) {
    if (token.startsWith('--')) continue;
    const node = level.find((n) => !n.word.startsWith('<') && matches(n, token)) ?? level.find((n) => matches(n, token));
    if (!node) {
      level = [];
      break;
    }
    if (node.hint) hint = node.hint;
    if (node.flags) flags = node.flags;
    if (node.long) long = true;
    level = node.children ?? [];
  }
  const lower = typing.toLowerCase();
  const options: Completion['options'] = [];
  for (const n of level) {
    if (n.word.startsWith('<')) options.push({ word: n.word, hint: n.hint });
    else if (n.word.toLowerCase().startsWith(lower)) options.push({ word: n.word, hint: n.hint });
  }
  if (typing.startsWith('-') || level.length === 0 || trailingSpace) {
    for (const f of flags) {
      if (f.startsWith(lower) && !done.includes(f)) options.push({ word: f, flag: true });
    }
  }
  return { options, hint, complete: done.length > 0, long };
}

/** A command line split into words: double quotes keep spaces. */
export function split(line: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line)) !== null) out.push(m[1] ?? m[2]);
  return out;
}
