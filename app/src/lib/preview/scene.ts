// The 3D preview: the deck with its finish, emboss, holo, glow and colour cycle, trucks and wheels, and a ride mode
// that shows the board's effects (aura light and trails, the solid rainbow trail). The board is the game's own once
// the back end has built it from the install (`setGameModel`: the deck mesh, black grip, the default trucks, the
// natural wheels or the wheel being looked at; the deck art goes on the deck's graphic layer, as in the game); until
// then, or without the game, a board drawn here stands in. The materials approximate the game's shaders with
// three.js's; the in-game look is the reference.

import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js';
import { EffectComposer } from 'three/examples/jsm/postprocessing/EffectComposer.js';
import { RenderPass } from 'three/examples/jsm/postprocessing/RenderPass.js';
import { UnrealBloomPass } from 'three/examples/jsm/postprocessing/UnrealBloomPass.js';
import { OutputPass } from 'three/examples/jsm/postprocessing/OutputPass.js';

import type { Settings } from '../types';
import { glowFactor } from '../effects';
import { AXLE_Y, GROUND_Y, TRUCK_Z, WHEEL_RADIUS, deckGeometries, trucksAndWheels } from './deck';
import { DECK_LANE, WHEEL_LANES, gritty, lanesOf } from './board';
import * as maps from './maps';

export type Mode = 'showcase' | 'ride';

const RIDE_SPEED = 5.5; // metres a second
const TRAIL_LIFE = 1.2; // seconds, as Caveman's rainbow trail (vfx NYAN_LIFE)
const NYAN_WIDTH = 0.28;
const NYAN_HEIGHT = 0.07; // above the ground, as Caveman attaches it
const NYAN_STEP = 0.32; // metres per band step (the game's step length is not known)
const AURA_WIDTH = 0.06;
const EXPOSURE = 0.95;
/**
 * The lights on the board at the light slider's 100% (the room's reflection, the key light, the sky): a print deck's
 * art comes out about as bright as the picture itself (measured on a print: 0.62 mean value on the board, 0.63 in the
 * PNG). They were half as bright again before 2026-11-02, which washed the colours out (saturation 0.40 against 0.61).
 */
const ROOM_LIGHT = 0.6;
const KEY_LIGHT = 0.72;
const SKY_LIGHT = 0.23;
/** The light slider's range: a share of the lights above. */
export const LIGHT_MIN = 0.3;
export const LIGHT_MAX = 2;
const SHOWCASE_BG = '#121016'; // the app's background, so the view has no edge
const RIDE_BG = '#0d0c12';

interface TrailPoint {
  x: number;
  z: number;
  age: number;
}

/** Where the board's parts are, in board space: what the effects are placed by. */
interface Dims {
  /** The trucks' centres along the board, nose first. */
  truckZ: number[];
  axleY: number;
  /** The ground under the wheels. */
  groundY: number;
  wheelRadius: number;
}

/** The game's board, loaded: its node (the deck's, under a holder that centres it) and what it is made of. */
interface GameBoard {
  holder: THREE.Group;
  /** The game's graphic material (its textures' placement on the second UV set, its occlusion and normal map). */
  graphic: THREE.MeshStandardMaterial;
  /** The wheels' nodes, their rotation as loaded and the board's axle (x) in each one's frame, for rolling. */
  wheels: { node: THREE.Object3D; bind: THREE.Quaternion; axle: THREE.Vector3 }[];
  dispose: () => void;
}

/** The art's most vivid colour at full brightness (mixins.art_colour): an aura's colour when none is given. */
export function artColour(pixels: Uint8ClampedArray): string {
  const samples: [number, number, number, number][] = [];
  for (let y = 0; y < maps.H; y += 8) {
    for (let x = 0; x < maps.W; x += 8) {
      const i = (y * maps.W + x) * 4;
      const r = pixels[i] / 255;
      const g = pixels[i + 1] / 255;
      const b = pixels[i + 2] / 255;
      const mx = Math.max(r, g, b);
      const mn = Math.min(r, g, b);
      samples.push([(mx - mn) * mx, r, g, b]);
    }
  }
  const best = Math.max(...samples.map((s) => s[0]));
  if (best < 0.15) return '#ffffff';
  const sorted = samples.map((s) => s[0]).sort((a, b) => a - b);
  const cut = sorted[Math.floor(sorted.length * 0.9)];
  const top = samples.filter((s) => s[0] >= cut);
  const mean = [1, 2, 3].map((k) => top.reduce((a, s) => a + s[k], 0) / top.length);
  const c = new THREE.Color(mean[0], mean[1], mean[2]);
  const hsl = { h: 0, s: 0, l: 0 };
  c.getHSL(hsl);
  // hsv(h, 0.85, 1) as hsl
  const v = 1;
  const sv = 0.85;
  const l = v * (1 - sv / 2);
  const sl = l === 0 || l === 1 ? 0 : (v - l) / Math.min(l, 1 - l);
  return `#${new THREE.Color().setHSL(hsl.h, sl, l).getHexString()}`;
}

/** The game patch's hue rotation about the grey axis (shaderpatch.patch), for the cycle effect. */
const CYCLE_GLSL = `
uniform float uCycleSpeed;
uniform float uTime;
vec3 cavemanCycle(vec3 c) {
  if (uCycleSpeed <= 0.0) return c;
  float a = 6.2831853 * fract(uTime * uCycleSpeed);
  float cs = cos(a);
  float ks = sin(a) * 0.57735026;
  float grey = (c.r + c.g + c.b) / 3.0 * (1.0 - cs);
  return clamp(vec3(c.r * cs + (c.b - c.g) * ks + grey,
                    c.g * cs + (c.r - c.b) * ks + grey,
                    c.b * cs + (c.g - c.r) * ks + grey), 0.0, 1.0);
}
`;

export class Preview {
  private renderer: THREE.WebGLRenderer;
  private composer: EffectComposer;
  private bloom: UnrealBloomPass;
  private scene = new THREE.Scene();
  private camera: THREE.PerspectiveCamera;
  private controls: OrbitControls;
  private board = new THREE.Group();
  private pose = new THREE.Group();
  private deckMaterial: THREE.MeshPhysicalMaterial;
  private wheelMaterial: THREE.MeshPhysicalMaterial;
  private faceMaterial: THREE.MeshPhysicalMaterial;
  private ground: THREE.Mesh;
  private groundTexture: THREE.CanvasTexture;
  private shadow: THREE.Mesh;
  private auraLights: THREE.PointLight[] = [];
  private keyLight: THREE.DirectionalLight;
  private skyLight: THREE.HemisphereLight;
  private auraHalos: THREE.Sprite[] = [];
  private nyanMesh: THREE.Mesh;
  private auraMeshes: THREE.Mesh[] = [];
  private nyanPoints: TrailPoint[] = [];
  private auraPoints: TrailPoint[][] = [[], []];
  private uniforms = { uTime: { value: 0 }, uCycleSpeed: { value: 0 } };
  private clock = new THREE.Clock();
  private mode: Mode = 'showcase';
  private poseBlend = 0; // 0 showcase, 1 ride
  private travelled = 0;
  private emitTimer = 0;
  private settings: Settings | null = null;
  private auraColour = '#ffffff';
  private artPixels: Uint8ClampedArray | null = null;
  private artTexture: THREE.Texture | null = null;
  private generated: THREE.Texture[] = [];
  private frame = 0;
  private frames = 0;
  private updatedAt = -1;
  private rideTime = 0;
  private resize: ResizeObserver;
  /** The board drawn here, shown until the game's is in. */
  private drawnBoard: THREE.Object3D[] = [];
  private game: GameBoard | null = null;
  private dims: Dims = { truckZ: [...TRUCK_Z], axleY: AXLE_Y, groundY: GROUND_Y, wheelRadius: WHEEL_RADIUS };
  private rolled = 0;
  private modelPending = false;

  constructor(private host: HTMLElement) {
    this.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = EXPOSURE;
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    host.appendChild(this.renderer.domElement);

    this.scene.background = beforeToneMapping(SHOWCASE_BG);
    const pmrem = new THREE.PMREMGenerator(this.renderer);
    this.scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    this.scene.environmentIntensity = ROOM_LIGHT;
    // from the side, so the glossy kicks do not catch it as a band in the showcase
    this.keyLight = new THREE.DirectionalLight('#ffffff', KEY_LIGHT);
    this.keyLight.position.set(2.2, 1.0, 2.0);
    this.skyLight = new THREE.HemisphereLight('#cfd6ff', '#221d18', SKY_LIGHT);
    this.scene.add(this.keyLight, this.skyLight);

    this.camera = new THREE.PerspectiveCamera(32, 1, 0.01, 200);
    this.camera.position.set(0, 0, 1.75);
    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.minDistance = 0.3;
    this.controls.maxDistance = 6;

    // the deck
    const geo = deckGeometries();
    this.deckMaterial = new THREE.MeshPhysicalMaterial({ color: '#d8c3a0', roughness: 0.4, metalness: 0 });
    this.deckMaterial.onBeforeCompile = (shader) => {
      shader.uniforms.uTime = this.uniforms.uTime;
      shader.uniforms.uCycleSpeed = this.uniforms.uCycleSpeed;
      shader.fragmentShader = shader.fragmentShader
        .replace('void main() {', `${CYCLE_GLSL}\nvoid main() {`)
        .replace('#include <map_fragment>', '#include <map_fragment>\n diffuseColor.rgb = cavemanCycle(diffuseColor.rgb);')
        .replace('#include <emissivemap_fragment>', '#include <emissivemap_fragment>\n totalEmissiveRadiance = cavemanCycle(totalEmissiveRadiance);');
    };
    this.deckMaterial.customProgramCacheKey = () => 'caveman-deck';
    const grip = new THREE.MeshStandardMaterial({ map: maps.gripTexture(), roughness: 0.95, metalness: 0 });
    const edge = new THREE.MeshStandardMaterial({ map: maps.edgeTexture(), roughness: 0.6, metalness: 0 });
    this.drawnBoard.push(new THREE.Mesh(geo.bottom, this.deckMaterial), new THREE.Mesh(geo.top, grip), new THREE.Mesh(geo.edge, edge));

    const metal = new THREE.MeshStandardMaterial({ color: '#8d9097', roughness: 0.45, metalness: 1 });
    this.wheelMaterial = new THREE.MeshPhysicalMaterial({ color: '#e9e5dc', roughness: 0.62, metalness: 0 });
    this.faceMaterial = new THREE.MeshPhysicalMaterial({ color: '#e9e5dc', roughness: 0.55, metalness: 0 });
    this.drawnBoard.push(trucksAndWheels(metal, this.wheelMaterial, this.faceMaterial));
    this.board.add(...this.drawnBoard);

    // the aura: a light under each truck, and a soft halo that shows it in the showcase (placed by `placeEffects`)
    const haloTexture = halo();
    for (let k = 0; k < TRUCK_Z.length; k++) {
      const light = new THREE.PointLight('#ffffff', 0, 1.4, 2);
      const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: haloTexture, blending: THREE.AdditiveBlending, depthWrite: false, transparent: true, opacity: 0.6 }));
      sprite.scale.set(0.32, 0.32, 1);
      sprite.visible = false;
      this.auraLights.push(light);
      this.auraHalos.push(sprite);
      this.board.add(light, sprite);
    }

    this.pose.add(this.board);
    this.scene.add(this.pose);

    // riding: the ground, a contact shadow, the trails
    this.groundTexture = maps.groundTexture();
    this.groundTexture.repeat.set(16, 16);
    this.ground = new THREE.Mesh(
      new THREE.PlaneGeometry(40, 40),
      new THREE.MeshStandardMaterial({ map: this.groundTexture, roughness: 0.92, metalness: 0 }),
    );
    this.ground.rotation.x = -Math.PI / 2;
    this.ground.visible = false;
    this.scene.add(this.ground);
    this.shadow = new THREE.Mesh(
      new THREE.PlaneGeometry(0.42, 1.0),
      new THREE.MeshBasicMaterial({ map: shadowTexture(), transparent: true, depthWrite: false, opacity: 0.75 }),
    );
    this.shadow.rotation.x = -Math.PI / 2;
    this.shadow.visible = false;
    this.scene.add(this.shadow);
    this.placeEffects();

    this.nyanMesh = new THREE.Mesh(new THREE.BufferGeometry(), new THREE.MeshBasicMaterial({ map: maps.nyanTexture(), side: THREE.DoubleSide }));
    this.nyanMesh.frustumCulled = false;
    this.scene.add(this.nyanMesh);
    for (let k = 0; k < 2; k++) {
      const m = new THREE.Mesh(
        new THREE.BufferGeometry(),
        new THREE.MeshBasicMaterial({ color: '#ffffff', vertexColors: true, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false, side: THREE.DoubleSide }),
      );
      m.frustumCulled = false;
      this.auraMeshes.push(m);
      this.scene.add(m);
    }

    this.composer = new EffectComposer(this.renderer);
    this.composer.addPass(new RenderPass(this.scene, this.camera));
    this.bloom = new UnrealBloomPass(new THREE.Vector2(512, 512), 0.4, 0.35, 1.2);
    this.bloom.enabled = false;
    this.composer.addPass(this.bloom);
    this.composer.addPass(new OutputPass());

    this.resize = new ResizeObserver(() => this.fit());
    this.resize.observe(host);
    this.fit();
    this.applyPose(0);
    this.loop();
  }

  private fit(): void {
    const w = Math.max(1, this.host.clientWidth);
    const h = Math.max(1, this.host.clientHeight);
    this.renderer.setSize(w, h, false);
    this.composer.setSize(w, h);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  }

  /** The deck's art (the build's fitted 512 x 2048) and settings. */
  setDeck(art: CanvasImageSource | null, settings: Settings | null): void {
    for (const t of this.generated) t.dispose();
    this.generated = [];
    this.artTexture?.dispose();
    this.artTexture = null;
    this.artPixels = null;
    let l: Float32Array | null = null;
    if (art) {
      const { canvas, pixels } = maps.artCanvas(art);
      this.artPixels = pixels;
      const tex = new THREE.CanvasTexture(canvas);
      tex.colorSpace = THREE.SRGBColorSpace;
      tex.anisotropy = 8;
      this.artTexture = tex;
      l = maps.luma(pixels);
    }
    this.settings = settings;
    const m = this.deckMaterial;
    m.map = this.artTexture;
    m.color.set(art ? '#ffffff' : '#d8c3a0');
    const finish = settings?.finish?.kind ?? 'none';
    const weight = settings?.finish?.spot && l ? maps.spotWeight(l) : null;
    const fm = maps.finishMap(finish, weight);
    this.generated.push(fm);
    m.roughnessMap = fm;
    m.metalnessMap = fm;
    m.roughness = 1;
    m.metalness = 1;
    const emboss = settings?.emboss ? settings.emboss.strength : null;
    const nm = l ? maps.normalMap(l, emboss, finish === 'glitter', weight) : null;
    if (nm) this.generated.push(nm);
    m.normalMap = nm;
    m.normalScale.set(1, 1);
    if (finish === 'holo') {
      const { mask, thickness } = maps.holoMaps(settings?.finish?.mode ?? 'pearl', weight);
      this.generated.push(mask, thickness);
      m.iridescence = 1;
      m.iridescenceIOR = 1.45;
      m.iridescenceThicknessRange = [180, 900];
      m.iridescenceMap = mask;
      m.iridescenceThicknessMap = thickness;
    } else {
      m.iridescence = 0;
      m.iridescenceMap = null;
      m.iridescenceThicknessMap = null;
    }
    const glow = settings?.shader?.glow ?? 0;
    if (glow > 0 && this.artTexture) {
      m.emissive.set('#ffffff');
      m.emissiveMap = this.artTexture;
      // the game adds the print x (2^(12 glow) - 1) as light; a gentler curve reads better on a monitor
      m.emissiveIntensity = 0.08 * Math.log2(1 + glowFactor(glow));
      // bloom only for a glowing print, above the brightest lit (not lit-up) surfaces
      this.bloom.enabled = true;
      this.bloom.strength = 0.25 + 0.25 * glow;
      this.bloom.threshold = 1.2;
    } else {
      m.emissive.set('#000000');
      m.emissiveMap = null;
      m.emissiveIntensity = 1;
      this.bloom.enabled = false;
    }
    this.uniforms.uCycleSpeed.value = settings?.shader?.effect === 'cycle' ? settings.shader.speed ?? 0.25 : 0;
    this.placeDeckTextures();
    this.updatedAt = this.frames;
    this.updateBoardEffect();
  }

  /**
   * The deck's textures where the board shown wants them. On the game's deck they go exactly where the game's
   * graphic texture goes: its UV set (the second: the first is the wood's layout) and its transform, the image's
   * first row at v = 0 (as glTF has it); the game's occlusion comes along, and its normal map where no emboss or
   * glitter makes one. On the drawn deck: the first set, flipped as three.js's canvases are.
   */
  private placeDeckTextures(): void {
    const m = this.deckMaterial;
    const stock = this.game?.graphic ?? null;
    const place = stock?.map ?? null;
    const ours = new Set<THREE.Texture>([...this.generated, ...(this.artTexture ? [this.artTexture] : [])]);
    if (!this.artTexture) {
      // no picture (none chosen, or an animated deck): the game's own graphic, the Natural Blank's wood
      m.map = stock?.map ?? null;
      m.color.set(stock ? '#ffffff' : '#d8c3a0');
    }
    if (!m.normalMap || !ours.has(m.normalMap)) {
      // the game's (or none on the drawn deck), with its scale: glTF's green runs the other way to three.js's
      m.normalMap = stock?.normalMap ?? null;
      if (stock) m.normalScale.copy(stock.normalScale);
    } else {
      // ours, made for the drawn deck's flipped rows: on the game's deck the rows run the other way, and so does green
      m.normalScale.y = place ? -Math.abs(m.normalScale.y) : Math.abs(m.normalScale.y);
    }
    m.aoMap = stock?.aoMap ?? null;
    for (const t of ours) {
      t.channel = place ? place.channel : 0;
      if (place) {
        t.offset.copy(place.offset);
        t.repeat.copy(place.repeat);
        t.rotation = place.rotation;
        t.center.copy(place.center);
        t.wrapS = THREE.RepeatWrapping;
        t.wrapT = THREE.RepeatWrapping;
      } else {
        t.offset.set(0, 0);
        t.repeat.set(1, 1);
        t.rotation = 0;
        t.center.set(0, 0);
      }
      const flip = !place;
      if (t.flipY !== flip) {
        t.flipY = flip;
        t.needsUpdate = true;
      }
    }
    m.needsUpdate = true;
  }

  /** The effects' places from where the board's parts are: the aura's lights and halos, the ground, the shadow. */
  private placeEffects(): void {
    this.auraLights.forEach((light, k) => {
      light.position.set(0, this.dims.axleY + 0.012, this.dims.truckZ[k] ?? 0);
      this.auraHalos[k].position.copy(light.position);
    });
    this.ground.position.y = this.dims.groundY - 0.001;
    this.shadow.position.y = this.dims.groundY;
  }

  /** Whether a board model is on its way (screenshots wait for it). */
  expectModel(pending: boolean): void {
    this.modelPending = pending;
  }

  /**
   * The game's board (the back end's glTF binary, `studio_board_model`) in place of the drawn one, or of the game's
   * board shown before (another wheel). The deck node leaves the rig behind (its frame is the board's) and is centred
   * as the drawn deck is: the middle of its bottom at the origin, so the showcase stands it up the same way.
   */
  async setGameModel(bytes: ArrayBuffer): Promise<void> {
    const gltf = await new GLTFLoader().parseAsync(bytes, '');
    const lanes = lanesOf(gltf.scene);
    const deck = gltf.scene.getObjectByName(lanes[DECK_LANE]);
    if (!deck) throw new Error('the board model has no deck bone');
    deck.removeFromParent();
    deck.position.set(0, 0, 0);
    deck.quaternion.identity();
    deck.scale.set(1, 1, 1);
    const holder = new THREE.Group();
    holder.add(deck);
    holder.updateMatrixWorld(true);

    const owned = new Set<{ dispose: () => void }>();
    const graphicMeshes: THREE.Mesh[] = [];
    // (assigned in the callback, which the compiler's narrowing does not follow)
    let graphic = null as THREE.MeshStandardMaterial | null;
    deck.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (!mesh.isMesh) return;
      owned.add(mesh.geometry);
      const mat = mesh.material as THREE.MeshStandardMaterial;
      owned.add(mat);
      for (const t of [mat.map, mat.normalMap, mat.roughnessMap, mat.metalnessMap, mat.aoMap, mat.emissiveMap]) {
        if (!t) continue;
        t.anisotropy = 8;
        owned.add(t);
      }
      if (mat.userData?.role === 'graphic') {
        graphic ??= mat;
        graphicMeshes.push(mesh);
      }
      if (mat.userData?.role === 'grip') gritty(mat);
    });
    if (!graphic) throw new Error('the board model has no deck graphic');

    // centred: the bottom's middle at the origin
    const bottom = new THREE.Box3();
    for (const mesh of graphicMeshes) bottom.expandByObject(mesh);
    holder.position.set(-(bottom.min.x + bottom.max.x) / 2, -bottom.min.y, -(bottom.min.z + bottom.max.z) / 2);
    holder.updateMatrixWorld(true);

    // where the wheels are: the trucks' places, the axles' height, the ground, and each wheel's axle in its own
    // frame (the board's x: a wheel rolling forward turns about it)
    const wheels: GameBoard['wheels'] = [];
    const centres: THREE.Vector3[] = [];
    let groundY = Infinity;
    let radius = 0;
    for (const lane of WHEEL_LANES) {
      const node = deck.getObjectByName(lanes[lane]);
      if (!node) continue;
      const centre = node.getWorldPosition(new THREE.Vector3());
      const box = new THREE.Box3().setFromObject(node);
      centres.push(centre);
      groundY = Math.min(groundY, box.min.y);
      radius = Math.max(radius, centre.y - box.min.y);
      const axle = new THREE.Vector3(1, 0, 0).applyQuaternion(node.getWorldQuaternion(new THREE.Quaternion()).invert());
      wheels.push({ node, bind: node.quaternion.clone(), axle });
    }
    if (centres.length === 4) {
      const front = centres.filter((c) => c.z > 0);
      const back = centres.filter((c) => c.z <= 0);
      const meanZ = (list: THREE.Vector3[]): number => list.reduce((a, c) => a + c.z, 0) / Math.max(list.length, 1);
      this.dims = {
        truckZ: [meanZ(front), meanZ(back)],
        axleY: centres.reduce((a, c) => a + c.y, 0) / centres.length,
        groundY,
        wheelRadius: radius > 0.005 ? radius : WHEEL_RADIUS,
      };
    }

    for (const mesh of graphicMeshes) mesh.material = this.deckMaterial;
    const old = this.game;
    this.game = {
      holder,
      graphic,
      wheels,
      dispose: () => {
        for (const d of owned) {
          if (d !== this.deckMaterial) d.dispose();
        }
      },
    };
    this.board.add(holder);
    for (const o of this.drawnBoard) o.visible = false;
    if (old) {
      this.board.remove(old.holder);
      old.dispose();
    }
    this.rolled = 0;
    this.placeDeckTextures();
    this.placeEffects();
    this.updatedAt = this.frames;
  }

  /** The aura's colour when the settings give none (the art's). */
  resolvedAuraColour(): string {
    return this.settings?.board?.colour ?? (this.artPixels ? artColour(this.artPixels) : '#ffffff');
  }

  private updateBoardEffect(): void {
    const effect = this.settings?.board?.effect ?? 'none';
    this.auraColour = this.resolvedAuraColour();
    const on = effect === 'aura';
    this.auraLights.forEach((l) => {
      l.color.set(this.auraColour);
      l.intensity = on ? 0.9 : 0;
    });
    this.auraHalos.forEach((s) => {
      s.visible = on && this.poseBlend < 0.5;
      (s.material as THREE.SpriteMaterial).color.set(this.auraColour);
    });
    this.nyanPoints = [];
    this.auraPoints = [[], []];
  }

  /** A wheel's colour and face graphic (null: plain). */
  setWheel(colour: string, face: CanvasImageSource | null): void {
    this.wheelMaterial.color.set(colour);
    const old = this.faceMaterial.map;
    if (face) {
      const c = maps.canvas(512, 512);
      const ctx = c.getContext('2d')!;
      ctx.fillStyle = colour;
      ctx.fillRect(0, 0, 512, 512);
      ctx.drawImage(face, 0, 0, 512, 512);
      const t = new THREE.CanvasTexture(c);
      t.colorSpace = THREE.SRGBColorSpace;
      this.faceMaterial.map = t;
      this.faceMaterial.color.set('#ffffff');
    } else {
      this.faceMaterial.map = null;
      this.faceMaterial.color.set(colour);
    }
    old?.dispose();
    this.faceMaterial.needsUpdate = true;
  }

  setMode(mode: Mode): void {
    this.mode = mode;
    this.nyanPoints = [];
    this.auraPoints = [[], []];
  }

  /**
   * How much light falls on the board, as a share of the usual (the light slider): the room's reflection, the key
   * light and the sky together, so the look stays the same, only dimmer or brighter. The background keeps its colour,
   * and the aura's lights and a glowing print keep theirs (they are the deck's own light).
   */
  setLight(share: number): void {
    const k = Math.min(LIGHT_MAX, Math.max(LIGHT_MIN, share));
    this.scene.environmentIntensity = ROOM_LIGHT * k;
    this.keyLight.intensity = KEY_LIGHT * k;
    this.skyLight.intensity = SKY_LIGHT * k;
  }

  /** The view for a mode: the deck standing with its bottom to the camera, or behind the rider. */
  resetView(): void {
    if (this.mode === 'showcase') {
      this.camera.position.set(0, 0, 1.75);
      this.controls.target.set(0, 0, 0);
    } else {
      this.camera.position.set(1.05, 0.5, -1.25);
      this.controls.target.set(0, -0.02, 0.15);
    }
    this.controls.update();
  }

  /** Whether a deck with art has been drawn for a while (and, riding, long enough for the trails): for screenshots. */
  ready(): boolean {
    const drawn = this.artTexture !== null && !this.modelPending && this.updatedAt >= 0 && this.frames - this.updatedAt > 20;
    return drawn && (this.mode !== 'ride' || this.rideTime > TRAIL_LIFE + 0.4);
  }

  /** The current frame as a PNG data URL (for screenshots). */
  snapshot(): string {
    return this.renderer.domElement.toDataURL('image/png');
  }

  private applyPose(t: number): void {
    // showcase: standing, nose up, bottom to the camera; ride: flat, wheels down
    const e = t * t * (3 - 2 * t);
    this.pose.rotation.x = -Math.PI / 2 * (1 - e);
    this.pose.position.y = 0;
    const riding = t > 0.98;
    this.ground.visible = riding;
    this.shadow.visible = riding;
    this.scene.background = beforeToneMapping(riding ? RIDE_BG : SHOWCASE_BG);
    this.scene.fog = riding ? new THREE.Fog(beforeToneMapping(RIDE_BG), 4, 16) : null;
    // the halos show the aura standing up; riding, the lit ground and the trails show it
    const aura = (this.settings?.board?.effect ?? 'none') === 'aura';
    this.auraHalos.forEach((h) => (h.visible = aura && t < 0.5));
  }

  private loop = (): void => {
    this.frame = requestAnimationFrame(this.loop);
    this.frames++;
    const dt = Math.min(0.05, this.clock.getDelta());
    const t = this.clock.elapsedTime;
    this.uniforms.uTime.value = t;
    const target = this.mode === 'ride' ? 1 : 0;
    if (this.poseBlend !== target) {
      const before = this.poseBlend;
      this.poseBlend = target > this.poseBlend ? Math.min(1, this.poseBlend + dt * 2.2) : Math.max(0, this.poseBlend - dt * 2.2);
      this.applyPose(this.poseBlend);
      if (this.poseBlend === target && before !== target) this.resetView();
    }
    if (this.mode === 'ride' && this.poseBlend === 1) {
      this.rideTime += dt;
      this.ride(dt, t);
    } else {
      this.rideTime = 0;
      this.nyanMesh.visible = false;
      this.auraMeshes.forEach((m) => (m.visible = false));
      this.board.position.set(0, 0, 0);
      this.board.rotation.set(0, 0, 0);
    }
    this.controls.update();
    this.composer.render();
  };

  private ride(dt: number, t: number): void {
    // the board carves gently while the ground slides under it
    this.travelled += RIDE_SPEED * dt;
    this.groundTexture.offset.y = (this.travelled / 2.5) % 1;
    const sway = 0.28 * Math.sin(t * 1.3);
    const yaw = 0.28 * 1.3 * Math.cos(t * 1.3) / RIDE_SPEED * 2.2;
    this.board.position.set(sway, 0, 0);
    this.board.rotation.set(0, yaw, -0.06 * Math.cos(t * 1.3));
    this.shadow.position.x = sway;
    // the game's wheels roll with the ground (forward is the nose's way: a turn about the board's x)
    if (this.game) {
      this.rolled = (this.rolled + (RIDE_SPEED * dt) / this.dims.wheelRadius) % (Math.PI * 2);
      const turn = new THREE.Quaternion();
      for (const w of this.game.wheels) w.node.quaternion.copy(w.bind).multiply(turn.setFromAxisAngle(w.axle, this.rolled));
    }
    // trail points stay where they were laid while the world moves back
    const move = RIDE_SPEED * dt;
    for (const p of this.nyanPoints) {
      p.z -= move;
      p.age += dt;
    }
    for (const list of this.auraPoints) {
      for (const p of list) {
        p.z -= move;
        p.age += dt;
      }
    }
    this.emitTimer += dt;
    if (this.emitTimer >= 0.016) {
      this.emitTimer = 0;
      const effect = this.settings?.board?.effect ?? 'none';
      if (effect === 'nyan') this.nyanPoints.push({ x: sway, z: 0, age: 0 });
      if (effect === 'aura') {
        this.dims.truckZ.forEach((z, k) => {
          const local = new THREE.Vector3(0, 0, z).applyEuler(this.board.rotation);
          this.auraPoints[k].push({ x: sway + local.x, z: local.z, age: 0 });
        });
      }
    }
    this.nyanPoints = this.nyanPoints.filter((p) => p.age < TRAIL_LIFE);
    this.auraPoints = this.auraPoints.map((list) => list.filter((p) => p.age < 0.6));
    this.nyanMesh.visible = this.nyanPoints.length > 1;
    if (this.nyanMesh.visible) ribbon(this.nyanMesh, this.nyanPoints, NYAN_WIDTH, this.dims.groundY + NYAN_HEIGHT, this.travelled, null);
    const colour = new THREE.Color(this.auraColour);
    this.auraMeshes.forEach((m, k) => {
      m.visible = this.auraPoints[k].length > 1;
      if (m.visible) ribbon(m, this.auraPoints[k], AURA_WIDTH, this.dims.axleY, this.travelled, colour);
    });
  }

  dispose(): void {
    cancelAnimationFrame(this.frame);
    this.resize.disconnect();
    this.controls.dispose();
    for (const t of this.generated) t.dispose();
    this.artTexture?.dispose();
    this.game?.dispose();
    this.game = null;
    this.renderer.dispose();
    this.renderer.domElement.remove();
  }
}

/** A flat ribbon through trail points (newest last), width across, at a height; nyan (colour null) gets the stepped
 *  band texture, the aura a colour fading with age. */
function ribbon(mesh: THREE.Mesh, points: TrailPoint[], width: number, y: number, travelled: number, colour: THREE.Color | null): void {
  const n = points.length;
  const pos = new Float32Array(n * 2 * 3);
  const uv = new Float32Array(n * 2 * 2);
  const col = new Float32Array(n * 2 * 3);
  const idx: number[] = [];
  for (let i = 0; i < n; i++) {
    const p = points[i];
    const q = points[Math.min(n - 1, i + 1)];
    const o = points[Math.max(0, i - 1)];
    let dx = q.x - o.x;
    let dz = q.z - o.z;
    const len = Math.hypot(dx, dz) || 1;
    dx /= len;
    dz /= len;
    // horizontal, perpendicular to the trail (Caveman's flat ribbon)
    const px = -dz * (width / 2);
    const pz = dx * (width / 2);
    pos.set([p.x - px, y, p.z - pz, p.x + px, y, p.z + pz], i * 6);
    const along = (travelled + p.z) / (NYAN_STEP * 2);
    uv.set([0, along, 1, along], i * 4);
    const fade = colour ? Math.max(0, 1 - p.age / 0.6) : 1;
    const c = colour ?? new THREE.Color('#ffffff');
    col.set([c.r * fade, c.g * fade, c.b * fade, c.r * fade, c.g * fade, c.b * fade], i * 6);
    if (i < n - 1) {
      const a = i * 2;
      idx.push(a, a + 1, a + 2, a + 1, a + 3, a + 2);
    }
  }
  const g = mesh.geometry as THREE.BufferGeometry;
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('uv', new THREE.BufferAttribute(uv, 2));
  g.setAttribute('color', new THREE.BufferAttribute(col, 3));
  g.setIndex(idx);
  g.computeBoundingSphere();
}

/** The colour to give the background (and fog) so it comes out as `hex` after the ACES tone mapping the output
 *  pass applies to everything, as three.js computes it; without this the dark background crushes to black. */
function beforeToneMapping(hex: string): THREE.Color {
  let v = toneCache.get(hex);
  if (!v) {
    const want = new THREE.Color(hex); // linear
    const target = [want.r, want.g, want.b];
    const x = [...target];
    for (let i = 0; i < 400; i++) {
      const out = aces(x);
      for (let k = 0; k < 3; k++) x[k] = Math.max(0, x[k] + (target[k] - out[k]) * 2);
    }
    v = x;
    toneCache.set(hex, v);
  }
  return new THREE.Color().setRGB(v[0], v[1], v[2], THREE.LinearSRGBColorSpace);
}
const toneCache = new Map<string, number[]>();

/** three.js's ACESFilmicToneMapping (tonemapping_pars_fragment), on a linear colour. */
function aces([r, g, b]: number[]): number[] {
  const s = EXPOSURE / 0.6;
  const x = [r * s, g * s, b * s];
  const i = [
    0.59719 * x[0] + 0.35458 * x[1] + 0.04823 * x[2],
    0.076 * x[0] + 0.90834 * x[1] + 0.01566 * x[2],
    0.0284 * x[0] + 0.13383 * x[1] + 0.83777 * x[2],
  ];
  const f = i.map((c) => (c * (c + 0.0245786) - 0.000090537) / (c * (0.983729 * c + 0.432951) + 0.238081));
  return [
    1.60475 * f[0] - 0.53108 * f[1] - 0.07367 * f[2],
    -0.10208 * f[0] + 1.10813 * f[1] - 0.00605 * f[2],
    -0.00327 * f[0] - 0.07276 * f[1] + 1.07602 * f[2],
  ].map((c) => Math.min(1, Math.max(0, c)));
}

function halo(): THREE.CanvasTexture {
  const c = maps.canvas(128, 128);
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(64, 64, 0, 64, 64, 64);
  g.addColorStop(0, 'rgba(255,255,255,0.9)');
  g.addColorStop(0.35, 'rgba(255,255,255,0.35)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 128, 128);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

function shadowTexture(): THREE.CanvasTexture {
  const c = maps.canvas(64, 160);
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(32, 80, 4, 32, 80, 78);
  g.addColorStop(0, 'rgba(0,0,0,0.85)');
  g.addColorStop(1, 'rgba(0,0,0,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 160);
  return new THREE.CanvasTexture(c);
}
