//! The coarser cascades' normals that drive sun glitter must not repeat at their FFT tile's edges. Exercise the real
//! field and the surface's sampler at negative world positions, with translations along both tile axes; keep the
//! finest cascade's samples and the coarser cascades' slope energy. Also check the finished sunlit surface.
use std::{borrow::Cow, sync::mpsc};

use native_offscreen::Gpu;
use naval_ocean::{
    QualityTier, WaveField,
    calibration::{NORTH_ATLANTIC, scene_parameters},
    shaders,
};

const SIDE: usize = 64;
const SAMPLES: usize = SIDE * SIDE;
const PROBE: &str = r#"
#import naval_ocean::waves::{waves, field_derivatives, field_sampler, scatter, copies_grad, mix_zero}
#import naval_ocean::noise::noise_hash
@group(0) @binding(0) var<storage, read_write> samples: array<vec4<f32>>;
@group(0) @binding(1) var<uniform> cascade: vec4<u32>;
fn slope_at(p: vec2<f32>, i: i32) -> vec2<f32> {
    let copies = scatter(p, i);
    let texel = 1.0 / waves.info.z;
    return mix_zero(copies_grad(field_derivatives, copies, i, vec2<f32>(texel, 0.0), vec2<f32>(0.0, texel)), copies).xy;
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = i32(cascade.x);
    let tile = waves.tile[i];
    let cell = vec2<i32>(i32(id.x % 64u), i32(id.x / 64u));
    // Jitter the regular lattice: without it, an eight-tile domain samples only 8 x 8 distinct raw UVs, giving a
    // biased energy reference for the longer waves.
    let jitter = vec2<f32>(noise_hash(cell.x, cell.y, 123u), noise_hash(cell.x, cell.y, 987u));
    let p = ((vec2<f32>(cell) + jitter) / 64.0 - 0.5) * (8.0 * tile)
        + vec2<f32>(123.4, -93.6);
    let slope = slope_at(p, i);
    let along = slope_at(p + vec2<f32>(tile, 0.0), i);
    let across = slope_at(p + vec2<f32>(0.0, tile), i);
    let raw = textureSampleLevel(field_derivatives, field_sampler, p / tile + 0.5 / waves.info.z, i, 0.0).xy;
    samples[2u * id.x] = vec4<f32>(slope, raw);
    samples[2u * id.x + 1u] = vec4<f32>(along, across);
}
"#;

fn correlation(a: &[[f32; 2]], b: &[[f32; 2]]) -> f64 {
    let mean = |s: &[[f32; 2]]| {
        s.iter().fold([0.0; 2], |sum, v| {
            [
                sum[0] + f64::from(v[0]) / SAMPLES as f64,
                sum[1] + f64::from(v[1]) / SAMPLES as f64,
            ]
        })
    };
    let (ma, mb) = (mean(a), mean(b));
    let (mut aa, mut bb, mut ab) = (0.0, 0.0, 0.0);
    for (a, b) in a.iter().zip(b) {
        for c in 0..2 {
            let (a, b) = (f64::from(a[c]) - ma[c], f64::from(b[c]) - mb[c]);
            aa += a * a;
            bb += b * b;
            ab += a * b;
        }
    }
    ab / (aa * bb).sqrt()
}

#[test]
fn coarse_wave_normals_do_not_repeat_and_keep_their_energy() {
    use wgpu::util::DeviceExt;
    let gpu = Gpu::headless().expect("a GPU adapter");
    let device = &gpu.device;
    let mut field = WaveField::new(device, &QualityTier::High.cascades());
    let mut parameters = scene_parameters(&NORTH_ATLANTIC, 9.0, false);
    parameters.gusts_enabled = false;
    let mut encoder = device.create_command_encoder(&Default::default());
    field.update(&gpu.queue, &mut encoder, &parameters, 123.456, 1.0 / 30.0);
    gpu.queue.submit([encoder.finish()]);
    let bytes = (SAMPLES * 2 * 16) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ocean tiling samples"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ocean tiling readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let cascade = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ocean tiling cascade"),
        contents: bytemuck::bytes_of(&[0u32; 4]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let own = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ocean tiling probe"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ocean tiling probe"),
        layout: &own,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cascade.as_entire_binding(),
            },
        ],
    });
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[],
    });
    let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &empty,
        entries: &[],
    });
    let module = shaders::naga_module(
        &mut shaders::composer().unwrap(),
        shaders::Entry {
            path: "ocean-tiling-probe.wgsl",
            source: PROBE,
        },
        &[],
    )
    .unwrap();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ocean tiling probe"),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ocean tiling probe"),
        bind_group_layouts: &[Some(&own), Some(&empty), Some(field.surface_layout())],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ocean tiling probe"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    for i in 0..=2u32 {
        gpu.queue
            .write_buffer(&cascade, 0, bytemuck::bytes_of(&[i, 0, 0, 0]));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &empty_group, &[]);
            pass.set_bind_group(2, field.surface_bind_group(), &[]);
            pass.dispatch_workgroups((SAMPLES as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
        gpu.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            sender.send(r).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        let rows: &[[f32; 8]] = bytemuck::cast_slice(&mapped);
        let channel = |offset| {
            rows.iter()
                .map(|r| [r[offset], r[offset + 1]])
                .collect::<Vec<_>>()
        };
        let (base, raw, along, across) = (channel(0), channel(2), channel(4), channel(6));
        let (x, z) = (correlation(&base, &along), correlation(&base, &across));
        let energy = |s: &[[f32; 2]]| {
            s.iter()
                .map(|v| f64::from(v[0]).powi(2) + f64::from(v[1]).powi(2))
                .sum::<f64>()
        };
        let energy_ratio = energy(&base) / energy(&raw);
        eprintln!("cascade {i}: tile correlations {x:.4}, {z:.4}; energy ratio {energy_ratio:.4}");
        if i < 2 {
            assert!(
                x.abs() < 0.3 && z.abs() < 0.3,
                "cascade {i} repeats: {x}, {z}"
            );
        } else {
            assert!(
                x > 0.999 && z > 0.999,
                "cascade {i} changed its sampling: {x}, {z}"
            );
        }
        assert!(
            (0.85..1.15).contains(&energy_ratio),
            "cascade {i} changed slope energy: {energy_ratio}"
        );
        drop(mapped);
        readback.unmap();
    }
}

/// Sample the finished, sunlit surface, including the horizontal Jacobian and filtered roughness. The old probe
/// sampled only the middle cascade's slopes: it passed while the larger cascade still repeated in the picture.
#[test]
fn sunlit_surface_does_not_repeat_its_coarse_fft_tiles() {
    use glam::Vec3;
    use native_offscreen::{CAPTURE_FORMAT, Capture};
    use naval_ocean::{
        Ocean,
        environment::{CelestialLight, CubeUvLayout, Equirect, StandInEnvironment, gradient_sky},
    };
    use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer};

    const WIDTH: u32 = 1024;
    // A narrow overhead lens gives nearly constant lighting and footprints over two 1024 m tiles. Each tile is
    // exactly 512 pixels across, so comparing both halves needs no perspective resampling.
    let gpu = Gpu::headless().expect("a GPU adapter");
    let sky = gradient_sky(
        256,
        128,
        Vec3::splat(0.3),
        Vec3::splat(0.3),
        Vec3::splat(0.03),
    );
    let environment = StandInEnvironment::new(
        &gpu.device,
        &gpu.queue,
        CubeUvLayout::from_bake_width(256),
        &Equirect {
            width: 256,
            height: 128,
            rgba: &sky,
        },
        &CelestialLight {
            direction: Vec3::new(0.1, 1.0, -0.1).normalize(),
            color: Vec3::ONE,
            intensity: 6.0,
            night: false,
        },
    );
    let mut renderer =
        Renderer::new(&gpu.device, &gpu.queue, WIDTH, WIDTH, CAPTURE_FORMAT).unwrap();
    let mut ocean = Ocean::new(
        &gpu.device,
        &gpu.queue,
        QualityTier::High,
        renderer.context().camera_layout,
        &environment.bind_group_layout,
        &environment.bind_group,
        60000.0,
    );
    ocean.parameters = scene_parameters(&NORTH_ATLANTIC, 9.0, false);
    ocean.parameters.gusts_enabled = false;
    ocean.surface.fog = false;
    ocean.time = 123.456;
    let camera = Camera {
        eye: Vec3::new(0.0, 20000.0, 0.001),
        target: Vec3::ZERO,
        fov_y_degrees: (2.0 * (1024.0f32 / 20000.0).atan()).to_degrees(),
        near: 0.5,
        far: 60000.0,
        curvature: 0.0,
    };
    let capture = Capture::new(&gpu, WIDTH, WIDTH);
    let layers: &mut [&mut dyn SceneLayer] = &mut [&mut ocean];
    renderer
        .render(
            FrameInput {
                camera: &camera,
                time: FrameTime::fixed(0, 0.0),
            },
            layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: WIDTH,
                height: WIDTH,
            },
        )
        .unwrap();
    let pixels = capture.read_rgba(&gpu).unwrap();
    // Remove only the slow change of illumination across the image; keep the waves' local contrast.
    let n = WIDTH as usize;
    let mut summed = vec![0.0; (n + 1) * (n + 1)];
    let luma: Vec<f64> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .collect();
    for y in 0..n {
        for x in 0..n {
            let i = (y + 1) * (n + 1) + x + 1;
            summed[i] = luma[y * n + x] + summed[i - 1] + summed[i - n - 1] - summed[i - n - 2];
        }
    }
    let contrast: Vec<f64> = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let (x0, y0) = (x.saturating_sub(16), y.saturating_sub(16));
            let (x1, y1) = ((x + 17).min(n), (y + 17).min(n));
            let sum =
                summed[y1 * (n + 1) + x1] - summed[y0 * (n + 1) + x1] - summed[y1 * (n + 1) + x0]
                    + summed[y0 * (n + 1) + x0];
            luma[i] - sum / ((x1 - x0) * (y1 - y0)) as f64
        })
        .collect();
    for (tile, name, dx, dy) in [
        (1024.0, "x", 512.0f64, 0.0f64),
        (1024.0, "z", 0.0, 512.0),
        (181.0, "x", 90.5, 0.0),
        (181.0, "z", 0.0, 90.5),
    ] {
        let (mut aa, mut bb, mut ab) = (0.0, 0.0, 0.0);
        for y in 20..n - dy.ceil() as usize - 20 {
            for x in 20..n - dx.ceil() as usize - 20 {
                let (x1, y1) = (x + dx.floor() as usize, y + dy.floor() as usize);
                let (fx, fy) = (dx.fract(), dy.fract());
                let b = (contrast[y1 * n + x1] * (1.0 - fx) + contrast[y1 * n + x1 + 1] * fx)
                    * (1.0 - fy)
                    + (contrast[(y1 + 1) * n + x1] * (1.0 - fx)
                        + contrast[(y1 + 1) * n + x1 + 1] * fx)
                        * fy;
                let a = contrast[y * n + x];
                aa += a * a;
                bb += b * b;
                ab += a * b;
            }
        }
        let correlation = ab / (aa * bb).sqrt();
        eprintln!("sunlit surface: {tile} m {name} tile correlation {correlation:.4}");
        let pairs = ((n - dx.ceil() as usize - 40) * (n - dy.ceil() as usize - 40)) as f64;
        assert!(
            aa > pairs && bb > pairs,
            "sunlit surface lost its wave detail"
        );
        assert!(
            correlation.abs() < 0.15,
            "sunlit surface repeats at {tile} m along {name}: {correlation}"
        );
    }
}
