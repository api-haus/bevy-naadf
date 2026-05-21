//! `bake` — runs Bevy's [`AssetProcessor`] over `src/assets/` once, then exits.
//!
//! Retained as an InstaMAT pre-bake-to-disk scaffold (see
//! `instamat-bake-to-disk` user memory): a headless, render-less Bevy app that
//! boots the asset pipeline in `AssetMode::Processed` and exits when the
//! processor reports `ProcessorState::Finished`. The runtime baked-material
//! consumer lives on a separate PBR branch; on master this binary processes no
//! assets — it is the template the future InstaMAT integration plugs into.
//!
//! Native-only: the production app (`bevy-naadf`) and the e2e harness both
//! run in `AssetMode::Unprocessed`; a Bevy `AssetProcessor` is app-global and
//! racing it against the render pipeline's shader loads is fragile.
//!
//! Run it with `cargo run --bin bake` (or `just bake-texarrays`).

#[cfg(target_arch = "wasm32")]
fn main() {
    // The web build never bakes — the asset processor is native-only.
    panic!("`bake` is a native-only binary; the web build cannot run the asset processor");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> bevy::app::AppExit {
    use std::time::Duration;

    use bevy::app::{App, AppExit, ScheduleRunnerPlugin};
    use bevy::asset::processor::{AssetProcessor, ProcessorState};
    use bevy::asset::{AssetApp, AssetMode, AssetPlugin};
    use bevy::image::{CompressedImageFormats, ImageLoader, ImagePlugin};
    use bevy::log::LogPlugin;
    use bevy::prelude::*;
    use bevy::tasks::block_on;
    use bevy::MinimalPlugins;

    /// Poll the [`AssetProcessor`] every `Update`; exit `Success` once it has
    /// finished, or `Error` if it stalls (a malformed source `.meta` must not
    /// hang the bake forever).
    fn exit_when_finished(
        processor: Option<Res<AssetProcessor>>,
        mut exit: MessageWriter<AppExit>,
        mut ticks: Local<u32>,
    ) {
        *ticks += 1;
        // ~30 s cap at the 10 ms/tick run-loop period below; processing this
        // asset set should take well under a second.
        let stalled = *ticks > 3_000;
        let state = processor.as_deref().map(|p| block_on(p.get_state()));
        if matches!(state, Some(ProcessorState::Finished)) {
            info!("asset processing finished — `imported_assets/` is up to date");
            exit.write(AppExit::Success);
        } else if stalled {
            // `ProcessorState` is not `Debug`, so name it by hand.
            let name = match state {
                Some(ProcessorState::Initializing) => "initializing",
                Some(ProcessorState::Processing) => "processing",
                Some(ProcessorState::Finished) => "finished",
                None => "not started (no AssetProcessor resource)",
            };
            error!("asset processor did not finish after ~30 s (state: {name}) — aborting");
            exit.write(AppExit::error());
        }
    }

    App::new()
        .add_plugins((
            // Task pools (the processor runs on `IoTaskPool`), the frame
            // counter, time, and a headless run loop — no window, no renderer.
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(10))),
            LogPlugin::default(),
            // `AssetMode::Processed` boots Bevy's `AssetProcessor` resource —
            // the InstaMAT-pattern entry point. With no asset-processor
            // plugins registered the processor finishes immediately
            // (`ProcessorState::Finished` on the first poll); future
            // InstaMAT integration registers its processor here.
            AssetPlugin {
                file_path: "src/assets".to_string(),
                mode: AssetMode::Processed,
                ..default()
            },
            // Sets up the `Image` asset type. It only *pre*-registers
            // `ImageLoader` — see the explicit registration below.
            ImagePlugin::default(),
        ))
        // `bevy_image::ImagePlugin` only *pre*-registers `ImageLoader`; the real
        // registration normally lives in `bevy_render` (it needs the GPU's
        // compressed-format list). This headless bake app has no renderer, so
        // register `ImageLoader` directly — the processor needs it to
        // deserialize source PNGs' `Load`-action `.meta` sidecars. Empty
        // compressed-format support is fine: any future InstaMAT processor
        // declares its own format support.
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .add_systems(Update, exit_when_finished)
        .run()
}
