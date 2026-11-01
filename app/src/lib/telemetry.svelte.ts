// What the game tells us this instant, for every widget in a window: the pose stream (`caveman-trainer pose --bones`,
// read from outside the game and relayed by the back end as `board-pose` events) and what follows from it, the board's
// speed first. Widgets call `start` and `stop`; the first start subscribes and asks the back end for the stream, the
// last stop lets go (the back end keeps one trainer process for all windows and stops it with the last). The stream
// ends with the game it read; while a game runs it is asked for again a few seconds later, so a new game session
// comes through without the window being reopened.
import type { UnlistenFn } from '@tauri-apps/api/event';
import { gameRunning } from './api';
import * as live from './live';
import type { PoseFrame } from './live';

export type Units = 'kmh' | 'mph' | 'ms';

export const telemetry = $state({
  /** Frames come and the game has a board (not the menu, not loading). */
  live: false,
  /** Why not, in words; 'ok' while live. */
  status: 'waiting for the game',
  /** The board's speed in m/s, smoothed over about a tenth of a second. */
  speed: 0,
  /** The fastest since the stream started, or since `resetMax`. */
  speedMax: 0,
  /** The last frame's time, seconds since the stream started. */
  t: 0,
});

/** How far back the speed looks (s): short enough to follow a push, long enough to ride over the reader's jitter. */
const SPEED_WINDOW = 0.12;
/** Samples older than this go. */
const KEEP = 0.6;
/** Faster than this between two frames is a jump (a respawn, a teleport, the menu), not skating. */
const TELEPORT = 60;
/** How much of a new estimate goes into the shown speed each frame. */
const SMOOTH = 0.35;

const samples: { t: number; pos: number[] }[] = [];
const frameListeners = new Set<(frame: PoseFrame) => void>();
let users = 0;
let rate = 60;
let unlisten: Promise<UnlistenFn> | null = null;
let restartTimer: ReturnType<typeof setTimeout> | null = null;
let stopped = true;

/** Every frame as it comes (for widgets that pose things by the bones). Returns the unsubscribe. */
export function onFrame(cb: (frame: PoseFrame) => void): () => void {
  frameListeners.add(cb);
  return () => frameListeners.delete(cb);
}

export function resetMax(): void {
  telemetry.speedMax = 0;
}

/** Speed in m/s shown in `units`. */
export function speedIn(ms: number, units: Units): number {
  return units === 'kmh' ? ms * 3.6 : units === 'mph' ? ms * 2.236936 : ms;
}

export function unitLabel(units: Units): string {
  return units === 'kmh' ? 'km/h' : units === 'mph' ? 'mph' : 'm/s';
}

const distance = (a: number[], b: number[]): number => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);

/** The speed from the board's positions: the distance over the last SPEED_WINDOW seconds, eased. */
function measure(frame: PoseFrame): void {
  const pos = frame.board?.pos;
  const t = frame.t;
  if (!pos || t === undefined) {
    samples.length = 0;
    telemetry.speed = 0;
    return;
  }
  const last = samples[samples.length - 1];
  if (last) {
    const dt = t - last.t;
    if (dt <= 0 || distance(last.pos, pos) / dt > TELEPORT) samples.length = 0;
  }
  samples.push({ t, pos });
  while (samples.length > 1 && samples[0].t < t - KEEP) samples.shift();
  let i = samples.length - 1;
  while (i > 0 && t - samples[i].t < SPEED_WINDOW) i--;
  const ref = samples[i];
  const dt = t - ref.t;
  if (dt < 0.03) return;
  const v = distance(ref.pos, pos) / dt;
  telemetry.speed += (v - telemetry.speed) * SMOOTH;
  if (telemetry.speed > telemetry.speedMax) telemetry.speedMax = telemetry.speed;
}

function restart(): void {
  if (stopped || restartTimer) return;
  restartTimer = setTimeout(() => {
    restartTimer = null;
    if (stopped) return;
    void gameRunning()
      .then((running) => (running ? live.poseStart(rate) : Promise.reject(new Error('no game'))))
      .catch(() => restart());
  }, 3000);
}

function onPose(frame: PoseFrame): void {
  if (frame.done) {
    telemetry.live = false;
    telemetry.status = frame.gone ? 'the game closed' : 'the stream ended';
    samples.length = 0;
    telemetry.speed = 0;
    restart();
  } else if (frame.error) {
    telemetry.status = frame.why ?? 'error';
  } else {
    if (frame.t !== undefined) telemetry.t = frame.t;
    const ok = !!frame.board && !!frame.camera;
    telemetry.live = ok;
    telemetry.status = ok ? 'ok' : frame.why && frame.why !== 'ok' ? frame.why : 'no board';
    measure(frame);
  }
  for (const cb of frameListeners) cb(frame);
}

/** One more reader of the stream (reads a second: the first caller's rate holds). */
export function start(readsPerSecond = 60): void {
  users++;
  if (users > 1) return;
  stopped = false;
  rate = Math.min(Math.max(Math.round(readsPerSecond), 1), 240);
  unlisten = live.onPose(onPose);
  live.poseStart(rate).catch((err) => {
    telemetry.status = String(err);
    restart();
  });
}

export function stop(): void {
  users = Math.max(0, users - 1);
  if (users > 0) return;
  stopped = true;
  if (restartTimer) {
    clearTimeout(restartTimer);
    restartTimer = null;
  }
  void unlisten?.then((u) => u());
  unlisten = null;
  void live.poseStop();
  telemetry.live = false;
  telemetry.status = 'waiting for the game';
}
