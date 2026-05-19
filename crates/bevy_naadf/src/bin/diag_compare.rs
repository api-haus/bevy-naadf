//! `cargo run --bin diag_compare -- <native.json> <web.json>` — structural
//! diff of two `DeviceSnapshot` JSON files written by the
//! `wasm-chunk-aadf-determinism` diagnostic package.
//!
//! Inputs: workspace-relative paths to two JSON files produced by
//! `src/diagnostics.rs::device_snapshot`. Defaults to
//! `target/diagnostics/device-snapshot-native.json` and
//! `target/diagnostics/device-snapshot-web.json` if no args are passed.
//!
//! Output: plain-text walk of the two JSON values. For every leaf-key, if
//! the values differ, prints `field path: native = <a>  |  web = <b>`. The
//! "expected divergences" list (target/captured_at/build.*/adapter_info.*)
//! is highlighted as `(expected)`; everything else is highlighted as
//! `>>> DIVERGENCE <<<` with a load-bearing-allowlist tag on the curated
//! short list.
//!
//! Exit codes:
//!   0 — only expected divergences (file format match)
//!   1 — at least one unexpected divergence
//!   2 — argument / IO / parse error

use std::collections::BTreeSet;
use std::process::ExitCode;

use serde_json::Value;

const DEFAULT_NATIVE: &str = "target/diagnostics/device-snapshot-native.json";
const DEFAULT_WEB: &str = "target/diagnostics/device-snapshot-web.json";

/// Fields whose value is EXPECTED to differ between native and web.
/// Diff against these never raises the unexpected-divergence flag.
const EXPECTED_DIVERGENCE_PREFIXES: &[&str] = &[
    "target",
    "captured_at_unix_seconds",
    "build.target_arch",
    "build.target_os",
    "build.profile",
    "adapter_info.backend",
    "adapter_info.driver",
    "adapter_info.driver_info",
    "adapter_info.name",
    "adapter_info.vendor",
    "adapter_info.device",
    "adapter_info.device_pci_bus_id",
    "adapter_info.device_type",
    "adapter_info.transient_saves_memory",
];

/// Fields the handoff has already implicated as load-bearing for the bug.
/// Any divergence here is highlighted in CAPS.
const LOAD_BEARING_FIELDS: &[&str] = &[
    "adapter_limits.max_compute_workgroups_per_dimension",
    "device_limits.max_compute_workgroups_per_dimension",
    "adapter_limits.max_storage_buffer_binding_size",
    "device_limits.max_storage_buffer_binding_size",
    "adapter_limits.max_buffer_size",
    "device_limits.max_buffer_size",
    "adapter_limits.max_storage_buffers_per_shader_stage",
    "device_limits.max_storage_buffers_per_shader_stage",
    "adapter_limits.max_compute_workgroup_storage_size",
    "device_limits.max_compute_workgroup_storage_size",
    "downlevel.flags",
    "downlevel_is_webgpu_compliant",
    "limit_deltas",
    "adapter_features",
    "device_features",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (native_path, web_path) = match args.len() {
        0 => (DEFAULT_NATIVE.to_string(), DEFAULT_WEB.to_string()),
        2 => (args[0].clone(), args[1].clone()),
        n => {
            eprintln!(
                "diag_compare: expected 0 or 2 args (got {n}). \
                 Usage: diag_compare [<native.json> <web.json>]"
            );
            return ExitCode::from(2);
        }
    };

    let native_str = match std::fs::read_to_string(&native_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("diag_compare: cannot read {native_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let web_str = match std::fs::read_to_string(&web_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("diag_compare: cannot read {web_path}: {e}");
            return ExitCode::from(2);
        }
    };

    let native: Value = match serde_json::from_str(&native_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("diag_compare: cannot parse {native_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let web: Value = match serde_json::from_str(&web_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("diag_compare: cannot parse {web_path}: {e}");
            return ExitCode::from(2);
        }
    };

    println!("== device-snapshot diff ==");
    println!("native: {native_path}");
    println!("   web: {web_path}");
    println!();

    let mut divergences: Vec<Divergence> = Vec::new();
    walk(&native, &web, "", &mut divergences);

    // Sort: load-bearing first, then unexpected, then expected.
    divergences.sort_by_key(|d| match d.kind {
        Kind::LoadBearing => 0,
        Kind::Unexpected => 1,
        Kind::Expected => 2,
    });

    let mut top10_printed = 0usize;
    println!("--- TOP DIVERGENCES (load-bearing + unexpected, max 10) ---");
    for d in divergences.iter().filter(|d| d.kind != Kind::Expected) {
        if top10_printed >= 10 {
            break;
        }
        println!(
            "  {} {}: native = {}  |  web = {}",
            d.kind.label(),
            d.path,
            d.native,
            d.web,
        );
        top10_printed += 1;
    }
    if top10_printed == 0 {
        println!("  (none — only expected divergences)");
    }

    println!();
    println!("--- ALL DIVERGENCES (full list) ---");
    for d in &divergences {
        println!(
            "  {} {}: native = {}  |  web = {}",
            d.kind.label(),
            d.path,
            d.native,
            d.web,
        );
    }

    let unexpected = divergences
        .iter()
        .filter(|d| d.kind != Kind::Expected)
        .count();
    let load_bearing = divergences
        .iter()
        .filter(|d| d.kind == Kind::LoadBearing)
        .count();
    println!();
    println!("== summary ==");
    println!("  total divergences:    {}", divergences.len());
    println!("  expected divergences: {}", divergences.len() - unexpected);
    println!("  unexpected:           {unexpected}");
    println!("  load-bearing:         {load_bearing}");

    if unexpected > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

#[derive(Debug, Clone)]
struct Divergence {
    path: String,
    native: String,
    web: String,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LoadBearing,
    Unexpected,
    Expected,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::LoadBearing => ">>> LOAD-BEARING <<<",
            Kind::Unexpected => ">>> DIVERGENCE   <<<",
            Kind::Expected => "(expected)         ",
        }
    }
}

fn classify(path: &str) -> Kind {
    if LOAD_BEARING_FIELDS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}.")) || path.starts_with(&format!("{p}[")))
    {
        return Kind::LoadBearing;
    }
    if EXPECTED_DIVERGENCE_PREFIXES.iter().any(|p| path == *p || path.starts_with(&format!("{p}."))) {
        return Kind::Expected;
    }
    Kind::Unexpected
}

fn walk(native: &Value, web: &Value, path: &str, out: &mut Vec<Divergence>) {
    match (native, web) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match (a.get(k), b.get(k)) {
                    (Some(av), Some(bv)) => walk(av, bv, &child_path, out),
                    (Some(av), None) => {
                        out.push(Divergence {
                            path: child_path.clone(),
                            native: short(av),
                            web: "<missing>".into(),
                            kind: classify(&child_path),
                        });
                    }
                    (None, Some(bv)) => {
                        out.push(Divergence {
                            path: child_path.clone(),
                            native: "<missing>".into(),
                            web: short(bv),
                            kind: classify(&child_path),
                        });
                    }
                    (None, None) => unreachable!(),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            // For arrays at the leaf-of-interest level (e.g.
            // adapter_features, downlevel.flags, limit_deltas), compare
            // as sets — print as set diff if asymmetric. The list ordering
            // is already sorted on the producer side for stable diff.
            if a == b {
                return;
            }
            // Set diff for string arrays.
            if a.iter().all(|v| v.is_string()) && b.iter().all(|v| v.is_string()) {
                let aset: BTreeSet<&str> =
                    a.iter().filter_map(|v| v.as_str()).collect();
                let bset: BTreeSet<&str> =
                    b.iter().filter_map(|v| v.as_str()).collect();
                let only_native: Vec<&str> = aset.difference(&bset).copied().collect();
                let only_web: Vec<&str> = bset.difference(&aset).copied().collect();
                if !only_native.is_empty() {
                    let child_path = format!("{path}.only_in_native");
                    out.push(Divergence {
                        path: child_path.clone(),
                        native: format!("[{}]", only_native.join(", ")),
                        web: "[]".into(),
                        kind: classify(path),
                    });
                }
                if !only_web.is_empty() {
                    let child_path = format!("{path}.only_in_web");
                    out.push(Divergence {
                        path: child_path.clone(),
                        native: "[]".into(),
                        web: format!("[{}]", only_web.join(", ")),
                        kind: classify(path),
                    });
                }
            } else {
                out.push(Divergence {
                    path: path.into(),
                    native: short(native),
                    web: short(web),
                    kind: classify(path),
                });
            }
        }
        _ => {
            if native != web {
                out.push(Divergence {
                    path: path.into(),
                    native: short(native),
                    web: short(web),
                    kind: classify(path),
                });
            }
        }
    }
}

fn short(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| String::new());
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}
