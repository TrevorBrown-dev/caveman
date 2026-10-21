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

export interface Config {
  game: string;
  decks: string;
  wheels: string;
  out: string;
  cache: string;
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
