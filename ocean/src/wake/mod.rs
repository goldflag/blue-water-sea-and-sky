//! The dispersive wake field (`src/game/ocean/wake/`): a height field in the spirit of iWave centred on a movable
//! anchor, stepped at a fixed 30 Hz on the GPU ([`WakeField`], `shaders/wake_field.wgsl`), with the fitted dispersion
//! kernels of [`kernel`], the hulls' swept pressure patches of [`generators`] and what strikes the sea ([`impulses`]:
//! craters, domes, upwellings). The surface samples its display textures (height, slope, foam), blending the last two
//! steps by the frame's fraction.

pub mod generators;
pub mod impulses;
pub mod kernel;

use bytemuck::{Pod, Zeroable};
use naval_render::{
    MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError, compute_recipe,
};

use crate::shaders;
use generators::{MAX_GENERATORS, WakeEmission, WakeGenerators};
use impulses::{Disturbances, MAX_IMPULSES, MAX_ZONES, Strike, WakeImpulse};

/// Fixed simulation step (s). Frames accumulate time and run whole steps; the sampler blends the last two.
pub const STEP: f64 = 1.0 / 30.0;
/// Steps one frame may run; a longer stall drops the excess instead of catching up.
pub const MAX_STEPS: usize = 4;
const GRAVITY: f64 = 9.81;
/// The coarsest pyramid level has this many cells per edge, so every tier covers the same wavelengths.
const COARSEST_CELLS: f64 = 32.0;
/// Turbulent spreading of wake foam (m²/s), so an old trail is wider than a fresh one.
const FOAM_SPREAD: f64 = 3.0;
/// Half the baseline (m) over which breaking steepness is measured, so a threshold means the same on every tier.
const BREAK_BASELINE: f64 = 6.0;
/// Footprints narrower than this many cells would alias on the grid.
const MIN_RADIUS_CELLS: f64 = 1.5;
/// Share of the field at each edge over which the sampler fades to calm (`EDGE_FADE`; the surface shader has it too).
pub const EDGE_FADE: f32 = 0.03;

/// Graphics → Ocean's wake resolution (`quality.ts`): none on Low (a calm wake), 256 on Medium, 512 on High, 1,024 on
/// Ultra.
pub fn resolution_for(tier: crate::QualityTier) -> u32 {
    match tier {
        crate::QualityTier::Low => 0,
        crate::QualityTier::Medium => 256,
        crate::QualityTier::High => 512,
        crate::QualityTier::Ultra => 1024,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct LevelUniforms {
    size: i32,
    source_size: i32,
    from_state: i32,
    coarser_size: i32,
    taps: [u32; 2],
    padding: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct StepUniforms {
    size: i32,
    top_size: i32,
    break_cells: i32,
    count: i32,
    shift: [i32; 2],
    taps: [u32; 2],
    cell_size: f32,
    acceleration: f32,
    damping: f32,
    foam_decay: f32,
    foam_diffusion: f32,
    foam_strength: f32,
    foam_threshold: f32,
    min_radius: f32,
    segments: [[f32; 4]; MAX_GENERATORS],
    shapes: [[f32; 4]; MAX_GENERATORS],
    /// Per generator: the far end of the stretch its own waves break along, relative to the field centre, and how far
    /// either side of it.
    breaks: [[f32; 4]; MAX_GENERATORS],
    /// x: impulses this step, y: ring trains.
    disturbances: [i32; 4],
    impulse_paths: [[f32; 4]; MAX_IMPULSES],
    impulse_shapes: [[f32; 4]; MAX_IMPULSES],
    zone_paths: [[f32; 4]; MAX_ZONES],
    zone_shapes: [[f32; 4]; MAX_ZONES],
}

/// Uniform slots are this far apart (a multiple of the 256-byte offset alignment WebGPU guarantees).
const LEVEL_SLOT: u64 = 256;
const STEP_SLOT: u64 = 2048;
const _: () = assert!(std::mem::size_of::<StepUniforms>() as u64 <= STEP_SLOT);

/// What the surface needs to sample the field this frame: each display texture's field origin and weight (the
/// previous step's times `1 − fraction`, the newest's times `fraction`, both times the gain) and the field's size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WakeSampling {
    pub origins: [[f32; 2]; 2],
    pub weights: [f32; 2],
    pub world_size: f32,
}

struct Textures {
    state: [wgpu::Texture; 2],
    display: [wgpu::Texture; 2],
    display_views: [wgpu::TextureView; 2],
    step_groups: [wgpu::BindGroup; 2],
    display_groups: [wgpu::BindGroup; 2],
    reduce_groups: [Vec<wgpu::BindGroup>; 2],
    collapse_groups: Vec<wgpu::BindGroup>,
}

/// The GPU wake field (`WakeField` in the web). Parameters are live, as the game sets them per frame.
pub struct WakeField {
    pub friction: f64,
    pub foam_strength: f64,
    pub foam_break_threshold: f64,
    pub foam_lifetime: f64,
    pub generators: WakeGenerators,
    disturbances: Disturbances,
    resolution: u32,
    levels: usize,
    world_size: f64,
    enabled: bool,
    emissions: Vec<WakeEmission>,
    current: usize,
    accumulator: f64,
    center: [f64; 2],
    cell: [i64; 2],
    origin: [f64; 2],
    previous_origin: [f64; 2],
    fraction: f64,
    gain: f64,
    level_taps: Vec<[u32; 2]>,
    reduce: wgpu::ComputePipeline,
    collapse: wgpu::ComputePipeline,
    step: wgpu::ComputePipeline,
    display: wgpu::ComputePipeline,
    layouts: Layouts,
    taps: wgpu::Buffer,
    level_uniforms: wgpu::Buffer,
    step_uniforms: wgpu::Buffer,
    textures: Textures,
    /// Steps run since creation, for diagnostics.
    pub steps_run: u64,
}

struct Layouts {
    reduce: wgpu::BindGroupLayout,
    collapse: wgpu::BindGroupLayout,
    step: wgpu::BindGroupLayout,
    display: wgpu::BindGroupLayout,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

fn taps_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 6,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        Self {
            reduce: layout(
                "wake reduce",
                &[
                    texture_entry(0),
                    storage_entry(2, wgpu::TextureFormat::R32Float),
                    uniform_entry(7, false),
                ],
            ),
            collapse: layout(
                "wake collapse",
                &[
                    texture_entry(0),
                    texture_entry(1),
                    storage_entry(2, wgpu::TextureFormat::R32Float),
                    taps_entry(),
                    uniform_entry(7, false),
                ],
            ),
            step: layout(
                "wake step",
                &[
                    texture_entry(0),
                    texture_entry(1),
                    storage_entry(3, wgpu::TextureFormat::Rgba32Float),
                    uniform_entry(5, true),
                    taps_entry(),
                ],
            ),
            display: layout(
                "wake display",
                &[
                    texture_entry(0),
                    storage_entry(4, wgpu::TextureFormat::Rgba16Float),
                    uniform_entry(5, true),
                ],
            ),
        }
    }
}

fn grid_texture(
    device: &wgpu::Device,
    label: &str,
    size: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn tex(view: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(view)
}

fn view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The wake field's pipelines on the renderer's registry ([`WakeField::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct WakeFieldPipelines {
    reduce: PipelineId,
    collapse: PipelineId,
    step: PipelineId,
    display: PipelineId,
}

impl WakeField {
    /// A field of `resolution`² cells (at least 64) over `world_size` metres.
    pub fn new(device: &wgpu::Device, resolution: u32, world_size: f64) -> Self {
        Self::build(device, resolution, world_size, MakePipelines::Now)
    }

    /// Builds the four passes through the renderer's registry, as [`WakeField::new`] builds them. The ocean's files
    /// must be registered first (`crate::shaders::register`).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<WakeFieldPipelines, ShaderError> {
        let l = &self.layouts;
        let mut add = |label: &str, layout: &wgpu::BindGroupLayout, entry| {
            ctx.add_pipeline(
                label,
                shaders::WAKE_FIELD.path,
                Vec::new(),
                compute_recipe(vec![layout.clone()], entry),
            )
        };
        Ok(WakeFieldPipelines {
            reduce: add("wake reduce", &l.reduce, "reduce_pass")?,
            collapse: add("wake collapse", &l.collapse, "collapse_pass")?,
            step: add("wake step", &l.step, "step_pass")?,
            display: add("wake display", &l.display, "display_pass")?,
        })
    }

    /// Takes the registry's current pipelines in place of the field's own; whether any changed.
    pub fn adopt_pipelines(
        &mut self,
        registry: &PipelineRegistry,
        ids: &WakeFieldPipelines,
    ) -> bool {
        let mut changed = false;
        for (slot, id) in [
            (&mut self.reduce, ids.reduce),
            (&mut self.collapse, ids.collapse),
            (&mut self.step, ids.step),
            (&mut self.display, ids.display),
        ] {
            let new = registry.compute(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn build(
        device: &wgpu::Device,
        resolution: u32,
        world_size: f64,
        make: MakePipelines<'_>,
    ) -> Self {
        let resolution = resolution.max(64);
        let levels = ((f64::from(resolution) / COARSEST_CELLS).log2().round() as usize).max(1);
        let pyramid = kernel::dispersion_pyramid(levels);
        let mut tap_data: Vec<[f32; 4]> = Vec::new();
        let mut level_taps = Vec::new();
        for level in &pyramid.levels {
            level_taps.push([tap_data.len() as u32, level.len() as u32]);
            for tap in level {
                tap_data.push([tap.x as f32, tap.y as f32, tap.weight as f32, 0.0]);
            }
        }
        let taps = buffer_with(
            device,
            "wake kernel taps",
            bytemuck::cast_slice(&tap_data),
            wgpu::BufferUsages::STORAGE,
        );
        let level_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wake levels"),
            size: LEVEL_SLOT * 2 * levels as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let step_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wake steps"),
            size: STEP_SLOT * MAX_STEPS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layouts = Layouts::new(device);
        let module = make.module(|| shaders::shader_module(device, shaders::WAKE_FIELD, &[]));
        let pipeline = |label: &str, layout: &wgpu::BindGroupLayout, entry: &str| {
            make.compute(|| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(
                        &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: Some(label),
                            bind_group_layouts: &[Some(layout)],
                            immediate_size: 0,
                        }),
                    ),
                    module: module.as_ref().expect("built now"),
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
        };
        let reduce = pipeline("wake reduce", &layouts.reduce, "reduce_pass");
        let collapse = pipeline("wake collapse", &layouts.collapse, "collapse_pass");
        let step = pipeline("wake step", &layouts.step, "step_pass");
        let display = pipeline("wake display", &layouts.display, "display_pass");
        let textures = Self::textures(
            device,
            &layouts,
            resolution,
            levels,
            &taps,
            &level_uniforms,
            &step_uniforms,
        );
        let mut field = Self {
            friction: 0.065,
            foam_strength: 1.2,
            foam_break_threshold: 0.09,
            foam_lifetime: 9.0,
            generators: WakeGenerators::default(),
            disturbances: Disturbances::default(),
            resolution,
            levels,
            world_size,
            enabled: true,
            emissions: Vec::new(),
            current: 0,
            accumulator: 0.0,
            center: [0.0; 2],
            cell: [0; 2],
            origin: [0.0; 2],
            previous_origin: [0.0; 2],
            fraction: 1.0,
            gain: 1.0,
            level_taps,
            reduce,
            collapse,
            step,
            display,
            layouts,
            taps,
            level_uniforms,
            step_uniforms,
            textures,
            steps_run: 0,
        };
        field.restart();
        field
    }

    #[allow(clippy::too_many_arguments)]
    fn textures(
        device: &wgpu::Device,
        layouts: &Layouts,
        n: u32,
        levels: usize,
        taps: &wgpu::Buffer,
        level_uniforms: &wgpu::Buffer,
        step_uniforms: &wgpu::Buffer,
    ) -> Textures {
        let state = [
            grid_texture(device, "wake state 0", n, wgpu::TextureFormat::Rgba32Float),
            grid_texture(device, "wake state 1", n, wgpu::TextureFormat::Rgba32Float),
        ];
        let display = [
            grid_texture(
                device,
                "wake display 0",
                n,
                wgpu::TextureFormat::Rgba16Float,
            ),
            grid_texture(
                device,
                "wake display 1",
                n,
                wgpu::TextureFormat::Rgba16Float,
            ),
        ];
        let reduced: Vec<wgpu::Texture> = (1..=levels)
            .map(|level| {
                grid_texture(
                    device,
                    "wake reduced",
                    n >> level,
                    wgpu::TextureFormat::R32Float,
                )
            })
            .collect();
        let collapsed: Vec<wgpu::Texture> = (1..=levels)
            .map(|level| {
                grid_texture(
                    device,
                    "wake collapsed",
                    n >> level,
                    wgpu::TextureFormat::R32Float,
                )
            })
            .collect();
        let state_views = [view(&state[0]), view(&state[1])];
        let display_views = [view(&display[0]), view(&display[1])];
        let reduced_views: Vec<_> = reduced.iter().map(view).collect();
        let collapsed_views: Vec<_> = collapsed.iter().map(view).collect();
        let level_binding = |slot: usize| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: level_uniforms,
                offset: LEVEL_SLOT * slot as u64,
                size: wgpu::BufferSize::new(std::mem::size_of::<LevelUniforms>() as u64),
            })
        };
        let step_binding = || {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: step_uniforms,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<StepUniforms>() as u64),
            })
        };
        // Reduce: level 1 reads the state (either parity), deeper levels the level above.
        let reduce_groups = [0, 1].map(|parity| {
            (0..levels)
                .map(|i| {
                    let source = if i == 0 {
                        &state_views[parity]
                    } else {
                        &reduced_views[i - 1]
                    };
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("wake reduce"),
                        layout: &layouts.reduce,
                        entries: &[
                            entry(0, tex(source)),
                            entry(2, tex(&reduced_views[i])),
                            entry(7, level_binding(i)),
                        ],
                    })
                })
                .collect::<Vec<_>>()
        });
        // Collapse: level l reads reduced[l] and collapsed[l + 1] (itself at the coarsest, unread there).
        let collapse_groups = (0..levels)
            .map(|i| {
                let coarser = if i + 1 < levels {
                    &collapsed_views[i + 1]
                } else {
                    &reduced_views[i]
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("wake collapse"),
                    layout: &layouts.collapse,
                    entries: &[
                        entry(0, tex(&reduced_views[i])),
                        entry(1, tex(coarser)),
                        entry(2, tex(&collapsed_views[i])),
                        entry(6, taps.as_entire_binding()),
                        entry(7, level_binding(levels + i)),
                    ],
                })
            })
            .collect();
        let step_groups = [0, 1].map(|parity| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wake step"),
                layout: &layouts.step,
                entries: &[
                    entry(0, tex(&state_views[parity])),
                    entry(1, tex(&collapsed_views[0])),
                    entry(3, tex(&state_views[1 - parity])),
                    entry(5, step_binding()),
                    entry(6, taps.as_entire_binding()),
                ],
            })
        });
        // Display of the state a step just wrote (state[1 − parity]) into display[1 − parity].
        let display_groups = [0, 1].map(|parity| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wake display"),
                layout: &layouts.display,
                entries: &[
                    entry(0, tex(&state_views[1 - parity])),
                    entry(4, tex(&display_views[1 - parity])),
                    entry(5, step_binding()),
                ],
            })
        });
        Textures {
            state,
            display,
            display_views,
            step_groups,
            display_groups,
            reduce_groups,
            collapse_groups,
        }
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    pub fn world_size(&self) -> f64 {
        self.world_size
    }

    /// The two display textures the surface samples (their weights and origins are in [`WakeField::sampling`]).
    pub fn display_views(&self) -> [&wgpu::TextureView; 2] {
        [
            &self.textures.display_views[0],
            &self.textures.display_views[1],
        ]
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switching the field off calms the sea at once (the sampler's gain goes to 0) and resets it.
    pub fn set_enabled(&mut self, device: &wgpu::Device, enabled: bool) {
        if enabled != self.enabled {
            self.enabled = enabled;
            self.reset(device);
        }
    }

    pub fn set_center(&mut self, x: f64, z: f64) {
        self.center = [x, z];
    }

    /// The narrowest footprint the grid carries without aliasing (m): what strikes the sea is laid no narrower.
    pub fn least_radius(&self) -> f64 {
        MIN_RADIUS_CELLS * self.world_size / f64::from(self.resolution)
    }

    /// Whether world `x`, `z` lies on the field about its centre.
    pub fn covers(&self, x: f64, z: f64) -> bool {
        let half = 0.5 * self.world_size;
        (x - self.center[0]).abs() < half && (z - self.center[1]).abs() < half
    }

    /// Lay `impulse` on the field from its delay on; false (and nothing laid) when the field is off or it lies off
    /// the field.
    pub fn add_impulse(&mut self, impulse: WakeImpulse) -> bool {
        let middle = [
            0.5 * (impulse.from[0] + impulse.to[0]),
            0.5 * (impulse.from[1] + impulse.to[1]),
        ];
        if !self.enabled || !self.covers(middle[0], middle[1]) {
            return false;
        }
        self.disturbances.add(impulse);
        true
    }

    /// What `strike` at world `x`, `z` does to the sea ([`impulses::strike_impulse`]), laid as [`WakeField::add_impulse`].
    pub fn strike(&mut self, x: f64, z: f64, strike: &Strike) -> bool {
        impulses::strike_impulse(x, z, strike, self.least_radius())
            .is_some_and(|impulse| self.add_impulse(impulse))
    }

    /// Whether anything that struck the sea is waiting, acting or still ringing.
    pub fn disturbed(&self) -> bool {
        self.disturbances.busy()
    }

    /// Clear the field (new zeroed textures) and every generator's history.
    pub fn reset(&mut self, device: &wgpu::Device) {
        self.textures = Self::textures(
            device,
            &self.layouts,
            self.resolution,
            self.levels,
            &self.taps,
            &self.level_uniforms,
            &self.step_uniforms,
        );
        self.current = 0;
        self.restart();
    }

    /// Everything `reset` does except clearing the textures.
    fn restart(&mut self) {
        self.generators.restart();
        self.disturbances.clear();
        self.accumulator = 0.0;
        let cell_size = self.world_size / f64::from(self.resolution);
        self.cell = [
            (self.center[0] / cell_size).round() as i64,
            (self.center[1] / cell_size).round() as i64,
        ];
        self.origin = [
            self.cell[0] as f64 * cell_size,
            self.cell[1] as f64 * cell_size,
        ];
        self.previous_origin = self.origin;
        self.fraction = 1.0;
        self.gain = if self.enabled { 1.0 } else { 0.0 };
    }

    /// Advance by `dt` seconds: whole 30 Hz steps (at most [`MAX_STEPS`]) encoded on `encoder`. A paused frame (`dt` 0)
    /// runs nothing.
    pub fn update(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, dt: f64) {
        self.update_timed(
            &mut naval_render::UploadSink::Queue(queue),
            encoder,
            dt,
            None,
        );
    }

    /// [`WakeField::update`], each step's compute pass timed by the renderer core's timer as `ocean wake step`, the
    /// steps' uniforms written through `queue` (the renderer core's staging belt in a layer's `prepare`).
    pub fn update_timed(
        &mut self,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        dt: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        if !self.enabled || dt.is_nan() || dt <= 0.0 {
            return;
        }
        self.generators.begin_frame();
        let before = self.accumulator;
        self.accumulator += dt;
        let steps = ((self.accumulator / STEP).floor() as usize).min(MAX_STEPS);
        if steps == MAX_STEPS {
            self.accumulator = self.accumulator.min(MAX_STEPS as f64 * STEP);
        }
        // Level uniforms are fixed per resolution; writing them each advancing frame keeps them after a reset.
        if steps > 0 {
            self.write_levels(queue);
        }
        for s in 1..=steps {
            let fraction = ((s as f64 * STEP - before) / dt).min(1.0);
            self.advance(queue, encoder, s - 1, fraction, timer);
        }
        self.accumulator -= steps as f64 * STEP;
        self.fraction = (self.accumulator / STEP).min(1.0);
    }

    fn write_levels(&self, queue: &mut naval_render::UploadSink<'_>) {
        let n = self.resolution as i32;
        for i in 0..self.levels {
            let level = i + 1;
            let reduce = LevelUniforms {
                size: n >> level,
                source_size: n >> (level - 1),
                from_state: i32::from(level == 1),
                coarser_size: 0,
                taps: [0, 0],
                padding: [0, 0],
            };
            let collapse = LevelUniforms {
                size: n >> level,
                source_size: n >> level,
                from_state: 0,
                coarser_size: if level < self.levels {
                    n >> (level + 1)
                } else {
                    0
                },
                taps: self.level_taps[level],
                padding: [0, 0],
            };
            queue.write_buffer(
                &self.level_uniforms,
                LEVEL_SLOT * i as u64,
                bytemuck::bytes_of(&reduce),
            );
            queue.write_buffer(
                &self.level_uniforms,
                LEVEL_SLOT * (self.levels + i) as u64,
                bytemuck::bytes_of(&collapse),
            );
        }
    }

    /// One fixed step ending at `fraction` of the current frame, its uniforms in slot `slot`.
    fn advance(
        &mut self,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        slot: usize,
        fraction: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let n = self.resolution;
        let cell_size = self.world_size / f64::from(n);
        let x = (self.center[0] / cell_size).round() as i64;
        let z = (self.center[1] / cell_size).round() as i64;
        let shift = [(x - self.cell[0]) as i32, (z - self.cell[1]) as i32];
        self.cell = [x, z];
        self.previous_origin = self.origin;
        self.origin = [x as f64 * cell_size, z as f64 * cell_size];
        self.generators.step(fraction, STEP, &mut self.emissions);
        let mut uniforms = StepUniforms {
            size: n as i32,
            top_size: (n >> 1) as i32,
            break_cells: ((BREAK_BASELINE / cell_size).round() as i32).max(1),
            count: self.emissions.len() as i32,
            shift,
            taps: self.level_taps[0],
            cell_size: cell_size as f32,
            acceleration: (GRAVITY * STEP * STEP / cell_size) as f32,
            damping: (self.friction.max(0.0) * STEP / 2.0) as f32,
            foam_decay: (-STEP / self.foam_lifetime.max(1e-3)).exp() as f32,
            foam_diffusion: (FOAM_SPREAD * STEP / (cell_size * cell_size)).min(0.2) as f32,
            foam_strength: self.foam_strength.max(0.0) as f32,
            foam_threshold: self.foam_break_threshold.max(1e-4) as f32,
            min_radius: (MIN_RADIUS_CELLS * cell_size) as f32,
            segments: [[0.0; 4]; MAX_GENERATORS],
            shapes: [[0.0; 4]; MAX_GENERATORS],
            breaks: [[0.0; 4]; MAX_GENERATORS],
            disturbances: [0; 4],
            impulse_paths: [[0.0; 4]; MAX_IMPULSES],
            impulse_shapes: [[0.0; 4]; MAX_IMPULSES],
            zone_paths: [[0.0; 4]; MAX_ZONES],
            zone_shapes: [[0.0; 4]; MAX_ZONES],
        };
        for (i, e) in self.emissions.iter().enumerate() {
            uniforms.segments[i] = [
                (e.x0 - self.origin[0]) as f32,
                (e.z0 - self.origin[1]) as f32,
                (e.x1 - self.origin[0]) as f32,
                (e.z1 - self.origin[1]) as f32,
            ];
            uniforms.shapes[i] = [e.radius as f32, e.depth as f32, e.gate as f32, 0.0];
            uniforms.breaks[i] = [
                (e.breaks_to[0] - self.origin[0]) as f32,
                (e.breaks_to[1] - self.origin[1]) as f32,
                e.break_reach as f32,
                0.0,
            ];
        }
        let load = self.disturbances.step(
            STEP,
            self.origin,
            &mut uniforms.impulse_paths,
            &mut uniforms.impulse_shapes,
            &mut uniforms.zone_paths,
            &mut uniforms.zone_shapes,
        );
        uniforms.disturbances = [load.impulses as i32, load.zones as i32, 0, 0];
        let offset = STEP_SLOT * slot as u64;
        queue.write_buffer(&self.step_uniforms, offset, bytemuck::bytes_of(&uniforms));
        let dynamic = offset as u32;
        let groups = |size: u32| size.div_ceil(8);
        let parity = self.current;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("wake step"),
            timestamp_writes: timer.and_then(|timer| timer.compute_pass("ocean wake step")),
        });
        pass.set_pipeline(&self.reduce);
        for (i, group) in self.textures.reduce_groups[parity].iter().enumerate() {
            pass.set_bind_group(0, group, &[]);
            let size = n >> (i + 1);
            pass.dispatch_workgroups(groups(size), groups(size), 1);
        }
        pass.set_pipeline(&self.collapse);
        for i in (0..self.levels).rev() {
            pass.set_bind_group(0, &self.textures.collapse_groups[i], &[]);
            let size = n >> (i + 1);
            pass.dispatch_workgroups(groups(size), groups(size), 1);
        }
        pass.set_pipeline(&self.step);
        pass.set_bind_group(0, &self.textures.step_groups[parity], &[dynamic]);
        pass.dispatch_workgroups(groups(n), groups(n), 1);
        pass.set_pipeline(&self.display);
        pass.set_bind_group(0, &self.textures.display_groups[parity], &[dynamic]);
        pass.dispatch_workgroups(groups(n), groups(n), 1);
        drop(pass);
        self.current = 1 - self.current;
        self.steps_run += 1;
    }

    /// The display textures' origins and weights this frame (`createSampler`'s blend of the last two steps).
    pub fn sampling(&self) -> WakeSampling {
        let current = self.current;
        let mut origins = [[0.0; 2]; 2];
        let mut weights = [0.0; 2];
        origins[current] = [self.origin[0] as f32, self.origin[1] as f32];
        origins[1 - current] = [
            self.previous_origin[0] as f32,
            self.previous_origin[1] as f32,
        ];
        weights[current] = (self.fraction * self.gain) as f32;
        weights[1 - current] = ((1.0 - self.fraction) * self.gain) as f32;
        WakeSampling {
            origins,
            weights,
            world_size: self.world_size as f32,
        }
    }

    /// The world x, z of the newest step's field centre (texel i lies at `origin + (i + ½ − n/2) × cell`).
    pub fn origin(&self) -> [f64; 2] {
        self.origin
    }

    /// The state textures, for tests and read-backs: (h, h previous, foam, head) of the newest step.
    pub fn state_texture(&self) -> &wgpu::Texture {
        &self.textures.state[self.current]
    }

    /// The newest display texture.
    pub fn display_texture(&self) -> &wgpu::Texture {
        &self.textures.display[self.current]
    }
}

fn buffer_with(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage,
    })
}
