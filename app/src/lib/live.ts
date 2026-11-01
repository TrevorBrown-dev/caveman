// The Live panel's back end: caveman-trainer.exe run by the app (the pipe to the FX DLL in the running game). In a
// plain browser (`pnpm dev:mock`) there is none: a stand-in answers with a made-up state and runs nothing.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { fittedArt } from './api';
import type { Library } from './types';

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** What a trainer command returned. `command` is the command line as typed in a terminal; `dry`: only reported. */
export interface LiveOut {
  ok: boolean;
  code: number | null;
  text: string;
  json: Record<string, unknown> | null;
  command: string;
  dry: boolean;
}

/** One of The Caveman's effects (fx.cave.<name>). */
export interface CaveFx {
  name: string;
  what: string;
  one_shot: boolean;
  cooldown: number;
}

/** The Caveman on the board, from `board status --json` (a few of its fields). */
export interface LiveBoard {
  found: boolean;
  usable: boolean;
  not_usable: string | null;
  /** "yes: ...", "not now: ..." or "unknown: ..." */
  equipped: string;
  active: boolean;
  finding: boolean;
  pending: boolean;
  /** The picture on it (its file name), null for The Caveman's own. */
  art: string | null;
  /** How many in-world copies were found (the textures the ridden board samples). */
  ridden: number | null;
  last_error: string;
  log: string;
}

/** The panel's poll (`live_status`). */
export interface LiveStatus {
  trainer: string | null;
  trainer_error?: string;
  dry: boolean;
  game: boolean;
  injected: boolean;
  error: string;
  dll?: { pid: number; phase: string; reason: string; version: string; build: string; hooked: boolean };
  fx?: { name: string; value: boolean; override_ms: number }[];
  /** The overlay's settings: the bars' and the board cam's (pip_*), as the DLL has them. */
  bars?: Record<string, unknown>;
  audio?: { status: string; running: boolean };
  /** The DLL's board module in brief (from the light status): an object once it is active (DLLs from 2026-10-30 on
   *  say `art`, the label of the picture on the board, null for none), null before, "busy" when its lock was taken. */
  brief?: { found: number; uploads: number; pending: boolean; error: string; art?: string | null } | 'busy' | null;
  board?: LiveBoard;
  board_error?: string;
}

/** A line from the video producer, or its end. */
export interface VideoEvent {
  line?: string;
  stream?: 'out' | 'err';
  done?: boolean;
}

/** Files dragged over the window. */
export type DropEvent = { type: 'enter'; paths: string[] } | { type: 'over' } | { type: 'drop'; paths: string[] } | { type: 'leave' };

/** Pictures the panel puts on the board (the trainer reads PNG; the others are made PNGs first). */
export const PICTURES = ['png', 'jpg', 'jpeg', 'webp', 'bmp'];
/** Videos for `board video FILE` (ffmpeg's; a GIF plays as a video). */
export const VIDEOS = ['mp4', 'mov', 'm4v', 'webm', 'mkv', 'avi', 'gif'];

export function extOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot < 0 ? '' : name.slice(dot + 1).toLowerCase();
}

export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

// ---- the stand-in (a plain browser): a made-up state, nothing run
const MOCK_FX = ['aura', 'nyan', 'impact', 'sparkler', 'fire', 'flametrail', 'ghost', 'smoke', 'splash', 'skull', 'dust'];
const MOCK_STATUS: LiveStatus = {
  trainer: '(mock: nothing runs)',
  dry: true,
  game: true,
  injected: true,
  error: '',
  dll: { pid: 12744, phase: 'active', reason: 'hooked', version: '0.1.0', build: '20260929', hooked: true },
  fx: MOCK_FX.map((n, i) => ({ name: `fx.cave.${n}`, value: i === 1, override_ms: 0 })),
  bars: {
    enabled: true,
    source: 'audio',
    track: 'band',
    window: 24,
    floor: 0.15,
    release: 12,
    rise: 35,
    fall: 90,
    gain: 1,
    colour: '#00e5ff',
    count: 120,
    pip: true,
    pip_size: 0.24,
    pip_corner: 'tr',
    pip_view: 'camera',
    pip_art: true,
    pip_mirror: false,
    pip_flip: false,
  },
  audio: { status: 'capturing the default output (48000 Hz, 2 channels, float)', running: true },
  board: { found: true, usable: true, not_usable: null, equipped: 'yes: the game reads its fx.cave.* states', active: true, finding: false, pending: false, art: 'Biggie Cheese.png', ridden: 3, last_error: '', log: '' },
};

function mockOut(args: string[]): LiveOut {
  const command = ['caveman-trainer', ...args.map((a) => (a.includes(' ') ? `"${a}"` : a))].join(' ');
  return { ok: true, code: 0, text: 'mock: not run', json: null, command, dry: true };
}

// ---- the calls
/** Runs the trainer with `args` and returns what it printed (JSON parsed when it printed JSON). */
export async function live(args: string[]): Promise<LiveOut> {
  if (!inTauri) return mockOut(args);
  return invoke<LiveOut>('live', { args });
}

/** The DLL's state; with `board`, The Caveman's too (a slower read: every few polls and after a change). */
export async function status(board: boolean): Promise<LiveStatus> {
  if (!inTauri) return structuredClone(MOCK_STATUS);
  return invoke<LiveStatus>('live_status', { board });
}

export async function caveEffects(): Promise<CaveFx[]> {
  if (!inTauri) return MOCK_FX.map((name, i) => ({ name, what: '(mock) what the effect does', one_shot: i % 3 === 2, cooldown: 0 }));
  return invoke<CaveFx[]>('cave_effects');
}

/** Starts the video producer: `board video ARGS...` kept running until stopped. Returns its command line. */
export async function videoStart(args: string[]): Promise<string> {
  if (!inTauri) return mockOut(['board', 'video', ...args]).command;
  return invoke<string>('live_video_start', { args });
}

/** Stops the producer and the DLL's side (the kept art returns). What the trainer said. */
export async function videoStop(): Promise<string> {
  if (!inTauri) return 'mock: not run';
  return invoke<string>('live_video_stop');
}

export async function videoRunning(): Promise<boolean> {
  if (!inTauri) return false;
  return invoke<boolean>('live_video_running');
}

/** The producer's lines as they come. */
export async function onVideo(handler: (event: VideoEvent) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<VideoEvent>('live-video', (event) => handler(event.payload));
}

// ---- the board cam window (components/BoardCam.svelte) and its pose stream (`caveman-trainer pose`)

export interface PoseFrame {
  /** Seconds since the stream started. */
  t?: number;
  /** The game camera's world axes and position; `back` points at the viewer. */
  camera?: { pos: number[]; right: number[]; up: number[]; back: number[]; fov: number };
  /** The board's world axes and position, or null when there is no board (menu, loading). */
  board?: { pos: number[]; right: number[]; up: number[]; forward: number[] } | null;
  /**
   * The board rig's 16 bones as the game's animation holds them, each [scale xyz, rotation xyzw, position xyz] in
   * its parent's frame, in the game's order (lane 3 the deck, 4 and 8 the trucks, 6, 7, 10 and 11 the wheels); see
   * caveman_core::boardmodel.
   */
  bones?: [number[], number[], number[]][];
  /** Why the bones could not be read this tick. */
  bones_why?: string;
  /** "ok", or why there is no board. */
  why?: string;
  /** The stream ended (the game went, or the trainer stopped). */
  done?: boolean;
  gone?: boolean;
  error?: boolean;
}

/** Starts the pose stream for this window (one `board-pose` event per tick; the back end keeps one trainer process
 *  for every window and stops it with the last). The command line. */
export async function poseStart(rate = 60): Promise<string> {
  if (!inTauri) return mockOut(['pose', '--rate', String(rate)]).command;
  return invoke<string>('pose_start', { rate });
}

export async function poseStop(): Promise<void> {
  if (!inTauri) return;
  await invoke('pose_stop');
}

export async function onPose(handler: (frame: PoseFrame) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<PoseFrame>('board-pose', (event) => handler(event.payload));
}

/**
 * To the board cam widget: the picture on the board may have changed, look now (`boardcam-check`). The window reads
 * the picture from the back end itself (`boardArt`), so nothing is pushed to it: a window opened late, or one that
 * missed an event while its page loaded, shows the right picture at its next look.
 */
export async function nudgeBoardcam(): Promise<void> {
  if (!inTauri) return;
  const { emit } = await import('@tauri-apps/api/event');
  await emit('boardcam-check', {});
}

export async function onBoardcamNudge(handler: () => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen('boardcam-check', () => handler());
}

/**
 * The picture on the board in the game, as a fitted PNG for the board cam window: the DLL labels it with its file
 * name (`board.art`); that is matched against a picture this app put on (`lastArt`), else a deck of the library by
 * file name, and fitted as the build fits deck art. Null for The Caveman's own plain white, a picture the app cannot
 * find, or a video.
 */
export async function artOnBoard(status: LiveStatus | null, library: Library | null, lastArt: { path: string; label: string } | null): Promise<string | null> {
  const label = status?.board?.art;
  if (!label) return null;
  let source: string | null = null;
  if (lastArt && (lastArt.label === label || baseName(lastArt.path) === label)) source = lastArt.path;
  else {
    const deck = library?.decks.find((d) => baseName(d.source) === label || d.display === label);
    if (deck) source = deck.source;
  }
  // a picture put on the board earlier (the OBS link's pictures, a file dropped on the Live panel): staged by name
  if (!source && inTauri) {
    try {
      source = await invoke<string | null>('live_staged_path', { name: label });
    } catch {
      source = null;
    }
  }
  if (!source || extOf(source) !== 'png') return null;
  try {
    return (await fittedArt(source)).path;
  } catch {
    return null;
  }
}

/** Files dragged onto the window (Tauri's own drag and drop: the webview gets the files' paths). */
export async function onFileDrop(handler: (event: DropEvent) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === 'enter' || p.type === 'drop') handler({ type: p.type, paths: p.paths });
    else handler({ type: p.type });
  });
}

/** A picture file's bytes (PNG, JPG, WebP, BMP). */
export async function readPicture(path: string): Promise<ArrayBuffer> {
  if (!inTauri) throw new Error('no back end (the UI in a browser)');
  return invoke<ArrayBuffer>('live_read_picture', { path });
}

/** Any picture the webview can decode as PNG bytes, its longest side at most `maxSide` (the board is 512 x 2048). */
export async function pngOf(picture: Blob, maxSide = 4096): Promise<Uint8Array> {
  const bitmap = await createImageBitmap(picture);
  try {
    const scale = Math.min(1, maxSide / Math.max(bitmap.width, bitmap.height));
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext('2d');
    if (!context) throw new Error('no 2D canvas');
    context.drawImage(bitmap, 0, 0, width, height);
    const png = await canvas.convertToBlob({ type: 'image/png' });
    return new Uint8Array(await png.arrayBuffer());
  } finally {
    bitmap.close();
  }
}

/** PNG bytes written to the app's cache as `<name>.png` (the name the board's status shows); returns the path. */
export async function stagePng(name: string, png: Uint8Array): Promise<string> {
  if (!inTauri) throw new Error('no back end (the UI in a browser)');
  const stem = name.replace(/\.[^.\\/]+$/, '');
  // a header value is ASCII: the back end cleans the name again
  const safe = stem.replace(/[^A-Za-z0-9 ._()-]+/g, '_').slice(0, 80) || 'picture';
  return invoke<string>('live_stage_png', png, { headers: { 'x-name': safe } });
}
