//! takram's atmosphere tables on the GPU (Bruneton's precomputed scattering, `shaders/precompute.wgsl`): the
//! runtime tables the sky's group 1 binds, and the precompute that fills them when the air changes. Each run
//! makes its own scratch (deltas, float32 accumulators: about 75 MB, dropped once the run is encoded) and ends in a
//! resolve that writes all of the runtime tables at once, so a recompute swaps them within one frame.

use naval_render::{
    MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError, compute_recipe,
};

use crate::gpu::{self, workgroups};
use crate::model::AtmosphereParameters;
use crate::shaders;

pub(crate) const TRANSMITTANCE_SIZE: (u32, u32) = (256, 64);
pub(crate) const SCATTERING_SIZE: (u32, u32, u32) = (256, 128, 32);
pub(crate) const IRRADIANCE_SIZE: (u32, u32) = (64, 16);
const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FLOAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The orders of scattering after the first that the precompute adds (takram's loop: 2 to 4).
const ORDERS: [i32; 3] = [2, 3, 4];

/// The runtime's tables: transmittance (float32), scattering, single Mie and the higher orders (4D in 3D half
/// floats), and the sky's irradiance.
pub(crate) struct AtmosphereTables {
    pub transmittance: wgpu::Texture,
    pub transmittance_view: wgpu::TextureView,
    pub scattering_view: wgpu::TextureView,
    pub single_mie_view: wgpu::TextureView,
    pub higher_order_view: wgpu::TextureView,
    pub irradiance_view: wgpu::TextureView,
    /// Scattering, single Mie, the higher orders and irradiance.
    textures: Vec<wgpu::Texture>,
}

impl AtmosphereTables {
    pub fn new(device: &wgpu::Device) -> Self {
        // Each table can be copied into another sky's (`Sky::take_tables_from`).
        let copies = wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        let sampled =
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | copies;
        let transmittance = gpu::texture_2d(
            device,
            "sky transmittance",
            TRANSMITTANCE_SIZE.0,
            TRANSMITTANCE_SIZE.1,
            1,
            FLOAT,
            wgpu::TextureUsages::TEXTURE_BINDING | copies,
        );
        let scattering = gpu::texture_3d(device, "sky scattering", SCATTERING_SIZE, HALF, sampled);
        let single_mie = gpu::texture_3d(device, "sky single mie", SCATTERING_SIZE, HALF, sampled);
        let higher_order = gpu::texture_3d(
            device,
            "sky higher order scattering",
            SCATTERING_SIZE,
            HALF,
            sampled,
        );
        let irradiance = gpu::texture_2d(
            device,
            "sky irradiance",
            IRRADIANCE_SIZE.0,
            IRRADIANCE_SIZE.1,
            1,
            HALF,
            sampled,
        );
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        Self {
            transmittance_view: view(&transmittance),
            scattering_view: view(&scattering),
            single_mie_view: view(&single_mie),
            higher_order_view: view(&higher_order),
            irradiance_view: view(&irradiance),
            transmittance,
            textures: vec![scattering, single_mie, higher_order, irradiance],
        }
    }

    /// Copies `from`'s tables into these on `encoder` (the same sizes and formats: about 25 MB).
    pub fn copy_from(&self, encoder: &mut wgpu::CommandEncoder, from: &AtmosphereTables) {
        let pairs = std::iter::once((&from.transmittance, &self.transmittance))
            .chain(from.textures.iter().zip(&self.textures));
        for (source, target) in pairs {
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                target.as_image_copy(),
                source.size(),
            );
        }
    }
}

/// The precompute's bind group layouts: the parameters (group 1), what each pass reads (group 2) and writes
/// (group 3).
struct Layouts {
    none: wgpu::BindGroupLayout,
    parameters: wgpu::BindGroupLayout,
    read_transmittance: wgpu::BindGroupLayout,
    read_density: wgpu::BindGroupLayout,
    read_indirect: wgpu::BindGroupLayout,
    read_multiple: wgpu::BindGroupLayout,
    write_transmittance: wgpu::BindGroupLayout,
    write_direct: wgpu::BindGroupLayout,
    write_single: wgpu::BindGroupLayout,
    write_density: wgpu::BindGroupLayout,
    write_indirect: wgpu::BindGroupLayout,
    write_multiple: wgpu::BindGroupLayout,
    write_resolve_scattering: wgpu::BindGroupLayout,
    write_resolve_irradiance: wgpu::BindGroupLayout,
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = |binding| gpu::buffer_entry(binding, false);
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            gpu::layout(device, label, entries)
        };
        Self {
            none: layout("sky precompute none", &[]),
            parameters: layout(
                "sky precompute parameters",
                &[gpu::uniform_entry(0, false), gpu::uniform_entry(1, false)],
            ),
            read_transmittance: layout(
                "sky precompute read transmittance",
                &[gpu::texture_entry(0, false)],
            ),
            read_density: layout(
                "sky precompute read density",
                &[
                    gpu::texture_entry(0, false),
                    gpu::texture_3d_entry(1),
                    gpu::texture_3d_entry(2),
                    gpu::texture_entry(4, true),
                    gpu::sampler_entry(5),
                ],
            ),
            read_indirect: layout(
                "sky precompute read indirect",
                &[
                    gpu::texture_3d_entry(1),
                    gpu::texture_3d_entry(2),
                    gpu::sampler_entry(5),
                ],
            ),
            read_multiple: layout(
                "sky precompute read multiple",
                &[
                    gpu::texture_entry(0, false),
                    gpu::texture_3d_entry(3),
                    gpu::sampler_entry(5),
                ],
            ),
            write_transmittance: layout(
                "sky precompute write transmittance",
                &[gpu::storage_entry(0, FLOAT)],
            ),
            write_direct: layout(
                "sky precompute write direct",
                &[gpu::storage_entry(1, HALF), buffer(8)],
            ),
            write_single: layout(
                "sky precompute write single",
                &[
                    gpu::storage_3d_entry(2, HALF),
                    gpu::storage_3d_entry(3, HALF),
                    buffer(5),
                    buffer(6),
                    buffer(7),
                ],
            ),
            write_density: layout(
                "sky precompute write density",
                &[gpu::storage_3d_entry(4, HALF)],
            ),
            write_indirect: layout(
                "sky precompute write indirect",
                &[gpu::storage_entry(1, HALF), buffer(8)],
            ),
            write_multiple: layout(
                "sky precompute write multiple",
                &[gpu::storage_3d_entry(2, HALF), buffer(5), buffer(6)],
            ),
            write_resolve_scattering: layout(
                "sky precompute write resolve scattering",
                &[
                    buffer(5),
                    buffer(6),
                    buffer(7),
                    gpu::storage_3d_entry(9, HALF),
                    gpu::storage_3d_entry(10, HALF),
                    gpu::storage_3d_entry(11, HALF),
                ],
            ),
            write_resolve_irradiance: layout(
                "sky precompute write resolve irradiance",
                &[buffer(8), gpu::storage_entry(12, HALF)],
            ),
        }
    }

    /// The groups of a pass: none, the parameters, its reads, its writes.
    fn groups(
        &self,
        reads: &wgpu::BindGroupLayout,
        writes: &wgpu::BindGroupLayout,
    ) -> Vec<wgpu::BindGroupLayout> {
        vec![
            self.none.clone(),
            self.parameters.clone(),
            reads.clone(),
            writes.clone(),
        ]
    }
}

/// One pass of the precompute: its entry point and its read and write layouts.
#[derive(Clone, Copy)]
enum Pass {
    Transmittance,
    DirectIrradiance,
    SingleScattering,
    ScatteringDensity,
    IndirectIrradiance,
    MultipleScattering,
    ResolveScattering,
    ResolveIrradiance,
}

const PASSES: [Pass; 8] = [
    Pass::Transmittance,
    Pass::DirectIrradiance,
    Pass::SingleScattering,
    Pass::ScatteringDensity,
    Pass::IndirectIrradiance,
    Pass::MultipleScattering,
    Pass::ResolveScattering,
    Pass::ResolveIrradiance,
];

impl Pass {
    fn entry(self) -> &'static str {
        match self {
            Self::Transmittance => "transmittance_main",
            Self::DirectIrradiance => "direct_irradiance_main",
            Self::SingleScattering => "single_scattering_main",
            Self::ScatteringDensity => "scattering_density_main",
            Self::IndirectIrradiance => "indirect_irradiance_main",
            Self::MultipleScattering => "multiple_scattering_main",
            Self::ResolveScattering => "resolve_scattering_main",
            Self::ResolveIrradiance => "resolve_irradiance_main",
        }
    }

    fn layouts(self, layouts: &Layouts) -> (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout) {
        match self {
            Self::Transmittance => (&layouts.none, &layouts.write_transmittance),
            Self::DirectIrradiance => (&layouts.read_transmittance, &layouts.write_direct),
            Self::SingleScattering => (&layouts.read_transmittance, &layouts.write_single),
            Self::ScatteringDensity => (&layouts.read_density, &layouts.write_density),
            Self::IndirectIrradiance => (&layouts.read_indirect, &layouts.write_indirect),
            Self::MultipleScattering => (&layouts.read_multiple, &layouts.write_multiple),
            Self::ResolveScattering => (&layouts.none, &layouts.write_resolve_scattering),
            Self::ResolveIrradiance => (&layouts.none, &layouts.write_resolve_irradiance),
        }
    }
}

/// The precompute's pipelines on the renderer's registry, in [`PASSES`] order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PrecomputePipelines {
    ids: [PipelineId; 8],
}

/// The precompute: its layouts, pipelines and the uniforms it reads.
pub(crate) struct Precompute {
    layouts: Layouts,
    pipelines: Vec<wgpu::ComputePipeline>,
    parameters: wgpu::Buffer,
    orders: Vec<wgpu::Buffer>,
    sampler: wgpu::Sampler,
}

impl Precompute {
    pub fn new(device: &wgpu::Device, make: MakePipelines<'_>) -> Self {
        let layouts = Layouts::new(device);
        let module = make.module(|| {
            shaders::shader_module(device, shaders::PRECOMPUTE_WGSL, shaders::PRECOMPUTE_PATH)
        });
        let pipelines = PASSES
            .iter()
            .map(|&pass| {
                make.compute(|| {
                    let (reads, writes) = pass.layouts(&layouts);
                    gpu::compute_pipeline(
                        device,
                        pass.entry(),
                        module.as_ref().expect("built now"),
                        pass.entry(),
                        &[None, Some(&layouts.parameters), Some(reads), Some(writes)],
                    )
                })
            })
            .collect();
        let uniform = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let parameters = uniform(
            "sky precompute parameters",
            std::mem::size_of::<AtmosphereParameters>() as u64,
        );
        let orders = ORDERS
            .iter()
            .map(|_| uniform("sky precompute order", 16))
            .collect();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky precompute linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layouts,
            pipelines,
            parameters,
            orders,
            sampler,
        }
    }

    /// Builds the precompute's pipelines through the renderer's registry (the sky's files registered first).
    pub fn register(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<PrecomputePipelines, ShaderError> {
        let mut ids = Vec::with_capacity(PASSES.len());
        for pass in PASSES {
            let (reads, writes) = pass.layouts(&self.layouts);
            ids.push(ctx.add_pipeline(
                pass.entry(),
                shaders::PRECOMPUTE_PATH,
                Vec::new(),
                compute_recipe(self.layouts.groups(reads, writes), pass.entry()),
            )?);
        }
        Ok(PrecomputePipelines {
            ids: ids.try_into().expect("one id per pass"),
        })
    }

    /// Takes the registry's current pipelines; true when any changed (the tables must be computed again).
    pub fn adopt(&mut self, registry: &PipelineRegistry, ids: &PrecomputePipelines) -> bool {
        let mut changed = false;
        for (slot, id) in self.pipelines.iter_mut().zip(ids.ids) {
            let new = registry.compute(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        changed
    }

    /// Computes the tables for `parameters` into `tables`: the passes in one compute pass, then the transmittance
    /// copied in. The scratch lives until the GPU has run what is encoded here.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        tables: &AtmosphereTables,
        parameters: &AtmosphereParameters,
        timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        use wgpu::BindingResource as R;
        queue.write_buffer(&self.parameters, 0, bytemuck::bytes_of(parameters));
        for (buffer, order) in self.orders.iter().zip(ORDERS) {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&[order, 0, 0, 0]));
        }
        let storage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let transmittance = gpu::texture_2d(
            device,
            "sky precompute transmittance",
            TRANSMITTANCE_SIZE.0,
            TRANSMITTANCE_SIZE.1,
            1,
            FLOAT,
            storage | wgpu::TextureUsages::COPY_SRC,
        );
        let delta_irradiance = gpu::texture_2d(
            device,
            "sky precompute delta irradiance",
            IRRADIANCE_SIZE.0,
            IRRADIANCE_SIZE.1,
            1,
            HALF,
            storage,
        );
        let rayleigh_or_multiple = gpu::texture_3d(
            device,
            "sky precompute delta rayleigh or multiple",
            SCATTERING_SIZE,
            HALF,
            storage,
        );
        let delta_mie = gpu::texture_3d(
            device,
            "sky precompute delta mie",
            SCATTERING_SIZE,
            HALF,
            storage,
        );
        let delta_density = gpu::texture_3d(
            device,
            "sky precompute scattering density",
            SCATTERING_SIZE,
            HALF,
            storage,
        );
        let texels = u64::from(SCATTERING_SIZE.0 * SCATTERING_SIZE.1 * SCATTERING_SIZE.2);
        let sum = |label: &str, texels: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: texels * 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let scattering_sum = sum("sky precompute scattering sum", texels);
        let higher_order_sum = sum("sky precompute higher order sum", texels);
        let single_mie_sum = sum("sky precompute single mie sum", texels);
        let irradiance_sum = sum(
            "sky precompute irradiance sum",
            u64::from(IRRADIANCE_SIZE.0 * IRRADIANCE_SIZE.1),
        );

        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let (transmittance_view, delta_irradiance_view) =
            (view(&transmittance), view(&delta_irradiance));
        let (rayleigh_view, mie_view, density_view) = (
            view(&rayleigh_or_multiple),
            view(&delta_mie),
            view(&delta_density),
        );
        let l = &self.layouts;
        let group = |label: &str, layout: &wgpu::BindGroupLayout, entries: &[(u32, R<'_>)]| {
            gpu::bind_group(device, label, layout, entries)
        };
        let none = group("sky precompute none", &l.none, &[]);
        let parameters: Vec<wgpu::BindGroup> = self
            .orders
            .iter()
            .map(|order| {
                group(
                    "sky precompute parameters",
                    &l.parameters,
                    &[
                        (0, self.parameters.as_entire_binding()),
                        (1, order.as_entire_binding()),
                    ],
                )
            })
            .collect();
        let read_transmittance = group(
            "sky precompute read transmittance",
            &l.read_transmittance,
            &[(0, R::TextureView(&transmittance_view))],
        );
        let read_density = group(
            "sky precompute read density",
            &l.read_density,
            &[
                (0, R::TextureView(&transmittance_view)),
                (1, R::TextureView(&rayleigh_view)),
                (2, R::TextureView(&mie_view)),
                (4, R::TextureView(&delta_irradiance_view)),
                (5, R::Sampler(&self.sampler)),
            ],
        );
        let read_indirect = group(
            "sky precompute read indirect",
            &l.read_indirect,
            &[
                (1, R::TextureView(&rayleigh_view)),
                (2, R::TextureView(&mie_view)),
                (5, R::Sampler(&self.sampler)),
            ],
        );
        let read_multiple = group(
            "sky precompute read multiple",
            &l.read_multiple,
            &[
                (0, R::TextureView(&transmittance_view)),
                (3, R::TextureView(&density_view)),
                (5, R::Sampler(&self.sampler)),
            ],
        );
        let write_transmittance = group(
            "sky precompute write transmittance",
            &l.write_transmittance,
            &[(0, R::TextureView(&transmittance_view))],
        );
        let write_direct = group(
            "sky precompute write direct",
            &l.write_direct,
            &[
                (1, R::TextureView(&delta_irradiance_view)),
                (8, irradiance_sum.as_entire_binding()),
            ],
        );
        let write_single = group(
            "sky precompute write single",
            &l.write_single,
            &[
                (2, R::TextureView(&rayleigh_view)),
                (3, R::TextureView(&mie_view)),
                (5, scattering_sum.as_entire_binding()),
                (6, higher_order_sum.as_entire_binding()),
                (7, single_mie_sum.as_entire_binding()),
            ],
        );
        let write_density = group(
            "sky precompute write density",
            &l.write_density,
            &[(4, R::TextureView(&density_view))],
        );
        let write_indirect = group(
            "sky precompute write indirect",
            &l.write_indirect,
            &[
                (1, R::TextureView(&delta_irradiance_view)),
                (8, irradiance_sum.as_entire_binding()),
            ],
        );
        let write_multiple = group(
            "sky precompute write multiple",
            &l.write_multiple,
            &[
                (2, R::TextureView(&rayleigh_view)),
                (5, scattering_sum.as_entire_binding()),
                (6, higher_order_sum.as_entire_binding()),
            ],
        );
        let write_resolve_scattering = group(
            "sky precompute write resolve scattering",
            &l.write_resolve_scattering,
            &[
                (5, scattering_sum.as_entire_binding()),
                (6, higher_order_sum.as_entire_binding()),
                (7, single_mie_sum.as_entire_binding()),
                (9, R::TextureView(&tables.scattering_view)),
                (10, R::TextureView(&tables.single_mie_view)),
                (11, R::TextureView(&tables.higher_order_view)),
            ],
        );
        let write_resolve_irradiance = group(
            "sky precompute write resolve irradiance",
            &l.write_resolve_irradiance,
            &[
                (8, irradiance_sum.as_entire_binding()),
                (12, R::TextureView(&tables.irradiance_view)),
            ],
        );

        let scattering_groups = (
            workgroups(SCATTERING_SIZE.0, 8),
            workgroups(SCATTERING_SIZE.1, 8),
            SCATTERING_SIZE.2,
        );
        let irradiance_groups = (
            workgroups(IRRADIANCE_SIZE.0, 8),
            workgroups(IRRADIANCE_SIZE.1, 8),
            1,
        );
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky precompute"),
                timestamp_writes: timestamps,
            });
            let mut dispatch = |pass_kind: Pass,
                                parameters: &wgpu::BindGroup,
                                reads: &wgpu::BindGroup,
                                writes: &wgpu::BindGroup,
                                groups: (u32, u32, u32)| {
                let index = PASSES
                    .iter()
                    .position(|p| p.entry() == pass_kind.entry())
                    .expect("a pass");
                pass.set_pipeline(&self.pipelines[index]);
                pass.set_bind_group(1, parameters, &[]);
                pass.set_bind_group(2, reads, &[]);
                pass.set_bind_group(3, writes, &[]);
                pass.dispatch_workgroups(groups.0, groups.1, groups.2);
            };
            let first = &parameters[0];
            dispatch(
                Pass::Transmittance,
                first,
                &none,
                &write_transmittance,
                (
                    workgroups(TRANSMITTANCE_SIZE.0, 8),
                    workgroups(TRANSMITTANCE_SIZE.1, 8),
                    1,
                ),
            );
            dispatch(
                Pass::DirectIrradiance,
                first,
                &read_transmittance,
                &write_direct,
                irradiance_groups,
            );
            dispatch(
                Pass::SingleScattering,
                first,
                &read_transmittance,
                &write_single,
                scattering_groups,
            );
            for order in &parameters {
                dispatch(
                    Pass::ScatteringDensity,
                    order,
                    &read_density,
                    &write_density,
                    scattering_groups,
                );
                dispatch(
                    Pass::IndirectIrradiance,
                    order,
                    &read_indirect,
                    &write_indirect,
                    irradiance_groups,
                );
                dispatch(
                    Pass::MultipleScattering,
                    order,
                    &read_multiple,
                    &write_multiple,
                    scattering_groups,
                );
            }
            dispatch(
                Pass::ResolveScattering,
                first,
                &none,
                &write_resolve_scattering,
                scattering_groups,
            );
            dispatch(
                Pass::ResolveIrradiance,
                first,
                &none,
                &write_resolve_irradiance,
                irradiance_groups,
            );
        }
        encoder.copy_texture_to_texture(
            transmittance.as_image_copy(),
            tables.transmittance.as_image_copy(),
            wgpu::Extent3d {
                width: TRANSMITTANCE_SIZE.0,
                height: TRANSMITTANCE_SIZE.1,
                depth_or_array_layers: 1,
            },
        );
    }
}
