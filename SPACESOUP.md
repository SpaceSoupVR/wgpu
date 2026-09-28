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
- [x] `naga` (2026-09-28): the SPIR-V writer declares the 16-bit STORAGE
      capabilities (`StorageBuffer16BitAccess`,
      `UniformAndStorageBuffer16BitAccess`, `StoragePushConstant16`) only
      for a uniform, storage or push-constant variable that holds a 16-bit
      type, instead of for every 16-bit type. `f16` arithmetic then needs
      only `Float16`, i.e. `shaderFloat16`. Tests:
      `naga/tests/naga/spirv_capabilities.rs`
      (`float16_arithmetic_needs_no_storage_capability`, which fails without
      the patch, and `float16_in_buffers_declares_their_storage_capability`).
      Upstreamable as a bug fix: the old behaviour required capabilities
      the module never used.
- [ ] `wgpu-hal`: expose `SHADER_F16` from `shaderFloat16` alone. Not needed
      for SpaceSoup's renderer: it builds its own `VkDevice`, enables
      `shaderFloat16` itself and claims the feature through `device_from_raw`.
- [x] `wgpu-hal` (2026-09-28): `device_from_raw` trusts robust buffer/image
      access only when its extension (`VK_EXT_robustness2`,
      `VK_EXT_image_robustness`) is in `enabled_extensions`. It used the
      adapter's caps, which describe SUPPORT: a device the caller built without
      robust access got shaders compiled WITHOUT naga's buffer and image-load
      bounds checks, so an out-of-range read was undefined. (On Quest 3,
      turning robust access on instead cost 0.2-1.7 ms of GPU a frame.)
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
