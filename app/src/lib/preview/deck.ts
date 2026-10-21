// The deck's shape for the preview: Caveman's measured outline (deck_outline.json, copied from Caveman/assets: the
// visible deck in art pixels, 512 x 2048) with kicked nose and tail and a slight concave, the art on the bottom.
// Board space: x across, y up (grip side), z along the board (nose +z), metres.

import * as THREE from 'three';
import outline from './deck_outline.json';

const ROWS = outline.rows as [number, number, number][];
const ART_W = outline.width;
const ART_H = outline.height;
/** Metres per art pixel: the deck's 488 px width (x 12 to 500) is 8 inches. */
export const PX = 0.2032 / 488;
export const THICKNESS = 0.0115;
const COLUMNS = 28;
/** Where the kicks start (art rows), past the truck bolts. */
const NOSE_KICK = 430;
const TAIL_KICK = 1660;
const KICK_RADIUS = 0.11;
const KICK_ANGLE = (19 * Math.PI) / 180;
const CONCAVE = 0.0045;

/** Along-board position (metres) of an art row. */
export function zOf(row: number): number {
  return ((ART_H / 2 + 0.5 - row) * PX);
}

function kick(row: number): number {
  const d = row < NOSE_KICK ? (NOSE_KICK - row) * PX : row > TAIL_KICK ? (row - TAIL_KICK) * PX : 0;
  if (d <= 0) return 0;
  const d0 = KICK_RADIUS * Math.sin(KICK_ANGLE);
  if (d <= d0) return KICK_RADIUS - Math.sqrt(KICK_RADIUS * KICK_RADIUS - d * d);
  const h0 = KICK_RADIUS - Math.sqrt(KICK_RADIUS * KICK_RADIUS - d0 * d0);
  return h0 + (d - d0) * Math.tan(KICK_ANGLE);
}

function height(row: number, col: number): number {
  const u = (col - ART_W / 2) / 244;
  return kick(row) + CONCAVE * u * u;
}

/** The deck: bottom (art), top (grip) and edge, as three meshes' geometries. */
export function deckGeometries(): { bottom: THREE.BufferGeometry; top: THREE.BufferGeometry; edge: THREE.BufferGeometry } {
  const nr = ROWS.length;
  const nc = COLUMNS + 1;
  const bottomPos: number[] = [];
  const topPos: number[] = [];
  const uvs: number[] = [];
  for (const [row, left, right] of ROWS) {
    for (let j = 0; j < nc; j++) {
      const col = left + ((right - left) * j) / COLUMNS;
      const x = (col - ART_W / 2) * PX;
      const z = zOf(row);
      const y = height(row, col);
      bottomPos.push(x, y, z);
      topPos.push(x, y + THICKNESS, z);
      uvs.push(col / ART_W, 1 - row / ART_H);
    }
  }
  const index = (flip: boolean): number[] => {
    const out: number[] = [];
    for (let i = 0; i < nr - 1; i++) {
      for (let j = 0; j < nc - 1; j++) {
        const a = i * nc + j;
        const b = a + 1;
        const c = a + nc;
        const d = c + 1;
        if (flip) out.push(a, b, c, b, d, c);
        else out.push(a, c, b, b, c, d);
      }
    }
    return out;
  };
  const bottom = new THREE.BufferGeometry();
  bottom.setAttribute('position', new THREE.Float32BufferAttribute(bottomPos, 3));
  bottom.setAttribute('uv', new THREE.Float32BufferAttribute(uvs, 2));
  bottom.setIndex(index(false));
  bottom.computeVertexNormals();
  const top = new THREE.BufferGeometry();
  top.setAttribute('position', new THREE.Float32BufferAttribute(topPos, 3));
  top.setAttribute('uv', new THREE.Float32BufferAttribute(uvs, 2));
  top.setIndex(index(true));
  top.computeVertexNormals();

  // The edge: the outline loop (left side down the deck, right side back up), bottom to top.
  const loop: number[] = [];
  for (let i = 0; i < nr; i++) loop.push(i * nc);
  for (let i = nr - 1; i >= 0; i--) loop.push(i * nc + nc - 1);
  const edgePos: number[] = [];
  const edgeUv: number[] = [];
  loop.forEach((v, k) => {
    edgePos.push(bottomPos[v * 3], bottomPos[v * 3 + 1], bottomPos[v * 3 + 2]);
    edgePos.push(topPos[v * 3], topPos[v * 3 + 1], topPos[v * 3 + 2]);
    edgeUv.push(k / loop.length, 0, k / loop.length, 1);
  });
  const edgeIdx: number[] = [];
  const n = loop.length;
  for (let k = 0; k < n; k++) {
    const a = k * 2;
    const b = ((k + 1) % n) * 2;
    edgeIdx.push(a, a + 1, b, b, a + 1, b + 1); // outward: (top - a) x (b - a) points away from the deck
  }
  const edge = new THREE.BufferGeometry();
  edge.setAttribute('position', new THREE.Float32BufferAttribute(edgePos, 3));
  edge.setAttribute('uv', new THREE.Float32BufferAttribute(edgeUv, 2));
  edge.setIndex(edgeIdx);
  edge.computeVertexNormals();
  return { bottom, top, edge };
}

/** Truck centres along the board (the baseplates' middle rows, art pixels 555 and 1529). */
export const TRUCK_Z = [zOf(555), zOf(1529)];
export const WHEEL_RADIUS = 0.0265;
export const WHEEL_WIDTH = 0.034;
export const AXLE_Y = -0.05;
export const AXLE_X = 0.104;
/** The ground under the wheels, in board space. */
export const GROUND_Y = AXLE_Y - WHEEL_RADIUS;

/** Trucks (baseplate, hanger, axle) and four wheels; the wheels' outer faces can carry a wheel graphic. */
export function trucksAndWheels(metal: THREE.Material, wheel: THREE.Material, face: THREE.Material): THREE.Group {
  const g = new THREE.Group();
  const base = new THREE.BoxGeometry(0.045, 0.012, 0.07);
  const hanger = new THREE.CylinderGeometry(0.011, 0.014, 0.17, 20);
  hanger.rotateZ(Math.PI / 2);
  const axle = new THREE.CylinderGeometry(0.004, 0.004, 0.23, 10);
  axle.rotateZ(Math.PI / 2);
  const riser = new THREE.CylinderGeometry(0.012, 0.018, 0.034, 16);
  const wheelGeo = new THREE.CylinderGeometry(WHEEL_RADIUS, WHEEL_RADIUS, WHEEL_WIDTH, 40, 1, true);
  wheelGeo.rotateZ(Math.PI / 2);
  const capGeo = new THREE.CircleGeometry(WHEEL_RADIUS, 40);
  for (const z of TRUCK_Z) {
    const b = new THREE.Mesh(base, metal);
    b.position.set(0, -0.006, z);
    const r = new THREE.Mesh(riser, metal);
    r.position.set(0, -0.026, z);
    const h = new THREE.Mesh(hanger, metal);
    h.position.set(0, AXLE_Y + 0.006, z);
    const a = new THREE.Mesh(axle, metal);
    a.position.set(0, AXLE_Y, z);
    g.add(b, r, h, a);
    for (const side of [-1, 1]) {
      const w = new THREE.Mesh(wheelGeo, wheel);
      w.position.set(side * AXLE_X, AXLE_Y, z);
      const outer = new THREE.Mesh(capGeo, face);
      outer.position.set(side * (AXLE_X + WHEEL_WIDTH / 2), AXLE_Y, z);
      outer.rotation.y = (side * Math.PI) / 2;
      const inner = new THREE.Mesh(capGeo, wheel);
      inner.position.set(side * (AXLE_X - WHEEL_WIDTH / 2), AXLE_Y, z);
      inner.rotation.y = (-side * Math.PI) / 2;
      g.add(w, outer, inner);
    }
  }
  return g;
}
