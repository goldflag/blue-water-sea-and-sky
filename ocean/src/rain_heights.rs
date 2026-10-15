//! The sea's height about the camera for the rain's splashes (native only).
//!
//! naval-clouds draws rain's ripple rings and crowns near the camera (`splashes.wgsl`); without the drawn sea's height
//! they stood on the flat mean sea, floating over troughs and hiding in crests, and in front of a hull's boot-topping.
//! While it rains the ocean evaluates its own surface (`height_at` and the wakes' `wake_height`, as the hulls' wet band
//! does, [`crate::hull_heights`]) on a [`SPAN`]-metre square about the camera at [`TEXELS`]² texels, laid on the world
//! so it repeats: a texel holds the world point nearest the camera of all that fall on it, and the splashes read it
//! with a repeating sampler at `world xz / SPAN`. The host hands [`RainHeights::view`] to the clouds' weather
//! (`Weather::set_sea_heights`).

use bytemuck::{Pod, Zeroable};
use naval_render::{
    CameraUniforms, MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError,
    compute_recipe,
};

use crate::sea_read::SeaRead;
use crate::shaders;
use crate::ship_wake::WakeBindings;

/// The square's edge (m): the splashes reach 140 m from the camera at most, their square of 64 m set ahead of it.
pub const SPAN: f32 = 256.0;
/// Texels per edge: a metre each, the sea's waves longer than two metres.
pub const TEXELS: u32 = 256;
/// The heights' format: metres in red, filterable.
pub const RAIN_HEIGHTS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// `RainPatch` in `rain_heights.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct RainHeightsUniforms {
    centre: [f32; 4],
}

/// The pass and its texture.
pub struct RainHeights {
    sea: SeaRead,
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    id: Option<PipelineId>,
}

impl RainHeights {
    pub(crate) fn new(
        device: &wgpu::Device,
        waves: &wgpu::BindGroupLayout,
        make: MakePipelines<'_>,
    ) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let sea = SeaRead::new(device, "ocean rain heights");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean rain heights"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: RAIN_HEIGHTS_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean rain heights"),
            size: std::mem::size_of::<RainHeightsUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ocean rain heights"),
                size: wgpu::Extent3d {
                    width: TEXELS,
                    height: TEXELS,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: RAIN_HEIGHTS_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean rain heights"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let layouts = sea.layouts(waves, &layout);
        let pipeline = make.compute(|| {
            let module = shaders::shader_module(device, shaders::RAIN_HEIGHTS, &[]);
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ocean rain heights"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("ocean rain heights"),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    }),
                ),
                module: &module,
                entry_point: Some("rain_heights_main"),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        Self {
            sea,
            layout,
            buffer,
            view,
            group,
            pipeline,
            id: None,
        }
    }

    /// Builds the pass's pipeline through the renderer's registry (hot reload).
    pub(crate) fn register(
        &mut self,
        ctx: &mut RenderContext<'_>,
        waves: &wgpu::BindGroupLayout,
    ) -> Result<(), ShaderError> {
        self.id = Some(ctx.add_pipeline(
            "ocean rain heights",
            shaders::RAIN_HEIGHTS.path,
            Vec::new(),
            compute_recipe(self.sea.layouts(waves, &self.layout), "rain_heights_main"),
        )?);
        Ok(())
    }

    /// Takes the registry's current pipeline; whether it changed.
    pub(crate) fn adopt(&mut self, registry: &PipelineRegistry) -> bool {
        let Some(id) = self.id else {
            return false;
        };
        let pipeline = registry.compute(id);
        let changed = *pipeline != self.pipeline;
        if changed {
            self.pipeline = pipeline.clone();
        }
        changed
    }

    /// Reads `wake` (or a calm sea's) from now on.
    pub(crate) fn bind_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        self.sea.bind_wake(device, wake);
    }

    /// The heights the splashes read: metres in red, [`SPAN`] metres a repeat.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Evaluates the sea about `camera` (the wave field's group `field` being this frame's).
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        field: &wgpu::BindGroup,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let uniforms = RainHeightsUniforms {
            centre: [camera.position[0], camera.position[2], SPAN, TEXELS as f32],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniforms));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean rain heights"),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass("ocean rain heights")),
        });
        pass.set_pipeline(&self.pipeline);
        self.sea.bind(device, queue, &mut pass, camera, field);
        pass.set_bind_group(3, &self.group, &[]);
        pass.dispatch_workgroups(TEXELS.div_ceil(8), TEXELS.div_ceil(8), 1);
    }
}
