//! The clouds on the GPU: a held sky converges to the same frame every time and redraws after a scene edit, clouds
//! feed the environment ships and the sea read, and their shadow falls on the sea.

use std::borrow::Cow;

use anyhow::Result;
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_clouds::{GameSky, Targets};
use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer};
use naval_sky::{SkyQuality, SkyScene};
use wgpu::util::DeviceExt;

fn compose(source: &str, extra: &[(&str, &str)]) -> wgpu::naga::Module {
    let mut composer = naga_oil::compose::Composer::default();
    naval_sky::add_sky_modules(&mut composer).expect("sky modules");
    for (path, module) in naval_clouds::shaders::modules() {
        composer
            .add_composable_module(naga_oil::compose::ComposableModuleDescriptor {
                source: &module,
                file_path: &path,
                ..Default::default()
            })
            .expect("cloud module");
    }
    for (path, module) in extra {
        composer
            .add_composable_module(naga_oil::compose::ComposableModuleDescriptor {
                source: module,
                file_path: path,
                ..Default::default()
            })
            .expect("extra module");
    }
    composer
        .make_naga_module(naga_oil::compose::NagaModuleDescriptor {
            source,
            file_path: "probe.wgsl",
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)))
}

/// Runs `entry` of a compute module over `count` invocations with `input` at group 0 binding 0 (storage, read) and
/// returns `outputs` floats from binding 1; `groups` are bound at 1, 2, ….
fn run(
    gpu: &Gpu,
    module: wgpu::naga::Module,
    input: &[f32],
    outputs: usize,
    count: u32,
    groups: &[(&wgpu::BindGroupLayout, &wgpu::BindGroup)],
) -> Result<Vec<f32>> {
    let device = &gpu.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe"),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    });
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let io_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("probe io"),
        entries: &[storage(0, true), storage(1, false)],
    });
    let input_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probe input"),
        contents: bytemuck::cast_slice(input),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe output"),
        size: (outputs * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe read"),
        size: (outputs * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let io = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe io"),
        layout: &io_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buffer.as_entire_binding(),
            },
        ],
    });
    let mut layouts = vec![Some(&io_layout)];
    layouts.extend(groups.iter().map(|(layout, _)| Some(*layout)));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("probe"),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("probe"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &io, &[]);
        for (index, (_, group)) in groups.iter().enumerate() {
            pass.set_bind_group(index as u32 + 1, *group, &[]);
        }
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output_buffer, 0, &read, 0, (outputs * 4) as u64);
    gpu.queue.submit([encoder.finish()]);
    let slice = read.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let values = bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range()?).to_vec();
    read.unmap();
    Ok(values)
}

const SIZE: (u32, u32) = (320, 180);

/// A game sky at the port's noon, over the given clouds, held and marched until converged.
fn held_frame(
    gpu: &Gpu,
    sky: &mut GameSky,
    renderer: &mut Renderer,
    capture: &Capture,
    scene: &SkyScene,
) -> Result<Vec<u8>> {
    sky.apply(&gpu.queue, scene);
    sky.hold(Some(30.0));
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    for _ in 0..naval_clouds::HOLD_UPDATES + 4 {
        let mut layers: [&mut dyn SceneLayer; 1] = [&mut *sky];
        renderer.render(
            FrameInput {
                camera: &camera,
                time: FrameTime::fixed(0, 1.0 / 60.0),
            },
            &mut layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: SIZE.0,
                height: SIZE.1,
            },
        )?;
    }
    capture.read_rgba(gpu)
}

/// The North Atlantic's noon under takram's default coverage (0.3): scattered cumulus and blue sky.
fn noon() -> SkyScene {
    let mut scene = SkyScene::north_atlantic(55.0, 200.0);
    scene.clouds.coverage = 0.3;
    scene
}

#[test]
fn a_held_sky_converges_to_the_same_frame() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut frames = Vec::new();
    for _ in 0..2 {
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
        let mut sky = GameSky::new(
            &gpu.device,
            &gpu.queue,
            SkyQuality::High,
            Targets::default(),
        );
        frames.push(held_frame(
            &gpu,
            &mut sky,
            &mut renderer,
            &capture,
            &noon(),
        )?);
    }
    assert_eq!(frames[0], frames[1], "two held skies differ");
    // Fair-weather cumulus: white cloud and blue sky both in the frame.
    let pixels: Vec<[u8; 3]> = frames[0].chunks(4).map(|p| [p[0], p[1], p[2]]).collect();
    let white = pixels
        .iter()
        .filter(|p| p[0] > 170 && p[2] > 170 && (i32::from(p[2]) - i32::from(p[0])).abs() < 40)
        .count();
    // Blue by 30 over red: under the display's saturation 1 (since 2026-10-04) the noon sky's blue leads its red
    // by 20–60, no longer by more than 60.
    let blue = pixels
        .iter()
        .filter(|p| i32::from(p[2]) - i32::from(p[0]) > 30)
        .count();
    let share = |n: usize| n as f64 / pixels.len() as f64;
    assert!(
        share(white) > 0.05,
        "cloud covers only {:.3} of the frame",
        share(white)
    );
    assert!(
        share(blue) > 0.05,
        "blue sky covers only {:.3} of the frame",
        share(blue)
    );
    Ok(())
}

/// A converged held sky whose history a small scene edit drops marches again rather than showing no cloud.
#[test]
fn a_held_sky_redraws_after_a_scene_edit() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = GameSky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Medium,
        Targets::default(),
    );
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &noon())?;
    assert!(
        sky.clouds.held_updates() >= naval_clouds::HOLD_UPDATES,
        "converged"
    );
    let mut edited = noon();
    edited.clouds.coverage += 0.01;
    sky.apply(&gpu.queue, &edited);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
    renderer.render(
        FrameInput {
            camera: &camera,
            time: FrameTime::fixed(0, 1.0 / 60.0),
        },
        &mut layers,
        RenderOutput {
            view: &capture.view,
            format: CAPTURE_FORMAT,
            width: SIZE.0,
            height: SIZE.1,
        },
    )?;
    let pixels = capture.read_rgba(&gpu)?;
    let white = pixels
        .chunks(4)
        .filter(|p| p[0] > 170 && p[2] > 170 && (i32::from(p[2]) - i32::from(p[0])).abs() < 40)
        .count();
    assert!(white * 20 > pixels.len() / 4, "no cloud after the edit");
    assert!(
        sky.clouds.held_updates() < naval_clouds::HOLD_UPDATES,
        "the held view restarted"
    );
    Ok(())
}

/// A fresh sky's first frame marches every pixel (a capture's picture is whole); one started in play
/// (`GameSky::start_in_play`, a theatre's scene swap) spreads the march over its updates as after a cut in play, and its
/// clouds come in all the same.
#[test]
fn a_sky_started_in_play_does_not_march_its_first_frame_whole() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let mut whole = Vec::new();
    let mut white = 0.0;
    for in_play in [false, true] {
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
        let mut sky = GameSky::new(
            &gpu.device,
            &gpu.queue,
            SkyQuality::Medium,
            Targets::default(),
        );
        sky.apply(&gpu.queue, &noon());
        if in_play {
            sky.start_in_play();
        }
        let mut marched = Vec::new();
        for frame in 0..24 {
            let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
            renderer.render(
                FrameInput {
                    camera: &camera,
                    time: FrameTime::fixed(frame, 1.0 / 60.0),
                },
                &mut layers,
                RenderOutput {
                    view: &capture.view,
                    format: CAPTURE_FORMAT,
                    width: SIZE.0,
                    height: SIZE.1,
                },
            )?;
            marched.push(sky.clouds.marched_whole());
        }
        whole.push(marched);
        if in_play {
            let pixels = capture.read_rgba(&gpu)?;
            let count = pixels
                .chunks(4)
                .filter(|p| {
                    p[0] > 170 && p[2] > 170 && (i32::from(p[2]) - i32::from(p[0])).abs() < 40
                })
                .count();
            white = count as f64 / (pixels.len() / 4) as f64;
        }
    }
    assert!(whole[0][0], "a fresh sky's first frame marches every pixel");
    assert!(
        whole[0][1..].iter().all(|&w| !w),
        "only its first: {:?}",
        whole[0]
    );
    assert!(
        whole[1].iter().all(|&w| !w),
        "a sky started in play: {:?}",
        whole[1]
    );
    assert!(
        white > 0.05,
        "cloud covers only {white:.3} of the in-play frame"
    );
    Ok(())
}

/// A theatre's cruise scene at 4×: its sky's `time_scale` drifts the clouds four times as far over the same frames.
#[test]
fn a_sky_at_four_times_drifts_four_times_as_far() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let mut drifts = Vec::new();
    for scale in [1.0, 4.0] {
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
        let mut sky = GameSky::new(
            &gpu.device,
            &gpu.queue,
            SkyQuality::Medium,
            Targets::default(),
        );
        sky.apply(&gpu.queue, &noon());
        sky.time_scale = scale;
        let mut start = (Vec3::ZERO, 0.0);
        for frame in 0..7 {
            let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
            renderer.render(
                FrameInput {
                    camera: &camera,
                    time: FrameTime::fixed(frame, 1.0 / 60.0),
                },
                &mut layers,
                RenderOutput {
                    view: &capture.view,
                    format: CAPTURE_FORMAT,
                    width: SIZE.0,
                    height: SIZE.1,
                },
            )?;
            if frame == 0 {
                start = (sky.sky.wind_offset(), sky.sky.time());
            }
        }
        drifts.push((
            (sky.sky.wind_offset() - start.0).length(),
            sky.sky.time() - start.1,
        ));
    }
    // Six frames of 1/60 s after the first, in the scene's wind.
    let wind = noon().clouds.wind_speed as f32;
    for ((drift, time), scale) in drifts.into_iter().zip([1.0, 4.0]) {
        let seconds = 0.1 * scale;
        assert!(
            (drift - wind * seconds as f32).abs() < 1e-3,
            "drift at {scale}×: {drift} m"
        );
        assert!(
            (time - seconds).abs() < 1e-6,
            "sky time at {scale}×: {time} s"
        );
    }
    Ok(())
}

/// A theatre's scene swap: a sky built on its renderer's registry (as the client's scenes are) put on screen in place of
/// another of the same air takes that sky's atmosphere tables and computes none of its own, through its first frames
/// and the adoption of the registry's pipelines in them.
#[test]
fn a_swapped_in_sky_takes_the_tables_of_the_sky_it_replaces() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let built = || -> Result<(Renderer, GameSky)> {
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
        let mut sky = GameSky::new_in(
            &mut renderer.context(),
            SkyQuality::Medium,
            Targets::default(),
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        sky.apply(&gpu.queue, &noon());
        renderer
            .finish_pipelines()
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok((renderer, sky))
    };
    let frames = |renderer: &mut Renderer, sky: &mut GameSky, count: u64| -> Result<()> {
        for frame in 0..count {
            let mut layers: [&mut dyn SceneLayer; 1] = [&mut *sky];
            renderer.render(
                FrameInput {
                    camera: &camera,
                    time: FrameTime::fixed(frame, 1.0 / 60.0),
                },
                &mut layers,
                RenderOutput {
                    view: &capture.view,
                    format: CAPTURE_FORMAT,
                    width: SIZE.0,
                    height: SIZE.1,
                },
            )?;
        }
        Ok(())
    };
    let (mut old_renderer, mut old) = built()?;
    frames(&mut old_renderer, &mut old, 3)?;
    assert_eq!(
        old.sky.precomputes(),
        1,
        "the sky on screen computed its tables"
    );

    let (mut renderer, mut new) = built()?;
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    assert!(
        new.take_tables_from(&gpu.queue, &mut encoder, &old),
        "the same air"
    );
    gpu.queue.submit([encoder.finish()]);
    drop((old, old_renderer));
    new.start_in_play();
    frames(&mut renderer, &mut new, 3)?;
    assert_eq!(
        new.sky.precomputes(),
        0,
        "the swapped-in sky computed tables of its own"
    );
    Ok(())
}

/// takram's layers closed: full coverage.
fn deck() -> SkyScene {
    let mut scene = noon();
    scene.clouds.coverage = 1.0;
    scene
}

fn clear() -> SkyScene {
    let mut scene = noon();
    scene.clouds.coverage = 0.0;
    scene.clouds.horizon_coverage = 0.0;
    scene
}

/// The environment overhead (`env_radiance(up, roughness)` at roughness 0 and 1), and the cloud shadow at sea-level
/// points round the camera, read through the consumers' own modules.
fn probe(gpu: &Gpu, sky: &GameSky) -> Result<(Vec3, Vec3, f32, f32)> {
    let values = probe_values(gpu, sky)?;
    let shadows = &values[6..];
    let mean = shadows.iter().sum::<f32>() / shadows.len() as f32;
    let least = shadows.iter().copied().fold(1.0, f32::min);
    Ok((
        Vec3::new(values[0], values[1], values[2]),
        Vec3::new(values[3], values[4], values[5]),
        mean,
        least,
    ))
}

/// [`probe`]'s readings: the sharp and rough environment overhead (six values), then the shadow at each point.
fn probe_values(gpu: &Gpu, sky: &GameSky) -> Result<Vec<f32>> {
    let source = r#"
#import naval_sky::environment::{SkyEnvironment, env_radiance}
#import naval_clouds::shadow::{CloudShadow, cloud_shadow}
@group(0) @binding(0) var<storage, read> points: array<f32>;
@group(0) @binding(1) var<storage, read_write> results: array<f32>;
@group(1) @binding(0) var sky_atlas: texture_2d<f32>;
@group(1) @binding(1) var sky_sampler: sampler;
@group(1) @binding(2) var<uniform> sky_env: SkyEnvironment;
@group(2) @binding(0) var shadow_map: texture_2d<f32>;
@group(2) @binding(1) var shadow_sampler: sampler;
@group(2) @binding(2) var<uniform> shadow: CloudShadow;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    let n = arrayLength(&points) / 3u;
    if i >= n { return; }
    if i == 0u {
        let sharp = env_radiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0), 0.0);
        let rough = env_radiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0), 1.0);
        results[0] = sharp.x; results[1] = sharp.y; results[2] = sharp.z;
        results[3] = rough.x; results[4] = rough.y; results[5] = rough.z;
    }
    let p = vec3<f32>(points[i * 3u], points[i * 3u + 1u], points[i * 3u + 2u]);
    results[6u + i] = cloud_shadow(shadow_map, shadow_sampler, shadow, p);
}
"#;
    // The sky's modules (environment included) are registered by `compose`; the shadow module is the consumers'.
    let module = compose(
        source,
        &[(
            "naval-clouds/shaders/shadow.wgsl",
            naval_clouds::shaders::SHADOW_WGSL,
        )],
    );
    let mut points = Vec::new();
    for i in 0..64 {
        let angle = i as f32 * 0.7;
        let radius = 200.0 + i as f32 * 90.0;
        points.extend([angle.cos() * radius, 0.0, angle.sin() * radius]);
    }
    let env = sky.sky.environment();
    let shadow = sky.clouds.shadow();
    run(
        gpu,
        module,
        &points,
        6 + 64,
        64,
        &[
            (&env.bind_group_layout, &env.bind_group),
            (&shadow.bind_group_layout, &shadow.bind_group),
        ],
    )
}

#[test]
fn clouds_feed_the_environment_and_shade_the_sea() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = GameSky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Medium,
        Targets::default(),
    );
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &clear())?;
    let (clear_sharp, clear_rough, clear_shadow, _) = probe(&gpu, &sky)?;
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &deck())?;
    let (deck_sharp, deck_rough, deck_shadow, deck_least) = probe(&gpu, &sky)?;
    let chroma = |c: Vec3| (c.z - c.x) / c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-6);
    // A clear sky is blue overhead; under a closed deck the environment turns grey, overhead and as ambient light.
    // (Not darker: takram lights a deck's base with the clear sky's light, which the deck does not occlude, so a deck
    // overhead is about as bright as the blue it hides.)
    assert!(chroma(clear_sharp) > 0.3, "clear zenith {clear_sharp}");
    assert!(
        chroma(deck_sharp) < chroma(clear_sharp) * 0.5,
        "deck zenith {deck_sharp} vs clear {clear_sharp}"
    );
    assert!(
        chroma(deck_rough) < chroma(clear_rough) * 0.75,
        "deck ambient {deck_rough} vs clear {clear_rough}"
    );
    // No cloud, no shadow; under the deck the beam is stopped (no floor: the light there is the sky's).
    assert!(
        (clear_shadow - 1.0).abs() < 1e-3,
        "clear shadow {clear_shadow}"
    );
    assert!(
        deck_shadow < 0.5 && deck_least >= 0.0,
        "deck shadow mean {deck_shadow}, least {deck_least}"
    );
    Ok(())
}

/// After dark the cloud shadow stops the moon's light, the scene's light then: a closed deck under a full moon shades
/// the sea (and the ships and land that read the same map), a clear night does not.
#[test]
fn clouds_shade_the_moon_at_night() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = GameSky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Medium,
        Targets::default(),
    );
    // The sun 28° down, so a full moon stands 28° up opposite it.
    let night = |coverage: f64| {
        let mut scene = SkyScene::north_atlantic(-28.0, 200.0);
        scene.clouds.coverage = coverage;
        if coverage == 0.0 {
            scene.clouds.horizon_coverage = 0.0;
        }
        scene
    };
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &night(0.0))?;
    assert!(sky.sky.light().night, "the moon is the light");
    let (_, _, clear_shadow, _) = probe(&gpu, &sky)?;
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &night(1.0))?;
    let (_, _, deck_shadow, deck_least) = probe(&gpu, &sky)?;
    assert!(
        (clear_shadow - 1.0).abs() < 1e-3,
        "clear night shadow {clear_shadow}"
    );
    assert!(
        deck_shadow < 0.5 && deck_least >= 0.0,
        "night deck shadow mean {deck_shadow}, least {deck_least}"
    );
    Ok(())
}

/// The environment (the hulls' and the sea's light) under the overcast's deck on a full-moon night is the same on every
/// tier, and darker than a clear night's: the bake takes a deck's light where it comes from, just past its base, as the
/// march on screen draws it (`clouds_bake`, `light_depth`). Lit at the middle of the bake's long steps, most of the way
/// up the deck, its ambient on Medium (16 steps) was 1.7 times Ultra's (32), and before the night sky's glow (2026-10-12)
/// brighter than a clear night.
#[test]
fn an_overcast_nights_ambient_is_the_same_on_every_tier() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let night = |coverage: f64| {
        let mut scene = SkyScene::north_atlantic(-28.0, 200.0);
        scene.clouds.coverage = coverage;
        if coverage == 0.0 {
            scene.clouds.horizon_coverage = 0.0;
        }
        scene
    };
    let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let mut ambient = Vec::new();
    for quality in [SkyQuality::Medium, SkyQuality::Ultra] {
        let mut sky = GameSky::new(&gpu.device, &gpu.queue, quality, Targets::default());
        held_frame(&gpu, &mut sky, &mut renderer, &capture, &night(0.0))?;
        let (_, clear, _, _) = probe(&gpu, &sky)?;
        // The overcast weather's cover: a stratocumulus deck.
        held_frame(&gpu, &mut sky, &mut renderer, &capture, &night(0.82))?;
        let (_, deck, _, _) = probe(&gpu, &sky)?;
        eprintln!(
            "{quality:?}: clear {} deck {}",
            luminance(clear),
            luminance(deck)
        );
        assert!(
            luminance(deck) < luminance(clear),
            "{quality:?}: deck ambient {deck} vs clear {clear}"
        );
        ambient.push(luminance(deck));
    }
    let (coarse, fine) = (ambient[0], ambient[1]);
    assert!(
        (coarse / fine - 1.0).abs() < 0.15,
        "the deck's ambient by 16 steps {coarse} and by 32 {fine}"
    );
    Ok(())
}

/// A held night that has converged under a new moon (the sun the light, below the horizon: no shadow) and then gets a
/// full moon with nothing else changed (no cut) marches the map toward the moon at once.
#[test]
fn a_held_night_shades_a_new_moonlight() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = GameSky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Medium,
        Targets::default(),
    );
    let mut scene = SkyScene::north_atlantic(-28.0, 200.0);
    scene.clouds.coverage = 1.0;
    scene.moon_phase = 0.0;
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &scene)?;
    assert!(
        !sky.sky.light().night,
        "a new moon leaves the sun the light"
    );
    let (_, _, dark_shadow, _) = probe(&gpu, &sky)?;
    assert!(
        (dark_shadow - 1.0).abs() < 1e-3,
        "no shadow from a sun below the horizon: {dark_shadow}"
    );
    // The full moon alone: no hold (which cuts), a few frames.
    scene.moon_phase = 0.5;
    sky.apply(&gpu.queue, &scene);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    for _ in 0..4 {
        let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
        renderer.render(
            FrameInput {
                camera: &camera,
                time: FrameTime::fixed(0, 1.0 / 60.0),
            },
            &mut layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: SIZE.0,
                height: SIZE.1,
            },
        )?;
    }
    assert!(sky.sky.light().night, "the full moon is the light");
    let (_, _, moon_shadow, _) = probe(&gpu, &sky)?;
    assert!(moon_shadow < 0.5, "the deck shades the moon: {moon_shadow}");
    Ok(())
}

/// A held night that has converged under one moon and then gets a later phase (the moon moved, still the light, no
/// cut) shades the sea as a sky held at that phase from the start does.
#[test]
fn a_held_night_follows_a_moved_moon() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 900.0, -3000.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let night = |phase: f64| {
        let mut scene = SkyScene::north_atlantic(-28.0, 200.0);
        scene.clouds.coverage = 0.5;
        scene.moon_phase = phase;
        scene
    };
    let new_sky = || {
        GameSky::new(
            &gpu.device,
            &gpu.queue,
            SkyQuality::Medium,
            Targets::default(),
        )
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = new_sky();
    held_frame(&gpu, &mut sky, &mut renderer, &capture, &night(0.5))?;
    let before = probe_values(&gpu, &sky)?;
    sky.apply(&gpu.queue, &night(0.62));
    for _ in 0..4 {
        let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
        renderer.render(
            FrameInput {
                camera: &camera,
                time: FrameTime::fixed(0, 1.0 / 60.0),
            },
            &mut layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: SIZE.0,
                height: SIZE.1,
            },
        )?;
    }
    assert!(sky.sky.light().night, "the moon is still the light");
    let moved = probe_values(&gpu, &sky)?;
    let mut fresh = new_sky();
    held_frame(&gpu, &mut fresh, &mut renderer, &capture, &night(0.62))?;
    let expected = probe_values(&gpu, &fresh)?;
    let largest = |a: &[f32], b: &[f32]| {
        a[6..]
            .iter()
            .zip(&b[6..])
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    };
    assert!(
        largest(&before, &expected) > 0.1,
        "the moved moon shades other points"
    );
    assert!(
        largest(&moved, &expected) < 0.02,
        "the map follows the moved moon: {}",
        largest(&moved, &expected)
    );
    Ok(())
}

/// A paused picture is play's picture, converged: from the air, 350 m over a cumulus, the clouds a paused frame settles
/// on are the ones play drew the moment before. Until 2026-10-08 a standing picture read a sharper near field there,
/// and pausing turned a near cumulus into a mottled web of dark creases.
#[test]
fn a_paused_picture_from_the_air_is_plays() -> Result<()> {
    let gpu = Gpu::headless()?;
    let capture = Capture::new(&gpu, SIZE.0, SIZE.1);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, SIZE.0, SIZE.1, CAPTURE_FORMAT)?;
    let mut sky = GameSky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Medium,
        Targets::default(),
    );
    let mut scene = noon();
    scene.clouds.coverage = 0.6;
    sky.apply(&gpu.queue, &scene);
    sky.hold(None);
    // Play whose clouds stand (no drift, no evolution), so the two pictures are of the same clouds.
    sky.time_scale = 0.0;
    let eye = Vec3::new(0.0, 1750.0, 0.0);
    let camera = Camera {
        eye,
        target: eye + Vec3::new(0.0, -1000.0, -360.0),
        fov_y_degrees: 60.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let mut frame = |sky: &mut GameSky, time: FrameTime| -> Result<()> {
        let mut layers: [&mut dyn SceneLayer; 1] = [&mut *sky];
        renderer.render(
            FrameInput {
                camera: &camera,
                time,
            },
            &mut layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: SIZE.0,
                height: SIZE.1,
            },
        )?;
        Ok(())
    };
    // In play until every pixel holds its 16 samples.
    let played = 300;
    for index in 1..=played {
        frame(&mut sky, FrameTime::fixed(index, 1.0 / 60.0))?;
    }
    let play = capture.read_rgba(&gpu)?;
    // Paused: new frames, the clock standing, until the picture stands.
    let seconds = FrameTime::fixed(played, 1.0 / 60.0).seconds;
    for index in 1..=u64::from(naval_clouds::HOLD_UPDATES) + 16 {
        frame(
            &mut sky,
            FrameTime {
                seconds,
                dt: 0.0,
                frame: played + index,
            },
        )?;
    }
    let paused = capture.read_rgba(&gpu)?;

    let luminance =
        |p: &[u8]| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
    let pixels = play.len() / 4;
    let cloudy = play.chunks(4).filter(|p| luminance(p) > 150.0).count() as f32 / pixels as f32;
    let difference = play
        .chunks(4)
        .zip(paused.chunks(4))
        .map(|(a, b)| (luminance(a) - luminance(b)).abs())
        .sum::<f32>()
        / pixels as f32;
    assert!(cloudy > 0.3, "the view looks down on a cumulus: {cloudy}");
    // 0.9 levels of the samples' own noise; the near field drew 14.
    assert!(
        difference < 3.0,
        "a paused picture keeps play's clouds: {difference} levels"
    );
    Ok(())
}
