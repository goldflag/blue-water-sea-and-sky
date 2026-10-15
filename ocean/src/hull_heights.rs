//! The sea's height about each hull, for the hulls' wet band.
//!
//! The wet band (ported from the removed web game's `HullWetBand`) darkens and glosses every hull just above the moving
//! sea, reading per hull fragment the sea's height there: the wave field's with the sea around hulls, plus the wakes'
//! (the wake field and the bow waves). The ship pipeline cannot read the field itself: like the
//! surface it uses the four bind groups WebGPU guarantees, and the field's arrays ping-pong from frame to frame. So the
//! ocean evaluates those heights here, once a frame, on a patch about each hull it couples ([`Ocean::set_hull_sea`]'s
//! footprints): a compute pass in its prepare (`shaders/hull_heights.wgsl`: `height_at` + `wake_height`, the same
//! functions the surface and the underwater probe read) writes one layer of an `R32Float` array per patch, and the ships
//! read the patch under each fragment bilinearly (naval-ships' `ship.wgsl`, `sea_height`). Where no patch reaches (more
//! hulls than slots, a mast's overhang) the ships fall back to the flat sea.
//!
//! A patch spans the hull's waterplane and a margin for the stem's rake and the flare, [`PATCH_ALONG`] texels from stern
//! to stem and [`PATCH_ACROSS`] from port to starboard: about a metre along a battleship and half a metre along a
//! destroyer, finer across, enough for the stem crest (at least 1.5 m wide) and every wave longer than two texels. The
//! shorter waves (centimetres high) are what the bilinear read smooths away.
//!
//! [`Ocean::set_hull_sea`]: crate::Ocean::set_hull_sea

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use naval_render::{
    CameraUniforms, MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError,
    compute_recipe,
};

use crate::hull_sea::{HULL_SEA_SLOTS, HullFootprint};
use crate::sea_read::SeaRead;
use crate::shaders;
use crate::ship_wake::WakeBindings;

/// Patches at once: one per coupled hull.
pub const HULL_HEIGHT_SLOTS: usize = HULL_SEA_SLOTS;
/// Texels along a patch, stern to stem.
pub const PATCH_ALONG: u32 = 256;
/// Texels across a patch, port to starboard.
pub const PATCH_ACROSS: u32 = 64;
/// The heights' format: metres, unfilterable (the ships read four texels and blend them).
pub const HULL_HEIGHTS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// A patch reaches this share of the waterplane's length beyond each end (the stem's rake above the water), and at least
/// [`MARGIN_ALONG_MIN`] metres.
pub const MARGIN_ALONG: f64 = 0.04;
pub const MARGIN_ALONG_MIN: f64 = 6.0;
/// And this share of the beam beyond each side (the flare), at least [`MARGIN_ACROSS_MIN`] metres.
pub const MARGIN_ACROSS: f64 = 0.1;
pub const MARGIN_ACROSS_MIN: f64 = 3.0;
/// Metres above and below the mean sea a patch's bounds reach for the view test: the band reads the sea from
/// −1.3 Hs − 2 to 1.3 Hs + the bow crest + 1 + the band.
const REACH_UP: f64 = 20.0;
/// Pixels between texels a distant patch is thinned to (never coarser than its fewest texels).
const TEXEL_PIXELS: f64 = 2.0;
/// The fewest texels along and across a patch.
const FEWEST: (u32, u32) = (16, 8);

/// One patch: its centre (world x, z), bow axis (unit x, z) and half extents along and across (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HullPatch {
    pub x: f64,
    pub z: f64,
    pub axis_x: f64,
    pub axis_z: f64,
    pub half_length: f64,
    pub half_breadth: f64,
}

impl HullPatch {
    /// The patch about a coupled hull's waterplane, with the margins.
    pub fn about(hull: &HullFootprint) -> Self {
        let length = hull.length.max(0.0);
        let beam = hull.beam.max(0.0);
        Self {
            x: hull.x,
            z: hull.z,
            axis_x: hull.bearing.cos(),
            axis_z: hull.bearing.sin(),
            half_length: length / 2.0 + (MARGIN_ALONG * length).max(MARGIN_ALONG_MIN),
            half_breadth: beam / 2.0 + (MARGIN_ACROSS * beam).max(MARGIN_ACROSS_MIN),
        }
    }

    /// The world (x, z) of texel (`i`, `j`) of a patch laid out at full resolution, as the shader places it.
    pub fn texel(&self, i: u32, j: u32) -> (f64, f64) {
        self.texel_of(i, j, (PATCH_ALONG, PATCH_ACROSS))
    }

    /// The world (x, z) of texel (`i`, `j`) of a patch `texels` (along, across) in size.
    pub fn texel_of(&self, i: u32, j: u32, texels: (u32, u32)) -> (f64, f64) {
        let along = (f64::from(i) / f64::from(texels.0 - 1) * 2.0 - 1.0) * self.half_length;
        let across = (f64::from(j) / f64::from(texels.1 - 1) * 2.0 - 1.0) * self.half_breadth;
        (
            self.x + self.axis_x * along - self.axis_z * across,
            self.z + self.axis_z * along + self.axis_x * across,
        )
    }

    /// Radius (m) of the sphere about the patch's centre at the mean sea that holds every point of a hull it serves.
    fn radius(&self) -> f64 {
        (self.half_length.powi(2) + self.half_breadth.powi(2) + REACH_UP.powi(2)).sqrt()
    }

    /// Whether any of the patch's bounds reach `camera`'s view (its side and near planes; a camera without a projection
    /// sees everything).
    pub fn seen_by(&self, camera: &CameraUniforms) -> bool {
        let m = Mat4::from_cols_array(&camera.view_projection);
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        // Where the camera draws the patch: lowered by the Earth's curve.
        let centre = naval_render::camera::curved(
            glam::Vec3::from(camera.position),
            camera.curvature,
            glam::Vec3::new(self.x as f32, 0.0, self.z as f32),
        )
        .extend(1.0);
        let radius = self.radius() as f32;
        // Reversed depth: in front of the near plane is z ≤ w.
        [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2]
            .iter()
            .all(|plane| {
                let normal = plane.truncate().length();
                normal == 0.0 || plane.dot(centre) / normal >= -radius
            })
    }

    /// Texels (along, across) the patch needs seen by `camera`: one every [`TEXEL_PIXELS`] pixels of its nearest point,
    /// between the fewest and the full [`PATCH_ALONG`] × [`PATCH_ACROSS`] (full without a perspective lens).
    pub fn texels_for(&self, camera: &CameraUniforms) -> (u32, u32) {
        let pixel = 2.0 * f64::from(camera.tan_half_fov_y) / f64::from(camera.viewport[1]);
        if !(pixel.is_finite() && pixel > 0.0) {
            return (PATCH_ALONG, PATCH_ACROSS);
        }
        let eye = camera.position.map(f64::from);
        let range =
            ((self.x - eye[0]).hypot(self.z - eye[2]).hypot(eye[1]) - self.radius()).max(1.0);
        let spacing = TEXEL_PIXELS * range * pixel;
        let count = |extent: f64, fewest: u32, most: u32| {
            ((2.0 * extent / spacing).ceil() + 1.0).clamp(f64::from(fewest), f64::from(most)) as u32
        };
        (
            count(self.half_length, FEWEST.0, PATCH_ALONG),
            count(self.half_breadth, FEWEST.1, PATCH_ACROSS),
        )
    }
}

/// `HullPatches` in `shaders/hull_heights.wgsl` and `SeaPatches` in naval-ships' `ship.wgsl`: per patch (centre x, z,
/// bow axis x, z) and (half length, half breadth, texels along, texels across), then (patches in use, ·, ·, ·). A
/// patch's texels sit in the corner of its layer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HullHeightsUniforms {
    pub patches: [[f32; 4]; HULL_HEIGHT_SLOTS * 2],
    pub count: [f32; 4],
}

impl Default for HullHeightsUniforms {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl HullHeightsUniforms {
    /// A patch about each of `hulls` (at most [`HULL_HEIGHT_SLOTS`]), every one at full resolution.
    pub fn set(&mut self, hulls: &[HullFootprint]) {
        self.count[0] = 0.0;
        for hull in hulls.iter().take(HULL_HEIGHT_SLOTS) {
            self.push(&HullPatch::about(hull), (PATCH_ALONG, PATCH_ACROSS));
        }
    }

    /// The `patches` `camera` sees (at most [`HULL_HEIGHT_SLOTS`]), each at the texels its distance needs: a hull out of
    /// view draws nothing, and a distant one covers few pixels.
    pub fn seen(&mut self, patches: &[HullPatch], camera: &CameraUniforms) {
        self.count[0] = 0.0;
        for patch in patches.iter().filter(|p| p.seen_by(camera)) {
            if self.count() == HULL_HEIGHT_SLOTS {
                break;
            }
            self.push(patch, patch.texels_for(camera));
        }
    }

    fn push(&mut self, patch: &HullPatch, texels: (u32, u32)) {
        let i = self.count();
        self.patches[i * 2] = [patch.x, patch.z, patch.axis_x, patch.axis_z].map(|v| v as f32);
        self.patches[i * 2 + 1] = [
            patch.half_length as f32,
            patch.half_breadth as f32,
            texels.0 as f32,
            texels.1 as f32,
        ];
        self.count[0] = (i + 1) as f32;
    }

    /// The largest texels (along, across) of any patch in use.
    pub fn largest(&self) -> (u32, u32) {
        (0..self.count()).fold((0, 0), |(a, b), i| {
            let texels = self.patches[i * 2 + 1];
            (a.max(texels[2] as u32), b.max(texels[3] as u32))
        })
    }

    /// Patches in use.
    pub fn count(&self) -> usize {
        self.count[0] as usize
    }
}

/// What the ships bind of the heights (their frame group's bindings 21 and 22).
pub struct HullHeightsBindings<'a> {
    /// The heights, an `R32Float` 2D array of [`HULL_HEIGHT_SLOTS`] layers of [`PATCH_ALONG`] × [`PATCH_ACROSS`].
    pub heights: &'a wgpu::TextureView,
    /// The patches, [`HullHeightsUniforms`].
    pub patches: &'a wgpu::Buffer,
}

/// The heights about the hulls (see the module docs). Owned by the ocean once enabled
/// ([`crate::Ocean::enable_hull_heights_in`]), which places the patches with the sea around hulls and runs the pass in
/// its prepare.
pub struct HullHeights {
    /// Groups 0–2: the camera, the wake and the wave field `height_at` and `wake_height` read.
    sea: SeaRead,
    layout: wgpu::BindGroupLayout,
    /// The patches about the coupled hulls, as last placed.
    placed: Vec<HullPatch>,
    /// The patches the last pass evaluated (those in view, at the texels their distance needs), which the ships read.
    uniforms: HullHeightsUniforms,
    buffer: wgpu::Buffer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    id: Option<PipelineId>,
}

impl HullHeights {
    /// The heights for a wave field whose group layout is `waves`.
    pub fn new(
        device: &wgpu::Device,
        waves: &wgpu::BindGroupLayout,
        make: MakePipelines<'_>,
    ) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let sea = SeaRead::new(device, "ocean hull heights");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean hull heights"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<
                            HullHeightsUniforms,
                        >() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: HULL_HEIGHTS_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
            ],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean hull heights"),
            size: std::mem::size_of::<HullHeightsUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean hull heights"),
            size: wgpu::Extent3d {
                width: PATCH_ALONG,
                height: PATCH_ACROSS,
                depth_or_array_layers: HULL_HEIGHT_SLOTS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HULL_HEIGHTS_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ocean hull heights"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean hull heights"),
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
            let module = shaders::shader_module(device, shaders::HULL_HEIGHTS, &[]);
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ocean hull heights"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("ocean hull heights"),
                        bind_group_layouts: &groups,
                        immediate_size: 0,
                    }),
                ),
                module: &module,
                entry_point: Some("heights_main"),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        Self {
            sea,
            layout,
            placed: Vec::new(),
            uniforms: HullHeightsUniforms::default(),
            buffer,
            texture,
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
            "ocean hull heights",
            shaders::HULL_HEIGHTS.path,
            Vec::new(),
            compute_recipe(self.sea.layouts(waves, &self.layout), "heights_main"),
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
    pub fn bind_wake(&mut self, device: &wgpu::Device, wake: Option<&WakeBindings<'_>>) {
        self.sea.bind_wake(device, wake);
    }

    /// A patch about each of `hulls` from the next pass on (the footprints the sea around hulls couples, at most
    /// [`HULL_HEIGHT_SLOTS`]).
    pub fn place(&mut self, hulls: &[HullFootprint]) {
        self.placed.clear();
        self.placed
            .extend(hulls.iter().take(HULL_HEIGHT_SLOTS).map(HullPatch::about));
    }

    /// The patches as last placed.
    pub fn placed(&self) -> &[HullPatch] {
        &self.placed
    }

    /// The patches the last pass evaluated, as the ships read them.
    pub fn uniforms(&self) -> &HullHeightsUniforms {
        &self.uniforms
    }

    /// The heights' texture (one layer per patch), for read-backs.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// What the ships bind.
    pub fn bindings(&self) -> HullHeightsBindings<'_> {
        HullHeightsBindings {
            heights: &self.view,
            patches: &self.buffer,
        }
    }

    /// Writes the patches `camera` sees and evaluates the sea on them, the wave field's group `field` being this frame's
    /// (after its update and the wakes' encode). Whether it ran (no patch in view: nothing to do, and the ships find
    /// none).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        field: &wgpu::BindGroup,
        timer: Option<&naval_render::GpuTimer>,
    ) -> bool {
        self.uniforms.seen(&self.placed, camera);
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniforms));
        let count = self.uniforms.count() as u32;
        if count == 0 {
            return false;
        }
        let (along, across) = self.uniforms.largest();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean hull heights"),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass("ocean hull heights")),
        });
        pass.set_pipeline(&self.pipeline);
        self.sea.bind(device, queue, &mut pass, camera, field);
        pass.set_bind_group(3, &self.group, &[]);
        pass.dispatch_workgroups(along.div_ceil(8), across.div_ceil(8), count);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESTROYER: HullFootprint = HullFootprint {
        x: -310.0,
        z: 75.0,
        bearing: 2.2,
        length: 112.0,
        beam: 12.0,
        contact: 1.0,
    };

    #[test]
    fn a_patch_spans_the_waterplane_and_its_margins() {
        let patch = HullPatch::about(&DESTROYER);
        assert!((patch.half_length - (56.0 + 6.0)).abs() < 1e-12);
        assert!((patch.half_breadth - (6.0 + 3.0)).abs() < 1e-12);
        let battleship = HullPatch::about(&HullFootprint {
            length: 250.0,
            beam: 36.0,
            ..DESTROYER
        });
        assert!((battleship.half_length - (125.0 + 10.0)).abs() < 1e-12);
        assert!((battleship.half_breadth - (18.0 + 3.6)).abs() < 1e-12);
    }

    #[test]
    fn texels_run_stern_to_stem_and_port_to_starboard() {
        let patch = HullPatch::about(&DESTROYER);
        let (sx, sz) = patch.texel(0, (PATCH_ACROSS - 1) / 2);
        let (bx, bz) = patch.texel(PATCH_ALONG - 1, (PATCH_ACROSS - 1) / 2);
        // Stem minus stern lies along the bow axis, the patch's length long (the middle row is half a texel off centre).
        let (dx, dz) = (bx - sx, bz - sz);
        assert!((dx.hypot(dz) - 2.0 * patch.half_length).abs() < 1e-9);
        assert!((dx * patch.axis_x + dz * patch.axis_z - 2.0 * patch.half_length).abs() < 1e-9);
        // Starboard: the bow axis turned a quarter clockwise seen from above (heading north, −z, starboard is +x).
        let north = HullPatch::about(&HullFootprint {
            bearing: -std::f64::consts::FRAC_PI_2,
            ..DESTROYER
        });
        let (px, _) = north.texel(0, 0);
        let (qx, _) = north.texel(0, PATCH_ACROSS - 1);
        assert!(qx - px > 2.0 * north.half_breadth - 1e-9);
    }

    #[test]
    fn only_patches_in_view_are_evaluated_and_distant_ones_thinly() {
        use naval_render::{Camera, FrameTime};
        let camera = Camera::look_at(
            glam::Vec3::new(0.0, 20.0, 0.0),
            glam::Vec3::new(0.0, 0.0, -500.0),
        )
        .uniforms(1920, 1080, FrameTime::fixed(0, 1.0 / 60.0));
        let at = |x: f64, z: f64| HullPatch::about(&HullFootprint { x, z, ..DESTROYER });
        let near = at(0.0, -150.0);
        let far = at(0.0, -6000.0);
        let behind = at(0.0, 900.0);
        let beside = at(2000.0, -100.0);
        assert!(near.seen_by(&camera) && far.seen_by(&camera));
        assert!(!behind.seen_by(&camera) && !beside.seen_by(&camera));
        // Right behind the lens, the patch's own bounds still reach the view.
        assert!(at(0.0, 30.0).seen_by(&camera));
        assert_eq!(near.texels_for(&camera), (PATCH_ALONG, PATCH_ACROSS));
        let (along, across) = far.texels_for(&camera);
        assert!(along < PATCH_ALONG / 2 && along >= FEWEST.0, "{along}");
        assert!(across < PATCH_ACROSS && across >= FEWEST.1, "{across}");

        let mut uniforms = HullHeightsUniforms::default();
        uniforms.seen(&[behind, near, beside, far], &camera);
        assert_eq!(uniforms.count(), 2);
        assert_eq!(uniforms.patches[0][1], near.z as f32);
        assert_eq!(uniforms.patches[3][2], along as f32);
        assert_eq!(uniforms.largest(), (PATCH_ALONG, PATCH_ACROSS));
    }

    #[test]
    fn at_most_the_slots_are_placed() {
        let mut uniforms = HullHeightsUniforms::default();
        uniforms.set(&[DESTROYER; HULL_HEIGHT_SLOTS + 3]);
        assert_eq!(uniforms.count(), HULL_HEIGHT_SLOTS);
        uniforms.set(&[]);
        assert_eq!(uniforms.count(), 0);
    }
}
