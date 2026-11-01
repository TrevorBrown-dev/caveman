// The shapes the back end (app/src-tauri) sends and takes. Settings follow docs/settings.md (schema version 1).

export type FinishKind = 'gloss' | 'matte' | 'foil' | 'chrome' | 'glitter' | 'holo';
export type HoloMode = 'pearl' | 'cellular' | 'frost' | 'vertical' | 'glitter';
export type BoardKind = 'none' | 'aura' | 'nyan';

export interface Settings {
  version: number;
  name?: string;
  finish?: { kind: FinishKind; mode?: HoloMode; spot?: boolean };
  emboss?: { strength: number };
  shader?: { effect?: 'none' | 'cycle'; speed?: number; glow?: number };
  board?: { effect?: BoardKind; colour?: string; steps?: number };
  /** Kept as it is (written by `caveman import`): the deck's own textures. */
  layers?: Record<string, unknown>;
  [other: string]: unknown;
}

export interface LibraryItem {
  kind: 'deck' | 'wheel';
  display: string;
  key: string;
  source: string;
  stem: string;
  group: string;
  settings: Settings | null;
  sidecar: boolean;
  sidecarPath: string;
  notes: string[];
  preview: string | null;
  colour: string | null;
  modified: number;
  /** Goes into the game (the switch in the library; `.caveman-off.json` in the drop folder). */
  enabled: boolean;
}

export interface Problem {
  kind: string;
  source: string;
  reason: string;
}

export interface Library {
  decks: LibraryItem[];
  wheels: LibraryItem[];
  problems: Problem[];
  decksDir: string;
  wheelsDir: string;
}

/** The plugins' settings, the front end's own (kept in the config's `plugins` object). */
/** A widget's saved settings (lib/widgets.ts): open it with the game, its window's last place (the back end keeps it;
 *  null resets it), and its options. */
export interface WidgetConfig {
  autostart: boolean;
  window?: { x: number; y: number; w: number; h: number } | null;
  options: Record<string, string | number | boolean | null>;
}

export interface PluginsConfig {
  obs: { autostart: boolean; source: 'obs' | 'file'; file: string; rotate: 'cw' | 'ccw' | 'none'; fit: 'cover' | 'contain' };
  bars: { autostart: boolean };
  /** The widgets by kind (the board cam, the speed meter, ...). */
  widgets: Record<string, WidgetConfig>;
  /** The Live panel's: the picture last put on the board, as the trainer command that put it there, so the app
   *  puts it back by itself in the next game session (null: nothing, or plain white chosen). */
  live: { lastDeck: { label: string; args: string[] } | null };
}

export const DEFAULT_PLUGINS: PluginsConfig = {
  obs: { autostart: false, source: 'obs', file: '', rotate: 'none', fit: 'cover' },
  bars: { autostart: false },
  widgets: {},
  live: { lastDeck: null },
};

/** The widgets' settings, with the board cam's from before the widgets (`boardcam` at the top, until 2026-11-01)
 *  carried over once. */
function widgetsOf(p: Record<string, unknown>): Record<string, WidgetConfig> {
  const widgets = { ...((p.widgets as Record<string, WidgetConfig> | undefined) ?? {}) };
  const old = p.boardcam as Record<string, unknown> | undefined;
  if (old && !widgets.boardcam) {
    const options: WidgetConfig['options'] = {};
    for (const key of ['elevation', 'azimuth', 'zoom', 'turn', 'mirror', 'nose']) {
      const v = old[key];
      if (typeof v === 'number' || typeof v === 'string' || typeof v === 'boolean') options[key] = v;
    }
    widgets.boardcam = { autostart: old.autostart === true, options };
  }
  return widgets;
}

/** The plugins' settings with the defaults under whatever the config holds. */
export function pluginsOf(config: Config | null): PluginsConfig {
  const p = (config?.plugins ?? {}) as Record<string, Record<string, unknown>>;
  return {
    obs: { ...DEFAULT_PLUGINS.obs, ...(p.obs ?? {}) } as PluginsConfig['obs'],
    bars: { ...DEFAULT_PLUGINS.bars, ...(p.bars ?? {}) } as PluginsConfig['bars'],
    widgets: widgetsOf(p),
    live: { ...DEFAULT_PLUGINS.live, ...(p.live ?? {}) } as PluginsConfig['live'],
  };
}

export interface Config {
  game: string;
  decks: string;
  wheels: string;
  out: string;
  cache: string;
  /** caveman-trainer.exe for the plugins; empty for the one beside the app. */
  trainer: string;
  /** The author in the built mods' manifest.json (empty: "Caveman"). */
  author: string;
  /** Put the FX DLL in when the game starts. */
  attach: boolean;
  /** The staging ring's size in MB (0: off). */
  ringMb: number;
  /** The plugins' settings (PluginsConfig, partial). */
  plugins: Record<string, unknown>;
}

export interface Fitted {
  path: string;
  notes: string[];
}

export interface BuildResult {
  outcome: 'built' | 'up_to_date' | 'nothing';
  decks: number;
  wheels: number;
  shaders: 'built' | 'removed' | 'none';
  seconds: number;
  modsDir: string;
}

/** One line of the build's progress (`caveman --json`). */
export interface BuildEvent {
  event: string;
  message?: string;
  display?: string;
  kind?: string;
  source?: string;
  reason?: string;
  notes?: string[];
  [field: string]: unknown;
}

/** The front page's status (`launcher_status`). */
export interface LauncherStatus {
  game: string;
  gameOk: boolean;
  skateBuild: 'match' | 'other' | 'missing';
  reskateVersion: string | null;
  dllInstalled: boolean;
  trainer: string | null;
  modsBuilt: 'up_to_date' | 'stale' | 'none';
  decks: number;
  decksOn: number;
  wheels: number;
  wheelsOn: number;
}

/** A pack to generate (`pack_build`). */
export interface PackSpec {
  owner: string;
  name: string;
  version: string;
  description: string;
  website: string;
  icon: string | null;
  out: string | null;
  items: { kind: 'deck' | 'wheel'; source: string }[];
}

export interface PackResult {
  folder: string;
  zip: string;
  zipBytes: number;
  decks: number;
  wheels: number;
  shaderDecks: string[];
}

export type PackEvent = BuildEvent;
