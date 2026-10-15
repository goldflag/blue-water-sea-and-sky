//! Before/after stills of the sky for the takram atmosphere port (native only): the dome from the bridge and from
//! aircraft altitudes over a day, a night and a hazy map, with radiance probes and the scene light.
//!
//!     cargo run -p naval-sky --example takram --release -- <tag>
//!
//! Writes `.build/native/sky/takram/<tag>/<scene>-<view>.png` and `probes.json` (sky radiance along fixed
//! directions from each viewpoint, the celestial light and the sea-level light per scene).

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_sky::diagnostics::Diagnostics;
use naval_sky::display::DisplayTransform;
use naval_sky::{Sky, SkyQuality, SkyScene, SkyTargets, SkyView};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SIZE: (u32, u32) = (960, 540);
const SUN_AZIMUTH: f64 = 270.0;

/// A named scene: the North Atlantic at a sun elevation, a night, the overcast and fog weathers' air
/// (`assets/maps/battle-conditions.v1.json`, the aureole at the client's 0.45 of it), the port.
fn scenes() -> Vec<(&'static str, SkyScene)> {
    let atlantic = |elevation| SkyScene::north_atlantic(elevation, SUN_AZIMUTH).cloudless();
    let mut night = atlantic(-30.0);
    night.moon_phase = 0.5;
    let mut crescent = atlantic(-30.0);
    crescent.moon_phase = 0.12;
    let mut haze = atlantic(30.0);
    haze.atmosphere.turbidity = 3.8;
    haze.atmosphere.mie = 0.5 * 0.45;
    haze.atmosphere.multiple = 1.4;
    let mut fog = atlantic(15.0);
    fog.atmosphere.turbidity = 6.0;
    fog.atmosphere.mie = 0.8 * 0.45;
    fog.atmosphere.multiple = 1.5;
    vec![
        ("noon", atlantic(60.0)),
        ("golden", atlantic(20.0)),
        ("low", atlantic(5.0)),
        ("sunset", atlantic(0.0)),
        ("twilight", atlantic(-4.0)),
        ("blue", atlantic(-8.0)),
        ("night", night),
        ("crescent", crescent),
        ("haze", haze),
        ("fog", fog),
        ("port", naval_sky::DEFAULT_SKY_SCENE.cloudless()),
    ]
}

/// A viewpoint: height (m), facing relative to the sun (degrees), pitch (degrees), vertical field (degrees).
struct Viewpoint {
    name: &'static str,
    height: f32,
    turn: f32,
    pitch: f32,
    fov: f32,
}

const VIEWS: [Viewpoint; 10] = [
    Viewpoint {
        name: "bridge-sun",
        height: 30.0,
        turn: 0.0,
        pitch: 8.0,
        fov: 52.0,
    },
    Viewpoint {
        name: "bridge-away",
        height: 30.0,
        turn: 180.0,
        pitch: 8.0,
        fov: 52.0,
    },
    Viewpoint {
        name: "bridge-side",
        height: 30.0,
        turn: 90.0,
        pitch: 25.0,
        fov: 70.0,
    },
    Viewpoint {
        name: "zenith",
        height: 30.0,
        turn: 0.0,
        pitch: 89.0,
        fov: 70.0,
    },
    Viewpoint {
        name: "air900-sun",
        height: 900.0,
        turn: 0.0,
        pitch: 0.0,
        fov: 60.0,
    },
    Viewpoint {
        name: "air2600-side",
        height: 2600.0,
        turn: 90.0,
        pitch: -2.0,
        fov: 60.0,
    },
    Viewpoint {
        name: "air5000-sun",
        height: 5000.0,
        turn: 0.0,
        pitch: -3.0,
        fov: 60.0,
    },
    Viewpoint {
        name: "air9000-away",
        height: 9000.0,
        turn: 180.0,
        pitch: -4.0,
        fov: 60.0,
    },
    Viewpoint {
        name: "dive",
        height: 4000.0,
        turn: 30.0,
        pitch: -40.0,
        fov: 60.0,
    },
    Viewpoint {
        name: "chart",
        height: 20000.0,
        turn: 0.0,
        pitch: -70.0,
        fov: 50.0,
    },
];

/// Probe directions (relative to the sun's azimuth): (name, elevation°, azimuth offset°).
const PROBES: [(&str, f32, f32); 7] = [
    ("zenith", 90.0, 0.0),
    ("sun10", 10.0, 0.0),
    ("horizonSun", 1.0, 0.0),
    ("horizonSide", 1.0, 90.0),
    ("horizonAway", 1.0, 180.0),
    ("away30", 30.0, 180.0),
    ("below", -5.0, 90.0),
];

fn heading(azimuth: f32, pitch: f32) -> Vec3 {
    let pitch = pitch.to_radians();
    Vec3::new(
        azimuth.sin() * pitch.cos(),
        pitch.sin(),
        azimuth.cos() * pitch.cos(),
    )
}

struct Frame {
    color: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Frame {
    fn new(gpu: &Gpu) -> Self {
        let size = wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 1,
        };
        let texture = |format, usage, label| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = texture(
            HDR,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "hdr",
        );
        let depth = texture(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            "depth",
        );
        Self {
            view: color.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            color,
        }
    }
}

fn still(
    gpu: &Gpu,
    sky: &mut Sky,
    display: &DisplayTransform,
    frame: &Frame,
    view: &SkyView,
    path: &Path,
) -> Result<()> {
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    sky.update(&gpu.queue, &mut encoder, view, 0.0);
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &frame.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &frame.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        sky.draw_dome(&mut pass);
    }
    gpu.queue.submit([encoder.finish()]);
    let capture = Capture::new(gpu, SIZE.0, SIZE.1);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    display.draw(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &frame.view,
        &capture.view,
        1.0,
        false,
    );
    gpu.queue.submit([encoder.finish()]);
    capture.save_png(gpu, path)?;
    let _ = &frame.color;
    Ok(())
}

fn read(gpu: &Gpu, buffer: &wgpu::Buffer) -> Result<Vec<[f32; 4]>> {
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("waiting for probes")?;
    receiver.recv()?.context("mapping probes")?;
    let data = slice.get_mapped_range().context("probe range")?;
    let values = bytemuck::cast_slice::<u8, [f32; 4]>(&data).to_vec();
    drop(data);
    buffer.unmap();
    Ok(values)
}

fn main() -> Result<()> {
    let tag = std::env::args().nth(1).unwrap_or_else(|| "now".to_owned());
    let gpu = Gpu::headless()?;
    let out = native_offscreen::build_dir("sky").join("takram").join(&tag);
    std::fs::create_dir_all(&out)?;
    let mut sky = Sky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::High,
        SkyTargets::default(),
    );
    let diagnostics = Diagnostics::new(&gpu.device, &sky, HDR);
    let display = DisplayTransform::new(&gpu.device, CAPTURE_FORMAT);
    let frame = Frame::new(&gpu);
    let mut json = String::from("{\n");
    for (scene_index, (name, scene)) in scenes().into_iter().enumerate() {
        sky.apply(&scene);
        let light = sky.light();
        let sea = sky.sea_level();
        let sun = sky.sun_direction();
        let facing = if scene.sun_elevation < -12.0 {
            sky.moon_direction()
        } else {
            sun
        };
        let azimuth = facing.x.atan2(facing.z);
        let _ = write!(
            json,
            "{}  \"{name}\": {{\n    \"light\": {{\"direction\": {:?}, \"color\": {:?}, \"intensity\": {:.4}, \"night\": {}}},\n    \"seaLevel\": {{\"sun\": [{:.4}, {:.4}, {:.4}], \"moon\": [{:.4}, {:.4}, {:.4}], \"sky\": [{:.4}, {:.4}, {:.4}]}},\n    \"probes\": {{",
            if scene_index > 0 { ",\n" } else { "" },
            light.direction.to_array(),
            light.color.to_array(),
            light.intensity,
            light.night,
            sea.sun.x,
            sea.sun.y,
            sea.sun.z,
            sea.moon.x,
            sea.moon.y,
            sea.moon.z,
            sea.sky.x,
            sea.sky.y,
            sea.sky.z,
        );
        for (view_index, viewpoint) in VIEWS.iter().enumerate() {
            let position = Vec3::new(0.0, viewpoint.height, 0.0);
            let forward = heading(azimuth + viewpoint.turn.to_radians(), viewpoint.pitch);
            let view = SkyView::looking_at(
                position,
                position + forward,
                viewpoint.fov,
                [SIZE.0, SIZE.1],
            );
            still(
                &gpu,
                &mut sky,
                &display,
                &frame,
                &view,
                &out.join(format!("{name}-{}.png", viewpoint.name)),
            )?;
            // Probes from this viewpoint (the camera's sections follow its altitude).
            let directions: Vec<[f32; 4]> = PROBES
                .iter()
                .map(|&(_, elevation, offset)| {
                    let d = heading(azimuth + offset.to_radians(), elevation);
                    [d.x, d.y, d.z, 0.0]
                })
                .collect();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let buffer = diagnostics.probe(&gpu.device, &mut encoder, &sky, &directions);
            gpu.queue.submit([encoder.finish()]);
            let values = read(&gpu, &buffer)?;
            let _ = write!(
                json,
                "{}\n      \"{}\": {{{}}}",
                if view_index > 0 { "," } else { "" },
                viewpoint.name,
                PROBES
                    .iter()
                    .zip(&values)
                    .map(|(&(probe, _, _), v)| format!(
                        "\"{probe}\": [{:.5}, {:.5}, {:.5}]",
                        v[0], v[1], v[2]
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        json.push_str("\n    }\n  }");
    }
    json.push_str("\n}\n");
    std::fs::write(out.join("probes.json"), &json)?;
    println!("wrote {}", out.display());
    Ok(())
}
