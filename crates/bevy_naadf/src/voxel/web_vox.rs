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

/// R2 key + URL for the default `.vox` model fetched on startup. The R2 proxy
/// worker (`workers/r2-proxy/src/index.js`) serves any key under the
/// `bevy-naadf-assets` bucket with `Cross-Origin-Resource-Policy: cross-origin`
/// so this cross-origin fetch succeeds from the Pages-served HTML.
const DEFAULT_VOX_URL: &str =
    "https://bevy-naadf-assets.yura415.workers.dev/models/oasis_hard_cover.vox";

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
pub fn startup_fetch_default_vox() {
    install_panic_hook();
    if let Err(e) = install_dnd_listeners() {
        error!("web_vox: failed to attach drag-drop listeners: {:?}", e);
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
/// so the "Parsing…" overlay paints before the synchronous (and currently
/// quite slow — see followup `02b-async-vox-load.md`) parse blocks the main
/// thread for several seconds:
///
/// * **Stage 2 first** — if a previous frame queued bytes for install, do the
///   install now. Then hide the overlay. (Stage 2 is checked first so the
///   browser's compositor gets to paint the overlay set up in stage 1 between
///   the two frames.)
/// * **Stage 1** — if new bytes have just landed (from the HTTP fetch or a
///   drag-drop), surface a "Parsing…" overlay and move the bytes into the
///   second-stage slot. The next frame will pick them up.
pub fn apply_pending_vox(mut commands: Commands) {
    // Stage 2: drain the queued-for-install slot first. The overlay set up
    // last frame has now been painted; the upcoming sync install can block
    // for several seconds and the user will see "Parsing…" the whole time.
    if let Some((bytes, source_label)) = QUEUED_FOR_INSTALL.with(|c| c.borrow_mut().take()) {
        info!(
            "web_vox: installing .vox ({} bytes) from {source_label} — \
             expect a few seconds of UI freeze (sync parse, see followup)",
            bytes.len()
        );
        crate::voxel::grid::install_vox_bytes_in_fixed_world(
            &mut commands,
            &bytes,
            &source_label,
        );
        hide_loading_overlay();
        return;
    }

    // Stage 1: pick up freshly delivered bytes, surface the "Parsing…"
    // overlay, and defer the actual install to the next frame so the
    // browser can paint the overlay between the two frames.
    if let Some((bytes, source_label)) = take_pending_bytes() {
        info!(
            "web_vox: bytes ready ({} bytes from {source_label}); deferring \
             install one frame so the loading overlay paints first",
            bytes.len()
        );
        show_loading_overlay("Parsing model…");
        QUEUED_FOR_INSTALL.with(|c| *c.borrow_mut() = Some((bytes, source_label)));
    }
}
