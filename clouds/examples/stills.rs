//! The clouds from fixed views, drawn through the renderer core with the sky alone (no sea or ships: the clouds stop
//! at the sea sphere), held and converged (native only):
//!
//!     cargo run -p naval-clouds --example stills --release [-- --only bridge,air2600] [--tier ultra]
//!         [--time morning|noon|evening|night] [--cover 0.4] [--frames 64] [--tag name] [--moving] [--hdr]
//!
//! Writes `.build/native/clouds/takram/<tag>/<view>.png`. `--moving` lets time run instead of holding the sky; `--hdr`
//! also writes the scene's linear radiance before the display grade (`<view>.rgbf`: f32 RGB, 1600 × 900, top row
//! first).

use anyhow::Result;
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_clouds::{GameSky, Targets};
use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer};
use naval_sky::{SkyQuality, SkyScene};

/// A view: eye (m), bearing from the sun's azimuth (degrees), pitch (degrees, + up), vertical field of view.
struct View {
    name: &'static str,
    eye: [f32; 3],
    bearing: f32,
    pitch: f32,
    fov: f32,
}

const VIEWS: [View; 12] = [
    View {
        name: "bridge",
        eye: [0.0, 25.0, 0.0],
        bearing: 70.0,
        pitch: 9.0,
        fov: 60.0,
    },
    View {
        name: "bridgeUp",
        eye: [0.0, 25.0, 0.0],
        bearing: 120.0,
        pitch: 35.0,
        fov: 60.0,
    },
    View {
        name: "bridgeSun",
        eye: [0.0, 25.0, 0.0],
        bearing: 15.0,
        pitch: 10.0,
        fov: 60.0,
    },
    View {
        name: "bridgeAway",
        eye: [0.0, 25.0, 0.0],
        bearing: 180.0,
        pitch: 9.0,
        fov: 60.0,
    },
    View {
        name: "air900",
        eye: [0.0, 900.0, 0.0],
        bearing: 70.0,
        pitch: 2.0,
        fov: 60.0,
    },
    View {
        name: "air1350",
        eye: [0.0, 1350.0, 0.0],
        bearing: 110.0,
        pitch: 0.0,
        fov: 60.0,
    },
    View {
        name: "air2600",
        eye: [0.0, 2600.0, 0.0],
        bearing: 80.0,
        pitch: -4.0,
        fov: 60.0,
    },
    View {
        name: "air2600Sun",
        eye: [0.0, 2600.0, 0.0],
        bearing: 20.0,
        pitch: -4.0,
        fov: 60.0,
    },
    View {
        name: "air5000",
        eye: [0.0, 5000.0, 0.0],
        bearing: 75.0,
        pitch: -10.0,
        fov: 60.0,
    },
    View {
        name: "air9000",
        eye: [0.0, 9000.0, 0.0],
        bearing: 90.0,
        pitch: -14.0,
        fov: 60.0,
    },
    View {
        name: "dive",
        eye: [0.0, 4200.0, 0.0],
        bearing: 100.0,
        pitch: -50.0,
        fov: 60.0,
    },
    View {
        name: "chart",
        eye: [0.0, 40000.0, 0.0],
        bearing: 0.0,
        pitch: -89.9,
        fov: 60.0,
    },
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let only: Option<Vec<String>> =
        option("--only").map(|list| list.split(',').map(str::to_owned).collect());
    let frames: u32 = option("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let cover: f64 = option("--cover")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.4);
    let moving = args.iter().any(|a| a == "--moving");
    let hdr = args.iter().any(|a| a == "--hdr");
    let bob = args.iter().any(|a| a == "--bob");
    let tier = match option("--tier").as_deref() {
        Some("low") => SkyQuality::Low,
        Some("medium") => SkyQuality::Medium,
        Some("high") => SkyQuality::High,
        _ => SkyQuality::Ultra,
    };
    let (elevation, azimuth) = match option("--time").as_deref() {
        Some("noon") => (55.0, 180.0),
        Some("evening") => (6.0, 270.0),
        Some("sunset") => (1.0, 270.0),
        Some("night") => (-20.0, 270.0),
        _ => (20.0, 100.0),
    };
    let tag = option("--tag").unwrap_or_else(|| "stills".into());
    let (width, height) = (1600, 900);
    let out = native_offscreen::build_dir("clouds")
        .join("takram")
        .join(&tag);
    std::fs::create_dir_all(&out)?;

    let gpu = Gpu::headless()?;
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, width, height, CAPTURE_FORMAT)?;
    let capture = Capture::new(&gpu, width, height);
    let mut sky = GameSky::new(&gpu.device, &gpu.queue, tier, Targets::default());
    let mut scene = SkyScene::north_atlantic(elevation, azimuth);
    scene.clouds.coverage = cover;
    sky.apply(&gpu.queue, &scene);

    for view in VIEWS.iter() {
        if only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|n| n == view.name))
        {
            continue;
        }
        if !moving && !bob {
            sky.hold(Some(30.0));
        }
        let bearing = (azimuth as f32 + view.bearing).to_radians();
        let pitch = view.pitch.to_radians();
        let direction = Vec3::new(
            bearing.sin() * pitch.cos(),
            pitch.sin(),
            bearing.cos() * pitch.cos(),
        );
        let eye = Vec3::from(view.eye);
        let camera = Camera {
            eye,
            target: eye + direction * 1000.0,
            fov_y_degrees: view.fov,
            near: 1.0,
            far: 200_000.0,
            curvature: naval_render::camera::EARTH_CURVATURE,
        };
        let started = std::time::Instant::now();
        for frame in 0..frames {
            let mut layers: [&mut dyn SceneLayer; 1] = [&mut sky];
            let time = if moving || bob {
                FrameTime::fixed(u64::from(frame), 1.0 / 60.0)
            } else {
                FrameTime::fixed(0, 1.0 / 60.0)
            };
            // A ship's chase camera: forward at 12 m/s, the view pitching ±1.5° over 8 s.
            let camera = if bob {
                let t = frame as f32 / 60.0;
                let swing = (1.5f32).to_radians() * (t * std::f32::consts::TAU / 8.0).sin();
                let pitched = Vec3::new(
                    bearing.sin() * (pitch + swing).cos(),
                    (pitch + swing).sin(),
                    bearing.cos() * (pitch + swing).cos(),
                );
                let ahead = Vec3::new(bearing.sin(), 0.0, bearing.cos()) * 12.0 * t;
                Camera {
                    eye: eye + ahead,
                    target: eye + ahead + pitched * 1000.0,
                    ..camera
                }
            } else {
                camera
            };
            renderer.render(
                FrameInput {
                    camera: &camera,
                    time,
                },
                &mut layers,
                RenderOutput {
                    view: &capture.view,
                    format: CAPTURE_FORMAT,
                    width,
                    height,
                },
            )?;
        }
        let path = out.join(format!("{}.png", view.name));
        capture.save_png(&gpu, &path)?;
        if hdr {
            let radiance = read_scene_hdr(&gpu, &renderer, width, height)?;
            let bytes: Vec<u8> = radiance.iter().flat_map(|v| v.to_le_bytes()).collect();
            std::fs::write(out.join(format!("{}.rgbf", view.name)), bytes)?;
        }
        println!(
            "{} ({:.1} s)",
            path.display(),
            started.elapsed().as_secs_f32()
        );
    }
    Ok(())
}

/// The scene's linear HDR radiance before the display grade (`scene.hdr`), as tightly packed f32 RGB, top row first.
fn read_scene_hdr(gpu: &Gpu, renderer: &Renderer, width: u32, height: u32) -> Result<Vec<f32>> {
    let hdr = renderer
        .resources()
        .texture(naval_render::SCENE_HDR)
        .expect("the scene's colour");
    let row = width * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hdr read-back"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hdr copy"),
        });
    encoder.copy_texture_to_buffer(
        hdr.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    let mapped = slice
        .get_mapped_range()
        .map_err(|e| anyhow::anyhow!("mapping the hdr read-back: {e:?}"))?;
    let mut out = Vec::with_capacity((width * height * 3) as usize);
    for chunk in mapped.chunks(padded as usize) {
        for texel in chunk[..row as usize].chunks(8) {
            for channel in 0..3 {
                out.push(half_to_f32(u16::from_le_bytes([
                    texel[channel * 2],
                    texel[channel * 2 + 1],
                ])));
            }
        }
    }
    drop(mapped);
    buffer.unmap();
    Ok(out)
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}
