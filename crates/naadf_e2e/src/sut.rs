//! `Sut` — the system-under-test process harness.
//!
//! Spawns the production binary `bin/bevy-naadf` as a subprocess under the e2e
//! SUT spawn contract (`--e2e-brp <port>` + optional `--vox` / `--e2e-window`),
//! polls the BRP port until it answers, and kills the child on `Drop` so no
//! orphan process survives a panicking test (design §7.2).
//!
//! ## Locating the SUT binary (the hybrid-layout decision)
//!
//! The design §7.2 had the runner shell `cargo build -p bevy-naadf --features
//! e2e-brp` because `naadf_e2e` is a *separate* crate and Cargo does not set
//! `CARGO_BIN_EXE_*` for it. The Phase 2 brief resolved a fork to the **hybrid
//! layout**: the gate test files live in `crates/bevy_naadf/tests/<gate>.rs`,
//! same-package as the `bevy-naadf` binary — so Cargo *does* set
//! `CARGO_BIN_EXE_bevy-naadf` for those test binaries. The gate test reads
//! `env!("CARGO_BIN_EXE_bevy-naadf")` (a compile-time literal in *its* crate)
//! and passes it to [`Sut::spawn`] via [`SutOpts::binary`]. Run with
//! `cargo test -p bevy_naadf --features e2e-brp --test <gate>`, that binary is
//! the `e2e-brp`-enabled SUT. No `cargo build` shell-out, no `OnceLock` dance.
//!
//! ## SUT working directory (Phase 0 forward-note)
//!
//! Bevy's `AssetPlugin { file_path: "src/assets" }` resolves shaders relative
//! to the process CWD. Phase 0 found that running the SUT with the wrong CWD
//! produces a wall of `Path not found: .../src/assets/shaders/*.wgsl` errors
//! and a blank renderer. [`Sut::spawn`] therefore sets the child's
//! `current_dir` to the `bevy_naadf` crate root (where `src/assets/` lives) —
//! the gate test passes `env!("CARGO_MANIFEST_DIR")` via [`SutOpts::cwd`].

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::client::BrpClient;

/// Options for spawning a SUT subprocess.
#[derive(Debug, Clone)]
pub struct SutOpts {
    /// Absolute path to the `bevy-naadf` binary (the `e2e-brp`-featured build).
    /// Set this from `env!("CARGO_BIN_EXE_bevy-naadf")` in the gate test.
    pub binary: std::path::PathBuf,
    /// Working directory for the child — must be the `bevy_naadf` crate root
    /// so `AssetPlugin`'s `src/assets` resolves. Set from
    /// `env!("CARGO_MANIFEST_DIR")` in the gate test.
    pub cwd: std::path::PathBuf,
    /// Optional `--vox <path>` fixture. The path is resolved by the SUT
    /// relative to its CWD (the crate root) — so for the bundled Oasis fixture
    /// pass `assets/test/oasis_hard_cover.vox`.
    pub vox: Option<String>,
    /// Optional `--e2e-window <w>x<h>` override of the SUT window size
    /// (default 256×256 from the e2e profile).
    pub window: Option<(u32, u32)>,
    /// `--e2e-vox-oracle-cpu` — route a `--vox` load through the test-only
    /// natural-bound CPU oracle loader (`E2eGateMode::VoxGpuOracleCpu`) instead
    /// of the production W5 GPU producer chain. The CPU phase of the
    /// BRP-driven `vox_gpu_oracle` compare gate (Phase 3b). Boot-time —
    /// `setup_test_grid` reads it at `Startup`, so it rides the spawn contract.
    pub vox_oracle_cpu: bool,
    /// `--e2e-entities` — spawn the Phase-C 4×4×4 emissive-voxel test fixture
    /// and enable the W4 entity track. The `entities` gate's boot-time config
    /// (Phase 3b). Both knobs are consumed before `app.run()`, so this rides
    /// the spawn contract per Forbidden Move #4.
    pub entities: bool,
    /// `--e2e-empty-world` — install `GridPreset::Empty` (pure-sky baseline)
    /// instead of the default embedded test scene. The skybox-baseline phase
    /// of the BRP-driven `vox_web_parity` compare gate (Phase 3b). Mutually
    /// exclusive with [`SutOpts::vox`] — a `--vox` path wins.
    pub empty_world: bool,
    /// `--e2e-resizable` — make the SUT window user-resizable
    /// (`Window.resizable = true`). The BRP-driven `resize_test` gate
    /// (Phase 3b) needs this: winit advertises the surface as fixed-size
    /// (Wayland compositors refuse the resize) unless `resizable` is `true`,
    /// so the `naadf/resize_window` verb is a no-op without it.
    pub resizable: bool,
    /// Explicit BRP port; `None` ⇒ an OS-assigned free port is picked.
    pub port: Option<u16>,
    /// How long [`Sut::spawn`] polls the BRP port for readiness before giving
    /// up (default 60 s — the SUT compiles pipelines synchronously at boot).
    pub boot_timeout: Duration,
    /// Inherit the child's stdout/stderr to the test's console (default
    /// `true` — the SUT's `e2e_render`-style logs are useful in CI output).
    pub inherit_io: bool,
}

impl SutOpts {
    /// New options for `binary` with crate-root CWD `cwd`. Both should come
    /// from the gate test's compile-time `env!` macros.
    pub fn new(
        binary: impl Into<std::path::PathBuf>,
        cwd: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            binary: binary.into(),
            cwd: cwd.into(),
            vox: None,
            window: None,
            vox_oracle_cpu: false,
            entities: false,
            empty_world: false,
            resizable: false,
            port: None,
            boot_timeout: Duration::from_secs(60),
            inherit_io: true,
        }
    }

    /// Set the `--vox` fixture path (resolved relative to the SUT CWD).
    pub fn vox(mut self, path: impl Into<String>) -> Self {
        self.vox = Some(path.into());
        self
    }

    /// Set the `--e2e-window` size override.
    pub fn window(mut self, width: u32, height: u32) -> Self {
        self.window = Some((width, height));
        self
    }

    /// Enable `--e2e-vox-oracle-cpu` — the CPU-oracle install path for the
    /// `vox_gpu_oracle` compare gate's CPU phase.
    pub fn vox_oracle_cpu(mut self, enabled: bool) -> Self {
        self.vox_oracle_cpu = enabled;
        self
    }

    /// Enable `--e2e-entities` — the Phase-C entity fixture spawn + W4 entity
    /// track for the `entities` gate.
    pub fn entities(mut self, enabled: bool) -> Self {
        self.entities = enabled;
        self
    }

    /// Enable `--e2e-empty-world` — install `GridPreset::Empty` (pure-sky
    /// baseline) for the `vox_web_parity` gate's skybox phase.
    pub fn empty_world(mut self, enabled: bool) -> Self {
        self.empty_world = enabled;
        self
    }

    /// Enable `--e2e-resizable` — make the SUT window user-resizable, required
    /// for the `resize_test` gate's `naadf/resize_window` verb to take effect.
    pub fn resizable(mut self, enabled: bool) -> Self {
        self.resizable = enabled;
        self
    }

    /// Pin an explicit BRP port (otherwise an OS-assigned free port is used).
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Override the boot-readiness poll timeout.
    pub fn boot_timeout(mut self, timeout: Duration) -> Self {
        self.boot_timeout = timeout;
        self
    }
}

/// A spawned SUT subprocess + its BRP client. Killing the child on `Drop`
/// guarantees no orphan process survives a panicking / failing test.
pub struct Sut {
    child: Child,
    port: u16,
    client: BrpClient,
}

impl Sut {
    /// Spawn `bin/bevy-naadf --e2e-brp <port> [--vox ..] [--e2e-window ..]`,
    /// poll the BRP port until `rpc.discover` answers, and return the harness.
    ///
    /// Panics on a spawn / boot failure — a SUT that will not start is a hard
    /// test failure, and a panic gives the cleanest backtrace.
    pub fn spawn(opts: SutOpts) -> Sut {
        let port = match opts.port {
            Some(p) => p,
            None => free_loopback_port(),
        };

        let mut cmd = Command::new(&opts.binary);
        cmd.current_dir(&opts.cwd);
        cmd.arg("--e2e-brp").arg(port.to_string());
        if let Some(vox) = &opts.vox {
            cmd.arg("--vox").arg(vox);
        }
        if let Some((w, h)) = opts.window {
            cmd.arg("--e2e-window").arg(format!("{w}x{h}"));
        }
        if opts.vox_oracle_cpu {
            cmd.arg("--e2e-vox-oracle-cpu");
        }
        if opts.entities {
            cmd.arg("--e2e-entities");
        }
        if opts.empty_world {
            cmd.arg("--e2e-empty-world");
        }
        if opts.resizable {
            cmd.arg("--e2e-resizable");
        }
        if opts.inherit_io {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }

        let child = cmd.spawn().unwrap_or_else(|e| {
            panic!(
                "Sut::spawn — failed to launch SUT binary {:?} (cwd {:?}): {e}",
                opts.binary, opts.cwd
            )
        });

        let mut sut = Sut {
            child,
            port,
            client: BrpClient::new(port),
        };

        // Poll BRP readiness. The BRP HTTP server task starts in `Startup`;
        // synchronous pipeline compilation means the first frames are slow,
        // so the boot timeout is generous.
        let deadline = Instant::now() + opts.boot_timeout;
        loop {
            // The child exiting during boot is a hard, immediate failure.
            if let Ok(Some(status)) = sut.child.try_wait() {
                panic!(
                    "Sut::spawn — SUT exited during boot with {status} \
                     (BRP port {port} never came up); check the SUT stderr above"
                );
            }
            match sut.client.ping() {
                Ok(()) => break,
                Err(last_err) => {
                    if Instant::now() >= deadline {
                        let _ = sut.child.kill();
                        panic!(
                            "Sut::spawn — BRP server on 127.0.0.1:{port} did not \
                             answer within {:?}; last error: {last_err:?}",
                            opts.boot_timeout
                        );
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        sut
    }

    /// The BRP client for issuing `naadf/*` verbs.
    pub fn client(&mut self) -> &mut BrpClient {
        &mut self.client
    }

    /// The BRP port the SUT is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Sut {
    fn drop(&mut self) {
        // Kill + reap — no orphan process, no zombie.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ask the OS for a free loopback TCP port: bind `127.0.0.1:0`, read the
/// assigned port, drop the listener. There is a small TOCTOU window between
/// dropping the listener and the SUT binding it, but on loopback in a test
/// harness it is negligible (design §7.2).
fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free_loopback_port — could not bind 127.0.0.1:0")
        .local_addr()
        .expect("free_loopback_port — listener has no local addr")
        .port()
}
