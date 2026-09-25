# SpaceSoupVR fork of wgpu

This branch (`spacesoup/v30.0.1`) is upstream **wgpu v30.0.1** (`40f4a34`), the
exact release our standalone-VR renderer builds against, plus the patches below.
Everything else is unchanged upstream code under its original licences
(MIT / Apache-2.0).

## Why a fork

Our renderer targets Meta Quest 3 (Adreno 740, Vulkan). Two features it needs
can't be reached through stock wgpu:

1. **Fixed foveated rendering.** Quest FFR attaches a Vulkan fragment density
   map (`VK_EXT_fragment_density_map` / `_map2`, supplied by the runtime via
   `XR_FB_foveation_vulkan`) to the render pass. It optionally renders into
   subsampled images (`VK_IMAGE_CREATE_SUBSAMPLED_BIT_EXT`). wgpu has no way to
   attach a density map to a render pass.
2. **Half-precision shading on Adreno.** `Features::SHADER_F16` requires
   `uniformAndStorageBuffer16BitAccess`, which no Qualcomm GPU exposes
   (gpuweb#5006), even though `shaderFloat16` is available. We want f16
   arithmetic in shaders without 16-bit uniform storage.

## Planned patches (each small, isolated, upstreamable where possible)

- [ ] `wgpu-hal`/`wgpu-core`: an optional fragment-density-map attachment on
      render passes (Vulkan backend), and subsampled image creation.
- [ ] `wgpu-hal`/`naga`: f16 arithmetic behind a feature that requires only
      `shaderFloat16` (+ 16-bit storage-buffer access where used), keeping
      uniforms 32-bit.
- [ ] Investigate: naga SPIR-V `RelaxedPrecision` decorations as an alternative
      route to mediump.
- [ ] Investigate: combined image-samplers on the Vulkan backend (Qualcomm
      reports a 2–5% fill cost for separate samplers).

## Using it

In the consuming workspace:

```toml
[patch.crates-io]
wgpu      = { git = "https://github.com/SpaceSoupVR/wgpu", branch = "spacesoup/v30.0.1" }
wgpu-core = { git = "https://github.com/SpaceSoupVR/wgpu", branch = "spacesoup/v30.0.1" }
wgpu-hal  = { git = "https://github.com/SpaceSoupVR/wgpu", branch = "spacesoup/v30.0.1" }
wgpu-types= { git = "https://github.com/SpaceSoupVR/wgpu", branch = "spacesoup/v30.0.1" }
naga      = { git = "https://github.com/SpaceSoupVR/wgpu", branch = "spacesoup/v30.0.1" }
```

## Policy

- A new upstream release gets a new branch `spacesoup/vX.Y.Z` with the patches
  rebased onto it. Old branches stay, so every build is reproducible.
- Every patch lands with a test, and with a headset measurement if it's a
  performance change.
