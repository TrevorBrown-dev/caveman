<script lang="ts">
  // The board cam widget: the game's own skateboard posed as the board under the skater is this instant, in a
  // see-through window (WidgetFrame.svelte around it). The model is the game's (the stock deck, trucks and wheels on
  // the board rig, with their textures, built from the install by the back end: `boardModel`), and the pose comes from
  // outside the game (`caveman-trainer pose --bones`, read-only, relayed as `board-pose` events through
  // lib/telemetry.svelte.ts): the board's turn from its entity, and the trucks' steering and the wheels' spin from the
  // rig's bones. The picture on the board (the DLL's label, resolved by the back end: `boardArt`) and the OBS link's
  // video frames (the DLL's ring, `boardFrame`) are read by this page itself, never pushed to it. Nothing is drawn in
  // the game itself; OBS captures this window ("Caveman board cam") and composes it over the game.
  //
  // Options (lib/widgets.ts): elevation and azimuth of the viewpoint (degrees), zoom, turn (camera: as the game camera
  // sees the board, so a flip looks as on screen; world: in the world's frame), mirror (left and right swapped), nose
  // (the nose the other way), art (a picture for the deck while the game is not running), rate (reads a second).
  import { onMount, untrack } from 'svelte';
  import * as THREE from 'three';
  import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
  import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
  import * as maps from '../../lib/preview/maps';
  import { DECK_LANE, LANES, gritty, lanesOf } from '../../lib/preview/board';
  import { boardArt, boardFrame, boardModel, imageBitmap, readBytes } from '../../lib/api';
  import * as live from '../../lib/live';
  import type { PoseFrame } from '../../lib/live';
  import { onFrame, start, stop, telemetry } from '../../lib/telemetry.svelte';
  import { report, type WidgetProps } from '../../lib/widgets';

  let { options, onstatus }: WidgetProps = $props();

  const num = (key: string, fallback: number): number => {
    const v = options[key];
    return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
  };
  // the view: from the options, then changed by a drag (elevation and azimuth) or the wheel (zoom) in the window,
  // which goes back to the app's settings (`report`); the app's own sliders come in through the options. Zoom 1 fits
  // the whole board whichever way it turns (see `fitDistance` below).
  let elevation = $state(num('elevation', -30));
  let azimuth = $state(num('azimuth', 20));
  let zoom = $state(Math.min(Math.max(num('zoom', 1), 0.3), 3));
  const turn = $derived(options.turn === 'world' ? 'world' : 'camera');
  const mirror = $derived(options.mirror === true);
  const nose = $derived(options.nose === true ? -1 : 1);
  const artOption = $derived(typeof options.art === 'string' && options.art !== '' ? options.art : null);
  const rate = Math.min(Math.max(num('rate', 60), 1), 240);
  $effect(() => {
    // the sliders in the app moved: follow them (a drag here changes the same values; only the options' change is
    // tracked, so the window's own changes do not snap back)
    const e = num('elevation', -30);
    const a = num('azimuth', 20);
    const z = Math.min(Math.max(num('zoom', 1), 0.3), 3);
    const [le, la, lz] = untrack(() => [elevation, azimuth, zoom]);
    if (Math.abs(e - le) > 0.5 || Math.abs(a - la) > 0.5 || Math.abs(z - lz) > 0.005) {
      elevation = e;
      azimuth = a;
      zoom = z;
      placeCamera?.();
    }
  });

  /** The lanes posed from the stream: the deck and everything under it (the trucks, their effectors, the wheels). */
  const POSED_LANES = [3, 4, 5, 6, 7, 8, 9, 10, 11];

  let host: HTMLDivElement | undefined = $state();
  let modelStatus = $state<string | null>('building the board from the game');
  let hasPose = $state(false);
  $effect(() => {
    onstatus(modelStatus ?? (telemetry.live && hasPose ? null : telemetry.status));
  });
  /** Called by the scene once it exists: put the camera where the view says. */
  let placeCamera: (() => void) | null = null;
  let viewTimer: ReturnType<typeof setTimeout> | null = null;
  function viewChanged(): void {
    placeCamera?.();
    if (viewTimer) clearTimeout(viewTimer);
    viewTimer = setTimeout(() => void report('boardcam', { elevation: Math.round(elevation), azimuth: Math.round(azimuth), zoom: Math.round(zoom * 100) / 100 }), 150);
  }
  let drag: { x: number; y: number; el: number; az: number } | null = null;
  function onPointerDown(e: PointerEvent): void {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, el: elevation, az: azimuth };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent): void {
    if (!drag) return;
    azimuth = ((drag.az + (e.clientX - drag.x) * 0.5 + 540) % 360) - 180;
    elevation = Math.max(-89, Math.min(89, drag.el - (e.clientY - drag.y) * 0.5));
    viewChanged();
  }
  function onPointerUp(): void {
    drag = null;
  }
  function onWheel(e: WheelEvent): void {
    e.preventDefault();
    zoom = Math.max(0.3, Math.min(3, zoom * Math.pow(1.1, -e.deltaY / 100)));
    viewChanged();
  }

  type V = [number, number, number];
  const dot = (a: number[], b: number[]): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const det = (r: V, u: V, f: V): number => r[0] * (u[1] * f[2] - u[2] * f[1]) - r[1] * (u[0] * f[2] - u[2] * f[0]) + r[2] * (u[0] * f[1] - u[1] * f[0]);

  /**
   * The board's rotation for the model (x across, y up, z along the board towards the nose: the deck bone's frame)
   * from the game's axes: in the game camera's frame (x right, y up, z towards the viewer: three.js camera space) or
   * the world's.
   */
  function rotationOf(p: PoseFrame): THREE.Matrix4 | null {
    const b = p.board;
    const c = p.camera;
    if (!b || !c) return null;
    let r: V, u: V, f: V;
    if (turn === 'camera') {
      const to = (v: number[]): V => [dot(v, c.right), dot(v, c.up), dot(v, c.back)];
      r = to(b.right);
      u = to(b.up);
      f = to(b.forward);
    } else {
      r = [b.right[0], b.right[1], b.right[2]];
      u = [b.up[0], b.up[1], b.up[2]];
      f = [b.forward[0], b.forward[1], b.forward[2]];
    }
    // a frame of the other handedness than three.js's would show the board mirrored: its z is turned around
    if (det(r, u, f) < 0) {
      r[2] = -r[2];
      u[2] = -u[2];
      f[2] = -f[2];
    }
    if (mirror) {
      r = [-r[0], r[1], r[2]];
      u = [-u[0], u[1], u[2]];
      f = [-f[0], f[1], f[2]];
      r = [-r[0], -r[1], -r[2]];
    }
    const m = new THREE.Matrix4();
    m.makeBasis(new THREE.Vector3(...r), new THREE.Vector3(...u), new THREE.Vector3(f[0] * nose, f[1] * nose, f[2] * nose));
    return m;
  }

  /** A lane's pose from the stream: scale, rotation and position in its parent's frame. */
  interface LaneTarget {
    position: THREE.Vector3;
    quaternion: THREE.Quaternion;
    scale: THREE.Vector3;
  }

  onMount(() => {
    const el = host!;
    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, premultipliedAlpha: true });
    renderer.setClearColor(0x000000, 0);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.0;
    el.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(28, 1, 0.01, 50);
    // a room's light for the metal and the gloss (the game lights the board with its world; this is the one thing
    // the window does not copy), plus a key and a fill for shape
    const pmrem = new THREE.PMREMGenerator(renderer);
    scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    scene.add(new THREE.HemisphereLight('#dfe6ff', '#2a241f', 0.35));
    const key = new THREE.DirectionalLight('#ffffff', 1.1);
    key.position.set(1.5, 2.0, 2.5);
    const fill = new THREE.DirectionalLight('#ffffff', 0.35);
    fill.position.set(-2.0, -1.5, 1.0);
    scene.add(key, fill);

    // the board: the deck bone's node (with the trucks and wheels under it) in a group that takes the entity's turn
    const board = new THREE.Group();
    scene.add(board);
    const laneNodes: (THREE.Object3D | null)[] = LANES.map(() => null);
    /** Each posed lane's rotation at bind (the model's nodes as loaded): what the game's animation gives before its
     * own pass spins the wheels and steers the trucks, which a stream frame read in that window shows (`atBind`). */
    const laneBind = new Map<number, THREE.Quaternion>();
    const deckMaterials: THREE.MeshStandardMaterial[] = [];
    const stockDeckMaps: (THREE.Texture | null)[] = [];
    let radius = 0.45;
    let artTexture: THREE.Texture | null = null;
    let artPath: string | null = null;
    let artSeq = 0;
    /** The OBS link's video on the board while its frames come (`boardFrame`); it covers the still. */
    let videoTexture: THREE.DataTexture | null = null;
    const applyArt = (): void => {
      deckMaterials.forEach((m, i) => {
        const stock = stockDeckMaps[i];
        const shown = videoTexture ?? artTexture;
        if (shown && stock) {
          // the picture goes exactly where the game's graphic texture goes: its UV set (the second; a new texture
          // would sample the first, the wood's layout, and show the right half of the picture across the deck) and
          // its transform
          shown.channel = stock.channel;
          shown.offset.copy(stock.offset);
          shown.repeat.copy(stock.repeat);
          shown.rotation = stock.rotation;
          shown.center.copy(stock.center);
        }
        m.map = shown ?? stock;
        m.needsUpdate = true;
      });
    };
    // the deck art: the picture on the board in the game, read from the back end by this window itself (`boardArt`:
    // the DLL's label resolved to the fitted PNG; nothing is pushed here, so a window opened late cannot miss it),
    // or the options' picture while the game is not running with the DLL in. The fitted art is the deck's graphic
    // texture (512 x 2048), so it goes on the graphic layer exactly as the game's: by the mesh's second UV set, the
    // image's first row at v = 0 (no flip, as glTF).
    const setArt = (path: string | null): void => {
      if (path === artPath) return;
      artPath = path;
      const seq = ++artSeq;
      if (!path) {
        artTexture?.dispose();
        artTexture = null;
        applyArt();
        return;
      }
      imageBitmap(path)
        .then((image) => {
          if (seq !== artSeq) return;
          const { canvas } = maps.artCanvas(image);
          const texture = new THREE.CanvasTexture(canvas);
          texture.flipY = false;
          texture.colorSpace = THREE.SRGBColorSpace;
          texture.anisotropy = Math.min(8, renderer.capabilities.getMaxAnisotropy());
          texture.wrapS = THREE.RepeatWrapping;
          texture.wrapT = THREE.RepeatWrapping;
          artTexture?.dispose();
          artTexture = texture;
          applyArt();
        })
        .catch(() => {
          modelStatus = 'the deck art could not be read';
        });
    };

    // the viewpoint: fixed; the board turns in front of it. At zoom 1 the whole board fits whichever way it turns:
    // the distance puts its bounding sphere (deck, trucks and wheels, about the deck bone) just inside the view's
    // shorter side; zoom > 1 comes closer (the ends then leave the window during a flip).
    const fitDistance = (): number => {
      const half = THREE.MathUtils.degToRad(camera.fov / 2);
      const halfX = Math.atan(Math.tan(half) * camera.aspect);
      return (radius * 1.04) / Math.sin(Math.min(half, halfX));
    };
    const place = (): void => {
      const e = THREE.MathUtils.degToRad(elevation);
      const a = THREE.MathUtils.degToRad(azimuth);
      const distance = fitDistance() / zoom;
      camera.position.set(distance * Math.cos(e) * Math.sin(a), distance * Math.sin(e), distance * Math.cos(e) * Math.cos(a));
      camera.lookAt(0, 0, 0);
    };
    place();
    placeCamera = place;

    // the model: built by the back end from the game for the parts the profile says are equipped; asked for again
    // now and then, and loaded anew when its path changes (another deck, grip, trucks or wheels equipped)
    const anisotropy = Math.min(8, renderer.capabilities.getMaxAnisotropy());
    let disposed = false;
    let modelPath: string | null = null;
    let loading = false;
    const disposeDeck = (): void => {
      const deck = laneNodes[DECK_LANE];
      if (!deck) return;
      board.remove(deck);
      deck.traverse((o) => {
        const mesh = o as THREE.Mesh;
        if (!mesh.isMesh) return;
        mesh.geometry.dispose();
        const m = mesh.material as THREE.MeshStandardMaterial;
        for (const t of [m.map, m.normalMap, m.roughnessMap, m.metalnessMap, m.aoMap]) {
          if (t && t !== artTexture && t !== videoTexture) t.dispose();
        }
        m.dispose();
      });
      laneNodes.fill(null);
      deckMaterials.length = 0;
      stockDeckMaps.length = 0;
      laneTargets.clear();
    };
    const loadModel = async (force = false): Promise<void> => {
      if (loading || disposed) return;
      loading = true;
      try {
        const path = await boardModel(force);
        if (disposed || (path === modelPath && !force)) return;
        const bytes = await readBytes(path);
        const gltf = await new GLTFLoader().parseAsync(bytes, '');
        if (disposed) return;
        disposeDeck();
        modelPath = path;
        const lanes = lanesOf(gltf.scene);
        lanes.forEach((name, i) => {
          laneNodes[i] = gltf.scene.getObjectByName(name) ?? null;
        });
        const deck = laneNodes[DECK_LANE];
        if (!deck) throw new Error('the model has no deck bone');
        laneBind.clear();
        for (const lane of POSED_LANES) {
          const node = laneNodes[lane];
          if (node) laneBind.set(lane, node.quaternion.clone());
        }
        gltf.scene.traverse((o) => {
          const mesh = o as THREE.Mesh;
          if (!mesh.isMesh) return;
          mesh.frustumCulled = false;
          const m = mesh.material as THREE.MeshStandardMaterial;
          for (const t of [m.map, m.normalMap, m.roughnessMap, m.metalnessMap, m.aoMap]) {
            if (t) t.anisotropy = anisotropy;
          }
          m.envMapIntensity = 0.7;
          if (m.userData?.role === 'graphic' && !deckMaterials.includes(m)) {
            deckMaterials.push(m);
            stockDeckMaps.push(m.map);
          }
          if (m.userData?.role === 'grip') gritty(m);
        });
        // the deck node leaves the rig's trajectory nodes behind: its frame is the board's, and the group turns it
        deck.position.set(0, 0, 0);
        deck.quaternion.identity();
        deck.scale.set(1, 1, 1);
        board.add(deck);
        board.updateMatrixWorld(true);
        const sphere = new THREE.Box3().setFromObject(board).getBoundingSphere(new THREE.Sphere());
        radius = Math.max(sphere.radius + sphere.center.length(), 0.1);
        place();
        modelStatus = null;
        hasPose = false;
        applyArt();
      } catch (err) {
        modelStatus = `the board model: ${String(err)}`;
      } finally {
        loading = false;
      }
    };
    void loadModel();
    const modelTimer = setInterval(() => void loadModel(), 4000);
    setArt(artOption);
    // the picture on the board: asked at start, every few seconds, and at once when the app says it changed
    let artBusy = false;
    const checkArt = async (force = false): Promise<void> => {
      if (artBusy || disposed) return;
      artBusy = true;
      try {
        const a = await boardArt(force);
        if (disposed) return;
        // the DLL's word while it answers; the options' picture while the game is not running; otherwise (the DLL
        // not in yet, a status that failed this once) whatever is shown stays
        if (a.injected) setArt(a.path);
        else if (a.how === 'the game is not running') setArt(artOption);
      } catch {
        // the next check
      } finally {
        artBusy = false;
      }
    };
    void checkArt(true);
    const artTimer = setInterval(() => void checkArt(), 3000);
    const unlistenNudge = live.onBoardcamNudge(() => void checkArt(true));
    // the OBS link's video: the frames the DLL puts on the board, from the same ring (`boardFrame`), on a texture of
    // their own over the still; the still again when they stop. The level of the chain: the full 512 x 2048 only for
    // a big window, else the half (a quarter of the bytes over the bridge). Asked often while frames come, seldom
    // while none do.
    let frameSeq = 0;
    let frameAt = 0;
    let frameBusy = false;
    let frameTimer: ReturnType<typeof setTimeout> | null = null;
    const pollFrame = async (): Promise<void> => {
      if (disposed) return;
      if (!frameBusy) {
        frameBusy = true;
        try {
          const level = Math.max(el.clientWidth, el.clientHeight) * window.devicePixelRatio > 900 ? 0 : 1;
          const f = await boardFrame(frameSeq, level);
          if (disposed) return;
          if (f) {
            frameSeq = f.seq;
            frameAt = performance.now();
            if (!videoTexture || videoTexture.image.width !== f.width || videoTexture.image.height !== f.height) {
              videoTexture?.dispose();
              videoTexture = new THREE.DataTexture(f.rgba, f.width, f.height, THREE.RGBAFormat, THREE.UnsignedByteType);
              videoTexture.flipY = false;
              videoTexture.colorSpace = THREE.SRGBColorSpace;
              videoTexture.wrapS = THREE.RepeatWrapping;
              videoTexture.wrapT = THREE.RepeatWrapping;
              videoTexture.magFilter = THREE.LinearFilter;
              videoTexture.minFilter = THREE.LinearMipmapLinearFilter;
              videoTexture.generateMipmaps = true;
              videoTexture.anisotropy = anisotropy;
              applyArt();
            } else {
              (videoTexture.image as { data: Uint8Array }).data = f.rgba;
            }
            videoTexture.needsUpdate = true;
          } else if (videoTexture && performance.now() - frameAt > 1500) {
            videoTexture.dispose();
            videoTexture = null;
            applyArt();
          }
        } catch {
          // the next poll
        } finally {
          frameBusy = false;
        }
      }
      if (!disposed) frameTimer = setTimeout(() => void pollFrame(), videoTexture ? 16 : 400);
    };
    void pollFrame();

    // the pose: the newest frame is the target; the drawn pose eases to it (the stream and the screen have their own
    // rates): the entity's turn on the group, each posed lane's scale, rotation and position on its node
    const target = new THREE.Quaternion();
    const shown = new THREE.Quaternion();
    const laneTargets = new Map<number, LaneTarget>();
    // The game writes the rig's pose in two passes: the animation's (the wheels and the trucks at their bind
    // rotation), then its own spin of the wheels and steer of the trucks. The stream reads from outside at moments of
    // its own and catches the pose between the passes about one frame in six: all four wheels snapped back to bind at
    // once, which showed as wheels twitching on a board standing still. Such a frame is skipped (the lanes keep their
    // targets); a run of them longer than a second is the real thing (wheels that never spun since the board was
    // made) and goes through.
    const WHEEL_LANES = [6, 7, 10, 11];
    const probe = new THREE.Quaternion();
    let bindRun = 0;
    const atBind = (bones: NonNullable<PoseFrame['bones']>): boolean => {
      let n = 0;
      for (const lane of WHEEL_LANES) {
        const bind = laneBind.get(lane);
        const r = bones[lane]?.[1];
        if (!bind || !r) return false;
        probe.set(r[0], r[1], r[2], r[3]);
        if (Math.abs(probe.dot(bind)) > 1 - 1e-5) n++;
      }
      if (n < WHEEL_LANES.length) {
        bindRun = 0;
        return false;
      }
      bindRun++;
      return bindRun < rate;
    };
    const onPose = (p: PoseFrame): void => {
      if (p.done || p.error) {
        if (p.done) hasPose = false;
        return;
      }
      const m = rotationOf(p);
      if (!m) {
        hasPose = false;
        return;
      }
      target.setFromRotationMatrix(m);
      const bones = p.bones;
      if (bones && bones.length >= 12 && !atBind(bones)) {
        for (const lane of POSED_LANES) {
          const [s, q, t] = bones[lane];
          let lt = laneTargets.get(lane);
          if (!lt) {
            lt = { position: new THREE.Vector3(), quaternion: new THREE.Quaternion(), scale: new THREE.Vector3(1, 1, 1) };
            laneTargets.set(lane, lt);
          }
          lt.position.set(t[0], t[1], t[2]);
          lt.quaternion.set(q[0], q[1], q[2], q[3]);
          lt.scale.set(s[0], s[1], s[2]);
          // the deck's own node is the group's child: only its pose relative to the mount counts
          if (lane === DECK_LANE) lt.position.set(0, 0, 0);
        }
      }
      if (!hasPose) {
        shown.copy(target);
        for (const [lane, lt] of laneTargets) {
          const node = laneNodes[lane];
          if (!node) continue;
          node.position.copy(lt.position);
          node.quaternion.copy(lt.quaternion);
          node.scale.copy(lt.scale);
        }
      }
      hasPose = true;
    };

    const fit = (): void => {
      const w = Math.max(el.clientWidth, 1);
      const h = Math.max(el.clientHeight, 1);
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      place();
    };
    const resize = new ResizeObserver(fit);
    resize.observe(el);
    fit();

    let raf = 0;
    const draw = (): void => {
      shown.slerp(target, 0.55);
      board.quaternion.copy(shown);
      for (const [lane, lt] of laneTargets) {
        const node = laneNodes[lane];
        if (!node) continue;
        node.position.lerp(lt.position, 0.55);
        node.quaternion.slerp(lt.quaternion, 0.55);
        node.scale.lerp(lt.scale, 0.55);
      }
      renderer.render(scene, camera);
      raf = requestAnimationFrame(draw);
    };
    draw();

    const offFrame = onFrame(onPose);
    start(rate);
    return () => {
      disposed = true;
      clearInterval(modelTimer);
      placeCamera = null;
      if (viewTimer) clearTimeout(viewTimer);
      cancelAnimationFrame(raf);
      resize.disconnect();
      offFrame();
      stop();
      void unlistenNudge.then((u) => u());
      clearInterval(artTimer);
      if (frameTimer) clearTimeout(frameTimer);
      artTexture?.dispose();
      videoTexture?.dispose();
      pmrem.dispose();
      renderer.dispose();
    };
  });
</script>

<div class="cam" bind:this={host}></div>
<!-- a drag over the board turns the view, the wheel zooms (the frame's strip at the top moves the window) -->
<div class="orbit" role="presentation" onpointerdown={onPointerDown} onpointermove={onPointerMove} onpointerup={onPointerUp} onpointercancel={onPointerUp} onwheel={onWheel}></div>

<style>
  .cam {
    position: fixed;
    inset: 0;
  }
  .cam :global(canvas) {
    display: block;
    width: 100%;
    height: 100%;
  }
  .orbit {
    position: fixed;
    inset: 0;
    cursor: grab;
    touch-action: none;
  }
  .orbit:active {
    cursor: grabbing;
  }
</style>
