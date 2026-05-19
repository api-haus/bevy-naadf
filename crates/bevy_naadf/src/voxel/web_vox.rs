//! Web-only `.vox` streaming + drag-and-drop loader.
//!
//! Two entry points feed the same single-slot inbox ([`take_pending_bytes`]),
//! and a Bevy `Update` system ([`apply_pending_vox`]) consumes the inbox each
//! frame, handing the bytes to [`crate::voxel::grid::install_vox_bytes_in_fixed_world`]
//! to swap the active scene:
//!
//! 1. **Startup HTTP fetch** ([`startup_fetch_default_vox`]) spawns a
//!    [`wasm_bindgen_futures::spawn_local`] task that `fetch()`-es the default
//!    `.vox` from the R2 bucket (or a `?vox=<url>` query-string override) and
//!    deposits the bytes in the inbox once the response is fully read.
//!
//! 2. **Drag-and-drop** — a pair of `dragover` / `drop` listeners are attached
//!    to the `#bevy` canvas at startup; the `drop` handler reads the dropped
//!    `File` via `arrayBuffer()` and deposits the bytes in the inbox.
//!
//! Bevy's `FileDragAndDrop` window event is *not* used on wasm — winit surfaces
//! a `PathBuf`, which is meaningless in a browser. The desktop equivalent
//! ([`crate::voxel::grid::native_vox_drop_listener`]) does use it.

use std::cell::RefCell;

use bevy::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// Re-export `wasm_bindgen_rayon::init_thread_pool` so the wasm-bindgen JS
// bindings expose it as `wasmBindings.initThreadPool(n)`. The JS bootstrap
// (`init.js.template` in production, `index.html` in dev) awaits this call
// after the regular `init({...})` and before dispatching
// `TrunkApplicationStarted` — without this re-export the JS-side
// `initThreadPool(...)` is undefined and Rayon's `spawn` lands on a pool
// that has zero worker threads (every `rayon::spawn` panics with "no thread
// pool installed").
//
// See `docs/orchestrate/web-vox-async-loading/03-architecture.md` Q1 for
// the full design + `/tmp/wasm-bindgen-rayon-1.3.0/README.md:44-80` for the
// upstream documented setup.
pub use wasm_bindgen_rayon::init_thread_pool;

/// R2 key + URL for the default voxel model fetched on startup. The R2 proxy
/// worker (`workers/r2-proxy/src/index.js`) serves any key under the
/// `bevy-naadf-assets` bucket with `Cross-Origin-Resource-Policy: cross-origin`
/// so this cross-origin fetch succeeds from the Pages-served HTML.
/// Format dispatch is magic-byte-based (`voxel_dispatch::parse_voxel_bytes`),
/// so `.cvox` bytes are handled transparently by the same fetch → parse path.
const DEFAULT_VOX_URL: &str =
    "https://bevy-naadf-assets.yura415.workers.dev/models/oasis.cvox";

thread_local! {
    /// Single-slot inbox shared by the HTTP-fetch task and the drag-drop
    /// closure. `thread_local!` is sufficient — wasm32-unknown-unknown is
    /// single-threaded, both producers and the Bevy consumer run on the same
    /// JS event-loop thread.
    static PENDING_VOX_BYTES: RefCell<Option<(Vec<u8>, String)>> =
        const { RefCell::new(None) };

    /// Second-stage slot used by the two-frame deferred-parse dance in
    /// [`apply_pending_vox`]. Frame N sees new bytes in `PENDING_VOX_BYTES`,
    /// paints the "Parsing…" overlay, and moves the bytes here. Frame N+1
    /// drains this slot and performs the actual (synchronous, blocking)
    /// install — by then the browser has had a chance to paint the overlay
    /// so the user sees a "Parsing…" message instead of a frozen page.
    static QUEUED_FOR_INSTALL: RefCell<Option<(Vec<u8>, String)>> =
        const { RefCell::new(None) };

    /// Tracks whether [`install_dnd_listeners`] has already attached its
    /// closures to the canvas. The Bevy startup system may run more than once
    /// in pathological cases; the listeners are global on the canvas, so we
    /// want to attach them exactly once.
    static DND_INSTALLED: RefCell<bool> = const { RefCell::new(false) };
}

fn submit_pending_bytes(bytes: Vec<u8>, source_label: String) {
    PENDING_VOX_BYTES.with(|cell| {
        if cell.borrow().is_some() {
            warn!(
                "web_vox: previous pending .vox payload replaced before it was \
                 consumed (new source: {source_label})"
            );
        }
        *cell.borrow_mut() = Some((bytes, source_label));
    });
}

fn take_pending_bytes() -> Option<(Vec<u8>, String)> {
    PENDING_VOX_BYTES.with(|cell| cell.borrow_mut().take())
}

/// Install a panic hook that forwards Rust panics to `console.error`. Without
/// this, wasm panics abort silently and the user just sees a frozen canvas.
/// Idempotent (the underlying `set_once` guards against double-install).
pub fn install_panic_hook() {
    console_error_panic_hook::set_once();
}

// ---------------------------------------------------------------------------
// Loading-overlay helpers — reuses the `#loading` div in `index.html` (which
// the wasm bootstrap hides once `TrunkApplicationStarted` fires) to surface
// "Downloading…" / "Parsing…" states during the .vox load. The DOM
// manipulation is cheap enough to do every time the state changes; failures
// silently fall through (the user just sees no overlay update, which is
// strictly better than a panic).
// ---------------------------------------------------------------------------

fn loading_overlay() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .get_element_by_id("loading")
}

fn loading_text() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .get_element_by_id("progress-text")
}

fn show_loading_overlay(message: &str) {
    if let Some(el) = loading_overlay() {
        el.class_list().remove_1("hidden").ok();
    }
    set_loading_text(message);
}

fn hide_loading_overlay() {
    if let Some(el) = loading_overlay() {
        el.class_list().add_1("hidden").ok();
    }
}

fn set_loading_text(message: &str) {
    if let Some(el) = loading_text() {
        el.set_text_content(Some(message));
    }
}

/// `setTimeout`-backed async sleep — yields to the JS event loop for `ms`
/// milliseconds, then resolves. Used by the failure path to flash a message
/// before hiding the overlay.
async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Resolve the `.vox` URL to fetch on startup. Checks `?vox=<url>` in
/// `window.location.search` first (for ad-hoc testing — drop any URL into the
/// address bar), falls back to [`DEFAULT_VOX_URL`].
fn resolve_startup_vox_url() -> String {
    let Some(window) = web_sys::window() else {
        return DEFAULT_VOX_URL.to_string();
    };
    let search = window.location().search().unwrap_or_default();
    let Some(stripped) = search.strip_prefix('?') else {
        return DEFAULT_VOX_URL.to_string();
    };
    for pair in stripped.split('&') {
        if let Some(val) = pair.strip_prefix("vox=") {
            if !val.is_empty() {
                return val.to_string();
            }
        }
    }
    DEFAULT_VOX_URL.to_string()
}

/// web-vox-async-loading 2026-05-18 follow-up Step 9 / Q6 — detect the
/// `?skybox=1` URL query parameter. When present, the wasm bootstrap skips
/// the HTTP fetch + installs an empty world (skybox-only baseline for the
/// Playwright SSIM compare).
///
/// Checks both `skybox=1` and `?skybox=1` after the leading `?`. The
/// architect's design (`03-architecture.md` § Q6 "Skybox-baseline-on-web
/// mechanism") locks this as the smallest possible URL surface.
pub fn resolve_skybox_only_param() -> bool {
    let Some(window) = web_sys::window() else { return false; };
    let search = window.location().search().unwrap_or_default();
    let Some(stripped) = search.strip_prefix('?') else { return false; };
    stripped.split('&').any(|p| p == "skybox=1")
}

async fn fetch_vox_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let resp_value =
        wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url)).await?;
    let resp: web_sys::Response = resp_value.dyn_into()?;
    if !resp.ok() {
        return Err(JsValue::from_str(&format!(
            "HTTP {} {}",
            resp.status(),
            resp.status_text()
        )));
    }
    let buf_value = wasm_bindgen_futures::JsFuture::from(resp.array_buffer()?).await?;
    let array = js_sys::Uint8Array::new(&buf_value);
    let mut bytes = vec![0u8; array.length() as usize];
    array.copy_to(&mut bytes);
    Ok(bytes)
}

/// Attach `dragover` + `drop` listeners to `document.body`. We use the body
/// rather than the `#bevy` canvas because the loading overlay sits above the
/// canvas at `z-index: 1000` and would intercept drop events; the body
/// receives them regardless of the overlay's visibility state.
///
/// `dragover.preventDefault()` is required for the browser to actually fire a
/// `drop` event (rather than treating the file as a navigation target);
/// `drop.preventDefault()` keeps the browser from opening the file in a new
/// tab. The drop handler reads the first dropped file via `arrayBuffer()` and
/// deposits its bytes into [`PENDING_VOX_BYTES`].
fn install_dnd_listeners() -> Result<(), JsValue> {
    if DND_INSTALLED.with(|c| *c.borrow()) {
        return Ok(());
    }
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let body = document
        .body()
        .ok_or_else(|| JsValue::from_str("no document.body"))?;

    let dragover = Closure::<dyn FnMut(web_sys::DragEvent)>::new(|e: web_sys::DragEvent| {
        e.prevent_default();
    });
    body.add_event_listener_with_callback(
        "dragover",
        dragover.as_ref().unchecked_ref(),
    )?;
    dragover.forget();

    let drop = Closure::<dyn FnMut(web_sys::DragEvent)>::new(|e: web_sys::DragEvent| {
        e.prevent_default();
        let Some(dt) = e.data_transfer() else { return };
        let files = dt.files();
        let Some(files) = files else { return };
        if files.length() == 0 {
            return;
        }
        let Some(file) = files.get(0) else { return };
        let name = file.name();
        info!("web_vox: drag-drop received file '{name}', reading…");
        show_loading_overlay(&format!("Reading {name}…"));
        let buf_promise = file.array_buffer();
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(buf_promise).await {
                Ok(buf_value) => {
                    let array = js_sys::Uint8Array::new(&buf_value);
                    let mut bytes = vec![0u8; array.length() as usize];
                    array.copy_to(&mut bytes);
                    info!(
                        "web_vox: drag-drop delivered {} bytes ({name})",
                        bytes.len()
                    );
                    submit_pending_bytes(bytes, format!("<dropped: {name}>"));
                }
                Err(e) => {
                    error!("web_vox: drag-drop arrayBuffer() failed: {:?}", e);
                    show_loading_overlay("Failed to read dropped file (see console).");
                }
            }
        });
    });
    body.add_event_listener_with_callback("drop", drop.as_ref().unchecked_ref())?;
    drop.forget();

    DND_INSTALLED.with(|c| *c.borrow_mut() = true);
    info!("web_vox: drag-drop listeners attached to document.body");
    Ok(())
}

/// `Startup` system: install the panic hook, attach the canvas drag-drop
/// listeners, and kick off the background fetch of the default `.vox`. The
/// resolved bytes land in [`PENDING_VOX_BYTES`] and are picked up by
/// [`apply_pending_vox`] on the next `Update`.
///
/// **`?skybox=1` short-circuit** (Q6): if the URL contains `skybox=1`, the
/// HTTP fetch is skipped and the DOM overlay is hidden immediately. The
/// `setup_test_grid` system (which runs in the same `Startup` schedule)
/// sees the `WebSkyboxOverride` resource inserted below and installs the
/// empty world. Used by the Playwright SSIM-baseline capture.
pub fn startup_fetch_default_vox(mut commands: Commands) {
    install_panic_hook();
    if let Err(e) = install_dnd_listeners() {
        error!("web_vox: failed to attach drag-drop listeners: {:?}", e);
    }

    if resolve_skybox_only_param() {
        info!(
            "web_vox: ?skybox=1 detected — skipping HTTP fetch + DND-installed \
             default; setup_test_grid will install the empty skybox-only world"
        );
        commands.insert_resource(crate::voxel::grid::WebSkyboxOverride);
        hide_loading_overlay();
        return;
    }

    let url = resolve_startup_vox_url();
    info!("web_vox: kicking off startup fetch — {url}");
    show_loading_overlay("Downloading default model…");
    let url_for_log = url.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match fetch_vox_bytes(&url).await {
            Ok(bytes) => {
                info!(
                    "web_vox: fetched {} bytes from {url_for_log}",
                    bytes.len()
                );
                submit_pending_bytes(bytes, url_for_log);
            }
            Err(e) => {
                error!(
                    "web_vox: failed to fetch .vox from {url_for_log}: {:?}",
                    e
                );
                // The default embedded scene installed by
                // `voxel::grid::setup_test_grid` at Startup is already live
                // underneath this overlay — flash a brief failure message,
                // then hide so the fallback becomes visible. Drag-drop
                // continues to work (its handler re-shows the overlay).
                show_loading_overlay(
                    "Download failed — using default scene (see console)",
                );
                sleep_ms(2500).await;
                // Only hide if our message is still the one shown — a
                // drag-drop in the meantime may have swapped it for
                // "Reading …" or "Parsing …", which we mustn't dismiss.
                if let Some(el) = loading_text() {
                    let txt = el.text_content().unwrap_or_default();
                    if txt.starts_with("Download failed") {
                        hide_loading_overlay();
                    }
                }
            }
        }
    });
}

/// `Update` system — runs every frame. Implements a two-stage deferred parse
/// so the "Parsing…" overlay paints before the parse dispatch sends the
/// bytes onto a rayon worker pool:
///
/// * **Stage 2 first** — if a previous frame queued bytes for install,
///   dispatch the parse via `spawn_wasm_vox_parse` (which uses
///   `rayon::spawn` against the `wasm-bindgen-rayon` worker pool spawned
///   by `bindings.initThreadPool` in `init-wasm-rayon.mjs`). The parse
///   runs off the main thread; the result is delivered through a
///   `crossbeam_channel::Receiver<...>` consumed by
///   [`crate::voxel::async_vox::poll_pending_vox_parse`].
/// * **Stage 1** — if new bytes have just landed (from the HTTP fetch or a
///   drag-drop), surface a "Parsing…" overlay and move the bytes into the
///   second-stage slot. The next frame will pick them up.
///
/// **web-vox-async-loading Step 5 (2026-05-18):** the previous body of
/// stage 2 ran the parse + install synchronously on the wasm main thread
/// (multi-second UI freeze). Replaced with `spawn_wasm_vox_parse`. The
/// overlay's `.indeterminate` class continues to animate during the
/// parse since the main thread is now responsive.
pub fn apply_pending_vox(
    mut commands: Commands,
    pending: Res<crate::voxel::async_vox::PendingVoxParse>,
    mut overlay_state: Local<OverlayState>,
) {
    // Stage 2: drain the queued-for-install slot first. Dispatch the parse
    // onto a rayon worker. If a previous parse is still in flight we
    // overwrite it (last-writer-wins matches the inbox semantic at
    // line ~63 below).
    if let Some((bytes, source_label)) = QUEUED_FOR_INSTALL.with(|c| c.borrow_mut().take()) {
        if pending.inner.is_some() {
            warn!(
                "web_vox: a previous .vox parse was still in flight; replacing \
                 it with the new payload (source: {source_label})"
            );
        }
        info!(
            "web_vox: dispatching async parse ({} bytes from {source_label}) \
             onto the wasm-bindgen-rayon worker pool",
            bytes.len()
        );
        // We re-show the overlay in indeterminate mode for the parse
        // duration — it stays visible until the install lands.
        show_loading_overlay("Parsing model…");
        overlay_state.parse_in_flight = true;
        spawn_wasm_vox_parse(&mut commands, bytes, source_label);
        return;
    }

    // Stage 1: pick up freshly delivered bytes, surface the "Parsing…"
    // overlay, and defer the actual dispatch to the next frame so the
    // browser can paint the overlay between the two frames.
    if let Some((bytes, source_label)) = take_pending_bytes() {
        info!(
            "web_vox: bytes ready ({} bytes from {source_label}); deferring \
             parse-dispatch one frame so the loading overlay paints first",
            bytes.len()
        );
        show_loading_overlay("Parsing model…");
        overlay_state.parse_in_flight = true;
        QUEUED_FOR_INSTALL.with(|c| *c.borrow_mut() = Some((bytes, source_label)));
        return;
    }

    // Overlay hide: the parse was in flight and the polling system has
    // cleared `pending.inner` (install done OR parse failed). Hide the
    // overlay so the user sees the rendered scene.
    if overlay_state.parse_in_flight && pending.inner.is_none() {
        info!("web_vox: async parse + install complete — hiding loading overlay");
        hide_loading_overlay();
        overlay_state.parse_in_flight = false;
    }
}

/// Per-run overlay state owned by `apply_pending_vox` as `Local<...>`.
/// Tracks whether a parse is currently in flight so the overlay hides on
/// the frame the polling system clears `PendingVoxParse.inner`.
#[derive(Default)]
pub struct OverlayState {
    pub parse_in_flight: bool,
}

/// Spawn a `.vox` parse off the wasm main thread via `rayon::spawn`. The
/// rayon worker is one of the `navigator.hardwareConcurrency` Web Workers
/// spawned by `bindings.initThreadPool` (called from `init-wasm-rayon.mjs`
/// in dev / `init.js.template` in production); when this function returns
/// the parse is already running concurrently with the main-thread render
/// loop.
///
/// Result is delivered through a `crossbeam_channel::bounded(1)` pair;
/// receiver lives in the [`crate::voxel::async_vox::PendingVoxParse`]
/// resource consumed by `poll_pending_vox_parse`.
///
/// `commands.insert_resource(PendingVoxParse { inner: Some(...) })`
/// overwrites any in-flight parse, matching the inbox's last-writer-wins
/// semantic.
fn spawn_wasm_vox_parse(commands: &mut Commands, bytes: Vec<u8>, source_label: String) {
    let (tx, rx) = crossbeam_channel::bounded::<crate::voxel::async_vox::ParseResult>(1);
    let label_for_task = source_label.clone();
    rayon::spawn(move || {
        let result = match crate::voxel::grid::parse_to_imported_vox(&bytes) {
            Ok(imp) => Ok((imp, label_for_task)),
            Err(e) => Err(e),
        };
        // Best-effort send — if the receiver was dropped (replaced by a
        // newer parse) the result is discarded silently.
        let _ = tx.send(result);
    });
    commands.insert_resource(crate::voxel::async_vox::PendingVoxParse {
        inner: Some(crate::voxel::async_vox::PendingVoxParseInner {
            rx,
            source_label,
        }),
    });
}

/// Hide the loading overlay once the async parse + install completes. The
/// `poll_pending_vox_parse` system runs `install_imported_vox` on the
/// main thread; this helper is wired as an observer so the overlay
/// hides the same frame the install lands.
///
/// Currently not wired — the architect's design called for the polling
/// system itself to call `hide_loading_overlay()` post-install, but
/// `poll_pending_vox_parse` lives in `voxel::async_vox` which is
/// platform-agnostic. The overlay-hide is left for a follow-up; the
/// existing `Update` system in this module will hide the overlay when
/// the inbox + queue are both empty (handled in the next iteration).
#[allow(dead_code)]
pub(crate) fn hide_overlay_if_install_done() {
    hide_loading_overlay();
}
