// Trunk `data-initializer` for the bevy-naadf dev build.
//
// Trunk 0.21 supports a `data-initializer="<file>.mjs"` attribute on the
// `<link rel="rust">` tag — the file is an ES module exporting a default
// function that, when called, returns an object with optional lifecycle
// callbacks (`onStart`, `onProgress`, `onComplete`, `onSuccess`,
// `onFailure`) that Trunk's `__trunkInitializer` invokes around its
// `await init(...)` call.
//
// We hook `onSuccess` — fired after `await init(...)` resolves and BEFORE
// Trunk dispatches `TrunkApplicationStarted` — to call
// `wasmBindings.initThreadPool(navigator.hardwareConcurrency)` so the
// `wasm-bindgen-rayon` worker pool is ready before Bevy's Update systems
// run `rayon::spawn`. Without this, the first `rayon::spawn` call panics
// with "no thread pool installed".
//
// `onSuccess` is allowed to be async (Trunk's `__trunkInitializer` awaits
// the chained promise). The TrunkApplicationStarted dispatch only happens
// after the awaited chain resolves, so the pool is guaranteed ready before
// Bevy's startup systems fire.
//
// In production, the CI-patched `init.js` calls `initThreadPool` inline
// after `init` (see `init.js.template`). This file is the dev/`trunk serve`
// mirror.
//
// See `docs/orchestrate/web-vox-async-loading/03-architecture.md` Q1 for
// the full design + `/tmp/wasm-bindgen-rayon-1.3.0/README.md:44-80` for
// the upstream documented setup.

export default function () {
  return {
    onSuccess: async (wasm) => {
      // `wasm` is the return value of `init(...)` — the wasm-bindgen
      // exports object. `initThreadPool` may not be reachable via the
      // top-level `wasm` value (depending on the wasm-bindgen output
      // shape); fall back to the global `wasmBindings` namespace which
      // Trunk's loader sets immediately before dispatching
      // `TrunkApplicationStarted` (see the `__trunkInitializer` glue in
      // the generated index.html). On the dev path the global is set just
      // after this callback resolves, so we look it up via the imported
      // bindings module instead.
      try {
        const initThreadPool =
          (wasm && wasm.initThreadPool) ||
          (typeof window !== 'undefined' &&
            window.wasmBindings &&
            window.wasmBindings.initThreadPool);
        if (typeof initThreadPool === 'function') {
          await initThreadPool(navigator.hardwareConcurrency);
          // eslint-disable-next-line no-console
          console.log(
            'wasm-bindgen-rayon: initThreadPool(',
            navigator.hardwareConcurrency,
            ') — worker pool ready',
          );
        } else {
          // eslint-disable-next-line no-console
          console.warn(
            'wasm-bindgen-rayon: initThreadPool not found on wasm exports ' +
              'or window.wasmBindings — `rayon::spawn` calls from Rust will ' +
              'panic. Expected `voxel::web_vox` to ' +
              '`pub use wasm_bindgen_rayon::init_thread_pool`.',
          );
        }
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error(
          'wasm-bindgen-rayon: initThreadPool failed:',
          e,
          '— rayon::spawn calls from Rust will panic.',
        );
      }
    },
  };
}
