//! WASM binary entry point for Emscripten builds.
//!
//! This minimal binary exists to:
//! 1. Provide the `main` function required by Emscripten
//! 2. Link the wasm_api exports from the library
//!
//! The actual C-API implementation lives in `native.rs::wasm_api`.
//! See that module for the full implementation alongside the native Rust API.

// Reference the wasm_api module to ensure C-API symbols are linked.
// The underscore import prevents "unused" warnings while keeping symbols alive.
#[cfg(all(target_arch = "wasm32", target_os = "emscripten"))]
use voxel_noise::wasm_api as _;

/// Required main function for Emscripten.
fn main() {}
