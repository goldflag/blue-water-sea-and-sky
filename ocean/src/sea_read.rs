//! What a compute pass beside the surface needs to read the sea's height there: `naval_ocean::waves::height_at` (the
//! wave field's group, which the pass binds at group 2) and `naval_ocean::wake::wake_height` (the wakes' uniform, the wake
//! field's two displays and a sampler at group 1, bindings 5–8, and the camera block the bow waves read at group 0, a copy
//! of the frame's). The underwater probe ([`crate::underwater::UnderwaterProbe`]) and the hulls' sea heights
//! ([`crate::hull_heights::HullHeights`]) share it; each pass adds its own group 3.

use naval_render::CameraUniforms;

use crate::ship_wake::{WAKE_VEC4S, WakeBindings};

/// Groups 0 and 1 of a pass that reads the sea's height, and a calm wake to bind until the wakes are.
pub(crate) struct SeaRead {
    label: &'static str,
    camera_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    wake_layout: wgpu::BindGroupLayout,
    wake_group: Option<wgpu::BindGroup>,
    wake_sampler: wgpu::Sampler,
    calm_uniforms: wgpu::Buffer,
    calm_display: wgpu::TextureView,
}

impl SeaRead {
    /// Groups for the pass `label` names (the GPU objects' labels start with it).
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let camera_label = format!("{label} camera");
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&camera_label),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: compute,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CameraUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&camera_label),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&camera_label),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let wake_label = format!("{label} wake");
        let wake_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&wake_label),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((WAKE_VEC4S * 16) as u64),
                    },
                    count: None,
                },
                texture(6),
                texture(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let wake_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&wake_label),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let calm_label = format!("{label} calm wake");
        let calm_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&calm_label),
            size: (WAKE_VEC4S * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let calm_display = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(&calm_label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            label,
            camera_layout,
            camera_buffer,
            camera_group,
            wake_layout,
            wake_group: None,
            wake_sampler,
            calm_uniforms,
            calm_display,
        }
    }

    /// The pass's four group layouts: the camera, the wake, the wave field's (`waves`) and the pass's `own`.
    pub(crate) fn layouts(
        &self,
        waves: &wgpu::BindGroupLayout,
        own: &wgpu::BindGroupLayout,
    ) -> Vec<wgpu::BindGroupLayout> {
        vec![
            self.camera_layout.clone(),
            self.wake_layout.clone(),
            waves.clone(),
            own.clone(),
        ]
    }

    /// Reads `wake` (or a calm sea's) from now on.
    pub(crate) fn bind_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        let (uniforms, displays) = match wake {
            Some(wake) => (wake.uniforms, wake.displays),
            None => (
                &self.calm_uniforms,
                [&self.calm_display, &self.calm_display],
            ),
        };
        let label = format!("{} wake", self.label);
        self.wake_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&label),
            layout: &self.wake_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(displays[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(displays[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.wake_sampler),
                },
            ],
        }));
    }

    /// Writes this frame's `camera` and binds groups 0–2 on `pass` (`field`: the wave field's group this frame), a calm
    /// wake when none was bound.
    pub(crate) fn bind(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        pass: &mut wgpu::ComputePass<'_>,
        camera: &CameraUniforms,
        field: &wgpu::BindGroup,
    ) {
        if self.wake_group.is_none() {
            self.bind_wake(device, None);
        }
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
        pass.set_bind_group(0, &self.camera_group, &[]);
        pass.set_bind_group(1, self.wake_group.as_ref(), &[]);
        pass.set_bind_group(2, field, &[]);
    }
}
