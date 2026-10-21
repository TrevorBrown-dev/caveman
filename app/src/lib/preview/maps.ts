// Textures for the preview, made from the deck art the way Caveman's mixins make the game's (tools/re/mixins.py,
// caveman_core::effects): gloss and metal from the finish (spot: only on the light parts), a normal map for emboss
// and glitter, holo masks and patterns. Approximations: three.js materials stand in for the game's shader.

import * as THREE from 'three';
import type { FinishKind, HoloMode } from '../types';

export const W = 512;
export const H = 2048;

export function canvas(w: number, h: number): HTMLCanvasElement {
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  return c;
}

/** A deterministic random generator (mulberry32), so maps look the same on every render. */
export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function smoothstep(lo: number, hi: number, x: number): number {
  const t = Math.min(1, Math.max(0, (x - lo) / (hi - lo)));
  return t * t * (3 - 2 * t);
}

/** The art drawn into a 512 x 2048 canvas, and its pixels. */
export function artCanvas(image: CanvasImageSource): { canvas: HTMLCanvasElement; pixels: Uint8ClampedArray } {
  const c = canvas(W, H);
  const ctx = c.getContext('2d', { willReadFrequently: true })!;
  ctx.drawImage(image, 0, 0, W, H);
  return { canvas: c, pixels: ctx.getImageData(0, 0, W, H).data };
}

/** 0-1 lightness per pixel (Rec. 709 weights on the sRGB values, as mixins.luma). */
export function luma(pixels: Uint8ClampedArray): Float32Array {
  const out = new Float32Array(W * H);
  for (let i = 0; i < W * H; i++) {
    out[i] = (0.2126 * pixels[i * 4] + 0.7152 * pixels[i * 4 + 1] + 0.0722 * pixels[i * 4 + 2]) / 255;
  }
  return out;
}

/** Where a `spot` finish goes: the light parts of the art (mixins.spot). */
export function spotWeight(l: Float32Array): Float32Array {
  const out = new Float32Array(l.length);
  for (let i = 0; i < l.length; i++) out[i] = smoothstep(0.55, 0.75, l[i]);
  return out;
}

/** Each finish as three.js roughness and metalness (the game's smoothness s is about roughness 1 - s / 255). */
const FINISH_PBR: Record<FinishKind | 'none', [number, number]> = {
  none: [0.17, 0],
  gloss: [0.04, 0],
  matte: [0.72, 0],
  foil: [0.16, 1],
  chrome: [0.03, 1],
  glitter: [0.1, 0.9],
  holo: [0.02, 0.12],
};

function flakes(seed: number, size: number): Float32Array {
  const r = rng(seed);
  const cw = Math.ceil(W / size);
  const ch = Math.ceil(H / size);
  const cells = new Float32Array(cw * ch);
  for (let i = 0; i < cells.length; i++) cells[i] = r();
  const out = new Float32Array(W * H);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) out[y * W + x] = cells[Math.floor(y / size) * cw + Math.floor(x / size)];
  }
  return out;
}

function toTexture(c: HTMLCanvasElement, colour = false): THREE.CanvasTexture {
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = colour ? THREE.SRGBColorSpace : THREE.NoColorSpace;
  t.anisotropy = 8;
  t.needsUpdate = true;
  return t;
}

/** Roughness (G) and metalness (B), the way three.js reads them, for a finish; spot blends it into the stock finish. */
export function finishMap(finish: FinishKind | 'none', weight: Float32Array | null): THREE.CanvasTexture {
  const c = canvas(W, H);
  const ctx = c.getContext('2d')!;
  const img = ctx.createImageData(W, H);
  const [r0, m0] = FINISH_PBR.none;
  const [r1, m1] = FINISH_PBR[finish];
  const metalFlakes = finish === 'glitter' ? flakes(1, 2) : null;
  const roughFlakes = finish === 'glitter' ? flakes(2, 2) : null;
  for (let i = 0; i < W * H; i++) {
    let rough = r1;
    let metal = m1;
    if (metalFlakes && roughFlakes) {
      metal = 0.7 + 0.3 * metalFlakes[i];
      rough = 0.02 + 0.22 * roughFlakes[i];
    }
    const w = weight ? weight[i] : 1;
    img.data[i * 4] = 255;
    img.data[i * 4 + 1] = Math.round(255 * (r0 + (rough - r0) * w));
    img.data[i * 4 + 2] = Math.round(255 * (m0 + (metal - m0) * w));
    img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  return toTexture(c);
}

/** A normal map: emboss raises the light parts of the art (strength as in the settings, default 4); glitter tilts
 *  each flake. Null when neither applies. */
export function normalMap(l: Float32Array, emboss: number | null, glitter: boolean, weight: Float32Array | null): THREE.CanvasTexture | null {
  if (emboss === null && !glitter) return null;
  // a blurred height (3 x 3 box twice, close to the Gaussian blur the build uses)
  let h = l;
  if (emboss !== null) {
    for (let pass = 0; pass < 2; pass++) {
      const b = new Float32Array(W * H);
      for (let y = 0; y < H; y++) {
        for (let x = 0; x < W; x++) {
          let s = 0;
          let n = 0;
          for (let dy = -1; dy <= 1; dy++) {
            const yy = y + dy;
            if (yy < 0 || yy >= H) continue;
            for (let dx = -1; dx <= 1; dx++) {
              const xx = x + dx;
              if (xx < 0 || xx >= W) continue;
              s += h[yy * W + xx];
              n++;
            }
          }
          b[y * W + x] = s / n;
        }
      }
      h = b;
    }
  }
  const fx = glitter ? flakes(3, 2) : null;
  const fy = glitter ? flakes(4, 2) : null;
  const k = emboss === null ? 0 : emboss;
  const c = canvas(W, H);
  const ctx = c.getContext('2d')!;
  const img = ctx.createImageData(W, H);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const i = y * W + x;
      let nx = 0;
      let ny = 0;
      if (emboss !== null) {
        const xl = h[y * W + Math.max(0, x - 1)];
        const xr = h[y * W + Math.min(W - 1, x + 1)];
        const yu = h[Math.max(0, y - 1) * W + x];
        const yd = h[Math.min(H - 1, y + 1) * W + x];
        nx = -k * (xr - xl) * 0.5;
        ny = -k * (yu - yd) * 0.5; // the texture's v runs up the image
      }
      if (fx && fy) {
        const w = weight ? weight[i] : 1;
        nx += 0.6 * (2 * fx[i] - 1) * w;
        ny += 0.6 * (2 * fy[i] - 1) * w;
      }
      const len = Math.hypot(nx, ny, 1);
      img.data[i * 4] = Math.round(((nx / len) * 0.5 + 0.5) * 255);
      img.data[i * 4 + 1] = Math.round(((ny / len) * 0.5 + 0.5) * 255);
      img.data[i * 4 + 2] = Math.round(((1 / len) * 0.5 + 0.5) * 255);
      img.data[i * 4 + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  return toTexture(c);
}

/** The holo foil's mask (R: where it is holo) and its pattern as a thin-film thickness (G), standing in for the game's
 *  five foil patterns. */
export function holoMaps(mode: HoloMode, weight: Float32Array | null): { mask: THREE.CanvasTexture; thickness: THREE.CanvasTexture } {
  const mask = canvas(W, H);
  const mctx = mask.getContext('2d')!;
  const mimg = mctx.createImageData(W, H);
  for (let i = 0; i < W * H; i++) {
    const v = Math.round(255 * (weight ? weight[i] : 1));
    mimg.data[i * 4] = v;
    mimg.data[i * 4 + 1] = v;
    mimg.data[i * 4 + 2] = v;
    mimg.data[i * 4 + 3] = 255;
  }
  mctx.putImageData(mimg, 0, 0);

  const thick = canvas(W, H);
  const tctx = thick.getContext('2d')!;
  const timg = tctx.createImageData(W, H);
  const r = rng(7);
  // cellular: random sites on a grid
  const cell = 48;
  const sites: [number, number, number][] = [];
  for (let gy = 0; gy <= H / cell; gy++) {
    for (let gx = 0; gx <= W / cell; gx++) sites.push([(gx + r()) * cell, (gy + r()) * cell, r()]);
  }
  const fine = flakes(11, 3);
  const speck = flakes(12, 2);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      let t: number;
      switch (mode) {
        case 'vertical':
          t = 0.5 + 0.5 * Math.sin((x / W) * Math.PI * 10);
          break;
        case 'cellular': {
          const gx = Math.floor(x / cell);
          const gy = Math.floor(y / cell);
          let best = Infinity;
          let val = 0;
          for (let dy = -1; dy <= 1; dy++) {
            for (let dx = -1; dx <= 1; dx++) {
              const sx = gx + dx;
              const sy = gy + dy;
              if (sx < 0 || sy < 0 || sx > W / cell || sy > H / cell) continue;
              const s = sites[sy * (Math.floor(W / cell) + 1) + sx];
              if (!s) continue;
              const d = (s[0] - x) ** 2 + (s[1] - y) ** 2;
              if (d < best) {
                best = d;
                val = s[2];
              }
            }
          }
          t = val;
          break;
        }
        case 'frost':
          t = 0.35 * fine[y * W + x] + 0.65 * (0.5 + 0.5 * Math.sin((x + y * 0.7) / 23 + Math.sin(y / 61) * 3));
          break;
        case 'glitter':
          t = speck[y * W + x];
          break;
        default: // pearl: soft, broad swirls
          t = 0.5 + 0.5 * Math.sin(x / 90 + Math.sin(y / 140) * 2.2 + y / 300);
      }
      const i = y * W + x;
      const v = Math.round(255 * t);
      timg.data[i * 4] = v;
      timg.data[i * 4 + 1] = v;
      timg.data[i * 4 + 2] = v;
      timg.data[i * 4 + 3] = 255;
    }
  }
  tctx.putImageData(timg, 0, 0);
  return { mask: toTexture(mask), thickness: toTexture(thick) };
}

/** The solid rainbow trail: Nyan Cat's six bands across the width, shifted half a band on every other step along it
 *  (one step per texture repeat's half). */
export function nyanTexture(): THREE.CanvasTexture {
  const bands = ['#ff0000', '#ff9900', '#ffff00', '#33ff00', '#0099ff', '#6633ff'];
  const c = canvas(96, 64);
  const ctx = c.getContext('2d')!;
  for (let step = 0; step < 2; step++) {
    // as Caveman's ribbon patch (nyanpatch.py): band = floor((u + offset) x 6), at most 5; offset is half a band on
    // every other step, so there the first band is narrower and the last wider
    const offset = step === 1 ? 1 / 12 : 0;
    for (let x = 0; x < 96; x++) {
      const band = Math.min(5, Math.floor(((x + 0.5) / 96 + offset) * 6));
      ctx.fillStyle = bands[band];
      ctx.fillRect(x, step * 32, 1, 32);
    }
  }
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.magFilter = THREE.NearestFilter;
  t.minFilter = THREE.NearestFilter;
  t.wrapT = THREE.RepeatWrapping;
  t.generateMipmaps = false;
  return t;
}

/** Asphalt for riding: speckled grey with faint seams. */
export function groundTexture(): THREE.CanvasTexture {
  const c = canvas(512, 512);
  const ctx = c.getContext('2d')!;
  const img = ctx.createImageData(512, 512);
  const r = rng(21);
  for (let i = 0; i < 512 * 512; i++) {
    const v = 52 + Math.floor(r() * 26) + (r() < 0.02 ? 30 : 0);
    img.data[i * 4] = v;
    img.data[i * 4 + 1] = v;
    img.data[i * 4 + 2] = v + 3;
    img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  ctx.strokeStyle = 'rgba(20, 20, 24, 0.55)';
  ctx.lineWidth = 3;
  ctx.beginPath();
  ctx.moveTo(0, 256);
  ctx.lineTo(512, 256);
  ctx.stroke();
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = THREE.RepeatWrapping;
  t.wrapT = THREE.RepeatWrapping;
  t.anisotropy = 8;
  return t;
}

/** Grip tape: near-black grit. */
export function gripTexture(): THREE.CanvasTexture {
  const c = canvas(256, 1024);
  const ctx = c.getContext('2d')!;
  const img = ctx.createImageData(256, 1024);
  const r = rng(31);
  for (let i = 0; i < 256 * 1024; i++) {
    const v = 18 + Math.floor(r() * 22);
    img.data[i * 4] = v;
    img.data[i * 4 + 1] = v;
    img.data[i * 4 + 2] = v + 2;
    img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

/** The deck's edge: seven plies of maple. */
export function edgeTexture(): THREE.CanvasTexture {
  const c = canvas(16, 64);
  const ctx = c.getContext('2d')!;
  const plies = ['#d9b98a', '#c69a63', '#e3c79c', '#b9895a', '#dcbf92', '#c4975f', '#e0c497'];
  plies.forEach((col, i) => {
    ctx.fillStyle = col;
    ctx.fillRect(0, (i * 64) / 7, 16, 64 / 7 + 1);
  });
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}
