//! The GPU stamp painter (the web game's `WakeFoamGpu.ts`): draws stamp collectors into a coverage atlas of `tiles` × `tiles`
//! squares, `resolution` texels each, collector `i` in tile (i % tiles, ⌊i / tiles⌋), with `max` blending. Four channels
//! (`Rgba8Unorm`: churned water, slick, and where each lies across its band) for the fleet's trails, one (`R8Unorm`) for
//! the torpedo tracks.

use bytemuck::{Pod, Zeroable};
use naval_render::{
    MakePipelines, Pipeline, PipelineId, PipelineRegistry, RenderContext, ShaderError,
};

use crate::shaders;
use crate::trail::StampCollector;

/// How far past its coverage a band's stamp writes where it lies across the band: a share of its extent, and at least
/// [`TRACK_REACH`] plus a texel of its channel. The surface reads that coordinate where the point lies but the band's
/// coverage through a warp of up to [`TRACK_REACH`] (`wake.wgsl`, `trail_read`), and filtering reaches a texel further:
/// coverage read where no band was written drew straight, texel-aligned cuts along the band's edges and its head.
const TRACK_MARGIN: f64 = 0.25;
/// The farthest the surface's warp moves where it reads a trail's coverage (m): `WakeShading::lobes` (2.5) times the
/// warp's two sways (1.5), with room.
const TRACK_REACH: f64 = 4.5;

/// One quad (box, axes, shape) as `WakeFoamGpu.paint` lays it, and where the stamp lies across its band
/// ([`crate::trail::Stamp::across`] in xy, the quad's growth past the stamp across and along it in zw).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct StampQuad {
    pub box_ndc: [f32; 4],
    pub axes: [f32; 4],
    pub shape: [f32; 4],
    pub across: [f32; 4],
}

/// One collector's quads for its tile (`WakeFoamGpu.paint`): each stamp's texel-aligned bounding quad in the atlas's
/// NDC, its half extent in the stamp's across/along axes and the quad centre in them.
pub fn paint(
    collector: &StampCollector,
    slot: usize,
    resolution: u32,
    tiles: u32,
    out: &mut Vec<StampQuad>,
) {
    let res = f64::from(resolution);
    let size = res * f64::from(tiles);
    let ox = (slot as u32 % tiles) as f64 * res;
    let oy = (slot as u32 / tiles) as f64 * res;
    for stamp in &collector.stamps {
        let (center, extent) = collector.frame(stamp.channel);
        let scale = res / extent;
        // A band's stamps reach past their coverage with where they lie across it, so the coordinate holds wherever
        // the surface can read the band's coverage, instead of filtering toward nothing at its edge.
        let margin = |extent: f64| (extent * TRACK_MARGIN).max(TRACK_REACH + 1.0 / scale);
        let (grow_across, grow_along) = if stamp.across[1] > 0.0 {
            (
                1.0 + margin(stamp.width) / stamp.width,
                1.0 + margin(stamp.length) / stamp.length,
            )
        } else {
            (1.0, 1.0)
        };
        let (width, length) = (stamp.width * grow_across, stamp.length * grow_along);
        let cx = (stamp.x - center[0]) * scale + res / 2.0 - 0.5;
        let cz = (stamp.z - center[1]) * scale + res / 2.0 - 0.5;
        let rx = (stamp.right_x.abs() * width + stamp.right_z.abs() * length) * scale;
        let rz = (stamp.right_z.abs() * width + stamp.right_x.abs() * length) * scale;
        let min_x = (cx - rx).floor().max(0.0);
        let max_x = (cx + rx).ceil().min(res - 1.0);
        let min_z = (cz - rz).floor().max(0.0);
        let max_z = (cz + rz).ceil().min(res - 1.0);
        if min_x > max_x || min_z > max_z {
            continue;
        }
        let center_x = (min_x + max_x) / 2.0;
        let center_z = (min_z + max_z) / 2.0;
        let dx = (center_x - cx) / scale;
        let dz = (center_z - cz) / scale;
        let half_x = (max_x - min_x + 1.0) / 2.0;
        let half_z = (max_z - min_z + 1.0) / 2.0;
        let (right_x, right_z) = (stamp.right_x, stamp.right_z);
        let kind = f64::from(u8::from(stamp.ring)) + 2.0 * f64::from(stamp.channel);
        out.push(StampQuad {
            box_ndc: [
                ((ox + center_x + 0.5) / size * 2.0 - 1.0) as f32,
                ((oy + center_z + 0.5) / size * 2.0 - 1.0) as f32,
                (half_x / size * 2.0) as f32,
                (half_z / size * 2.0) as f32,
            ],
            axes: [
                (half_x / scale * right_x / width) as f32,
                (half_z / scale * right_z / width) as f32,
                (-half_x / scale * right_z / length) as f32,
                (half_z / scale * right_x / length) as f32,
            ],
            shape: [
                ((dx * right_x + dz * right_z) / width) as f32,
                ((-dx * right_z + dz * right_x) / length) as f32,
                stamp.strength as f32,
                kind as f32,
            ],
            across: [
                stamp.across[0] as f32,
                stamp.across[1] as f32,
                grow_across as f32,
                grow_along as f32,
            ],
        });
    }
}

/// The atlas and the pipeline that paints it.
pub struct StampPainter {
    pub resolution: u32,
    pub tiles: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    /// The atlas's format, which the pipeline writes.
    format: wgpu::TextureFormat,
    instances: wgpu::Buffer,
    capacity: u64,
    quads: Vec<StampQuad>,
    count: u32,
    /// Repaints so far and stamps in the last, for diagnostics.
    pub draws: u64,
}

/// The painter's pipeline: instanced stamp quads, blended by maximum into an atlas of `format`.
fn stamp_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("wake stamps"),
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wake stamps"),
            bind_group_layouts: &[],
            immediate_size: 0,
        })),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<StampQuad>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Max,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Max,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache,
    })
}

impl StampPainter {
    /// An atlas of `tiles`² tiles of `resolution`² texels: the fleet's trails' four channels, or one.
    pub fn new(device: &wgpu::Device, resolution: u32, tiles: u32, trails: bool) -> Self {
        Self::build(device, resolution, tiles, trails, MakePipelines::Now)
    }

    /// Builds the painter's pipeline through the renderer's registry, as [`StampPainter::new`] builds it. The
    /// ocean's files must be registered first (`crate::shaders::register`).
    pub fn register_pipeline(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<PipelineId, ShaderError> {
        let format = self.format;
        ctx.add_pipeline(
            "wake stamps",
            shaders::STAMPS.path,
            Vec::new(),
            Box::new(move |build| {
                Pipeline::Render(stamp_pipeline(
                    build.device,
                    build.module,
                    format,
                    build.cache,
                ))
            }),
        )
    }

    /// Takes the registry's current pipeline in place of the painter's own; whether it changed.
    pub fn adopt_pipeline(&mut self, registry: &PipelineRegistry, id: PipelineId) -> bool {
        let new = registry.render(id);
        let changed = self.pipeline != *new;
        if changed {
            self.pipeline = new.clone();
        }
        changed
    }

    pub(crate) fn build(
        device: &wgpu::Device,
        resolution: u32,
        tiles: u32,
        trails: bool,
        make: MakePipelines<'_>,
    ) -> Self {
        let format = if trails {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::R8Unorm
        };
        let size = resolution * tiles;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if trails {
                "fleet wake foam"
            } else {
                "torpedo tracks"
            }),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let pipeline = make.render(|| {
            let module = shaders::shader_module(device, shaders::STAMPS, &[]);
            stamp_pipeline(device, &module, format, None)
        });
        let capacity = 256;
        let instances = Self::buffer(device, capacity);
        Self {
            resolution,
            tiles,
            texture,
            view,
            pipeline,
            format,
            instances,
            capacity,
            quads: Vec::new(),
            count: 0,
            draws: 0,
        }
    }

    fn buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wake stamps"),
            size: capacity * std::mem::size_of::<StampQuad>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Stamps in the last repaint.
    pub fn stamps(&self) -> u32 {
        self.count
    }

    /// Clear the atlas and paint every collector into its tile, on `encoder` (the stamps written through `queue`).
    pub fn repaint(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        collectors: &[&StampCollector],
    ) {
        self.quads.clear();
        for (slot, collector) in collectors.iter().enumerate() {
            paint(
                collector,
                slot,
                self.resolution,
                self.tiles,
                &mut self.quads,
            );
        }
        let count = self.quads.len() as u64;
        if count > self.capacity {
            self.capacity = count.max(self.capacity * 2);
            self.instances = Self::buffer(device, self.capacity);
        }
        if count > 0 {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.quads));
        }
        self.count = count as u32;
        self.draws += 1;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("wake stamps"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if count > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(
                0,
                self.instances
                    .slice(..count * std::mem::size_of::<StampQuad>() as u64),
            );
            pass.draw(0..6, 0..count as u32);
        }
    }
}
