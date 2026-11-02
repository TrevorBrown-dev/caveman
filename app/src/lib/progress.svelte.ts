// A build's progress for whoever waits on it (the Play button, the Studio's build log), from the build's events as
// `caveman --json` prints them: the scan reads each picture (`scanned` gives the counts, then an `item`, `skipped` or
// `warning` per file), the build places each item in the mod (`progress`, stage "items", `done` of `total`, and the
// item it is on now; the label counts from 1, the item in hand), writes
// the mod's files (`progress`, stage "writing", `mb`) and the shader effects (`progress`, stage "shaders"). The
// counts and the time since the last event are real: a build that stops moving shows it.

import type { BuildEvent } from './types';

export type Stage = 'starting' | 'reading' | 'items' | 'writing' | 'shaders' | 'done';

/** Each stage's share of the bar (a guess at its share of the time; the label carries the real counts). */
const SHARES: Record<Stage, [number, number]> = {
  starting: [0, 0],
  reading: [0, 0.25],
  items: [0.25, 0.9],
  writing: [0.9, 0.97],
  shaders: [0.97, 1],
  done: [1, 1],
};

/** After this long without an event (outside the writing, which sends none), the label says so. */
const QUIET_SECONDS = 45;

const count = (n: number): string => n.toLocaleString();

export class BuildProgress {
  stage = $state<Stage>('starting');
  done = $state(0);
  total = $state(0);
  mb = $state<number | null>(null);
  current = $state<string | null>(null);
  startedAt = $state(0);
  lastAt = $state(0);
  /** The clock, ticking once a second while the build runs. */
  now = $state(0);
  private read = 0;
  private timer: ReturnType<typeof setInterval> | null = null;

  start(): void {
    this.stage = 'starting';
    this.done = 0;
    this.total = 0;
    this.mb = null;
    this.current = null;
    this.read = 0;
    this.startedAt = this.lastAt = this.now = Date.now();
    if (this.timer) clearInterval(this.timer);
    this.timer = setInterval(() => (this.now = Date.now()), 1000);
  }

  finish(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    this.stage = 'done';
    this.now = Date.now();
  }

  feed(e: BuildEvent): void {
    this.lastAt = Date.now();
    switch (e.event) {
      case 'scanned':
        if (this.stage === 'starting') this.stage = 'reading';
        this.total += Number(e.count ?? 0);
        break;
      case 'item':
      case 'skipped':
      case 'warning':
        this.read++;
        this.done = Math.min(this.read, this.total);
        this.current = e.display ?? String(e.source ?? '').split(/[\\/]/).pop() ?? null;
        break;
      case 'ready':
        this.stage = 'items';
        this.done = 0;
        this.total = Number(e.decks ?? 0) + Number(e.wheels ?? 0);
        this.current = null;
        break;
      case 'progress':
        if (e.stage === 'items') {
          this.stage = 'items';
          this.done = Number(e.done ?? 0);
          this.total = Number(e.total ?? this.total);
          this.current = typeof e.display === 'string' ? e.display : null;
        } else if (e.stage === 'writing') {
          this.stage = 'writing';
          this.mb = typeof e.mb === 'number' ? e.mb : null;
          this.current = null;
        } else if (e.stage === 'shaders') {
          this.stage = 'shaders';
          this.current = null;
        }
        break;
    }
  }

  /** The whole build's share done, 0 to 1. */
  get fraction(): number {
    const [from, to] = SHARES[this.stage];
    const within = this.stage === 'reading' || this.stage === 'items' ? (this.total > 0 ? this.done / this.total : 0) : this.stage === 'done' ? 1 : 0.5;
    return from + (to - from) * Math.min(Math.max(within, 0), 1);
  }

  /** `n` of the total, counted from 1 (1 of 1,325 as a stage starts, 1,325 at its last), with the item named below. */
  private of(n: number): string {
    return `${count(Math.min(Math.max(n, 1), this.total))} of ${count(this.total)}`;
  }

  /** What it is doing, with the counts. */
  get label(): string {
    switch (this.stage) {
      case 'starting':
        return 'Starting';
      case 'reading':
        // each picture's event comes once it is read: the count and the name are of that picture
        return `Reading the pictures: ${this.of(this.done)}`;
      case 'items':
        // `done` items are in; the build is on the next, the one the event names
        return `Building: ${this.of(this.done + 1)}`;
      case 'writing':
        return this.mb !== null ? `Writing the mod files (${count(this.mb)} MB)` : 'Writing the mod files';
      case 'shaders':
        return 'Building the shader effects';
      case 'done':
        return 'Done';
    }
  }

  /** Time since the start, as m:ss. */
  get elapsed(): string {
    const s = Math.max(0, Math.floor((this.now - this.startedAt) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  /** Seconds without an event, when that is long enough to say (the writing sends none while it writes). */
  get quiet(): number | null {
    if (this.stage === 'writing' || this.stage === 'done') return null;
    const s = Math.floor((this.now - this.lastAt) / 1000);
    return s >= QUIET_SECONDS ? s : null;
  }
}
