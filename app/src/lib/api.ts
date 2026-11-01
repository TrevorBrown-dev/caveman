// The back end: Tauri commands in the app window, or a stand-in in a plain browser (`pnpm dev:mock`, for reviewing
// the UI; see vite.config.ts).

import { convertFileSrc, invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { BuildEvent, BuildResult, Config, Fitted, LauncherStatus, Library, PackEvent, PackResult, PackSpec, Settings } from './types';

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const isMock = !inTauri;

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(command, args);
}

/** A URL the webview can load for a file on disk. */
export function fileUrl(path: string, version = 0): string {
  if (inTauri) return `${convertFileSrc(path)}?v=${version}`;
  return `/__mock/file?path=${encodeURIComponent(path)}&v=${version}`;
}

// ---- the stand-in back end
interface MockFixture {
  library: Library;
  config: Config;
  fitted: Record<string, Fitted>;
  roots: string[];
}

let fixture: MockFixture | null = null;
const mockSettings = new Map<string, Settings | null>();
const mockOff = new Set<string>();

async function mock(): Promise<MockFixture> {
  if (!fixture) {
    const response = await fetch('/__mock/library');
    fixture = (await response.json()) as MockFixture;
    // older fixtures lack the newer fields
    const defaults: Partial<Config> = { author: '', attach: true, ringMb: 12288, plugins: {}, trainer: '' };
    fixture.config = { ...defaults, ...fixture.config } as Config;
  }
  return fixture;
}

function wait(ms: number): Promise<void> {
  return new Promise((done) => setTimeout(done, ms));
}

// ---- the API
export async function loadConfig(): Promise<Config> {
  if (inTauri) return invoke<Config>('load_config');
  return (await mock()).config;
}

export async function saveConfig(config: Config): Promise<Config> {
  if (inTauri) return invoke<Config>('save_config', { config });
  (await mock()).config = config;
  return config;
}

export async function library(): Promise<Library> {
  if (inTauri) return invoke<Library>('library');
  const lib = structuredClone((await mock()).library);
  for (const item of [...lib.decks, ...lib.wheels]) {
    item.enabled = !mockOff.has(item.source);
    if (item.kind === 'deck' && mockSettings.has(item.source)) {
      const s = mockSettings.get(item.source);
      item.sidecar = s !== null;
      if (s) item.settings = s;
    }
  }
  return lib;
}

export async function saveSettings(source: string, settings: Settings): Promise<string> {
  if (inTauri) return invoke<string>('save_settings', { source, settings });
  mockSettings.set(source, structuredClone(settings));
  return source.replace(/\.png$/i, '.caveman.json');
}

export async function clearSettings(source: string): Promise<boolean> {
  if (inTauri) return invoke<boolean>('clear_settings', { source });
  mockSettings.set(source, null);
  return true;
}

/** Switches a deck or wheel off or on for the build. */
export async function setItemOff(kind: 'deck' | 'wheel', source: string, off: boolean): Promise<boolean> {
  if (inTauri) return invoke<boolean>('set_item_off', { kind, source, off });
  if (off) mockOff.add(source);
  else mockOff.delete(source);
  return true;
}

export async function fittedArt(source: string): Promise<Fitted> {
  if (inTauri) return invoke<Fitted>('fitted_art', { source });
  const f = (await mock()).fitted[source];
  return f ?? { path: source, notes: [] };
}

/** A file's bytes: a PNG or the board model (.glb); nothing else is served. */
export async function readBytes(path: string): Promise<ArrayBuffer> {
  if (inTauri) return invoke<ArrayBuffer>('read_bytes', { path });
  const response = await fetch(fileUrl(path));
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.arrayBuffer();
}

/**
 * The game's skateboard as a glTF binary (the stock deck, trucks and wheels on the board rig, with their textures),
 * built from the install once and cached: its path. `force` builds it again.
 */
export async function boardModel(force = false): Promise<string> {
  if (inTauri) return invoke<string>('board_model', { force });
  throw new Error('the board model is built from the game, which the stand-in back end has not');
}

/** What is on the board in the game (`board_art`): the DLL's label for the picture and the fitted PNG for it. */
export interface BoardArt {
  /** The DLL answered; else the label and the path are nothing. */
  injected: boolean;
  label: string | null;
  path: string | null;
  how: string;
}

/** The picture on the board in the game, for the board cam window; `force` reads it afresh however old. */
export async function boardArt(force = false): Promise<BoardArt> {
  if (inTauri) return invoke<BoardArt>('board_art', { force });
  return { injected: false, label: null, path: null, how: 'the stand-in' };
}

/** A frame of the OBS link's video on the board (`board_frame`): RGBA8 rows, the top row first. */
export interface BoardFrame {
  seq: number;
  width: number;
  height: number;
  level: number;
  rgba: Uint8Array;
}

/** The newest frame on the board when it is not frame `after`, at level `level` of the ring's chain (0 full size). */
export async function boardFrame(after: number, level: number): Promise<BoardFrame | null> {
  if (!inTauri) return null;
  const raw = await invoke<ArrayBuffer | Uint8Array>('board_frame', { after, level });
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
  if (bytes.byteLength < 24) return null;
  const head = new DataView(bytes.buffer, bytes.byteOffset, 24);
  const seq = Number(head.getBigUint64(0, true));
  const width = head.getUint32(8, true);
  const height = head.getUint32(12, true);
  const level_ = head.getUint32(16, true);
  if (bytes.byteLength !== 24 + width * height * 4) return null;
  return { seq, width, height, level: level_, rgba: bytes.subarray(24) };
}

/** A PNG on disk as an ImageBitmap whose pixels can be read. */
export async function imageBitmap(path: string): Promise<ImageBitmap> {
  const bytes = await readBytes(path);
  const blob = new Blob([bytes], { type: 'image/png' });
  return createImageBitmap(blob);
}

export async function gameRunning(): Promise<boolean> {
  if (inTauri) return invoke<boolean>('game_running');
  return false;
}

export async function build(force: boolean, onEvent: (e: BuildEvent) => void): Promise<BuildResult> {
  if (inTauri) {
    const stop = await listen<BuildEvent>('build-event', (e) => onEvent(e.payload));
    try {
      return await invoke<BuildResult>('build', { force });
    } finally {
      stop();
    }
  }
  // The stand-in: the events a build of the fixture's decks would send.
  const lib = await library();
  const start = performance.now();
  const on = lib.decks.filter((d) => d.enabled);
  onEvent({ event: 'scanned', kind: 'deck', count: lib.decks.length });
  for (const deck of on) {
    await wait(60);
    onEvent({ event: 'item', kind: 'deck', display: deck.display, notes: deck.notes });
  }
  for (const deck of on) {
    await wait(40);
    onEvent({ event: 'log', message: `  built ${deck.display} (${deck.key})` });
  }
  onEvent({ event: 'built', decks: on.length, wheels: lib.wheels.length });
  return {
    outcome: 'built',
    decks: on.length,
    wheels: lib.wheels.length,
    shaders: 'built',
    seconds: (performance.now() - start) / 1000,
    modsDir: '(mock)',
  };
}

/** Starts ReSkate's launcher; the attach command line when the FX DLL goes in after it. */
export async function launch(): Promise<string | null> {
  if (inTauri) return invoke<string | null>('launch');
  return null;
}

/** Puts the FX DLL into the running game now. */
export async function attachDll(): Promise<string> {
  if (inTauri) return invoke<string>('attach_dll');
  return 'mock: not run';
}

export async function reveal(path: string): Promise<void> {
  if (inTauri) return invoke<void>('reveal', { path });
}

export async function launcherStatus(): Promise<LauncherStatus> {
  if (inTauri) return invoke<LauncherStatus>('launcher_status');
  const lib = await library();
  return {
    game: (await mock()).config.game,
    gameOk: true,
    skateBuild: 'match',
    reskateVersion: '2.1.3',
    dllInstalled: true,
    trainer: '(mock)',
    modsBuilt: 'up_to_date',
    decks: lib.decks.length,
    decksOn: lib.decks.filter((d) => d.enabled).length,
    wheels: lib.wheels.length,
    wheelsOn: lib.wheels.filter((w) => w.enabled).length,
  };
}

/** Writes the deck and wheel templates to a folder beside the game; returns the folder. */
export async function templatesSave(): Promise<string> {
  if (inTauri) return invoke<string>('templates_save');
  return 'C:\\(mock)\\Caveman\\Templates';
}

/** Builds a Thunderstore-ready package from chosen decks and wheels. */
export async function packBuild(spec: PackSpec, onEvent: (e: PackEvent) => void): Promise<PackResult> {
  if (inTauri) {
    const stop = await listen<PackEvent>('pack-event', (e) => onEvent(e.payload));
    try {
      return await invoke<PackResult>('pack_build', { spec });
    } finally {
      stop();
    }
  }
  for (const i of spec.items) {
    await wait(50);
    onEvent({ event: 'item', display: i.source.split(/[\\/]/).pop() });
  }
  const folder = `${spec.out ?? 'C:\\(mock)\\Caveman Packs'}\\${spec.owner}-${spec.name}`;
  return { folder, zip: `${folder}-${spec.version}.zip`, zipBytes: 1234567, decks: spec.items.filter((i) => i.kind === 'deck').length, wheels: spec.items.filter((i) => i.kind === 'wheel').length, shaderDecks: [] };
}

/** The app's version (tauri.conf.json). */
export async function appVersion(): Promise<string> {
  if (!inTauri) return '0.2.0 (mock)';
  const { getVersion } = await import('@tauri-apps/api/app');
  return getVersion();
}
