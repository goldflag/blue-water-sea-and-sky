//! The clouds' detail noise: one 128³ four-channel volume (low- and high-frequency wisps, low- and high-frequency
//! billows) with its mips, filled on the GPU at startup by `detail_noise.wgsl`. It holds the clouds' structure from a
//! few tens of metres down to about a metre (`field.wgsl`'s `sample_media`), where takram's 32³ shape detail stopped
//! at 20–40 m. The generator is the crate's own: layered "alligator" noise (a cellular noise of rounded bumps with
//! creases between them, the kind Guerrilla's Nubis³ names for billows), curl-displaced for the wisps, on a lattice
//! hashed with integers, so every GPU places the same bumps and no asset is shipped. [`reference_texel`] is the same
//! noise on the CPU, which a GPU test holds the fill to.

use naval_render::{
    MakePipelines, PipelineId, PipelineRegistry, RenderContext, ShaderError, compute_recipe,
};

use crate::gpu::{self, Texture, workgroups};
use crate::shaders;

/// Texels per edge, and the mips kept (down to 4³: a coarser mip is the channel's mean).
pub const SIZE: u32 = 128;
pub const MIPS: u32 = 6;

/// The volume (`Rgba8Unorm`, 128³ and its mips), for [`DetailFill`] to fill.
pub fn texture(device: &wgpu::Device) -> Texture {
    gpu::texture(
        device,
        "cloud detail noise",
        (SIZE, SIZE, SIZE),
        wgpu::TextureDimension::D3,
        MIPS,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    )
}

/// The registry's pipelines of the fill: the noise and its mips.
#[derive(Clone, Copy, Debug)]
pub struct DetailPipelines {
    fill: PipelineId,
    mip: PipelineId,
}

/// The detail noise's fill: its groups, its pipelines and whether the volume holds their output.
pub struct DetailFill {
    fill_layout: wgpu::BindGroupLayout,
    mip_layout: wgpu::BindGroupLayout,
    fill_group: wgpu::BindGroup,
    /// Mip `i + 1` from mip `i`.
    mip_groups: Vec<wgpu::BindGroup>,
    fill_pipeline: wgpu::ComputePipeline,
    mip_pipeline: wgpu::ComputePipeline,
    filled: bool,
}

impl DetailFill {
    pub fn new(device: &wgpu::Device, texture: &Texture, make: MakePipelines<'_>) -> Self {
        let storage = gpu::storage_entry(
            0,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureViewDimension::D3,
        );
        let fill_layout = gpu::layout(device, "cloud detail noise fill", &[storage]);
        let mip_layout = gpu::layout(
            device,
            "cloud detail noise mip",
            &[
                storage,
                wgpu::BindGroupLayoutEntry {
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ..gpu::texture_entry(1, wgpu::TextureViewDimension::D3, false)
                },
            ],
        );
        let level = |mip: u32| {
            texture.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("cloud detail noise mip"),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let fill_group = gpu::bind_group(
            device,
            "cloud detail noise fill",
            &fill_layout,
            &[(0, wgpu::BindingResource::TextureView(&level(0)))],
        );
        let mip_groups = (1..MIPS)
            .map(|mip| {
                gpu::bind_group(
                    device,
                    "cloud detail noise mip",
                    &mip_layout,
                    &[
                        (0, wgpu::BindingResource::TextureView(&level(mip))),
                        (1, wgpu::BindingResource::TextureView(&level(mip - 1))),
                    ],
                )
            })
            .collect();
        let module = || {
            shaders::shader_module(
                device,
                shaders::DETAIL_NOISE_WGSL,
                shaders::DETAIL_NOISE_PATH,
                &[],
            )
        };
        let fill_pipeline = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "cloud detail noise",
                &module(),
                "detail_main",
                &[Some(&fill_layout)],
            )
        });
        let mip_pipeline = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "cloud detail noise mip",
                &module(),
                "mip_main",
                &[Some(&mip_layout)],
            )
        });
        Self {
            fill_layout,
            mip_layout,
            fill_group,
            mip_groups,
            fill_pipeline,
            mip_pipeline,
            filled: false,
        }
    }

    /// The fill's pipelines on the renderer's registry (hot reload), for [`DetailFill::adopt`].
    pub fn register(&self, ctx: &mut RenderContext<'_>) -> Result<DetailPipelines, ShaderError> {
        Ok(DetailPipelines {
            fill: ctx.add_pipeline(
                "cloud detail noise",
                shaders::DETAIL_NOISE_PATH,
                Vec::new(),
                compute_recipe(vec![self.fill_layout.clone()], "detail_main"),
            )?,
            mip: ctx.add_pipeline(
                "cloud detail noise mip",
                shaders::DETAIL_NOISE_PATH,
                Vec::new(),
                compute_recipe(vec![self.mip_layout.clone()], "mip_main"),
            )?,
        })
    }

    /// Takes the registry's current pipelines; a rebuilt one fills the volume again at the next
    /// [`DetailFill::encode_if_stale`]. Returns whether either changed.
    pub fn adopt(&mut self, registry: &PipelineRegistry, ids: &DetailPipelines) -> bool {
        let mut changed = false;
        for (slot, id) in [
            (&mut self.fill_pipeline, ids.fill),
            (&mut self.mip_pipeline, ids.mip),
        ] {
            let new = registry.compute(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        if changed {
            self.filled = false;
        }
        changed
    }

    /// Fills the volume and its mips on `encoder` unless it holds the current pipelines' output already.
    pub fn encode_if_stale(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.filled {
            return;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud detail noise"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.fill_group, &[]);
            pass.set_pipeline(&self.fill_pipeline);
            let groups = workgroups(SIZE, 4);
            pass.dispatch_workgroups(groups, groups, groups);
        }
        // Each mip in a pass of its own: it reads the mip the pass before wrote.
        for (index, group) in self.mip_groups.iter().enumerate() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud detail noise mip"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, group, &[]);
            pass.set_pipeline(&self.mip_pipeline);
            let groups = workgroups(SIZE >> (index as u32 + 1), 4);
            pass.dispatch_workgroups(groups, groups, groups);
        }
        self.filled = true;
    }
}

/// `hash_u32` of `detail_noise.wgsl` (Chris Wellons's "lowbias32", public domain).
fn hash_u32(value: u32) -> u32 {
    let mut h = value;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

fn hash_cell(cell: [u32; 3], seed: u32) -> u32 {
    hash_u32(cell[0].wrapping_add(hash_u32(
        cell[1].wrapping_add(hash_u32(cell[2].wrapping_add(seed))),
    )))
}

fn wrap_cell(cell: [i32; 3], cells: i32) -> [u32; 3] {
    cell.map(|c| c.rem_euclid(cells) as u32)
}

fn alligator(point: [f32; 3], cells: i32, seed: u32) -> f32 {
    let scaled = point.map(|p| p * cells as f32);
    let base = scaled.map(f32::floor);
    let within = [
        scaled[0] - base[0],
        scaled[1] - base[1],
        scaled[2] - base[2],
    ];
    let origin = base.map(|b| b as i32);
    let (mut first, mut second) = (0.0f32, 0.0f32);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                let offset = [x, y, z];
                let cell = [origin[0] + x, origin[1] + y, origin[2] + z];
                let h = hash_cell(wrap_cell(cell, cells), seed);
                let place = [h, h >> 10, h >> 20].map(|bits| (bits & 1023) as f32 * (1.0 / 1024.0));
                let height = 0.5 + 0.5 * (hash_u32(h) >> 8) as f32 * (1.0 / 16_777_216.0);
                let mut squared = 0.0f32;
                for axis in 0..3 {
                    let d = offset[axis] as f32 + place[axis] - within[axis];
                    squared += d * d;
                }
                let reach = (1.0 - squared.sqrt()).clamp(0.0, 1.0);
                let bump = reach * reach * (3.0 - 2.0 * reach) * height;
                if bump > first {
                    second = first;
                    first = bump;
                } else {
                    second = second.max(bump);
                }
            }
        }
    }
    first - second
}

fn alligator_layers(point: [f32; 3], cells: i32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut weight = 1.0;
    for octave in 0..3u32 {
        total += weight * alligator(point, cells << octave, seed + 17 * octave);
        weight *= 0.5;
    }
    total / 1.75
}

fn value_gradient(point: [f32; 3], cells: i32, seed: u32) -> [f32; 3] {
    let scaled = point.map(|p| p * cells as f32);
    let base = scaled.map(f32::floor);
    let f = [
        scaled[0] - base[0],
        scaled[1] - base[1],
        scaled[2] - base[2],
    ];
    let origin = base.map(|b| b as i32);
    let u = f.map(|f| f * f * f * (f * (f * 6.0 - 15.0) + 10.0));
    let du = f.map(|f| 30.0 * f * f * (f * (f - 2.0) + 1.0));
    let corner = |i: i32| {
        let cell = [
            origin[0] + (i & 1),
            origin[1] + ((i >> 1) & 1),
            origin[2] + ((i >> 2) & 1),
        ];
        (hash_cell(wrap_cell(cell, cells), seed) >> 8) as f32 * (1.0 / 16_777_216.0)
    };
    let (a, b, c, d) = (corner(0), corner(1), corner(2), corner(3));
    let (e, g, h, k) = (corner(4), corner(5), corner(6), corner(7));
    let k1 = b - a;
    let k2 = c - a;
    let k3 = e - a;
    let k4 = a - b - c + d;
    let k5 = a - c - e + h;
    let k6 = a - b - e + g;
    let k7 = -a + b + c - d + e - g - h + k;
    let scale = cells as f32;
    [
        scale * du[0] * (k1 + k4 * u[1] + k6 * u[2] + k7 * u[1] * u[2]),
        scale * du[1] * (k2 + k5 * u[2] + k4 * u[0] + k7 * u[2] * u[0]),
        scale * du[2] * (k3 + k6 * u[0] + k5 * u[1] + k7 * u[0] * u[1]),
    ]
}

fn curl(point: [f32; 3], cells: i32, seed: u32) -> [f32; 3] {
    let a = value_gradient(point, cells, seed);
    let b = value_gradient(point, cells, seed + 101);
    let c = value_gradient(point, cells, seed + 202);
    let scale = 1.0 / cells as f32;
    [
        (c[1] - b[2]) * scale,
        (a[2] - c[0]) * scale,
        (b[0] - a[1]) * scale,
    ]
}

/// The four channels at a point of the tile (`detail_channels` of `detail_noise.wgsl`), each in [0, 1].
pub fn reference_channels(point: [f32; 3]) -> [f32; 4] {
    let swirled = |amounts: [(i32, u32, f32); 2]| {
        let mut at = point;
        for (cells, seed, amount) in amounts {
            let c = curl(point, cells, seed);
            for axis in 0..3 {
                at[axis] += c[axis] * amount;
            }
        }
        at.map(|v| v - v.floor())
    };
    let low = swirled([(3, 900, 0.045), (7, 950, 0.018)]);
    let high = swirled([(5, 990, 0.02), (13, 995, 0.008)]);
    [
        alligator_layers(low, 4, 100) * 1.55,
        alligator_layers(high, 12, 200) * 1.75,
        alligator_layers(point, 5, 300) * 2.1,
        alligator_layers(point, 14, 400) * 1.75,
    ]
    .map(|v| v.clamp(0.0, 1.0))
}

/// The texel at (x, y, z) of the volume's first mip as the GPU fill writes it, to float rounding.
pub fn reference_texel(x: u32, y: u32, z: u32) -> [u8; 4] {
    let point = [x, y, z].map(|v| (v as f32 + 0.5) / SIZE as f32);
    reference_channels(point).map(|v| (v * 255.0 + 0.5) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each channel fills its range: a mean near a third, bumps up to the top, creases down to the bottom.
    #[test]
    fn the_channels_fill_their_range() {
        let mut sums = [0.0f64; 4];
        let mut lows = [1.0f32; 4];
        let mut highs = [0.0f32; 4];
        let mut count = 0.0;
        for z in (0..SIZE).step_by(16) {
            for y in (0..SIZE).step_by(5) {
                for x in (0..SIZE).step_by(5) {
                    let point = [x, y, z].map(|v| (v as f32 + 0.5) / SIZE as f32);
                    let channels = reference_channels(point);
                    for (i, &v) in channels.iter().enumerate() {
                        sums[i] += f64::from(v);
                        lows[i] = lows[i].min(v);
                        highs[i] = highs[i].max(v);
                    }
                    count += 1.0;
                }
            }
        }
        for i in 0..4 {
            let mean = sums[i] / count;
            assert!((0.25..0.45).contains(&mean), "channel {i} mean {mean}");
            assert!(
                lows[i] < 0.05 && highs[i] > 0.85,
                "channel {i} spans {}..{}",
                lows[i],
                highs[i]
            );
        }
    }

    /// The noise tiles: a point and the same point a tile on are one.
    #[test]
    fn the_noise_tiles() {
        for point in [
            [0.013f32, 0.5, 0.25],
            [0.4, 0.007, 0.99],
            [0.77, 0.31, 0.002],
        ] {
            let here = reference_channels(point);
            for axis in 0..3 {
                let mut there = point;
                there[axis] -= 1.0;
                let wrapped = [
                    alligator_layers(there, 5, 300) * 2.1,
                    alligator_layers(there, 14, 400) * 1.75,
                ];
                assert!((wrapped[0].clamp(0.0, 1.0) - here[2]).abs() < 1e-3);
                assert!((wrapped[1].clamp(0.0, 1.0) - here[3]).abs() < 1e-3);
            }
        }
    }
}
