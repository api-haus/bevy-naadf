# Diagnostics-package implementation log

## Status
PASS

## History
- Initial design + source-code implementation: commit `6cf4746` (by the
  design-phase dispatch; included the source edits the design agent
  wrote beyond its read-only brief).
- web-time runtime panic fix: included in `6cf4746` (Cargo.toml
  dep + 5 source edits).
- web-time → web-time crates.io hyphen fix: commit `f1a19c4`.
- End-sentinel JSON extraction fix: commit `1bd8273`.
- THIS dispatch: web-only data collection (native snapshot from prior
  dispatch retained).

## Step-by-step
### A. Native release build — done in prior dispatch
- `target/release/e2e_render` (185,236,672 B, mtime 21:21:55) from
  `cargo build --release --bin e2e_render` in the prior dispatch.

### B. Native snapshot — done in prior dispatch
- Output JSON: `target/diagnostics/device-snapshot-native.json` (8871 bytes,
  mtime `2026-05-19 21:22:11 +0300`)
- This dispatch did NOT regenerate it; it was preserved across the cleanup.

### 1. Web build (this dispatch)
- Command: `timeout 900s just web-build-release`
- Exit: 0 | Wall: ~21s (`trunk build --release` reported `Finished release
  profile in 8.38s`; total wall-clock from trunk start `18:36:18` to
  `success` at `18:36:39` = ~21s)
- Log: `target/diagnostics/logs/re-01-web-build.log` (2732 bytes)
- Dist wasm: `2026-05-19 21:36:39.347519153 +0300 114651355
  crates/bevy_naadf/dist/bevy-naadf-f0ba4e1b547d857c_bg.wasm`
  (newer than prior dispatch's `21:22:59` — confirms rebuild emitted a
  fresh wasm bundle.)
- Error grep (`error\[E[0-9]+\]|^error:|panicked|FATAL`): no matches.
  Only the usual 6 lint warnings (unused_imports, unreachable_code,
  unused_mut, unused_variables) plus the `-Ctarget-feature` atomics
  unstable-feature warning — none are build failures.

### 2. Web snapshot (this dispatch)
- Command: `cd e2e && timeout 240s npx playwright test
  device-snapshot.spec.ts --headed`
- Exit: 0
- Log: `target/diagnostics/logs/re-02-diag-web.log` (430 bytes)
- Output JSON: `target/diagnostics/device-snapshot-web.json` (5730 bytes,
  mtime `2026-05-19 21:36:54 +0300`)
- JSON parses OK: yes (`python3 -c "import json;
  json.load(open('target/diagnostics/device-snapshot-web.json'));
  print('parses ok')"` → `parses ok`)
- Browser-console grep (`panicked|RuntimeError|Uncaught|DeviceLost|
  fatal|Browser closed|Test timeout`): no matches.
- Playwright artefacts: only `e2e/test-results/.last-run.json` (45 bytes,
  contents `{"status":"passed","failedTests":[]}`). No per-test
  stderr/stdout/console captures created (test passed cleanly).
- Playwright stdout (verbatim, full file):
  ```
  Running 1 test using 1 worker

  [device-snapshot.spec] captured snapshot line (5729 bytes)
  [device-snapshot.spec] wrote /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/device-snapshot-web.json (5729 bytes)
    [pass] 1 [chromium] > tests/device-snapshot.spec.ts:44:3 > WASM device snapshot capture > capture device-snapshot sentinel and write JSON to disk (3.3s)

    1 passed (4.7s)
  ```

### 3. Compare (this dispatch)
- Command: `timeout 30s just diag-compare`
- Exit: 1 from `just` recipe (the `diag_compare` binary exits non-zero
  when unexpected divergences exist — that is the design; full stdout
  was captured via `tee` before the recipe's non-zero exit).
- Log: `target/diagnostics/logs/re-03-diag-compare.log` (15925 bytes)

## Comparison output (FULL, verbatim, NO truncation)
```
cargo run --quiet --bin diag_compare
== device-snapshot diff ==
native: target/diagnostics/device-snapshot-native.json
   web: target/diagnostics/device-snapshot-web.json

--- TOP DIVERGENCES (load-bearing + unexpected, max 10) ---
  >>> LOAD-BEARING <<< adapter_features.only_in_native: native = [acceleration-structure-binding-array, address-mode-clamp-to-border, address-mode-clamp-to-zero, buffer-binding-array, clear-texture, conservative-rasterization, experimental-cooperative-matrix, experimental-mesh-shader, experimental-mesh-shader-multiview, experimental-mesh-shader-points, experimental-ray-hit-vertex-return, experimental-ray-query, extended-acceleration-structure-vertex-formats, immediates, mappable-primary-buffers, memory-decoration-coherent, memory-decoration-volatile, multi-draw-indirect-count, multisample-array, multiview, partially-bound-binding-array, passthrough-shaders, pipeline-cache, pipeline-statistics-query, polygon-mode-line, polygon-mode-point, primitive-index, sampled-texture-and-storage-buffer-array-non-uniform-indexing, selective-multiview, shader-barycentrics, shader-draw-index, shader-early-depth-test, shader-f16, shader-f64, shader-float32-atomic, shader-i16, shader-int64, shader-int64-atomic-all-ops, shader-int64-atomic-min-max, shader-per-vertex, storage-resource-binding-array, storage-texture-array-non-uniform-indexing, subgroup, subgroup-barrier, subgroup-vertex, texture-adapter-specific-format-features, texture-atomic, texture-binding-array, texture-format-16bit-norm, texture-format-nv12, texture-format-p010, texture-int64-atomic, timestamp-query-inside-encoders, timestamp-query-inside-passes, vertex-writable-storage]  |  web = []
  >>> LOAD-BEARING <<< adapter_limits.max_buffer_size: native = 1099511627776  |  web = 4294967292
  >>> LOAD-BEARING <<< adapter_limits.max_storage_buffers_per_shader_stage: native = 524288  |  web = 16
  >>> LOAD-BEARING <<< device_features.only_in_native: native = [acceleration-structure-binding-array, address-mode-clamp-to-border, address-mode-clamp-to-zero, buffer-binding-array, clear-texture, conservative-rasterization, experimental-cooperative-matrix, experimental-mesh-shader, experimental-mesh-shader-multiview, experimental-mesh-shader-points, experimental-ray-hit-vertex-return, experimental-ray-query, extended-acceleration-structure-vertex-formats, immediates, memory-decoration-coherent, memory-decoration-volatile, multi-draw-indirect-count, multisample-array, multiview, partially-bound-binding-array, passthrough-shaders, pipeline-cache, pipeline-statistics-query, polygon-mode-line, polygon-mode-point, primitive-index, sampled-texture-and-storage-buffer-array-non-uniform-indexing, selective-multiview, shader-barycentrics, shader-draw-index, shader-early-depth-test, shader-f16, shader-f64, shader-float32-atomic, shader-i16, shader-int64, shader-int64-atomic-all-ops, shader-int64-atomic-min-max, shader-per-vertex, storage-resource-binding-array, storage-texture-array-non-uniform-indexing, subgroup, subgroup-barrier, subgroup-vertex, texture-adapter-specific-format-features, texture-atomic, texture-binding-array, texture-format-16bit-norm, texture-format-nv12, texture-format-p010, texture-int64-atomic, timestamp-query-inside-encoders, timestamp-query-inside-passes, vertex-writable-storage]  |  web = []
  >>> LOAD-BEARING <<< device_limits.max_buffer_size: native = 1099511627776  |  web = 4294967292
  >>> LOAD-BEARING <<< device_limits.max_storage_buffers_per_shader_stage: native = 524288  |  web = 16
  >>> DIVERGENCE   <<< adapter_features_bits: native = [262031,9223178486116433915]  |  web = [64911,0]
  >>> DIVERGENCE   <<< adapter_info.subgroup_max_size: native = 32  |  web = 128
  >>> DIVERGENCE   <<< adapter_info.subgroup_min_size: native = 32  |  web = 4
  >>> DIVERGENCE   <<< adapter_limits.max_acceleration_structures_per_shader_stage: native = 524288  |  web = 0

--- ALL DIVERGENCES (full list) ---
  >>> LOAD-BEARING <<< adapter_features.only_in_native: native = [acceleration-structure-binding-array, address-mode-clamp-to-border, address-mode-clamp-to-zero, buffer-binding-array, clear-texture, conservative-rasterization, experimental-cooperative-matrix, experimental-mesh-shader, experimental-mesh-shader-multiview, experimental-mesh-shader-points, experimental-ray-hit-vertex-return, experimental-ray-query, extended-acceleration-structure-vertex-formats, immediates, mappable-primary-buffers, memory-decoration-coherent, memory-decoration-volatile, multi-draw-indirect-count, multisample-array, multiview, partially-bound-binding-array, passthrough-shaders, pipeline-cache, pipeline-statistics-query, polygon-mode-line, polygon-mode-point, primitive-index, sampled-texture-and-storage-buffer-array-non-uniform-indexing, selective-multiview, shader-barycentrics, shader-draw-index, shader-early-depth-test, shader-f16, shader-f64, shader-float32-atomic, shader-i16, shader-int64, shader-int64-atomic-all-ops, shader-int64-atomic-min-max, shader-per-vertex, storage-resource-binding-array, storage-texture-array-non-uniform-indexing, subgroup, subgroup-barrier, subgroup-vertex, texture-adapter-specific-format-features, texture-atomic, texture-binding-array, texture-format-16bit-norm, texture-format-nv12, texture-format-p010, texture-int64-atomic, timestamp-query-inside-encoders, timestamp-query-inside-passes, vertex-writable-storage]  |  web = []
  >>> LOAD-BEARING <<< adapter_limits.max_buffer_size: native = 1099511627776  |  web = 4294967292
  >>> LOAD-BEARING <<< adapter_limits.max_storage_buffers_per_shader_stage: native = 524288  |  web = 16
  >>> LOAD-BEARING <<< device_features.only_in_native: native = [acceleration-structure-binding-array, address-mode-clamp-to-border, address-mode-clamp-to-zero, buffer-binding-array, clear-texture, conservative-rasterization, experimental-cooperative-matrix, experimental-mesh-shader, experimental-mesh-shader-multiview, experimental-mesh-shader-points, experimental-ray-hit-vertex-return, experimental-ray-query, extended-acceleration-structure-vertex-formats, immediates, memory-decoration-coherent, memory-decoration-volatile, multi-draw-indirect-count, multisample-array, multiview, partially-bound-binding-array, passthrough-shaders, pipeline-cache, pipeline-statistics-query, polygon-mode-line, polygon-mode-point, primitive-index, sampled-texture-and-storage-buffer-array-non-uniform-indexing, selective-multiview, shader-barycentrics, shader-draw-index, shader-early-depth-test, shader-f16, shader-f64, shader-float32-atomic, shader-i16, shader-int64, shader-int64-atomic-all-ops, shader-int64-atomic-min-max, shader-per-vertex, storage-resource-binding-array, storage-texture-array-non-uniform-indexing, subgroup, subgroup-barrier, subgroup-vertex, texture-adapter-specific-format-features, texture-atomic, texture-binding-array, texture-format-16bit-norm, texture-format-nv12, texture-format-p010, texture-int64-atomic, timestamp-query-inside-encoders, timestamp-query-inside-passes, vertex-writable-storage]  |  web = []
  >>> LOAD-BEARING <<< device_limits.max_buffer_size: native = 1099511627776  |  web = 4294967292
  >>> LOAD-BEARING <<< device_limits.max_storage_buffers_per_shader_stage: native = 524288  |  web = 16
  >>> DIVERGENCE   <<< adapter_features_bits: native = [262031,9223178486116433915]  |  web = [64911,0]
  >>> DIVERGENCE   <<< adapter_info.subgroup_max_size: native = 32  |  web = 128
  >>> DIVERGENCE   <<< adapter_info.subgroup_min_size: native = 32  |  web = 4
  >>> DIVERGENCE   <<< adapter_limits.max_acceleration_structures_per_shader_stage: native = 524288  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_bind_groups: native = 8  |  web = 4
  >>> DIVERGENCE   <<< adapter_limits.max_binding_array_acceleration_structure_elements_per_shader_stage: native = 524288  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_binding_array_elements_per_shader_stage: native = 1048576  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_binding_array_sampler_elements_per_shader_stage: native = 1048576  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_bindings_per_bind_group: native = 4294967295  |  web = 1000
  >>> DIVERGENCE   <<< adapter_limits.max_blas_geometry_count: native = 16777215  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_blas_primitive_count: native = 536870911  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_dynamic_storage_buffers_per_pipeline_layout: native = 16  |  web = 8
  >>> DIVERGENCE   <<< adapter_limits.max_dynamic_uniform_buffers_per_pipeline_layout: native = 15  |  web = 10
  >>> DIVERGENCE   <<< adapter_limits.max_immediate_size: native = 256  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_inter_stage_shader_variables: native = 31  |  web = 28
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_invocations_per_dimension: native = 128  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_invocations_per_workgroup: native = 128  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_multiview_view_count: native = 4  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_output_layers: native = 2048  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_output_primitives: native = 256  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_mesh_output_vertices: native = 256  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_multiview_view_count: native = 32  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_non_sampler_bindings: native = 4294967295  |  web = 1000000
  >>> DIVERGENCE   <<< adapter_limits.max_sampled_textures_per_shader_stage: native = 524288  |  web = 48
  >>> DIVERGENCE   <<< adapter_limits.max_samplers_per_shader_stage: native = 524288  |  web = 16
  >>> DIVERGENCE   <<< adapter_limits.max_storage_textures_per_shader_stage: native = 524288  |  web = 8
  >>> DIVERGENCE   <<< adapter_limits.max_task_invocations_per_dimension: native = 128  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_task_invocations_per_workgroup: native = 128  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_task_mesh_workgroup_total_count: native = 4194304  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_task_mesh_workgroups_per_dimension: native = 65535  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_task_payload_size: native = 16384  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_texture_dimension_1d: native = 32768  |  web = 16384
  >>> DIVERGENCE   <<< adapter_limits.max_texture_dimension_2d: native = 32768  |  web = 16384
  >>> DIVERGENCE   <<< adapter_limits.max_texture_dimension_3d: native = 16384  |  web = 2048
  >>> DIVERGENCE   <<< adapter_limits.max_tlas_instance_count: native = 16777215  |  web = 0
  >>> DIVERGENCE   <<< adapter_limits.max_uniform_buffers_per_shader_stage: native = 524288  |  web = 12
  >>> DIVERGENCE   <<< adapter_limits.max_vertex_attributes: native = 32  |  web = 30
  >>> DIVERGENCE   <<< adapter_limits.max_vertex_buffers: native = 16  |  web = 8
  >>> DIVERGENCE   <<< adapter_limits.min_storage_buffer_offset_alignment: native = 32  |  web = 256
  >>> DIVERGENCE   <<< adapter_limits.min_uniform_buffer_offset_alignment: native = 64  |  web = 256
  >>> DIVERGENCE   <<< device_features_bits: native = [262031,9223178486116433787]  |  web = [64911,0]
  >>> DIVERGENCE   <<< device_limits.max_acceleration_structures_per_shader_stage: native = 524288  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_bind_groups: native = 8  |  web = 4
  >>> DIVERGENCE   <<< device_limits.max_binding_array_acceleration_structure_elements_per_shader_stage: native = 524288  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_binding_array_elements_per_shader_stage: native = 1048576  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_binding_array_sampler_elements_per_shader_stage: native = 1048576  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_bindings_per_bind_group: native = 4294967295  |  web = 1000
  >>> DIVERGENCE   <<< device_limits.max_blas_geometry_count: native = 16777215  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_blas_primitive_count: native = 536870911  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_dynamic_storage_buffers_per_pipeline_layout: native = 16  |  web = 8
  >>> DIVERGENCE   <<< device_limits.max_dynamic_uniform_buffers_per_pipeline_layout: native = 15  |  web = 10
  >>> DIVERGENCE   <<< device_limits.max_immediate_size: native = 256  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_inter_stage_shader_variables: native = 31  |  web = 28
  >>> DIVERGENCE   <<< device_limits.max_mesh_invocations_per_dimension: native = 128  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_mesh_invocations_per_workgroup: native = 128  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_mesh_multiview_view_count: native = 4  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_mesh_output_layers: native = 2048  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_mesh_output_primitives: native = 256  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_mesh_output_vertices: native = 256  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_multiview_view_count: native = 32  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_non_sampler_bindings: native = 4294967295  |  web = 1000000
  >>> DIVERGENCE   <<< device_limits.max_sampled_textures_per_shader_stage: native = 524288  |  web = 48
  >>> DIVERGENCE   <<< device_limits.max_samplers_per_shader_stage: native = 524288  |  web = 16
  >>> DIVERGENCE   <<< device_limits.max_storage_textures_per_shader_stage: native = 524288  |  web = 8
  >>> DIVERGENCE   <<< device_limits.max_task_invocations_per_dimension: native = 128  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_task_invocations_per_workgroup: native = 128  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_task_mesh_workgroup_total_count: native = 4194304  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_task_mesh_workgroups_per_dimension: native = 65535  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_task_payload_size: native = 16384  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_texture_dimension_1d: native = 32768  |  web = 16384
  >>> DIVERGENCE   <<< device_limits.max_texture_dimension_2d: native = 32768  |  web = 16384
  >>> DIVERGENCE   <<< device_limits.max_texture_dimension_3d: native = 16384  |  web = 2048
  >>> DIVERGENCE   <<< device_limits.max_tlas_instance_count: native = 16777215  |  web = 0
  >>> DIVERGENCE   <<< device_limits.max_uniform_buffers_per_shader_stage: native = 524288  |  web = 12
  >>> DIVERGENCE   <<< device_limits.max_vertex_attributes: native = 32  |  web = 30
  >>> DIVERGENCE   <<< device_limits.max_vertex_buffers: native = 16  |  web = 8
  >>> DIVERGENCE   <<< device_limits.min_storage_buffer_offset_alignment: native = 32  |  web = 256
  >>> DIVERGENCE   <<< device_limits.min_uniform_buffer_offset_alignment: native = 64  |  web = 256
  (expected)          adapter_info.backend: native = "vulkan"  |  web = "browserwebgpu"
  (expected)          adapter_info.device: native = 11266  |  web = 0
  (expected)          adapter_info.device_pci_bus_id: native = "0000:01:00.0"  |  web = ""
  (expected)          adapter_info.device_type: native = "DiscreteGpu"  |  web = "Other"
  (expected)          adapter_info.driver: native = "NVIDIA"  |  web = ""
  (expected)          adapter_info.driver_info: native = "595.71.05"  |  web = ""
  (expected)          adapter_info.vendor: native = 4318  |  web = 0
  (expected)          build.target_arch: native = "x86_64"  |  web = "wasm32"
  (expected)          build.target_os: native = "linux"  |  web = ""
  (expected)          captured_at_unix_seconds: native = 1779214931  |  web = 1779215812
  (expected)          target: native = "native"  |  web = "web"

== summary ==
  total divergences:    95
  expected divergences: 11
  unexpected:           84
  load-bearing:         6
error: Recipe `diag-compare` failed on line 210 with exit code 1
```

## Artifacts on disk (absolute paths)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/device-snapshot-native.json` (8871 B)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/device-snapshot-web.json` (5730 B)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/01-cargo-check.log` (294 B — prior dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/01-cargo-build-release.log` (595 B — prior dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/01-diag-native.log` (14546 B — prior dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/02-diag-native.log` (13693 B — prior dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/03-web-build.log` (2732 B — prior dispatch, stale)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/04-diag-web.log` (2644 B — prior dispatch, stale)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/re-01-web-build.log` (2732 B — THIS dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/re-02-diag-web.log` (430 B — THIS dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/re-03-diag-compare.log` (15925 B — THIS dispatch)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.last-run.json` (45 B — Playwright result marker, `{"status":"passed"}`)

## Anomalies observed this dispatch (raw, no diagnosis)
- Compare-recipe exit is 1 (`error: Recipe `diag-compare` failed on line 210
  with exit code 1`) but full diff output was already streamed before exit —
  this is the design (non-zero on unexpected divergences) and was not a
  Playwright/build failure. The `tee` pipeline reported exit=0 because tee
  was the last process in the pipe.
- Web snapshot is 5730 bytes; native is 8871 bytes — web JSON is meaningfully
  smaller. The shorter wasm-side JSON correlates with the long `only_in_native`
  feature lists and zero-valued web limits seen in the diff (the web side
  emits empty arrays / 0 ints rather than absent fields).
- 84 unexpected divergences total, 6 flagged as load-bearing.
- The load-bearing set includes both `adapter_*` and `device_*` mirrors of
  the same three categories: `*_features.only_in_native`,
  `*_limits.max_buffer_size`, `*_limits.max_storage_buffers_per_shader_stage`.
- Web `adapter_info.subgroup_max_size = 128` but native = 32 (web reports a
  larger max-size than native — opposite direction from the rest of the
  limits, which are typically equal-or-smaller on web).
- Web `adapter_limits.max_non_sampler_bindings = 1000000` vs native
  `4294967295` — web reports a finite cap while native reports `u32::MAX`.
- `e2e/test-results/` did NOT receive per-test stderr/stdout/console.log
  files (only `.last-run.json`); Playwright only persists those when a test
  fails. With the test passing, no extra artefacts to grep.
- `crates/bevy_naadf/dist/bevy-naadf-f0ba4e1b547d857c_bg.wasm` retained the
  same content-hash filename (`f0ba4e1b547d857c`) as the prior dispatch —
  i.e. the source hash that drives the trunk asset name was unchanged,
  consistent with this being a rebuild of identical source.
- Captured-at timestamps: native = 1779214931, web = 1779215812 (881 seconds
  apart; the web snapshot is newer, as expected for this dispatch ordering).
