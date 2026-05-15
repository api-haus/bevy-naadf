# 00b — Research: how does C# NAADF handle resize for GI + TAA rings?

## C# reference location
- Absolute path: `/mnt/archive4/DEV/NAADF/NAADF/`
- Framework: MonoGame (XNA-compatible: `Microsoft.Xna.Framework.*` imports throughout)
- Verification: `App.cs:1` imports `Microsoft.Xna.Framework`; `WorldRenderBase.cs:2` imports
  `Microsoft.Xna.Framework.Graphics`; `App.cs:12` declares `public class App : Game` (the
  MonoGame `Game` base class).

---

## Resize event handling (high level)

The C# code handles window resize explicitly and correctly. `App.cs` wires up a `form.Resize`
event on initialization (`App.cs:79`) to the handler `Window_ClientSizeChanged`
(`App.cs:33-42`). That handler records the new width/height into `graphics.PreferredBackBuffer*`
and sets a debounce counter `GraphicsNeedApplyChanges = 10`. In `Update()` (`App.cs:102-108`)
the counter decrements each frame; when it reaches 1, `graphics.ApplyChanges()` is called and
immediately followed by firing the static `GraphicsChanged` event (`App.cs:104-105`). That
event is subscribed at construction time (`App.cs:55`) to `UpdateEverythingOfGraphics`
(`App.cs:44-50`), which:

1. Updates `App.ScreenWidth` / `App.ScreenHeight` to the new values.
2. Calls `worldHandler.ScreenUpdate()`.

`WorldHandler.ScreenUpdate()` (`WorldHandler.cs:65-67`) delegates to
`WorldRender.render.ScreenUpdate()`.  
`WorldRender.ScreenUpdate()` (`WorldRender.cs:59-63`) updates the camera projection and then
calls the virtual `CreateScreenTextures()`.

So the resize path is:

```
form.Resize event
  → Window_ClientSizeChanged (deferred: GraphicsNeedApplyChanges = 10)
  → Update() loop: when counter reaches 1
      → graphics.ApplyChanges()
      → GraphicsChanged event
          → UpdateEverythingOfGraphics
              → worldHandler.ScreenUpdate()
                  → WorldRender.render.ScreenUpdate()
                      → CreateScreenTextures()   ← all buffer recreation lives here
```

---

## GI sample_counts ring on resize

**Reallocated and zero-cleared — no preservation.**

`WorldRenderBase.CreateScreenTextures()` (`WorldRenderBase.cs:104-171`) is the single
method called on every resize. It disposes every buffer unconditionally, including
`globalIlumSampleCounts`, and then reallocates the full set from scratch:

```csharp
// WorldRenderBase.cs:118
globalIlumSampleCounts?.Dispose();

// WorldRenderBase.cs:165
globalIlumSampleCounts = new StructuredBuffer(
    App.graphicsDevice,
    typeof(Uint2),
    128 + 3,                   // ← fixed size: the 128-frame ring + 3 overhead entries
    BufferUsage.None,
    ShaderAccess.ReadWrite
);
```

`StructuredBuffer` in MonoGame allocates fresh GPU memory. The DirectX/MonoGame
runtime initialises newly-allocated `StructuredBuffer` contents to zero by
convention (no explicit clear call is present, but the DX12 heap allocation
guarantee applies). The 128-frame ring is therefore zero-cleared on every resize,
regardless of whether any screen-space dimensions changed.

This is **confirmed from static reading** (`WorldRenderBase.cs:104-171`). The
`globalIlumSampleCounts` destruction + recreation is unconditional — there is no
"if screen size actually changed" guard, and there is no copy/blit from the old buffer.

Note: `globalIlumSampleCounts` is sized at **`128 + 3` entries** (not
`ScreenWidth × ScreenHeight`), matching the Bevy port's `SAMPLE_COUNTS_LEN`
(`gi.rs:497-501`). It is therefore dimensionally invariant across resize — the same
buffer size is allocated before and after resize. The C# code does not exploit this
invariance; it drops and reallocates regardless.

---

## TAA sample ring on resize

**Reallocated and zero-cleared — no preservation.**

`WorldRenderAlbedo.CreateScreenTextures()` (`WorldRenderAlbedo.cs:51-65`) and
`WorldRenderBase.CreateScreenTextures()` (`WorldRenderBase.cs:104-171`) both unconditionally
dispose and reallocate `taaSamples` and `taaSampleAccum` on every `ScreenUpdate()` call:

```csharp
// WorldRenderAlbedo.cs:53-54 (Albedo path) / WorldRenderBase.cs:125-127 (Base path)
taaSamples?.Dispose();
taaSampleAccum?.Dispose();

// WorldRenderBase.cs:146-148
taaSamples = new StructuredBuffer(
    App.graphicsDevice, typeof(Uint2),
    App.ScreenWidth * App.ScreenHeight * 32,  // ← 32-deep ring, pixel-count sized
    BufferUsage.None, ShaderAccess.ReadWrite
);
taaSampleAccum = new StructuredBuffer(
    App.graphicsDevice, typeof(Uint2),
    App.ScreenWidth * App.ScreenHeight,
    BufferUsage.None, ShaderAccess.ReadWrite
);
```

The camera-history CPU-side arrays (`taaSampleCamTransform`, `taaSampleCamTransformInvers`,
`oldCamPositions`, `taaSampleJitter`, `taaOldCamPosFromCurCamInt`) are also recreated as
fresh zero-initialised C# arrays on each `CreateScreenTextures()` call
(`WorldRenderBase.cs:150-154`; `WorldRenderAlbedo.cs:61-64`). This effectively clears all
128-frame camera-matrix history on resize as well.

The TAA ring is `pixel_count × 32` entries. Its disposal+reallocation on resize is
unconditional; no copy/blit from the old ring is performed.

---

## If preserved — by what mechanism

N/A — C# reallocates and zero-clears both rings on every resize.

---

## If reallocated — does it have the same visible-black-shadow symptom?

**Likely yes, by construction — but no explicit acknowledgement found in the C# source.**

The zero-clear of `taaSamples` (32-deep ring) and `globalIlumSampleCounts` (128-deep) on
resize necessarily produces the same drain window as the Bevy bug: the first 32 frames
post-resize have zero TAA history to draw from; the first 128 frames have a depleted GI
sample ring. During those drain windows, GI bounce lighting would be dark or absent,
producing exactly the "shadows go pitch black" symptom.

No user-facing bug report, README comment, TODO, or inline comment in the C# source
acknowledges this symptom for the resize case. This is consistent with one of two
explanations: (a) the C# application was primarily used at a fixed resolution during
development (the `Settings.cs` / `BuildFlags` structure suggests a preset-resolution
design model rather than free-drag resizing), or (b) the symptom was considered acceptable
transient behaviour at the time and not tracked. **This is an inference from static reading
— the C# codebase does not state which.**

A secondary observation: `WorldRender.ScreenUpdate()` also calls
`camera.UpdateProjection(…)` (`WorldRender.cs:61`), which updates the projection matrix
before any frame is rendered at the new resolution. The camera-history arrays are reset to
zero on resize (`WorldRenderBase.cs:150-154`), so post-resize frames 0..127 have a fresh
camera history with identity matrices at uninitialized slots. This is a separate source of
stale-reprojection artefacts beyond the GI/TAA ring drain.

---

## Implications for the Bevy fix

The proposed Bevy fix (Impl-B, `02-design.md`) — **preserve `sample_counts` across resize
and skip the zero-clear of `taa_samples` / `taa_sample_accum` on resize** — diverges from
the C# behaviour, which zero-clears everything. The divergence is a **deliberate bug fix**
on the Bevy port's part: the C# original has the same latent blackness symptom on resize,
but the C# codebase never explicitly addressed it.

The fix is sound precisely because `globalIlumSampleCounts` / `sample_counts` is
`128 + 3` entries regardless of screen resolution — the C# source confirms this at
`WorldRenderBase.cs:165` — so preserving it across a pixel-count change is safe and
correct. The C# code's unconditional dispose+reallocate (`WorldRenderBase.cs:118, 165`)
was conservative but unnecessary for this specific buffer.

The Bevy Impl-B fix therefore **corrects a bug that also exists in the C# original**, not
one the C# had already solved. The C# reference does not validate Impl-B; Impl-B's
correctness rests on the structural invariant (fixed-size `sample_counts`, shader-level
rejects discarding stale `taa_samples` entries) rather than on C# precedent.
