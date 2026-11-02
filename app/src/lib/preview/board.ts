// The game's skateboard as the back end builds it from the player's install (caveman_core::boardmodel: a glTF binary
// of the deck, trucks and wheels on the board rig, with their textures): what the windows that draw it share. The
// board cam (widgets/BoardCamWidget.svelte) poses it from the running game; the Studio's preview (scene.ts) shows it
// with the deck being made on it.

import * as THREE from 'three';

/** The rig's bones in the pose's order (caveman_core::boardmodel::LANES); the model's nodes carry these names. */
export const LANES = ['Reference', 'AITrajectory', 'Skateboard_Root', 'Deck', 'Truck_Back', 'eff_truckBack', 'Right_WheelBack', 'Left_WheelBack', 'Truck_Front', 'eff_truckFront', 'Right_WheelFront', 'Left_WheelFront', 'Trajectory', 'Connect', 'Connect2', 'Ground'];
/** The deck's lane: the board's own frame (x across, y up on the grip side, z along the board towards the nose). */
export const DECK_LANE = 3;
/** The wheels' lanes. */
export const WHEEL_LANES = [6, 7, 10, 11];

/** The model's lane names, as its root says (the back end writes them), else the known ones. */
export function lanesOf(scene: THREE.Object3D): string[] {
  return (scene.getObjectByName(LANES[0])?.userData?.caveman?.lanes as string[] | undefined) ?? LANES;
}

/**
 * Grip tape's grit and sparkle on the grip's material: the grain's normal map pressed harder, and a glitter term
 * (the game's grip shader has `sparklestrength` and `sparklescale`): the surface is cut into small cells, each
 * with a random tilt, and a cell whose tilted face looks at the viewer glints, so the glints move as the board
 * turns. Added to the specular light after three.js's own lighting.
 */
export function gritty(m: THREE.MeshStandardMaterial): void {
  m.normalScale.set(1.8, 1.8);
  m.onBeforeCompile = (shader) => {
    shader.uniforms.uSparkleScale = { value: new THREE.Vector2(380, 1520) };
    shader.uniforms.uSparkleStrength = { value: 1.6 };
    shader.fragmentShader = shader.fragmentShader
      .replace(
        '#include <common>',
        `#include <common>
uniform vec2 uSparkleScale;
uniform float uSparkleStrength;
vec3 cavemanHash(vec2 p) {
  vec3 q = vec3(dot(p, vec2(127.1, 311.7)), dot(p, vec2(269.5, 183.3)), dot(p, vec2(419.2, 371.9)));
  return fract(sin(q) * 43758.5453);
}`
      )
      .replace(
        '#include <lights_fragment_end>',
        `#include <lights_fragment_end>
#ifdef USE_MAP
{
  vec2 cell = floor(vMapUv * uSparkleScale);
  vec3 h = cavemanHash(cell);
  vec3 flake = normalize(normal + (h - 0.5) * 0.9);
  vec3 toEye = normalize(vViewPosition);
  float facing = pow(max(dot(flake, toEye), 0.0), 160.0);
  float lit = step(0.90, cavemanHash(cell + 17.0).x);
  reflectedLight.directSpecular += vec3(facing * lit * uSparkleStrength);
}
#endif`
      );
  };
  m.customProgramCacheKey = () => 'caveman-grip';
  m.needsUpdate = true;
}
