//! Offscreen stills of the native ocean beside the web game's (`native/docs/ocean.md`).
//!
//! `cargo run -p naval-ocean --example stills --release [-- --only bridge9,glint9] [--frames 120]`
//!
//! For every scene the web driver wrote to `.build/native/ocean/web/` (`bun native/crates/naval-ocean/web/ocean-stills.ts`)
//! it reads the camera, wind, sun and the web sky's equirectangular bake, fills the stand-in environment atlas from that
//! bake, freezes the waves as the driver did (foam wiped at t = 50 − 1/30, 300 updates to t = 60, then a paused frame),
//! and renders the frame through the renderer core (`naval_render::Renderer`): the ocean and the stand-in sky are its
//! scene layers, its display pass applies the game's grade and AgX. It writes `.build/native/ocean/native/<scene>.png`,
//! a web | native pair in `.build/native/ocean/side-by-side/<scene>.png`, a difference image in `diff/` and the numbers
//! in `compare.json`. Without web data it draws the same poses under a gradient sky.
//! `--frames N` also times N frames: the field update's compute pass with GPU timestamps (when the adapter has them) and
//! serialised wall clock (encode, submit and wait) of whole frames, paused frames (the surface and the sky alone) and
//! field-only submissions, written to `.build/native/ocean/timings.json`. Render-pass timestamps read zero on Apple's
//! tiled GPUs, so the surface's cost is the wall-clock difference.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu, build_dir};
use naval_ocean::backdrop::StandInSky;
use naval_ocean::calibration::{NORTH_ATLANTIC, scene_parameters};
use naval_ocean::environment::{
    CelestialLight, CubeUvLayout, Equirect, StandInEnvironment, gradient_sky,
};
use naval_ocean::{Ocean, OceanParameters, QualityTier};
use naval_render::{
    Camera, DisplaySettings, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer,
};
use serde_json::Value;

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;

struct Scene {
    name: String,
    position: Vec3,
    target: Vec3,
    fov: f32,
    near: f32,
    far: f32,
    wind: f64,
    light: CelestialLight,
    sky: Option<(u32, u32, Vec<f32>)>,
    json: Option<Value>,
}

fn vec3(value: &Value) -> Vec3 {
    Vec3::new(
        value[0].as_f64().unwrap_or(0.0) as f32,
        value[1].as_f64().unwrap_or(0.0) as f32,
        value[2].as_f64().unwrap_or(0.0) as f32,
    )
}

/// The web driver's scenes, or its poses under a gradient sky when it has not run.
fn scenes(web: &Path, only: &Option<Vec<String>>) -> Result<Vec<Scene>> {
    let mut names: Vec<String> = std::fs::read_dir(web)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .filter_map(|n| n.strip_suffix(".json").map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let noon = CelestialLight {
        direction: Vec3::new(0.0, 0.939_692_6, -0.342_020_1),
        color: Vec3::new(1.0, 0.947, 0.844),
        intensity: 6.6,
        night: false,
    };
    let mut scenes = Vec::new();
    if names.is_empty() {
        for (wind, suffix) in [(9.0, "9"), (15.0, "15")] {
            for (pose, position, target) in [
                (
                    "bridge",
                    Vec3::new(0.0, 30.0, 2500.0),
                    Vec3::new(0.0, -22.0, 4500.0),
                ),
                (
                    "chase",
                    Vec3::new(0.0, 60.0, 2500.0),
                    Vec3::new(0.0, -240.0, 4600.0),
                ),
                (
                    "overhead",
                    Vec3::new(0.0, 1200.0, 2000.0),
                    Vec3::new(0.0, 0.0, 3400.0),
                ),
            ] {
                scenes.push(Scene {
                    name: format!("{pose}{suffix}"),
                    position,
                    target,
                    fov: 55.0,
                    near: 0.5,
                    far: 60000.0,
                    wind,
                    light: noon,
                    sky: None,
                    json: None,
                });
            }
        }
    }
    for name in names {
        let json: Value =
            serde_json::from_str(&std::fs::read_to_string(web.join(format!("{name}.json")))?)?;
        let camera = &json["camera"];
        let sun = &json["sun"];
        let sky = &json["sky"];
        let (width, height) = (
            sky["width"].as_u64().unwrap_or(0) as u32,
            sky["height"].as_u64().unwrap_or(0) as u32,
        );
        let bytes = std::fs::read(web.join(format!("{name}-sky.bin")))
            .with_context(|| format!("{name}-sky.bin"))?;
        let floats: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        scenes.push(Scene {
            name: name.clone(),
            position: vec3(&camera["position"]),
            target: vec3(&camera["target"]),
            fov: camera["fov"].as_f64().unwrap_or(55.0) as f32,
            near: camera["near"].as_f64().unwrap_or(0.5) as f32,
            far: camera["far"].as_f64().unwrap_or(60000.0) as f32,
            wind: json["wind"].as_f64().unwrap_or(9.0),
            light: CelestialLight {
                direction: vec3(&sun["direction"]).normalize(),
                color: vec3(&sun["color"]),
                intensity: sun["intensity"].as_f64().unwrap_or(6.6) as f32,
                night: false,
            },
            sky: Some((width, height, floats)),
            json: Some(json),
        });
    }
    if let Some(only) = only {
        scenes.retain(|s| only.contains(&s.name));
    }
    Ok(scenes)
}

/// Where the native parameters differ from what the web page reported for the same scene.
fn compare_parameters(scene: &Scene, parameters: &OceanParameters) {
    let Some(json) = &scene.json else { return };
    let waves = &json["waves"];
    let checks = [
        (
            "significantHeight",
            parameters.waves.significant_height,
            waves["significantHeight"].as_f64(),
        ),
        (
            "windSpeed",
            parameters.waves.wind_speed,
            waves["windSpeed"].as_f64(),
        ),
        (
            "windDirection",
            parameters.waves.wind_direction,
            waves["windDirection"].as_f64(),
        ),
        (
            "peakWavelength",
            parameters.waves.peak_wavelength,
            waves["peakWavelength"].as_f64(),
        ),
        (
            "choppiness",
            parameters.waves.choppiness,
            waves["choppiness"].as_f64(),
        ),
        ("seed", parameters.waves.seed, waves["seed"].as_f64()),
        (
            "windrow coverage",
            parameters.foam.surface.coverage,
            json["foam"]["surface"]["coverage"].as_f64(),
        ),
        (
            "fog end",
            f64::from(parameters.fog.end),
            json["fog"]["end"].as_f64(),
        ),
    ];
    for (name, ours, theirs) in checks {
        if let Some(theirs) = theirs
            && (ours - theirs).abs() > 1e-6 * theirs.abs().max(1.0)
        {
            eprintln!(
                "  {}: {name} is {ours} here, {theirs} on the web",
                scene.name
            );
        }
    }
}

/// A headless device, with GPU timestamps when the adapter has them.
fn device() -> Result<(Gpu, bool)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .context("no GPU adapter")?;
    let wanted = wgpu::Features::TIMESTAMP_QUERY;
    let timestamps = adapter.features().contains(wanted);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ocean stills"),
        required_features: if timestamps {
            wanted
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }))?;
    Ok((
        Gpu {
            instance,
            adapter,
            device,
            queue,
        },
        timestamps,
    ))
}

/// One frame through the renderer core at `time` with step `dt` (0: paused).
fn render(
    renderer: &mut Renderer,
    ocean: &mut Ocean,
    sky: &mut StandInSky,
    camera: &Camera,
    capture: &Capture,
    frame: u64,
    dt: f32,
) -> Result<()> {
    let layers: &mut [&mut dyn SceneLayer] = &mut [ocean, sky];
    renderer
        .render(
            FrameInput {
                camera,
                time: FrameTime {
                    seconds: frame as f64 / 60.0,
                    dt,
                    frame,
                },
            },
            layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width: WIDTH,
                height: HEIGHT,
            },
        )
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    let value_of = |flag: &str| {
        arguments
            .iter()
            .position(|a| a == flag)
            .and_then(|i| arguments.get(i + 1))
            .cloned()
    };
    let only =
        value_of("--only").map(|list| list.split(',').map(str::to_owned).collect::<Vec<_>>());
    let frames: u32 = value_of("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let root = build_dir("ocean");
    let web = root.join("web");
    let (gpu, has_timestamps) = device()?;
    let info = gpu.adapter.get_info();
    println!(
        "adapter: {} ({:?}), timestamps: {has_timestamps}",
        info.name, info.backend
    );

    let gradient = gradient_sky(
        384,
        192,
        Vec3::new(0.18, 0.3, 0.6),
        Vec3::new(0.55, 0.62, 0.7),
        Vec3::new(0.01, 0.02, 0.03),
    );
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, WIDTH, HEIGHT, CAPTURE_FORMAT)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let camera_layout = renderer.context().camera_layout.clone();
    let capture = Capture::new(&gpu, WIDTH, HEIGHT);
    let tier = QualityTier::High;
    let mut ocean: Option<Ocean> = None;
    let mut timings = Vec::new();
    let mut comparisons = Vec::new();

    for scene in scenes(&web, &only)? {
        println!("{}", scene.name);
        let (width, height, rgba) = match &scene.sky {
            Some((width, height, rgba)) => (*width, *height, rgba.as_slice()),
            None => (384, 192, gradient.as_slice()),
        };
        let environment = StandInEnvironment::new(
            &gpu.device,
            &gpu.queue,
            CubeUvLayout::from_bake_width(width),
            &Equirect {
                width,
                height,
                rgba,
            },
            &scene.light,
        );
        let ocean = match &mut ocean {
            Some(ocean) => {
                ocean.set_environment(&environment.bind_group);
                ocean
            }
            None => ocean.insert(Ocean::new(
                &gpu.device,
                &gpu.queue,
                tier,
                &camera_layout,
                &environment.bind_group_layout,
                &environment.bind_group,
                f64::from(scene.far),
            )),
        };
        let mut sky = StandInSky::new(
            &gpu.device,
            &camera_layout,
            &environment.bind_group_layout,
            &environment.bind_group,
        );
        ocean.parameters = scene_parameters(&NORTH_ATLANTIC, scene.wind, false);
        compare_parameters(&scene, &ocean.parameters);
        let camera = Camera {
            eye: scene.position,
            target: scene.target,
            fov_y_degrees: scene.fov,
            near: scene.near,
            far: scene.far,
            curvature: naval_render::camera::EARTH_CURVATURE,
        };
        let exposure = scene
            .json
            .as_ref()
            .map(|j| {
                j["display"]["skyExposure"].as_f64().unwrap_or(1.0)
                    * j["display"]["eyeAdaptation"]["factor"]
                        .as_f64()
                        .unwrap_or(1.0)
            })
            .unwrap_or(1.0) as f32;
        renderer.set_display_settings(DisplaySettings {
            exposure,
            tone_mapping_exposure: 1.0,
        });

        // Freeze as the web driver did: foam wiped, ten seconds of waves, then the paused frame at t = 60.
        let step = |ocean: &mut Ocean, time: f64, dt: f64| {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            ocean
                .field
                .update(&gpu.queue, &mut encoder, &ocean.parameters, time, dt);
            gpu.queue.submit([encoder.finish()]);
        };
        step(ocean, 50.0 - 1.0 / 30.0, 1000.0);
        for i in 0..300 {
            step(ocean, 50.0 + f64::from(i) / 30.0, 1.0 / 30.0);
        }
        ocean.time = 60.0;
        render(&mut renderer, ocean, &mut sky, &camera, &capture, 0, 0.0)?;
        let native_path = root.join("native").join(format!("{}.png", scene.name));
        capture.save_png(&gpu, &native_path)?;
        let web_path = web.join(format!("{}.png", scene.name));
        if web_path.exists() {
            side_by_side(
                &web_path,
                &native_path,
                &root
                    .join("side-by-side")
                    .join(format!("{}.png", scene.name)),
            )?;
            let result = compare(
                &web_path,
                &native_path,
                &scene,
                &root.join("diff").join(format!("{}.png", scene.name)),
            )?;
            println!("  {result}");
            comparisons.push(result);
        }
        if frames > 0 {
            timings.push(time_frames(
                &gpu,
                has_timestamps,
                &mut renderer,
                ocean,
                &mut sky,
                &camera,
                &capture,
                frames,
                &scene.name,
            )?);
        }
    }
    if !comparisons.is_empty() {
        std::fs::write(
            root.join("compare.json"),
            serde_json::to_string_pretty(&comparisons)?,
        )?;
    }
    if !timings.is_empty() {
        let path = root.join("timings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "adapter": info.name,
                "backend": format!("{:?}", info.backend),
                "size": [WIDTH, HEIGHT],
                "tier": "high",
                "scenes": timings,
            }))?,
        )?;
        println!("timings in {}", path.display());
    }
    println!("stills in {}", root.display());
    Ok(())
}

/// How far the native still is from the web's below the horizon (the sea; the sky is a stand-in): each side's mean
/// sRGB colour, the mean absolute difference per channel, and the share of pixels more than 24 levels apart. Writes a
/// difference image (red where native is brighter, blue where darker, ×4) to `diff`.
fn compare(web: &Path, native: &Path, scene: &Scene, diff: &Path) -> Result<Value> {
    let web = image::open(web)?.to_rgb8();
    let native = image::open(native)?.to_rgb8();
    let (width, height) = web.dimensions();
    let direction = scene.target - scene.position;
    let pitch = direction.y.atan2(direction.x.hypot(direction.z));
    let focal = 1.0 / (scene.fov.to_radians() / 2.0).tan();
    let horizon = ((height as f32 / 2.0 + pitch.tan() * focal * height as f32 / 2.0) as i64 + 3)
        .clamp(0, i64::from(height) - 1) as u32;
    let mut sums = [[0f64; 3]; 2];
    let (mut absolute, mut far_apart, mut count) = (0f64, 0u64, 0u64);
    let mut picture = image::RgbImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let a = web.get_pixel(x, y).0;
            let b = native.get_pixel(x, y).0;
            let signed = (0..3)
                .map(|c| f32::from(b[c]) - f32::from(a[c]))
                .sum::<f32>()
                / 3.0;
            picture.put_pixel(
                x,
                y,
                image::Rgb([
                    (signed * 4.0).clamp(0.0, 255.0) as u8,
                    signed.abs().min(255.0) as u8,
                    (-signed * 4.0).clamp(0.0, 255.0) as u8,
                ]),
            );
            if y < horizon {
                continue;
            }
            let mut largest = 0u8;
            for c in 0..3 {
                sums[0][c] += f64::from(a[c]);
                sums[1][c] += f64::from(b[c]);
                let d = a[c].abs_diff(b[c]);
                absolute += f64::from(d);
                largest = largest.max(d);
            }
            far_apart += u64::from(largest > 24);
            count += 1;
        }
    }
    if let Some(parent) = diff.parent() {
        std::fs::create_dir_all(parent)?;
    }
    picture.save(diff)?;
    let n = count.max(1) as f64;
    let mean = |side: usize| sums[side].map(|s| (s / n * 10.0).round() / 10.0);
    Ok(serde_json::json!({
        "scene": scene.name,
        "seaFromRow": horizon,
        "webMeanSrgb": mean(0),
        "nativeMeanSrgb": mean(1),
        "meanAbsoluteDifference": (absolute / (n * 3.0) * 100.0).round() / 100.0,
        "shareOver24Levels": (far_apart as f64 / n * 1e4).round() / 1e4,
    }))
}

/// Web on the left, native on the right, with a thin gap.
fn side_by_side(web: &Path, native: &Path, out: &Path) -> Result<()> {
    let left = image::open(web)?.to_rgba8();
    let right = image::open(native)?.to_rgba8();
    let gap = 8;
    let height = left.height().max(right.height());
    let mut canvas = image::RgbaImage::from_pixel(
        left.width() + gap + right.width(),
        height,
        image::Rgba([255, 255, 255, 255]),
    );
    image::imageops::overlay(&mut canvas, &left, 0, 0);
    image::imageops::overlay(&mut canvas, &right, i64::from(left.width() + gap), 0);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    canvas.save(out)?;
    Ok(())
}

/// GPU time of the field update (compute timestamps) and serialised wall-clock frames: whole frames through the
/// renderer, paused frames (the surface and the sky alone) and field-only submissions.
#[allow(clippy::too_many_arguments)]
fn time_frames(
    gpu: &Gpu,
    has_timestamps: bool,
    renderer: &mut Renderer,
    ocean: &mut Ocean,
    sky: &mut StandInSky,
    camera: &Camera,
    capture: &Capture,
    frames: u32,
    name: &str,
) -> Result<Value> {
    let queries = has_timestamps.then(|| {
        (
            gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("ocean timings"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ocean timings resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ocean timings read"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        )
    });
    let period = f64::from(gpu.queue.get_timestamp_period());
    let median = |values: &mut Vec<f64>| {
        if values.is_empty() {
            return None;
        }
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(values[values.len() / 2])
    };
    let wait = || gpu.device.poll(wgpu::PollType::wait_indefinitely());
    // Whole frames (the field steps 1/60 s) and paused frames (it encodes nothing).
    let (mut whole, mut paused) = (Vec::new(), Vec::new());
    for (dt, wall) in [(1.0 / 60.0, &mut whole), (0.0, &mut paused)] {
        for i in 0..frames + 10 {
            let started = Instant::now();
            render(renderer, ocean, sky, camera, capture, u64::from(i), dt)?;
            wait()?;
            if i >= 10 {
                wall.push(started.elapsed().as_secs_f64() * 1e3);
            }
        }
    }
    // The field alone, with GPU timestamps around its compute pass.
    let (mut field_wall, mut field_gpu) = (Vec::new(), Vec::new());
    for i in 0..frames + 10 {
        let started = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        ocean.time += 1.0 / 60.0;
        ocean.field.update_timed(
            &gpu.queue,
            &mut encoder,
            &ocean.parameters,
            ocean.time,
            1.0 / 60.0,
            queries.as_ref().map(|(set, _, _)| (set, 0, 1)),
        );
        if let Some((set, resolve, read)) = &queries {
            encoder.resolve_query_set(set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        }
        gpu.queue.submit([encoder.finish()]);
        wait()?;
        if i < 10 {
            continue;
        }
        field_wall.push(started.elapsed().as_secs_f64() * 1e3);
        if let Some((_, _, read)) = &queries {
            let slice = read.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            wait()?;
            let data = slice
                .get_mapped_range()
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let ticks: Vec<u64> = data
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| u64::from_le_bytes(*b))
                .collect();
            drop(data);
            read.unmap();
            field_gpu.push(ticks[1].saturating_sub(ticks[0]) as f64 * period / 1e6);
        }
    }
    let result = serde_json::json!({
        "scene": name,
        "frames": frames,
        "fieldGpuMs": median(&mut field_gpu),
        "serialisedFrameMs": median(&mut whole),
        "serialisedPausedFrameMs": median(&mut paused),
        "serialisedFieldOnlyMs": median(&mut field_wall),
        "triangles": ocean.surface.triangles(),
    });
    println!("  {result}");
    Ok(result)
}
