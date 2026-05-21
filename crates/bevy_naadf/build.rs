//! Build script — currently used **only** for the Android cross-compile.
//!
//! ## Why this exists
//!
//! Rust's prebuilt `libstd` for `aarch64-linux-android` is compiled with
//! LLVM outline-atomics enabled, so it references runtime LSE-detection
//! helpers (`__aarch64_swp1_acq_rel`, `__aarch64_ldadd*_acq_rel`, …) that
//! live in `libclang_rt.builtins-aarch64-android.a` inside the NDK.
//! `cargo-ndk` 4.1 does not link that archive automatically, so the
//! resulting `.so` fails `dlopen` on-device with
//! `cannot locate symbol "__aarch64_swp1_acq_rel"`. Captured 2026-05-21 on
//! a Galaxy Tab A8 against NDK r28 / rustc nightly 2026-03-31.
//!
//! ## What this does
//!
//! Resolves the NDK install from environment variables (`ANDROID_NDK_HOME`
//! / `ANDROID_NDK_ROOT` / `NDK_HOME` — same precedence cargo-ndk uses),
//! finds the `libclang_rt.builtins-aarch64-android.a` inside it (the
//! `clang/<major>/` sub-path follows the NDK's bundled clang major version,
//! which changes per NDK release), and emits the linker argument that
//! pulls it in. Portable across host machines and NDK versions — anyone
//! with an NDK on `$PATH`-equivalent env var can build the APK without
//! touching `.cargo/config.toml`.
//!
//! Native (non-Android) and wasm builds skip the entire script body.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Cargo sets `CARGO_CFG_TARGET_OS` in build scripts to the *target*'s OS,
    // not the host's. So this fires only when cross-compiling to Android,
    // even when invoked from a Linux/macOS/Windows host.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_os != "android" {
        return;
    }
    // Outline-atomics affects aarch64 only; armv7 has its own atomics ABI
    // and does not need the builtins archive for these symbols.
    if target_arch != "aarch64" {
        return;
    }

    // Re-run the build script if any of the env vars change. Cargo defaults
    // to re-running when *any* env var changes, but being explicit makes
    // the rerun trigger predictable on NDK swaps.
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_ROOT");
    println!("cargo:rerun-if-env-changed=NDK_HOME");

    let ndk = match find_ndk_root() {
        Some(p) => p,
        None => panic!(
            "android-build: could not locate the Android NDK. Set one of \
             ANDROID_NDK_HOME / ANDROID_NDK_ROOT / NDK_HOME to the NDK install \
             directory (the one containing toolchains/llvm/prebuilt/) before \
             running `cargo ndk`."
        ),
    };

    let builtins = match find_aarch64_builtins(&ndk) {
        Some(p) => p,
        None => panic!(
            "android-build: located NDK at {} but could not find \
             libclang_rt.builtins-aarch64-android.a under \
             toolchains/llvm/prebuilt/*/lib/clang/*/lib/linux/. \
             Possible causes: NDK install is partial, or the NDK layout \
             changed in a future release — bump the glob in build.rs.",
            ndk.display()
        ),
    };

    // The linker accepts a bare path to a `.a` file as a positional input
    // and links it statically. This is more portable than the `-L<dir>
    // -l<libname>` form because we don't have to strip the `lib` prefix
    // or the `.a` suffix from the filename.
    println!("cargo:rustc-link-arg={}", builtins.display());
}

/// Search the well-known NDK env vars in cargo-ndk's documented order:
/// `ANDROID_NDK_HOME` → `ANDROID_NDK_ROOT` → `NDK_HOME`. Returns the first
/// one that points at an existing directory.
fn find_ndk_root() -> Option<PathBuf> {
    for var in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"] {
        if let Ok(value) = env::var(var) {
            let path = PathBuf::from(value);
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

/// Glob (manually — no glob crate dep) for
/// `<ndk>/toolchains/llvm/prebuilt/<host>/lib/clang/<major>/lib/linux/libclang_rt.builtins-aarch64-android.a`.
/// `<host>` is something like `linux-x86_64` / `darwin-x86_64` / `windows-x86_64`.
/// `<major>` is the clang major version (`19` for NDK r28, `17` for r26, …).
/// We resolve the host directory first (usually exactly one entry), then the
/// clang directory.
fn find_aarch64_builtins(ndk: &Path) -> Option<PathBuf> {
    let prebuilt = ndk.join("toolchains").join("llvm").join("prebuilt");
    let host_dir = first_subdir(&prebuilt)?;
    let clang_dir = first_subdir(&host_dir.join("lib").join("clang"))?;
    let candidate = clang_dir
        .join("lib")
        .join("linux")
        .join("libclang_rt.builtins-aarch64-android.a");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Return the first sub-directory of `dir` (sorted lexicographically), or
/// `None` if `dir` doesn't exist or is empty.
fn first_subdir(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    entries.into_iter().next()
}
