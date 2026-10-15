//! Where the opaque scene's depth copy holds any surface, for the screen-space reflections (`shaders/reflection_tiles.wgsl`).
//!
//! Every water pixel on the tiers that trace traced its mirrored ray through the depth copy step by step, and most found
//! only sky: the open sea mirrors a ship only where one stands in the ray's way on screen. After the opaque copies,
//! `occupancy` marks each [`TILE`]² pixel tile of the copy that holds a surface (any depth above the clear), and `sum` adds
//! the marks into a summed-area table, (tiles + 1)² entries with a zero first row and column after a header of its width
//! and height, in a storage buffer (the surface's fragment stage already binds the 16 sampled textures WebGPU allows). The surface reads four
//! texels of it for the screen box of a ray's whole path (`surface_between` in `shaders/screen_reflection.wgsl`): a box
//! holding no surface means every step would read sky and the trace would find nothing, so it returns at once with what
//! the trace would have returned. The picture is the same; only the rays with a surface in their box are traced.
//!
//! Only surfaces a traced ray can reach are marked ([`reach`]): nearer than where the drawn sea ends (a crest's distance
//! past its horizon, `surface.wgsl`) and the longest ray beyond, and never past [`REACH_M`]. A theatre view drawing the
//! land beyond its window (`naval_render::camera::HORIZON_FAR`) otherwise marched every ray whose box crossed that land's
//! band along the horizon, almost always for nothing: the water's pass 0.4-0.9 ms dearer from a ship's bridge in Sagami
//! Bay or off Alboran. Within the edge the picture is the same, since a ray starts on water and none is drawn past it;
//! from high up, land past 60 km is not mirrored, as a battle view drew nothing there.
//!
//! Native only: the web traced every ray.

use naval_render::{
    MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError, compute_recipe,
};

use crate::shaders;

/// Pixels per tile edge (`REFLECTION_TILE` in both shaders).
pub const TILE: u32 = 16;
/// The farthest surface the tiles ever mark (m along the view): a battle view's far plane, so a view drawing farther
/// (from high up, or flat) marks no more than a battle view would.
pub const REACH_M: f32 = naval_render::camera::DEFAULT_FAR;
/// A crest this high (m) past the sea's horizon still shows over it: `surface.wgsl`'s `CREST_BEYOND`, where the drawn
/// sea ends.
pub const CREST_BEYOND_M: f32 = 20.0;

/// The farthest surface a traced ray can meet (m) for an eye `height` m over the sea, the world drawn `curvature`
/// (`naval_render::Camera::curvature`) and rays at most `ray` m long: the drawn sea's edge (the horizon and a crest's
/// distance past it, `surface.wgsl`'s close-up), the eye's height over it with the drop and a crest, and a ray beyond;
/// never past [`REACH_M`]. Flat, the sea has no edge short of [`REACH_M`].
pub fn reach(height: f32, curvature: f32, ray: f32) -> f32 {
    if curvature <= 0.0 {
        return REACH_M;
    }
    let height = height.max(0.0);
    let edge = (height / curvature).sqrt() + (CREST_BEYOND_M / curvature).sqrt();
    let below = height + curvature * edge * edge + CREST_BEYOND_M;
    (edge.hypot(below) + ray.max(0.0)).min(REACH_M)
}

/// The reversed depth (finite far plane, as `naval_render::Camera`) of a surface `distance` along the view: surfaces
/// nearer have more. 0 (every surface) when the far plane is no farther.
pub fn reach_depth(near: f32, far: f32, distance: f32) -> f32 {
    if far <= distance || far <= near {
        return 0.0;
    }
    near * (far - distance) / (distance * (far - near))
}
const LABEL: &str = "ocean reflection tiles";

/// Tiles (across, down) over a depth copy `width` × `height` pixels.
pub fn tile_counts(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(TILE), height.div_ceil(TILE))
}

/// The table for one depth copy.
struct Table {
    /// The depth copy's version it was built for.
    version: u64,
    group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    tiles: (u32, u32),
}

/// The tiles pass and its table (see the module docs).
pub struct ReflectionTiles {
    layout: wgpu::BindGroupLayout,
    /// The reversed depth below which a surface is past [`REACH_M`] (x; the rest unused).
    reach: wgpu::Buffer,
    occupancy: wgpu::ComputePipeline,
    sum: wgpu::ComputePipeline,
    ids: Option<(PipelineId, PipelineId)>,
    table: Option<Table>,
}

impl ReflectionTiles {
    pub fn new(device: &wgpu::Device, make: MakePipelines<'_>) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // Zeroed: every surface marked until the first frame sets the reach.
        let reach = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = |entry: &str| {
            make.compute(|| {
                let module = shaders::shader_module(device, shaders::REFLECTION_TILES, &[]);
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(LABEL),
                    layout: Some(
                        &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: Some(LABEL),
                            bind_group_layouts: &[Some(&layout)],
                            immediate_size: 0,
                        }),
                    ),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
        };
        let occupancy = pipeline("occupancy");
        let sum = pipeline("sum");
        Self {
            layout,
            reach,
            occupancy,
            sum,
            ids: None,
            table: None,
        }
    }

    /// Builds both pipelines through the renderer's registry (hot reload).
    pub(crate) fn register(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), ShaderError> {
        let mut add = |label: &str, entry: &'static str| {
            ctx.add_pipeline(
                label,
                shaders::REFLECTION_TILES.path,
                Vec::new(),
                compute_recipe(vec![self.layout.clone()], entry),
            )
        };
        self.ids = Some((
            add("ocean reflection occupancy", "occupancy")?,
            add("ocean reflection table", "sum")?,
        ));
        Ok(())
    }

    /// Takes the registry's current pipelines; whether either changed.
    pub(crate) fn adopt(&mut self, registry: &PipelineRegistry) -> bool {
        let Some((occupancy, sum)) = self.ids else {
            return false;
        };
        let mut changed = false;
        for (slot, id) in [(&mut self.occupancy, occupancy), (&mut self.sum, sum)] {
            let pipeline = registry.compute(id);
            if slot != pipeline {
                *slot = pipeline.clone();
                changed = true;
            }
        }
        changed
    }

    /// Builds the table for the depth copy `depth` (`width` × `height`, remade whenever `version` moves); whether the
    /// table changed (the surface binds it: [`ReflectionTiles::table`]).
    pub fn follow(
        &mut self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        version: u64,
        (width, height): (u32, u32),
    ) -> bool {
        if self
            .table
            .as_ref()
            .is_some_and(|table| table.version == version)
        {
            return false;
        }
        let tiles = tile_counts(width, height);
        let marks = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: u64::from(tiles.0 * tiles.1).max(1) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: table_bytes(tiles),
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: marks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.reach.as_entire_binding(),
                },
            ],
        });
        self.table = Some(Table {
            version,
            group,
            buffer,
            tiles,
        });
        true
    }

    /// The table the surface reads (after [`ReflectionTiles::follow`]).
    pub fn table(&self) -> Option<&wgpu::Buffer> {
        self.table.as_ref().map(|table| &table.buffer)
    }

    /// Sets this frame's reach from the camera the copies were drawn with and the longest ray (m) the surface traces
    /// ([`reach`]; before [`ReflectionTiles::encode`]).
    pub fn set_reach(
        &self,
        uploads: &mut naval_render::Uploads,
        camera: &naval_render::CameraUniforms,
        ray: f32,
    ) {
        let distance = reach(camera.position[1], camera.curvature, ray);
        let depth = [
            reach_depth(camera.near, camera.far, distance),
            0.0,
            0.0,
            0.0,
        ];
        uploads.write_buffer(&self.reach, 0, bytemuck::cast_slice(&depth));
    }

    /// Marks the tiles of this frame's depth copy and sums them (after the copies are taken, before the water draws).
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let Some(table) = &self.table else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(LABEL),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass(LABEL)),
        });
        pass.set_bind_group(0, &table.group, &[]);
        pass.set_pipeline(&self.occupancy);
        pass.dispatch_workgroups(table.tiles.0, table.tiles.1, 1);
        pass.set_pipeline(&self.sum);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// Bytes of the table for `tiles` (across, down): the header and (tiles + 1)² entries.
pub fn table_bytes((across, down): (u32, u32)) -> u64 {
    (2 + u64::from(across + 1) * u64::from(down + 1)) * 4
}

/// The table the surface binds while no tiles are built: a zero header (new buffers are zeroed), whose size matches no
/// depth copy's, so the surface traces every ray, as it did before the tiles.
pub(crate) fn placeholder_table(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ocean reflection tiles placeholder"),
        size: table_bytes((0, 0)),
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reach_ends_where_a_ray_from_the_drawn_sea_can() {
        let curvature = (0.5 / naval_render::camera::EARTH_RADIUS) as f32;
        // A bridge 22 m up: the horizon 16.7 km off, a 20 m crest seen 16 km past it, a 400 m ray beyond.
        let bridge = reach(22.0, curvature, 400.0);
        assert!((bridge - 33_100.0).abs() < 300.0, "{bridge}");
        // From 3 km up, or under binoculars' long rays, no farther than a battle view's far plane; flat, that.
        assert_eq!(reach(3_000.0, curvature, 400.0), REACH_M);
        assert_eq!(reach(22.0, curvature, 40_000.0), REACH_M);
        assert_eq!(reach(22.0, 0.0, 400.0), REACH_M);
    }

    /// `CREST_BEYOND_M` is the surface's.
    #[test]
    fn the_crest_is_the_surfaces() {
        let line = format!("const CREST_BEYOND: f32 = {CREST_BEYOND_M:.1};");
        assert!(
            include_str!("../shaders/surface.wgsl").contains(&line),
            "surface.wgsl lacks `{line}`"
        );
    }

    #[test]
    fn the_reach_is_a_battle_views_far_plane() {
        // A battle view's far plane is the reach: every surface is marked, as before.
        assert_eq!(reach_depth(0.5, REACH_M, REACH_M), 0.0);
        assert_eq!(reach_depth(0.5, 1_000.0, REACH_M), 0.0);
        // A view drawing the land beyond its window: the depth a surface at the reach is drawn at.
        let (near, far) = (0.5, naval_render::camera::HORIZON_FAR);
        let depth = reach_depth(near, far, REACH_M);
        let distance = near * far / (depth * (far - near) + near);
        assert!((distance - REACH_M).abs() < 1.0, "{distance}");
    }

    #[test]
    fn tiles_cover_the_copy() {
        assert_eq!(tile_counts(2560, 1440), (160, 90));
        assert_eq!(tile_counts(1921, 1081), (121, 68));
        assert_eq!(tile_counts(1, 1), (1, 1));
    }

    /// The shaders and this module agree on the tile.
    #[test]
    fn the_shaders_use_this_tile() {
        let line = format!("const REFLECTION_TILE: u32 = {TILE}u;");
        for (name, source) in [
            ("reflection_tiles.wgsl", shaders::REFLECTION_TILES.source),
            (
                "screen_reflection.wgsl",
                include_str!("../shaders/screen_reflection.wgsl"),
            ),
        ] {
            assert!(source.contains(&line), "{name} lacks `{line}`");
        }
    }
}
