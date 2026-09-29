//! FIXED FOVEATED RENDERING through `VK_EXT_fragment_density_map`
//! (SpaceSoupVR fork).
//!
//! A fragment density map tells the GPU how coarsely it may shade each part of
//! a render pass: one texel per block of pixels, two bytes (horizontal and
//! vertical density, 255 = every pixel shaded). Toward the edges of a headset's
//! lenses, where the optics blur the image anyway, a quarter of the fragments
//! is enough.
//!
//! # Every render pass carries one
//!
//! A pipeline is created against a render pass it must stay compatible with,
//! and a density map is part of a render pass. Giving foveated passes a map and
//! others none would need every pipeline twice. Instead, once foveation is
//! enabled EVERY render pass gets a map: the foveated one for a pass drawing
//! into a registered target (the eye images), and [`Foveation::full`] -- density
//! 1 everywhere, i.e. no foveation -- for everything else. Every pipeline is
//! then created compatible with a pass that has a map, and nothing above
//! wgpu-hal changes.
//!
//! Attachments stay ordinary images: the device must have
//! `fragmentDensityMapNonSubsampledImages`, which lets the driver write the
//! full-resolution result itself at the end of the pass.
//!
//! # Static maps
//!
//! A map's contents are written once, before any pass reads it, and never
//! again (`fragmentDensityMapDynamic` is not needed and not used). Changing the
//! pattern means adding another map and pointing the target at it.

use alloc::vec::Vec;

use ash::vk;
use naga::FastHashMap;

use super::IdentifiedTextureView;

/// A density map's contents: `width` x `height` texels, row-major, two bytes
/// each -- horizontal then vertical density, 255 for every pixel shaded, 127
/// for every other one, 63 for one in four. (128 asks for a fragment 1.99
/// pixels wide, which a driver keeping at least the requested density rounds
/// down to one.)
///
/// The GPU stretches the map over the whole render area: a map of
/// `ceil(w / 32)` x `ceil(h / 32)` texels gives each texel a 32-pixel square
/// (the texel size is the power of two nearest the ratio, clamped to the
/// device's range).
#[derive(Clone, Copy, Debug)]
pub struct DensityPattern<'a> {
    pub width: u32,
    pub height: u32,
    pub texels: &'a [u8],
}

/// One density map: an `R8G8_UNORM` image, left in
/// `FRAGMENT_DENSITY_MAP_OPTIMAL_EXT` from its upload on.
pub(super) struct DensityMap {
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub(super) view: IdentifiedTextureView,
}

/// The texels of [`Foveation::full`] a side. A map must be at least
/// `ceil(extent / maxFragmentDensityTexelSize)` texels, and Adreno's largest
/// texel is 256 pixels: 64 texels covers passes up to 16384 pixels.
const FULL_MAP_TEXELS: u32 = 64;

pub(super) struct Foveation {
    /// Density 1 everywhere: the map of every pass not drawing into a
    /// registered target.
    pub(super) full: DensityMap,
    /// The foveated maps, in the order they were added.
    pub(super) maps: Vec<DensityMap>,
    /// Render target view -> the map a pass drawing into it gets.
    pub(super) targets: FastHashMap<vk::ImageView, usize>,
}

impl Foveation {
    /// The map for a pass that draws into `views` (its colour targets and
    /// resolve targets): the first registered one's, else the full map.
    pub(super) fn map_for(&self, views: impl IntoIterator<Item = vk::ImageView>) -> IdentifiedTextureView {
        for v in views {
            if let Some(&i) = self.targets.get(&v) {
                return self.maps[i].view;
            }
        }
        self.full.view
    }

    pub(super) unsafe fn destroy(&self, device: &ash::Device) {
        for map in core::iter::once(&self.full).chain(&self.maps) {
            unsafe {
                device.destroy_image_view(map.view.raw, None);
                device.destroy_image(map.image, None);
                device.free_memory(map.memory, None);
            }
        }
    }
}

fn memory_type(
    shared: &super::DeviceShared,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let props = unsafe {
        shared
            .instance
            .raw
            .get_physical_device_memory_properties(shared.physical_device)
    };
    (0..props.memory_type_count).find(|&i| {
        type_bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(flags)
    })
}

/// Create one map per pattern, write them, and leave them ready to read: one
/// staging buffer, one command buffer, one wait. The queue must not be in use
/// by anything else meanwhile.
unsafe fn create_maps(
    shared: &super::DeviceShared,
    patterns: &[DensityPattern],
) -> Result<Vec<DensityMap>, crate::DeviceError> {
    let device = &shared.raw;
    let oom = |_| crate::DeviceError::OutOfMemory;
    let mut maps = Vec::with_capacity(patterns.len());
    for p in patterns {
        assert_eq!(p.texels.len(), (p.width * p.height * 2) as usize, "a density pattern is two bytes a texel");
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8_UNORM)
            .extent(vk::Extent3D { width: p.width, height: p.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::FRAGMENT_DENSITY_MAP_EXT | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&info, None) }.map_err(oom)?;
        let req = unsafe { device.get_image_memory_requirements(image) };
        let Some(ty) = memory_type(shared, req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL) else {
            return Err(crate::DeviceError::OutOfMemory);
        };
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty),
                None,
            )
        }
        .map_err(oom)?;
        unsafe { device.bind_image_memory(image, memory, 0) }.map_err(oom)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let raw = unsafe { device.create_image_view(&view_info, None) }.map_err(oom)?;
        let identity = shared.texture_view_identity_factory.next();
        maps.push(DensityMap { image, memory, view: IdentifiedTextureView { raw, identity } });
    }
    if maps.is_empty() {
        return Ok(maps);
    }

    // Staging: every pattern back to back.
    let bytes: usize = patterns.iter().map(|p| p.texels.len()).sum();
    let buffer = unsafe {
        device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(bytes as u64)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )
    }
    .map_err(oom)?;
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let Some(ty) = memory_type(shared, req.memory_type_bits, host) else {
        return Err(crate::DeviceError::OutOfMemory);
    };
    let staging = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty),
            None,
        )
    }
    .map_err(oom)?;
    unsafe { device.bind_buffer_memory(buffer, staging, 0) }.map_err(oom)?;
    unsafe {
        let ptr = device
            .map_memory(staging, 0, bytes as u64, vk::MemoryMapFlags::empty())
            .map_err(oom)? as *mut u8;
        let mut at = 0;
        for p in patterns {
            core::ptr::copy_nonoverlapping(p.texels.as_ptr(), ptr.add(at), p.texels.len());
            at += p.texels.len();
        }
        device.unmap_memory(staging);
    }

    let pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                .queue_family_index(shared.family_index),
            None,
        )
    }
    .map_err(oom)?;
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }
    .map_err(oom)?[0];
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    unsafe {
        device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(oom)?;
        let to_copy: Vec<_> = maps
            .iter()
            .map(|m| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(m.image)
                    .subresource_range(range)
            })
            .collect();
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to_copy,
        );
        let mut at = 0u64;
        for (m, p) in maps.iter().zip(patterns) {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(at)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D { width: p.width, height: p.height, depth: 1 });
            device.cmd_copy_buffer_to_image(cmd, buffer, m.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            at += p.texels.len() as u64;
        }
        let to_read: Vec<_> = maps
            .iter()
            .map(|m| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::FRAGMENT_DENSITY_MAP_READ_EXT)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::FRAGMENT_DENSITY_MAP_OPTIMAL_EXT)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(m.image)
                    .subresource_range(range)
            })
            .collect();
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_DENSITY_PROCESS_EXT,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to_read,
        );
        device.end_command_buffer(cmd).map_err(oom)?;
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(oom)?;
        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        let submitted = device.queue_submit(shared.raw_queue, &[submit], fence);
        let waited = submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(buffer, None);
        device.free_memory(staging, None);
        waited.map_err(|_| crate::DeviceError::Lost)?;
    }
    Ok(maps)
}

impl super::Device {
    /// FIXED FOVEATED RENDERING, from here on: every render pass carries a
    /// fragment density map -- full density unless it draws into a target
    /// registered with [`Self::set_foveation_target`]. See the module docs.
    ///
    /// # Safety
    ///
    /// - The device was created with `VK_EXT_fragment_density_map` and the
    ///   features `fragmentDensityMap` and
    ///   `fragmentDensityMapNonSubsampledImages` enabled.
    /// - No pipeline has been created and no render pass recorded yet: those
    ///   made before would be incompatible with every pass after.
    /// - Nothing else uses the queue during the call.
    pub unsafe fn enable_foveation(&self) -> Result<(), crate::DeviceError> {
        let n = FULL_MAP_TEXELS;
        let texels = alloc::vec![255u8; (n * n * 2) as usize];
        let mut full = unsafe {
            create_maps(&self.shared, &[DensityPattern { width: n, height: n, texels: &texels }])?
        };
        *self.shared.foveation.write() = Some(Foveation {
            full: full.remove(0),
            maps: Vec::new(),
            targets: FastHashMap::default(),
        });
        Ok(())
    }

    /// Whether [`Self::enable_foveation`] has run.
    pub fn foveation_enabled(&self) -> bool {
        self.shared.foveation.read().is_some()
    }

    /// Add density maps, written and ready to read; returns their indices for
    /// [`Self::set_foveation_target`].
    ///
    /// # Safety
    ///
    /// Nothing else uses the queue during the call.
    pub unsafe fn add_foveation_maps(&self, patterns: &[DensityPattern]) -> Result<Vec<usize>, crate::DeviceError> {
        let maps = unsafe { create_maps(&self.shared, patterns)? };
        let mut guard = self.shared.foveation.write();
        let Some(fov) = guard.as_mut() else {
            for m in &maps {
                unsafe {
                    self.shared.raw.destroy_image_view(m.view.raw, None);
                    self.shared.raw.destroy_image(m.image, None);
                    self.shared.raw.free_memory(m.memory, None);
                }
            }
            return Err(crate::DeviceError::Unexpected);
        };
        let first = fov.maps.len();
        fov.maps.extend(maps);
        Ok((first..fov.maps.len()).collect())
    }

    /// A pass drawing into `target` -- as a colour target or a resolve target
    /// -- gets the map `map` (an index from [`Self::add_foveation_maps`]), or
    /// full density for `None`.
    pub fn set_foveation_target(&self, target: vk::ImageView, map: Option<usize>) {
        if let Some(fov) = self.shared.foveation.write().as_mut() {
            match map {
                Some(i) if i < fov.maps.len() => {
                    fov.targets.insert(target, i);
                }
                _ => {
                    fov.targets.remove(&target);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk::Handle;

    fn map(n: u64, ids: &super::super::ResourceIdentityFactory<vk::ImageView>) -> DensityMap {
        DensityMap {
            image: vk::Image::from_raw(n),
            memory: vk::DeviceMemory::from_raw(n),
            view: IdentifiedTextureView { raw: vk::ImageView::from_raw(n), identity: ids.next() },
        }
    }

    /// A pass drawing into an eye image -- as its resolve target behind a
    /// multisampled colour target, or directly -- gets that eye's map; any
    /// other pass gets full density.
    #[test]
    fn a_pass_gets_its_targets_map_and_every_other_pass_full_density() {
        let ids = super::super::ResourceIdentityFactory::new();
        let mut fov = Foveation {
            full: map(1, &ids),
            maps: alloc::vec![map(2, &ids), map(3, &ids)],
            targets: FastHashMap::default(),
        };
        let (eye_l, eye_r, msaa) = (vk::ImageView::from_raw(100), vk::ImageView::from_raw(101), vk::ImageView::from_raw(102));
        fov.targets.insert(eye_l, 0);
        fov.targets.insert(eye_r, 1);
        assert_eq!(fov.map_for([msaa, eye_l]).raw, vk::ImageView::from_raw(2), "the resolve target's map");
        assert_eq!(fov.map_for([eye_r]).raw, vk::ImageView::from_raw(3));
        assert_eq!(fov.map_for([msaa]).raw, vk::ImageView::from_raw(1), "an unregistered pass: full density");
        assert_eq!(fov.map_for([]).raw, vk::ImageView::from_raw(1));
    }
}
