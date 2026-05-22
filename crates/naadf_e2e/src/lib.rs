//! `naadf_e2e` — the external BRP-driven e2e test runner library for
//! `bevy-naadf`.
//!
//! This crate is the runner *library* only. It owns:
//!
//! - [`Sut`] — the system-under-test process harness: spawns the production
//!   binary `bin/bevy-naadf` under the `--e2e-brp` spawn contract, polls the
//!   BRP port until ready, kills the child on `Drop`.
//! - [`BrpClient`] — a blocking BRP-over-HTTP JSON-RPC 2.0 client.
//! - [`scenario`] — high-level helpers (`advance`, `capture`, `set_camera`,
//!   `erase_sphere`, `region_gate`, `pipeline_scan`, …) that compose the
//!   `naadf/*` verbs into the operations a gate test body needs.
//!
//! ## Layout (the Phase 2 hybrid decision)
//!
//! The 13 gate `#[test]` files do **not** live here — they live in
//! `crates/bevy_naadf/tests/<gate>.rs`, same-package as the `bevy-naadf`
//! binary, so Cargo sets `CARGO_BIN_EXE_bevy-naadf` for them and [`Sut`] can
//! locate the SUT binary without a `cargo build` shell-out. `bevy_naadf` has
//! `naadf_e2e` as a `[dev-dependencies]`; each gate file
//! `use naadf_e2e::{Sut, SutOpts, scenario}` for the harness and
//! `use bevy_naadf::e2e::framebuffer` for the pure assertion code. See
//! `docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md` §7.
//!
//! ## Wire schema
//!
//! The verb param / return structs are `bevy_naadf::e2e_brp::schema` — compiled
//! unconditionally in `bevy_naadf` (design D8 / A7) so this crate imports them
//! without ever building `bevy_naadf` with the `e2e-brp` feature. Only the
//! spawned SUT subprocess carries `e2e-brp`.

pub mod client;
pub mod scenario;
pub mod sut;

pub use client::{BrpClient, BrpClientError, BrpResult};
pub use sut::{Sut, SutOpts};

/// Re-export the verb wire schema so a gate test can `use naadf_e2e::schema`.
pub use bevy_naadf::e2e_brp::schema;
