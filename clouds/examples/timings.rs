//! GPU cost of the clouds and weather per Graphics → Clouds tier (native only):
//!
//!     cargo run -p naval-clouds --example timings --release [-- --size 2400x1350] [--frames 120] [--only noon,air]
//!         [--tiers low,medium,high,ultra] [--save]
//!
//! For fixed scenes (the sea at noon, a sunset toward the sun, an overcast deck overhead, 2.6 km over the deck, a dive)
//! at every tier: the clouds' compute passes by GPU timestamps through the renderer core's timer (the shadow maps and
//! their temporal pass, the ground shadow slice, the march, the reconstruction, and the sky's environment pass, which
//! with clouds sweeps a band every fourth frame), medians over the frames; the first update after a cut, which marches
//! every pixel (`fullMarchMs`); and the whole sky's own work by wall clock, as the web's `benchmarkSky` measures it:
//! frames with the sky (its passes and draws) alternating with frames without it, each serialised to GPU idle, the
//! trimmed mean of the paired differences; and the scene pass that holds the sky phase (the dome and the clouds'
//! composite over it) by its timestamps, in play and on the cut frames (`sceneSkyCutMs`, where the composite's spatial
//! filter runs on every pixel). The march, the reconstruction and the sky pass also give their tenth percentiles
//! (`…P10Ms`): other processes' GPU work stretches a pass's span and never shortens it, so on a shared machine those are
//! the costs with the least contention. The clouds run live (not held), a sixtieth of a second per frame. The web's
//! budget table was taken at 1600 × 900 at the game's 1.5 pixel ratio, hence the default size. Writes
//! `.build/native/clouds/timings.json`, and with `--save` each scene's last frame beside it (`timings-<tier>-<scene>.png`).

use anyhow::Result;
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_clouds::{GameSky, Targets};
use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer};
use naval_sky::{SkyQuality, SkyScene};

/// A measured scene: the eye (m), its bearing from the sun's azimuth and pitch (degrees), the sun's elevation and
/// azimuth (degrees), the cloud cover, the rain and the fog banks (the weather's layers, `naval_clouds::weathers`).
struct Scene {
    name: &'static str,
    eye: [f32; 3],
    bearing: f32,
    pitch: f32,
    sun: (f64, f64),
    cover: f64,
    rain: f64,
    fog: f64,
}

const SCENES: [Scene; 9] = [
    Scene {
        name: "noon",
        eye: [0.0, 25.0, 0.0],
        bearing: 70.0,
        pitch: 9.0,
        sun: (55.0, 180.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    Scene {
        name: "sunset",
        eye: [0.0, 25.0, 0.0],
        bearing: 15.0,
        pitch: 10.0,
        sun: (6.0, 270.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    Scene {
        name: "overcast",
        eye: [0.0, 25.0, 0.0],
        bearing: 120.0,
        pitch: 35.0,
        sun: (55.0, 180.0),
        cover: 0.82,
        rain: 0.0,
        fog: 0.0,
    },
    Scene {
        name: "air",
        eye: [0.0, 2600.0, 0.0],
        bearing: 80.0,
        pitch: -4.0,
        sun: (55.0, 180.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    Scene {
        name: "dive",
        eye: [0.0, 4200.0, 0.0],
        bearing: 100.0,
        pitch: -50.0,
        sun: (55.0, 180.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    // An aircraft's views: 2.6 km looking down 20° over the layers, and 1,150 m among the cumulus with one a few
    // hundred metres ahead (the harness's `fly.ts` views, north being −z there).
    Scene {
        name: "above",
        eye: [0.0, 2600.0, 0.0],
        bearing: 0.0,
        pitch: -20.0,
        sun: (55.0, 180.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    Scene {
        name: "near",
        eye: [-50.0, 1150.0, -1540.0],
        bearing: 0.0,
        pitch: 0.0,
        sun: (55.0, 180.0),
        cover: 0.38,
        rain: 0.0,
        fog: 0.0,
    },
    // The storm weather's deck and rain from the sea, and the fog weather's banks.
    Scene {
        name: "storm",
        eye: [0.0, 25.0, 0.0],
        bearing: 120.0,
        pitch: 12.0,
        sun: (55.0, 180.0),
        cover: 0.94,
        rain: 0.9,
        fog: 0.0,
    },
    Scene {
        name: "fog",
        eye: [0.0, 25.0, 0.0],
        bearing: 60.0,
        pitch: 4.0,
        sun: (28.0, 115.0),
        cover: 0.65,
        rain: 0.0,
        fog: 1.0,
    },
];

fn device() -> Result<Gpu> {
    let base = Gpu::headless()?;
    let wanted = wgpu::Features::TIMESTAMP_QUERY;
    if !base.adapter.features().contains(wanted) {
        return Ok(base);
    }
    let (device, queue) =
        pollster::block_on(base.adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cloud timings"),
            required_features: wanted,
            ..Default::default()
        }))?;
    Ok(Gpu {
        instance: base.instance,
        adapter: base.adapter,
        device,
        queue,
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The tenth percentile: other processes' GPU work interleaved with a pass stretches its span, never shortens it, so
/// on a shared machine this is the pass's cost with the least contention.
fn p10(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 10]
}

/// Mean without the lowest and highest tenth.
fn trimmed(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let cut = values.len() / 10;
    let kept = &values[cut..values.len() - cut];
    kept.iter().sum::<f64>() / kept.len() as f64
}

/// The passes timed per frame, in the order the JSON reports them.
const PASSES: [&str; 6] = [
    "cloud shadow maps",
    "cloud shadow resolve",
    "cloud ground shadow",
    "cloud march",
    "cloud resolve",
    "sky environment",
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let size = option("--size").unwrap_or_else(|| "2400x1350".to_owned());
    let (width, height) = size
        .split_once('x')
        .map(|(w, h)| (w.parse().unwrap_or(2400), h.parse().unwrap_or(1350)))
        .unwrap_or((2400, 1350));
    let frames: usize = option("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let only: Option<Vec<String>> =
        option("--only").map(|list| list.split(',').map(str::to_owned).collect());
    let tiers: Option<Vec<String>> =
        option("--tiers").map(|list| list.split(',').map(str::to_lowercase).collect());
    let save = args.iter().any(|a| a == "--save");
    let build = native_offscreen::build_dir("clouds");
    std::fs::create_dir_all(&build)?;
    let gpu = device()?;
    let info = gpu.adapter.get_info();
    let timestamps = gpu
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY);
    println!(
        "adapter: {} ({:?}), timestamps: {timestamps}, {width} x {height}",
        info.name, info.backend
    );
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, width, height, CAPTURE_FORMAT)?;
    let capture = Capture::new(&gpu, width, height);
    let mut results = Vec::new();
    for quality in [
        SkyQuality::Low,
        SkyQuality::Medium,
        SkyQuality::High,
        SkyQuality::Ultra,
    ] {
        if tiers
            .as_ref()
            .is_some_and(|tiers| !tiers.contains(&format!("{quality:?}").to_lowercase()))
        {
            continue;
        }
        let mut sky = GameSky::new(&gpu.device, &gpu.queue, quality, Targets::default());
        for scene in SCENES.iter() {
            if only
                .as_ref()
                .is_some_and(|only| !only.iter().any(|n| n == scene.name))
            {
                continue;
            }
            let (elevation, azimuth) = scene.sun;
            let mut sky_scene = SkyScene::north_atlantic(elevation, azimuth);
            sky_scene.clouds.coverage = scene.cover;
            sky_scene.weather.precipitation = scene.rain;
            sky_scene.clouds.fog = scene.fog;
            sky.apply(&gpu.queue, &sky_scene);
            sky.hold(None);
            let bearing = (azimuth as f32 + scene.bearing).to_radians();
            let pitch = scene.pitch.to_radians();
            let direction = Vec3::new(
                bearing.sin() * pitch.cos(),
                pitch.sin(),
                bearing.cos() * pitch.cos(),
            );
            let eye = Vec3::from(scene.eye);
            let camera = Camera {
                eye,
                target: eye + direction * 1000.0,
                fov_y_degrees: 60.0,
                near: 0.5,
                far: 60_000.0,
                curvature: naval_render::camera::EARTH_CURVATURE,
            };
            let mut frame_index = 0u64;
            let (mut with_sky, mut without_sky, mut differences) =
                (Vec::new(), Vec::new(), Vec::new());
            // Frames' pass timings as the renderer's timer reads them back (stamped frames only).
            let mut timed: Vec<naval_render::FrameTimings> = Vec::new();
            // The 20 warm-up frames below are not timed.
            let mut seen = renderer.frames_submitted() + 20;
            // A frame drawn with time standing (as a capture is), for the cut's full march.
            let hold_time = std::cell::Cell::new(false);
            let mut render = |sky: &mut GameSky,
                              enabled: bool,
                              stamps: bool,
                              timed: &mut Vec<naval_render::FrameTimings>|
             -> Result<(f64, u64)> {
                sky.enabled = enabled;
                if !hold_time.get() {
                    frame_index += 1;
                }
                gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
                let start = std::time::Instant::now();
                let mut layers: [&mut dyn SceneLayer; 1] = [&mut *sky];
                renderer.render(
                    FrameInput {
                        camera: &camera,
                        time: FrameTime::fixed(frame_index, 1.0 / 60.0),
                    },
                    &mut layers,
                    RenderOutput {
                        view: &capture.view,
                        format: CAPTURE_FORMAT,
                        width,
                        height,
                    },
                )?;
                gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if stamps {
                    renderer.drain_gpu_timings();
                    for timings in renderer.gpu_timer().history() {
                        if timings.frame > seen {
                            seen = timings.frame;
                            timed.push(timings.clone());
                        }
                    }
                }
                Ok((elapsed, renderer.frames_submitted()))
            };
            // Warm up (pipelines, history).
            for _ in 0..20 {
                render(&mut sky, true, false, &mut timed)?;
            }
            // The compute passes by GPU timestamps, through the renderer core's timer: it resolves a frame's queries
            // in a later submission (render-core.md, "GPU timings"); a few extra frames bring the last ones in.
            let mut full = Vec::new();
            // The sky pass on those cut frames, whose composite filters every pixel (each holds one sample).
            let mut full_sky = Vec::new();
            if timestamps {
                for _ in 0..frames + 4 {
                    render(&mut sky, true, true, &mut timed)?;
                }
                // The first update after a cut on a frame that leaves time standing (a capture): every pixel marched. Each
                // cut frame's march, then frames to let the timer bring its span back.
                for _ in 0..8 {
                    sky.reset_history();
                    hold_time.set(true);
                    let (_, cut) = render(&mut sky, true, true, &mut timed)?;
                    hold_time.set(false);
                    for _ in 0..4 {
                        render(&mut sky, true, true, &mut timed)?;
                    }
                    // The timer numbers frames from 0: the frame just submitted is the count less one.
                    let cut_timings = timed.iter().find(|timings| timings.frame + 1 == cut);
                    if let Some(ms) = cut_timings
                        .and_then(|timings| {
                            timings.passes.iter().find(|p| p.label == "cloud march")
                        })
                        .and_then(|pass| pass.span_ms)
                    {
                        full.push(ms);
                    }
                    if let Some(ms) = cut_timings.and_then(|timings| {
                        timings
                            .passes
                            .iter()
                            .filter(|p| p.label.starts_with("scene ") && p.label.contains("sky"))
                            .map(|p| p.span_ms)
                            .sum::<Option<f64>>()
                    }) {
                        full_sky.push(ms);
                    }
                }
            }
            // The sky's own work (passes and draws), frames with and without it alternating.
            for i in 0..frames {
                let first = i % 2 == 0;
                let a = render(&mut sky, first, false, &mut timed)?.0;
                let b = render(&mut sky, !first, false, &mut timed)?.0;
                let (on, off) = if first { (a, b) } else { (b, a) };
                with_sky.push(on);
                without_sky.push(off);
                differences.push(on - off);
            }
            sky.enabled = true;
            if save {
                render(&mut sky, true, false, &mut timed)?;
                let name = format!("timings-{quality:?}-{}.png", scene.name).to_lowercase();
                capture.save_png(&gpu, &build.join(name))?;
            }
            // A pass counts on the frames it ran; the environment's sweep runs on its band frames, zero on the others,
            // so its mean is per frame. The stack is every cloud pass of a frame together.
            let mut passes: [Vec<f64>; 6] = Default::default();
            let mut stack = Vec::new();
            let mut scene_sky = Vec::new();
            // The steady frames only (the cut frames' full marches are reported apart).
            for timings in timed.iter().take(frames) {
                let span = |label: &str| {
                    timings
                        .passes
                        .iter()
                        .find(|p| p.label == label)
                        .and_then(|p| p.span_ms)
                };
                for (i, label) in PASSES.iter().enumerate().take(5) {
                    if let Some(ms) = span(label) {
                        passes[i].push(ms);
                    }
                }
                passes[5].push(span(PASSES[5]).unwrap_or(0.0));
                // The scene pass with the sky phase: the dome, then the composite.
                if let Some(ms) = timings
                    .passes
                    .iter()
                    .filter(|p| p.label.starts_with("scene ") && p.label.contains("sky"))
                    .map(|p| p.span_ms)
                    .sum::<Option<f64>>()
                {
                    scene_sky.push(ms);
                }
                stack.push(
                    PASSES[..5]
                        .iter()
                        .map(|label| span(label).unwrap_or(0.0))
                        .sum::<f64>(),
                );
            }
            // Other processes' GPU work interleaved with a pass stretches its span, never shortens it: the tenth
            // percentile is the cost with the least contention, comparable between runs on a shared machine.
            let mut sorted = stack.clone();
            sorted.sort_by(f64::total_cmp);
            let stack_p10 = sorted.get(sorted.len() / 10).copied().unwrap_or(0.0);
            let [
                shadow_maps,
                shadow_resolve,
                ground,
                march,
                resolve,
                environment,
            ] = passes;
            let line = format!(
                "{{\"tier\": \"{quality:?}\", \"scene\": \"{}\", \"shadowMapsMs\": {:.3}, \"shadowResolveMs\": {:.3}, \"groundMs\": {:.3}, \"marchMs\": {:.3}, \"resolveMs\": {:.3}, \"marchP10Ms\": {:.3}, \"resolveP10Ms\": {:.3}, \"cloudsMedianMs\": {:.3}, \"cloudsP10Ms\": {:.3}, \"fullMarchMs\": {:.3}, \"sceneSkyMs\": {:.3}, \"sceneSkyP10Ms\": {:.3}, \"sceneSkyCutMs\": {:.3}, \"environmentMeanMs\": {:.3}, \"skyWallMs\": {:.3}, \"frameWithSkyMs\": {:.3}, \"frameWithoutSkyMs\": {:.3}}}",
                scene.name,
                median(shadow_maps),
                median(shadow_resolve),
                median(ground),
                median(march.clone()),
                median(resolve.clone()),
                p10(march),
                p10(resolve),
                median(stack),
                stack_p10,
                median(full),
                median(scene_sky.clone()),
                p10(scene_sky),
                p10(full_sky),
                environment.iter().sum::<f64>() / environment.len().max(1) as f64,
                trimmed(differences),
                trimmed(with_sky),
                trimmed(without_sky),
            );
            println!("{line}");
            results.push(line);
        }
    }
    // The machine's load as the run ended (Linux's /proc, macOS's sysctl).
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .or_else(|| {
            std::process::Command::new("sysctl")
                .args(["-n", "vm.loadavg"])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        })
        .unwrap_or_default();
    let json = format!(
        "{{\n\"adapter\": \"{} ({:?})\", \"size\": [{width}, {height}], \"frames\": {frames}, \"load\": \"{}\",\n\"results\": [\n{}\n]\n}}\n",
        info.name,
        info.backend,
        load.trim(),
        results.join(",\n")
    );
    std::fs::write(build.join("timings.json"), json)?;
    println!("wrote {}", build.join("timings.json").display());
    Ok(())
}
