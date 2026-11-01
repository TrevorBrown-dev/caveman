// Widgets: the see-through, frameless, always-on-top windows that OBS captures one by one, each a page of this build
// (`?view=widget&kind=<kind>`, components/widgets/WidgetFrame.svelte) with one component inside. This is the registry
// (what exists, how big it opens, what it needs, its options' defaults) and the calls around a widget's life: open and
// close, which are open, options going to an open window and coming back from it, and the saved settings' shape
// (`plugins.widgets.<kind>`: autostart, the window's place, the options). The back end (src-tauri/src/widgets.rs)
// makes the windows, saves their place and size as they are moved, and closes them with the app.
import type { Component } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { PluginsConfig, WidgetConfig } from './types';

export type { WidgetConfig };

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** A widget's options: flat, so they travel in a query string and sit in the settings as they are. */
export type Options = Record<string, string | number | boolean | null>;

/** An item of the window's right-click menu that a widget adds to the frame's own. */
export interface MenuItem {
  label: string;
  run: () => void;
}

/** What a widget component gets from the frame. */
export interface WidgetProps {
  options: Options;
  /** A line for the window's corner (why nothing shows yet), or null when all is well. */
  onstatus: (status: string | null) => void;
  /** The widget's own menu items, given once. */
  onmenu?: (items: MenuItem[]) => void;
}

/** What a widget's settings pane gets in the app. */
export interface SettingsProps {
  options: Options;
  onchange: (patch: Options) => void;
}

export interface WidgetDef {
  kind: string;
  /** The card's name. */
  name: string;
  /** The window's title: what OBS's window capture lists. Keep it: scenes refer to it. */
  title: string;
  blurb: string;
  /** The window's size when none is saved (logical pixels). */
  width: number;
  height: number;
  /** What the widget reads: `pose` is the stream from outside the game (lib/telemetry.svelte.ts). */
  needs: 'pose'[];
  defaults: Options;
  /** Shown in the window for its first seconds. */
  hint: string;
  component: () => Promise<{ default: Component<WidgetProps> }>;
  settings: () => Promise<{ default: Component<SettingsProps> }>;
}

export const WIDGETS: WidgetDef[] = [
  {
    kind: 'boardcam',
    name: 'Board cam',
    title: 'Caveman board cam',
    blurb: "The game's own board turning as it does under you, read from outside the game",
    width: 360,
    height: 360,
    needs: ['pose'],
    defaults: { elevation: -30, azimuth: 20, zoom: 1, turn: 'camera', mirror: false, nose: false, art: null, rate: 60 },
    hint: 'drag to turn the view, scroll to zoom, move by the top strip, resize by the edges, right-click for the menu, Escape closes',
    component: () => import('../components/widgets/BoardCamWidget.svelte'),
    settings: () => import('../components/widgets/BoardCamSettings.svelte'),
  },
  {
    kind: 'speed',
    name: 'Speed',
    title: 'Caveman speed',
    blurb: "How fast the board goes, with the session's top speed",
    width: 260,
    height: 120,
    needs: ['pose'],
    defaults: { units: 'kmh', decimals: 0, max: true, label: true, dim: true, rate: 60 },
    hint: 'move by the top strip, resize by the edges, right-click for the menu, Escape closes',
    component: () => import('../components/widgets/SpeedWidget.svelte'),
    settings: () => import('../components/widgets/SpeedSettings.svelte'),
  },
];

export function widgetDef(kind: string): WidgetDef | undefined {
  return WIDGETS.find((w) => w.kind === kind);
}

/** A widget's settings with the defaults under whatever is saved. */
export function widgetConfig(plugins: PluginsConfig, kind: string): WidgetConfig {
  const saved = plugins.widgets[kind];
  const def = widgetDef(kind);
  return { autostart: saved?.autostart ?? false, window: saved?.window, options: { ...(def?.defaults ?? {}), ...(saved?.options ?? {}) } };
}

/**
 * The plugins' settings as they go to the back end: a widget's window geometry is the back end's to keep (it saves it
 * as the window moves), so it is left out unless it is being reset (null), and the back end fills the current one in.
 */
export function forSave(plugins: PluginsConfig): PluginsConfig {
  const widgets: Record<string, WidgetConfig> = {};
  for (const [kind, w] of Object.entries(plugins.widgets)) {
    const { window: geometry, ...rest } = w;
    widgets[kind] = geometry === null ? { ...rest, window: null } : rest;
  }
  return { ...plugins, widgets };
}

/** The options as a query string (booleans 1 and 0, nulls left out). */
export function query(options: Options): string {
  const q = new URLSearchParams();
  for (const [key, value] of Object.entries(options)) {
    if (value === null || value === undefined) continue;
    q.set(key, typeof value === 'boolean' ? (value ? '1' : '0') : String(value));
  }
  return q.toString();
}

/** The options back from a page's query, typed by the defaults (a key not in the defaults comes as a string). */
export function optionsFromQuery(params: URLSearchParams, defaults: Options): Options {
  const out: Options = { ...defaults };
  for (const [key, raw] of params) {
    if (key === 'view' || key === 'kind') continue;
    const d = defaults[key];
    if (typeof d === 'number') {
      const n = Number(raw);
      if (Number.isFinite(n)) out[key] = n;
    } else if (typeof d === 'boolean') {
      out[key] = raw === '1' || raw === 'true';
    } else {
      out[key] = raw;
    }
  }
  return out;
}

/** Opens a widget's window (an open one is made again, so new options apply). In a plain browser: a popup. */
export async function open(kind: string, options: Options): Promise<void> {
  const def = widgetDef(kind);
  if (!def) throw new Error(`no widget "${kind}"`);
  const q = query(options);
  if (!inTauri) {
    window.open(`${window.location.pathname}?view=widget&kind=${kind}&${q}`, `caveman-widget-${kind}`, `width=${def.width},height=${def.height}`);
    return;
  }
  await invoke('widget_open', { kind, title: def.title, query: q, width: def.width, height: def.height, index: WIDGETS.indexOf(def) });
}

export async function close(kind: string): Promise<void> {
  if (!inTauri) return;
  await invoke('widget_close', { kind });
}

/** The kinds whose windows are open. */
export async function list(): Promise<string[]> {
  if (!inTauri) return [];
  return invoke<string[]>('widget_list');
}

/** A widget window opened or closed (`widget-state`, from the back end). */
export async function onState(handler: (kind: string, open: boolean) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<{ kind: string; open: boolean }>('widget-state', (event) => handler(event.payload.kind, event.payload.open));
}

/** From the app to an open widget window: its options changed (`widget-options-set`). */
export async function setOptions(kind: string, options: Options): Promise<void> {
  if (!inTauri) return;
  const { emit } = await import('@tauri-apps/api/event');
  await emit('widget-options-set', { kind, options });
}

export async function onOptionsSet(kind: string, handler: (options: Options) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<{ kind: string; options: Options }>('widget-options-set', (event) => {
    if (event.payload.kind === kind) handler(event.payload.options);
  });
}

/** From a widget window to the app: options it changed itself (a view dragged), to be saved (`widget-options`). */
export async function report(kind: string, patch: Options): Promise<void> {
  if (!inTauri) return;
  const { emit } = await import('@tauri-apps/api/event');
  await emit('widget-options', { kind, patch });
}

export async function onReport(handler: (kind: string, patch: Options) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<{ kind: string; patch: Options }>('widget-options', (event) => handler(event.payload.kind, event.payload.patch));
}
