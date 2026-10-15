//! Development views of the sky (`shaders/diagnostics.wgsl`), for captures and tests: the atmosphere's
//! panorama as `scripts/diagnostics/sky-atmosphere.html` draws it, radiance probes, and the environment seen
//! through the consumer's own lookup.

use wgpu::util::DeviceExt;

use crate::environment::{EnvironmentMap, environment_bind_group_layout};
use crate::gpu;
use crate::shaders;
use crate::sky::Sky;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DiagnosticsUniforms {
    origin: [f32; 2],
    tile: [f32; 2],
    centre_azimuth: f32,
    low: f32,
    high: f32,
    from_sea: f32,
    roughness: f32,
    padding: [f32; 3],
}

/// A rectangle of the target, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub struct Diagnostics {
    panorama: wgpu::RenderPipeline,
    environment_view: wgpu::RenderPipeline,
    atlas_view: wgpu::RenderPipeline,
    probe: wgpu::ComputePipeline,
    tile_layout: wgpu::BindGroupLayout,
    probe_layout: wgpu::BindGroupLayout,
}

/// The panorama's elevation span (degrees), as the removed web game's sky page drew it.
pub const PANORAMA_LOW: f32 = -6.0;
pub const PANORAMA_HIGH: f32 = 90.0;

impl Diagnostics {
    /// Views drawn into targets of `color` format (linear HDR).
    pub fn new(device: &wgpu::Device, sky: &Sky, color: wgpu::TextureFormat) -> Self {
        let module = shaders::shader_module(
            device,
            shaders::DIAGNOSTICS_WGSL,
            "naval-sky/shaders/diagnostics.wgsl",
        );
        let tile_layout = gpu::layout(device, "sky diagnostics", &[gpu::uniform_entry(0, false)]);
        let probe_layout = gpu::layout(
            device,
            "sky probes",
            &[
                gpu::uniform_entry(0, false),
                gpu::buffer_entry(1, true),
                gpu::buffer_entry(2, false),
            ],
        );
        let environment_layout = environment_bind_group_layout(device);
        let render = |label: &str, fragment: &str, layouts: &[Option<&wgpu::BindGroupLayout>]| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("fullscreen_vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(color.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sky_layout = sky.bind_group_layout();
        Self {
            panorama: render(
                "sky panorama",
                "panorama_fragment",
                &[None, Some(sky_layout), Some(&tile_layout)],
            ),
            environment_view: render(
                "sky environment view",
                "environment_fragment",
                &[None, None, Some(&tile_layout), Some(&environment_layout)],
            ),
            atlas_view: render(
                "sky atlas view",
                "atlas_fragment",
                &[None, None, Some(&tile_layout), Some(&environment_layout)],
            ),
            probe: gpu::compute_pipeline(
                device,
                "sky probes",
                &module,
                "probe_main",
                &[None, Some(sky_layout), Some(&probe_layout)],
            ),
            tile_layout,
            probe_layout,
        }
    }

    fn tile_group(
        &self,
        device: &wgpu::Device,
        tile: Tile,
        centre_azimuth: f32,
        from_sea: bool,
        roughness: f32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let uniforms = DiagnosticsUniforms {
            origin: [tile.x as f32, tile.y as f32],
            tile: [tile.width as f32, tile.height as f32],
            centre_azimuth,
            low: PANORAMA_LOW,
            high: PANORAMA_HIGH,
            from_sea: if from_sea { 1.0 } else { 0.0 },
            roughness,
            padding: [0.0; 3],
        };
        let buffer = uniform_buffer(device, bytemuck::bytes_of(&uniforms));
        let group = gpu::bind_group(
            device,
            "sky diagnostics",
            &self.tile_layout,
            &[(0, buffer.as_entire_binding())],
        );
        (buffer, group)
    }

    /// One panorama tile of the atmosphere with plain discs, already graded (display it with `graded`):
    /// azimuth across with `centre_azimuth` (radians, compass) in the middle, elevation −6° to 90° down.
    #[allow(clippy::too_many_arguments)]
    pub fn panorama(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        sky: &Sky,
        target: &wgpu::TextureView,
        tile: Tile,
        centre_azimuth: f32,
        from_sea: bool,
    ) {
        let (_buffer, group) = self.tile_group(device, tile, centre_azimuth, from_sea, 0.0);
        let mut pass = tile_pass(encoder, target, tile);
        pass.set_pipeline(&self.panorama);
        pass.set_bind_group(1, sky.bind_group(), &[]);
        pass.set_bind_group(2, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The environment at one roughness, unwrapped (azimuth across, elevation 90° to −90° down), through
    /// `env_radiance`: what a consumer reads.
    #[allow(clippy::too_many_arguments)]
    pub fn environment(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        environment: &EnvironmentMap,
        target: &wgpu::TextureView,
        tile: Tile,
        centre_azimuth: f32,
        roughness: f32,
    ) {
        let (_buffer, group) = self.tile_group(device, tile, centre_azimuth, false, roughness);
        let mut pass = tile_pass(encoder, target, tile);
        pass.set_pipeline(&self.environment_view);
        pass.set_bind_group(2, &group, &[]);
        pass.set_bind_group(3, &environment.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The atlas as it sits in the texture, stretched over the tile.
    pub fn atlas(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        environment: &EnvironmentMap,
        target: &wgpu::TextureView,
        tile: Tile,
    ) {
        let (_buffer, group) = self.tile_group(device, tile, 0.0, false, 0.0);
        let mut pass = tile_pass(encoder, target, tile);
        pass.set_pipeline(&self.atlas_view);
        pass.set_bind_group(2, &group, &[]);
        pass.set_bind_group(3, &environment.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// One reading per probe, copied into the returned buffer (MAP_READ, one vec4 per probe) once the encoder is
    /// submitted. Its w picks it: 0 the sky's radiance (`sky_radiance`, no discs) along xyz from the camera, 1 from
    /// sea level, 2 the transmittance to space along xyz from sea level, 3 from the camera, 4 and 5 the sun's and the
    /// sky's scalar irradiance (`sun_sky_scalar_irradiance`) at the point xyz of the planet frame (km), 6 the
    /// transmittance table at radius x (km) and zenith cosine y, 7 and 8 the sun's and the sky's irradiance on a level
    /// surface (`sun_sky_irradiance`, facing up) at the point xyz.
    pub fn probe(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        sky: &Sky,
        directions: &[[f32; 4]],
    ) -> wgpu::Buffer {
        let size = (directions.len() * 16) as u64;
        let input = storage_buffer(device, bytemuck::cast_slice(directions));
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky probe radiance"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky probe read-back"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let unused = uniform_buffer(device, &[0u8; std::mem::size_of::<DiagnosticsUniforms>()]);
        let group = gpu::bind_group(
            device,
            "sky probes",
            &self.probe_layout,
            &[
                (0, unused.as_entire_binding()),
                (1, input.as_entire_binding()),
                (2, output.as_entire_binding()),
            ],
        );
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky probes"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.probe);
            pass.set_bind_group(1, sky.bind_group(), &[]);
            pass.set_bind_group(2, &group, &[]);
            pass.dispatch_workgroups(gpu::workgroups(directions.len() as u32, 64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        readback
    }
}

fn tile_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &'a wgpu::TextureView,
    tile: Tile,
) -> wgpu::RenderPass<'a> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("sky diagnostics"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(
        tile.x as f32,
        tile.y as f32,
        tile.width as f32,
        tile.height as f32,
        0.0,
        1.0,
    );
    pass.set_scissor_rect(tile.x, tile.y, tile.width, tile.height);
    pass
}

fn uniform_buffer(device: &wgpu::Device, bytes: &[u8]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sky diagnostics uniforms"),
        contents: bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn storage_buffer(device: &wgpu::Device, bytes: &[u8]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sky probe directions"),
        contents: bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}
