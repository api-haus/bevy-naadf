//! `GrowableBuffer<T>` — the wgpu/Bevy equivalent of NAADF's
//! `Common/DynamicStructuredBuffer.cs` (`03-design.md` §3).
//!
//! A growable GPU storage buffer: when `reserve` is asked for more elements
//! than the current capacity, a larger buffer is allocated and the old
//! contents are copied across with `copy_buffer_to_buffer` (the wgpu analogue
//! of the C# `Resize` + `CopyData`). Growth is by a factor of [`GROWTH_FACTOR`]
//! (the C# "resize by 100 %" / `02-research.md` open question #2 "growth factor
//! 2×").
//!
//! Phase A uses this for the `blocks`, `voxels`, and `voxel_types` buffers
//! (`03-design.md` §2.5, §3.3). In Phase A all three are sized once at
//! test-grid build time, so `reserve` is exercised but growth is rare.
//!
//! **No chunked copies** — the DX11 ~2 GB structured-buffer copy limit the C#
//! `Helper`/`dataCopy.fx` works around does not apply to wgpu/Vulkan
//! (`03-design.md` §3.2). A `debug_assert!` keeps `max_buffer_size` visible;
//! a chunked-copy loop is a localised Phase-B extension point if larger worlds
//! ever approach the ceiling.

use std::marker::PhantomData;

use bevy::render::{
    render_resource::{Buffer, BufferDescriptor, BufferUsages, CommandEncoder},
    renderer::{RenderDevice, RenderQueue},
};
use bytemuck::Pod;

/// Capacity growth factor on a `reserve` that exceeds the current capacity —
/// the C# `DynamicStructuredBuffer` "resize by 100 %" (`03-design.md` §3.1).
pub const GROWTH_FACTOR: u64 = 2;

/// The buffer usages every [`GrowableBuffer`] is created with: a storage
/// buffer that can also be the source / destination of a buffer copy (needed
/// for the grow-and-copy path).
pub const GROWABLE_BUFFER_USAGES: BufferUsages = BufferUsages::STORAGE
    .union(BufferUsages::COPY_SRC)
    .union(BufferUsages::COPY_DST);

/// A growable GPU storage buffer of `T` elements.
///
/// `capacity` is the allocated element count; `len` is the logical element
/// count currently in use. `reserve` grows the allocation (copying old
/// contents); `write` uploads element data via the queue.
pub struct GrowableBuffer<T: Pod> {
    buffer: Buffer,
    /// Allocated capacity, in elements.
    capacity: u64,
    /// Logical element count in use.
    len: u64,
    label: &'static str,
    _t: PhantomData<T>,
}

impl<T: Pod> GrowableBuffer<T> {
    /// Size of one `T` element, in bytes.
    fn elem_size() -> u64 {
        std::mem::size_of::<T>() as u64
    }

    /// Create a new growable buffer with `capacity` elements allocated and a
    /// logical length of zero. `capacity` is clamped to at least 1 (a wgpu
    /// buffer cannot have size 0).
    pub fn new(device: &RenderDevice, label: &'static str, capacity: u64) -> Self {
        let capacity = capacity.max(1);
        let size = capacity * Self::elem_size();
        debug_assert!(
            size <= device.limits().max_buffer_size,
            "GrowableBuffer `{label}` initial size {size} exceeds max_buffer_size {}",
            device.limits().max_buffer_size,
        );
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: GROWABLE_BUFFER_USAGES,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            capacity,
            len: 0,
            label,
            _t: PhantomData,
        }
    }

    /// Ensure the buffer can hold at least `min_capacity` elements, growing
    /// (and copying the old contents across) if it currently cannot.
    ///
    /// On growth the new capacity is
    /// `max(min_capacity, capacity * GROWTH_FACTOR)` and the existing
    /// `capacity` elements are copied old → new via `copy_buffer_to_buffer`
    /// on the supplied `encoder` (mirroring the C# `Resize` + `CopyData`).
    /// The old buffer is returned so the caller can keep it alive until the
    /// encoder is submitted; drop it (or stash it for one frame) afterwards.
    /// Returns `None` when no growth was needed.
    #[must_use = "the returned old buffer must outlive the encoder submission"]
    pub fn reserve(
        &mut self,
        min_capacity: u64,
        device: &RenderDevice,
        encoder: &mut CommandEncoder,
    ) -> Option<Buffer> {
        if min_capacity <= self.capacity {
            return None;
        }
        let new_cap = min_capacity.max(self.capacity * GROWTH_FACTOR);
        let new_size = new_cap * Self::elem_size();
        debug_assert!(
            new_size <= device.limits().max_buffer_size,
            "GrowableBuffer `{}` grow to {new_size} exceeds max_buffer_size {}",
            self.label,
            device.limits().max_buffer_size,
        );
        let new_buffer = device.create_buffer(&BufferDescriptor {
            label: Some(self.label),
            size: new_size,
            usage: GROWABLE_BUFFER_USAGES,
            mapped_at_creation: false,
        });
        // Copy the whole old allocation across — the C# `CopyData` path.
        encoder.copy_buffer_to_buffer(
            &self.buffer,
            0,
            &new_buffer,
            0,
            self.capacity * Self::elem_size(),
        );
        let old = std::mem::replace(&mut self.buffer, new_buffer);
        self.capacity = new_cap;
        Some(old)
    }

    /// Upload `data` into the buffer starting at element offset `offset`.
    ///
    /// Extends the logical length to cover the written range. Panics in debug
    /// builds if the write would run past the allocated capacity — call
    /// [`reserve`](Self::reserve) first.
    pub fn write(&mut self, offset: u64, data: &[T], queue: &RenderQueue) {
        if data.is_empty() {
            return;
        }
        let end = offset + data.len() as u64;
        debug_assert!(
            end <= self.capacity,
            "GrowableBuffer `{}` write to element {end} exceeds capacity {} — reserve first",
            self.label,
            self.capacity,
        );
        queue.write_buffer(
            &self.buffer,
            offset * Self::elem_size(),
            bytemuck::cast_slice(data),
        );
        self.len = self.len.max(end);
    }

    /// Reserve capacity for at least `min_capacity` elements *without*
    /// preserving the old contents — for the case where the whole buffer is
    /// about to be overwritten ([`upload_all`](Self::upload_all)).
    ///
    /// Unlike [`reserve`](Self::reserve) this does **not** issue a
    /// `copy_buffer_to_buffer`, so it needs no encoder and has no ordering
    /// hazard against a subsequent `queue.write_buffer`. The old buffer is
    /// dropped immediately (nothing references it once the contents are
    /// discarded).
    fn reserve_discard(&mut self, min_capacity: u64, device: &RenderDevice) {
        if min_capacity <= self.capacity {
            return;
        }
        let new_cap = min_capacity.max(self.capacity * GROWTH_FACTOR);
        let new_size = new_cap * Self::elem_size();
        debug_assert!(
            new_size <= device.limits().max_buffer_size,
            "GrowableBuffer `{}` grow to {new_size} exceeds max_buffer_size {}",
            self.label,
            device.limits().max_buffer_size,
        );
        self.buffer = device.create_buffer(&BufferDescriptor {
            label: Some(self.label),
            size: new_size,
            usage: GROWABLE_BUFFER_USAGES,
            mapped_at_creation: false,
        });
        self.capacity = new_cap;
        self.len = 0;
    }

    /// Reserve (growing if needed) then write `data` at element 0, setting the
    /// logical length to `data.len()`. The convenience path Phase A's
    /// build-once upload uses.
    ///
    /// Because the whole buffer is replaced, the grow path here discards the
    /// old contents rather than copying them — so there is no ordering hazard
    /// between the realloc and the `queue.write_buffer` (a `reserve` +
    /// `copy_buffer_to_buffer` would race the staged write, since wgpu applies
    /// queued writes *before* the command buffer of the same submit).
    pub fn upload_all(&mut self, data: &[T], device: &RenderDevice, queue: &RenderQueue) {
        self.reserve_discard(data.len() as u64, device);
        self.write(0, data, queue);
    }

    /// The underlying GPU buffer handle (for bind-group construction).
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Allocated capacity, in elements.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Logical element count in use.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the logical length is zero.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::render_resource::{CommandEncoderDescriptor, MapMode, PollType};
    use bevy::render::settings::RenderCreation;
    use bevy::render::{RenderApp, RenderPlugin};
    use bevy::MinimalPlugins;

    /// Build a headless render world and pull out its `RenderDevice` +
    /// `RenderQueue` so the buffer paths can be exercised against a real (or
    /// software-fallback) wgpu device. Returns `None` if no adapter is
    /// available (e.g. a CI box with no GPU) — the device-dependent tests then
    /// skip rather than fail.
    ///
    /// `RenderPlugin` creates its device asynchronously; its `Plugin::ready`
    /// blocks `App::finish` until the future resolves, and `Plugin::finish`
    /// then unpacks the `RenderDevice`/`RenderQueue` into the render sub-app.
    /// So `finish()` + `cleanup()` is enough — we never run a render schedule,
    /// which would panic without a window / full plugin set.
    fn render_device_queue() -> Option<(RenderDevice, RenderQueue)> {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            // `RenderPlugin` pulls in the render-asset plugins, which need the
            // asset + image plugins present to boot.
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::default()),
                synchronous_pipeline_compilation: true,
                debug_flags: Default::default(),
            });
        app.finish();
        app.cleanup();
        let render_app = app.get_sub_app(RenderApp)?;
        let device = render_app.world().get_resource::<RenderDevice>()?.clone();
        let queue = render_app.world().get_resource::<RenderQueue>()?.clone();
        Some((device, queue))
    }

    /// Read the first `count` `u32`s of `src` back to the CPU (test helper).
    fn readback_u32(
        device: &RenderDevice,
        queue: &RenderQueue,
        src: &Buffer,
        count: u64,
    ) -> Vec<u32> {
        let size = count * 4;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("readback"),
        });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(MapMode::Read, |r| r.unwrap());
        device.poll(PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        out
    }

    #[test]
    fn new_clamps_capacity_to_one() {
        let Some((device, _queue)) = render_device_queue() else {
            eprintln!("no wgpu device — skipping GrowableBuffer device test");
            return;
        };
        let buf = GrowableBuffer::<u32>::new(&device, "test", 0);
        assert_eq!(buf.capacity(), 1);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn write_within_capacity_no_grow() {
        let Some((device, queue)) = render_device_queue() else {
            eprintln!("no wgpu device — skipping GrowableBuffer device test");
            return;
        };
        let mut buf = GrowableBuffer::<u32>::new(&device, "test", 8);
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("t"),
        });
        // 4 elements fit in capacity 8 — reserve must not grow.
        assert!(buf.reserve(4, &device, &mut encoder).is_none());
        buf.write(0, &[10u32, 11, 12, 13], &queue);
        queue.submit([encoder.finish()]);
        assert_eq!(buf.capacity(), 8);
        assert_eq!(buf.len(), 4);
        let back = readback_u32(&device, &queue, buf.buffer(), 4);
        assert_eq!(back, vec![10, 11, 12, 13]);
    }

    #[test]
    fn reserve_grows_and_copies_old_contents() {
        let Some((device, queue)) = render_device_queue() else {
            eprintln!("no wgpu device — skipping GrowableBuffer device test");
            return;
        };
        let mut buf = GrowableBuffer::<u32>::new(&device, "grow", 4);
        // Fill the initial 4-element allocation.
        {
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("fill"),
            });
            assert!(buf.reserve(4, &device, &mut encoder).is_none());
            buf.write(0, &[1u32, 2, 3, 4], &queue);
            queue.submit([encoder.finish()]);
        }
        // Reserve for 6 — must grow. capacity * GROWTH_FACTOR = 8 >= 6, so the
        // new capacity is 8. The old contents must survive the copy.
        let old = {
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("grow"),
            });
            let old = buf.reserve(6, &device, &mut encoder);
            assert!(old.is_some(), "reserve(6) on capacity 4 must grow");
            // Write two more elements into the freshly grown region.
            buf.write(4, &[5u32, 6], &queue);
            queue.submit([encoder.finish()]);
            old
        };
        // Old buffer kept alive across the submit, then dropped.
        drop(old);
        assert_eq!(buf.capacity(), 8);
        assert_eq!(buf.len(), 6);
        let back = readback_u32(&device, &queue, buf.buffer(), 6);
        assert_eq!(back, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn reserve_honours_min_capacity_over_growth_factor() {
        let Some((device, _queue)) = render_device_queue() else {
            eprintln!("no wgpu device — skipping GrowableBuffer device test");
            return;
        };
        let mut buf = GrowableBuffer::<u32>::new(&device, "min", 4);
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("t"),
        });
        // min_capacity 100 dwarfs capacity * GROWTH_FACTOR (8) — new cap is 100.
        let old = buf.reserve(100, &device, &mut encoder);
        assert!(old.is_some());
        drop(encoder);
        drop(old);
        assert_eq!(buf.capacity(), 100);
    }

    #[test]
    fn upload_all_grows_then_writes() {
        let Some((device, queue)) = render_device_queue() else {
            eprintln!("no wgpu device — skipping GrowableBuffer device test");
            return;
        };
        let mut buf = GrowableBuffer::<u32>::new(&device, "upload", 2);
        let data: Vec<u32> = (100..110).collect();
        // Capacity 2 → must grow to hold 10. `upload_all` discards the old
        // (never-written) contents rather than copying them, so no realloc /
        // queued-write ordering hazard.
        buf.upload_all(&data, &device, &queue);
        assert!(buf.capacity() >= 10);
        assert_eq!(buf.len(), 10);
        let back = readback_u32(&device, &queue, buf.buffer(), 10);
        assert_eq!(back, data);
    }
}
