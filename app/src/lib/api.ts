// The back end: Tauri commands in the app window, or a stand-in in a plain browser (`pnpm dev:mock`, for reviewing
// the UI; see vite.config.ts).

import { convertFileSrc, invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { BuildEvent, BuildResult, Config, Fitted, Library, Settings } from './types';

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

async function mock(): Promise<MockFixture> {
  if (!fixture) {
    const response = await fetch('/__mock/library');
    fixture = (await response.json()) as MockFixture;
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
  for (const deck of lib.decks) {
    if (mockSettings.has(deck.source)) {
      const s = mockSettings.get(deck.source);
      deck.sidecar = s !== null;
      if (s) deck.settings = s;
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

export async function fittedArt(source: string): Promise<Fitted> {
  if (inTauri) return invoke<Fitted>('fitted_art', { source });
  const f = (await mock()).fitted[source];
  return f ?? { path: source, notes: [] };
}

/** A PNG on disk as an ImageBitmap whose pixels can be read. */
export async function imageBitmap(path: string): Promise<ImageBitmap> {
  let blob: Blob;
  if (inTauri) {
    const bytes = await invoke<ArrayBuffer>('read_png', { path });
    blob = new Blob([bytes], { type: 'image/png' });
  } else {
    const response = await fetch(fileUrl(path));
    if (!response.ok) throw new Error(`${path}: ${response.status}`);
    blob = await response.blob();
  }
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
  onEvent({ event: 'scanned', kind: 'deck', count: lib.decks.length });
  for (const deck of lib.decks) {
    await wait(60);
    onEvent({ event: 'item', kind: 'deck', display: deck.display, notes: deck.notes });
  }
  for (const deck of lib.decks) {
    await wait(40);
    onEvent({ event: 'log', message: `  built ${deck.display} (${deck.key})` });
  }
  onEvent({ event: 'built', decks: lib.decks.length, wheels: lib.wheels.length });
  return {
    outcome: 'built',
    decks: lib.decks.length,
    wheels: lib.wheels.length,
    shaders: 'built',
    seconds: (performance.now() - start) / 1000,
    modsDir: '(mock)',
  };
}

export async function launch(): Promise<void> {
  if (inTauri) return invoke<void>('launch');
}

export async function reveal(path: string): Promise<void> {
  if (inTauri) return invoke<void>('reveal', { path });
}
