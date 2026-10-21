// A deck's settings as the editor's controls, and back (docs/settings.md). Parts the editor does not show (`name`,
// `layers` from `caveman import`, anything newer) are kept as they are.

import type { BoardKind, FinishKind, HoloMode, Settings } from './types';

export const FINISHES: { id: FinishKind | 'none'; label: string; hint: string }[] = [
  { id: 'none', label: 'Print', hint: 'The stock deck finish' },
  { id: 'gloss', label: 'Gloss', hint: 'Wet-look shine' },
  { id: 'matte', label: 'Matte', hint: 'Flat, no shine' },
  { id: 'foil', label: 'Foil', hint: 'Metal tinted by the art' },
  { id: 'chrome', label: 'Chrome', hint: 'Mirror metal' },
  { id: 'glitter', label: 'Glitter', hint: 'Metal flakes that sparkle' },
  { id: 'holo', label: 'Holo', hint: "The game's own holographic foil" },
];

export const HOLO_MODES: { id: HoloMode; label: string }[] = [
  { id: 'pearl', label: 'Pearl' },
  { id: 'cellular', label: 'Cellular' },
  { id: 'frost', label: 'Frost' },
  { id: 'vertical', label: 'Vertical' },
  { id: 'glitter', label: 'Glitter' },
];

export const BOARD_EFFECTS: { id: BoardKind; label: string; hint: string }[] = [
  { id: 'none', label: 'None', hint: '' },
  { id: 'aura', label: 'Aura', hint: 'A light under the board and trails while riding' },
  { id: 'nyan', label: 'Rainbow trail', hint: 'A solid six-band rainbow behind the board while riding' },
];

export const DEFAULT_SPEED = 0.25;
export const DEFAULT_GLOW = 128 / 255;
export const DEFAULT_EMBOSS = 4;
export const DEFAULT_STEPS = 8;

export interface EditModel {
  finish: FinishKind | 'none';
  mode: HoloMode;
  spot: boolean;
  emboss: boolean;
  embossStrength: number;
  cycle: boolean;
  speed: number;
  glowOn: boolean;
  glow: number;
  board: BoardKind;
  /** null: the art's most vivid colour (the build picks it). */
  auraColour: string | null;
  steps: number;
}

export function toModel(s: Settings | null): EditModel {
  const glow = s?.shader?.glow ?? 0;
  return {
    finish: s?.finish?.kind ?? 'none',
    mode: s?.finish?.mode ?? 'pearl',
    spot: s?.finish?.spot ?? false,
    emboss: s?.emboss !== undefined,
    embossStrength: s?.emboss?.strength ?? DEFAULT_EMBOSS,
    cycle: s?.shader?.effect === 'cycle',
    speed: s?.shader?.speed ?? DEFAULT_SPEED,
    glowOn: glow > 0,
    glow: glow > 0 ? glow : DEFAULT_GLOW,
    board: s?.board?.effect ?? 'none',
    auraColour: s?.board?.colour ?? null,
    steps: s?.board?.steps ?? DEFAULT_STEPS,
  };
}

/** The settings for a model, keeping what the editor does not show from `original`. */
export function toSettings(m: EditModel, original: Settings | null): Settings {
  const out: Settings = { ...(original ?? {}), version: 1 };
  delete out.finish;
  delete out.emboss;
  delete out.shader;
  delete out.board;
  if (m.finish !== 'none') {
    out.finish = { kind: m.finish };
    if (m.finish === 'holo') out.finish.mode = m.mode;
    if (m.spot) out.finish.spot = true;
  }
  if (m.emboss) out.emboss = { strength: round(m.embossStrength, 2) };
  if (m.cycle || m.glowOn) {
    out.shader = { effect: m.cycle ? 'cycle' : 'none', speed: round(m.speed, 3), glow: m.glowOn ? round(m.glow, 3) : 0 };
  }
  if (m.board === 'aura') {
    out.board = { effect: 'aura' };
    if (m.auraColour) out.board.colour = m.auraColour;
  } else if (m.board === 'nyan') {
    out.board = { effect: 'nyan' };
    if (m.steps !== DEFAULT_STEPS) out.board.steps = m.steps;
  }
  return out;
}

function round(x: number, digits: number): number {
  const k = 10 ** digits;
  return Math.round(x * k) / k;
}

/** How much the game multiplies the print by for its glow (Caveman's shader: 2^(12 x glow) - 1). */
export function glowFactor(glow: number): number {
  return 2 ** (12 * glow) - 1;
}

/** Short labels for a deck's effects (library cards). */
export function chips(s: Settings | null): string[] {
  if (!s) return [];
  const out: string[] = [];
  if (s.finish) {
    const f = FINISHES.find((x) => x.id === s.finish?.kind)?.label ?? s.finish.kind;
    const mode = s.finish.kind === 'holo' ? HOLO_MODES.find((x) => x.id === (s.finish?.mode ?? 'pearl'))?.label : undefined;
    out.push(mode ? `${f} ${mode}` : f);
    if (s.finish.spot) out.push('Spot');
  }
  if (s.emboss) out.push('Emboss');
  if (s.shader?.effect === 'cycle') out.push('Cycle');
  if ((s.shader?.glow ?? 0) > 0) out.push('Glow');
  if (s.board?.effect === 'aura') out.push('Aura');
  if (s.board?.effect === 'nyan') out.push('Rainbow');
  if (s.layers) out.push('Own layers');
  return out;
}

/** Whether a deck needs the Caveman-Shaders mod (built automatically). */
export function needsShaders(m: EditModel): boolean {
  return m.cycle || m.glowOn || m.board === 'nyan';
}

export function sameModel(a: EditModel, b: EditModel): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
