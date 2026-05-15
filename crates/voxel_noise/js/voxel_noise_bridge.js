// voxel_noise JS Bridge - Per-Worker Initialization
//
// Each JS context (main thread or worker) initializes its own
// FastNoise2 Emscripten module instance.
//
// Uses top-level await to ensure module is ready before any exports are used.

let module = null;

// Top-level await: Block module from being "ready" until init completes.
// This ensures any code importing this module waits for initialization.
try {
  // Compute module URL relative to this script's location
  // Script: /voxel_world/snippets/voxel_bevy-xxx/js/voxel_noise_bridge.js
  // Target: /voxel_world/dist/voxel_noise.js
  // import.meta.url works reliably in both main thread and workers
  const moduleUrl = new URL('../../../dist/voxel_noise.js', import.meta.url).href;
  const { default: createVoxelNoiseModule } = await import(moduleUrl);
  module = await createVoxelNoiseModule();
  console.log('[voxel_noise] Module initialized in context:', typeof WorkerGlobalScope !== 'undefined' ? 'worker' : 'main');
} catch (e) {
  console.error('[voxel_noise] Failed to initialize:', e);
  throw e;
}

/**
 * Wait for initialization (no-op now since top-level await handles it).
 */
export async function vx_init() {
  return Promise.resolve();
}

/**
 * Create a noise node from an encoded node tree string.
 */
export function vx_create(encoded) {
  const len = module.lengthBytesUTF8(encoded) + 1;
  const strPtr = module._malloc(len);
  module.stringToUTF8(encoded, strPtr, len);

  const handle = module._vx_noise_create(strPtr);
  module._free(strPtr);

  return handle;
}

/**
 * Create a noise node from a preset ID.
 * Returns handle (0 on failure / unknown preset).
 */
export function vx_create_preset(presetId) {
  return module._vx_noise_create_preset(presetId);
}

/**
 * Generate 3D noise and return as Float32Array.
 */
export function vx_gen_3d(handle, xOff, yOff, zOff, xCnt, yCnt, zCnt, xStep, yStep, zStep, seed) {
  const count = xCnt * yCnt * zCnt;
  const outPtr = module._malloc(count * 4);

  module._vx_noise_gen_3d(
    handle, outPtr,
    xOff, yOff, zOff,
    xCnt, yCnt, zCnt,
    xStep, yStep, zStep,
    seed
  );

  const result = new Float32Array(module.HEAPF32.buffer, outPtr, count).slice();
  module._free(outPtr);

  return result;
}

/**
 * Generate 2D noise and return as Float32Array.
 */
export function vx_gen_2d(handle, xOff, yOff, xCnt, yCnt, xStep, yStep, seed) {
  const count = xCnt * yCnt;
  const outPtr = module._malloc(count * 4);

  module._vx_noise_gen_2d(
    handle, outPtr,
    xOff, yOff,
    xCnt, yCnt,
    xStep, yStep,
    seed
  );

  const result = new Float32Array(module.HEAPF32.buffer, outPtr, count).slice();
  module._free(outPtr);

  return result;
}

/**
 * Generate a single 3D noise value at the given coordinates.
 * Returns a scalar f32 — no buffer allocation needed.
 */
export function vx_gen_single_3d(handle, x, y, z, seed) {
  return module._vx_noise_gen_single_3d(handle, x, y, z, seed);
}

/**
 * Destroy a noise node and free its resources.
 */
export function vx_destroy(handle) {
  if (module && handle) {
    module._vx_noise_destroy(handle);
  }
}
