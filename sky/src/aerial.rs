//! The aerial-perspective volume in the sky's own group: a buffer copy of the renderer's (`naval_render::aerial`, which
//! every material samples at group 0) for the sky's passes, bound at group 1 beside the tables (binding 12, read through
//! `naval_sky::aerial`). The dome takes the haze along each pixel's ray from the dome slice; the clouds' march takes the
//! air in front of the clouds. A buffer rather than textures: the passes that bind the sky's group (the march, the
//! environment bake) already read as many textures as a stage may. The pass that fills the renderer's volume
//! (naval-clouds' aerial perspective) copies it here after it ([`AerialCopy::copy_from`]).

use naval_render::{AERIAL_SKY_BYTES, AERIAL_VOLUME_BYTES, AerialVolume};

/// The sky's copy of the volume and its dome slice: clear air until the first copy.
pub struct AerialCopy {
    pub buffer: wgpu::Buffer,
}

impl AerialCopy {
    pub fn new(device: &wgpu::Device) -> Self {
        // wgpu zeroes a new buffer: clear air.
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky aerial"),
            size: AERIAL_VOLUME_BYTES + AERIAL_SKY_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer }
    }

    /// Copies the renderer's volume and dome slice here (after the pass that filled them).
    pub fn copy_from(&self, encoder: &mut wgpu::CommandEncoder, volume: &AerialVolume) {
        volume.copy_to_buffer(encoder, &self.buffer, 0);
    }

    /// Group 1's entry for the copy: read-only storage, for the fragment and compute stages.
    pub fn layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }
}
