//! Offscreen stills of the native sky and its GPU cost. They were made for comparison with the web; since the native
//! sky took takram's atmosphere (`native/docs/sky.md`) they no longer match it by design, and `examples/takram.rs`
//! compares the sky with the one it replaced.
//!
//!     cargo run -p naval-sky --example stills --release
//!
//! Writes to `.build/native/sky/native/`:
//! - `panorama-camera.png`, `panorama-sea.png`: the atmosphere alone with plain discs, as
//!   `scripts/diagnostics/sky-atmosphere.html` draws it (North Atlantic, sun at 60°, 20°, 5°, 0°, −6°), and
//!   `probes-camera.json`, `probes-sea.json` with the same page's radiance readings;
//! - `dome-<elevation>-sun.png` and `dome-<elevation>-away.png`: the dome as the game draws it (sky, sun
//!   disc, moon, stars, Milky Way), a 52° camera 30 m up facing the sun and facing away from it;
//! - `stars-savo.png`, `stars-atlantic.png`: the real stars over two places of the war (`Sky::set_place`), the
//!   camera on the Southern Cross from Savo on the evening of 8 August 1942 and on Orion from 50°N 30°W at midnight
//!   in January. `-- stars` after the command writes these two alone;
//! - `environment-<elevation>.png`: the environment through `env_radiance` at roughness 0, 0.25, 0.5, 0.75
//!   and 1 (rows; azimuth across, the sun in the middle), `atlas-<elevation>.png`, the atlas itself, and
//!   `atlas-<elevation>.f16`, its raw half floats for `tools/compare.py`;
//! - `timings.json`: GPU time of the tables' precompute (a change of the air), of an environment bake and prefilter,
//!   and of the dome at 1920 x 1080 by day and by night. A change of the sun runs no table pass (takram's tables hold
//!   every sun angle), so the `sunChange` tables figure is the query set's last reading, not a cost.

use std::path::Path;

use anyhow::{Context, Result};
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_sky::diagnostics::{Diagnostics, Tile};
use naval_sky::display::DisplayTransform;
use naval_sky::{Sky, SkyQuality, SkyScene, SkyTargets, SkyView};

const ELEVATIONS: [f64; 5] = [60.0, 20.0, 5.0, 0.0, -6.0];
/// The web page's sun azimuth (compass degrees).
const SUN_AZIMUTH: f64 = 270.0;
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const PANORAMA_TILE: (u32, u32) = (800, 190);

struct Frame {
    color: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Frame {
    fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            view: color.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            color,
            width,
            height,
        }
    }

    fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    fn save(&self, gpu: &Gpu, display: &DisplayTransform, graded: bool, path: &Path) -> Result<()> {
        let capture = Capture::new(gpu, self.width, self.height);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        display.draw(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &self.view,
            &capture.view,
            1.0,
            graded,
        );
        gpu.queue.submit([encoder.finish()]);
        capture.save_png(gpu, path)?;
        let _ = &self.color;
        Ok(())
    }
}

fn camera_view(width: u32, height: u32) -> SkyView {
    SkyView::looking_at(
        Vec3::new(0.0, 30.0, 0.0),
        Vec3::new(0.0, 30.0, -1.0),
        52.0,
        [width, height],
    )
}

/// The web page's probes: (name, elevation or None for 5° above the sun, azimuth offset from the sun).
const PROBES: [(&str, Option<f64>, f64); 8] = [
    ("zenith", Some(90.0), 0.0),
    ("sun+5", None, 0.0),
    ("horizon sun", Some(1.0), 0.0),
    ("horizon side", Some(1.0), 90.0),
    ("horizon away", Some(1.0), 180.0),
    ("away 20", Some(20.0), 180.0),
    ("away 45", Some(45.0), 180.0),
    ("below", Some(-5.0), 90.0),
];

fn read_probes(gpu: &Gpu, buffer: &wgpu::Buffer) -> Result<Vec<[f32; 4]>> {
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

fn panorama(
    gpu: &Gpu,
    sky: &mut Sky,
    diagnostics: &Diagnostics,
    display: &DisplayTransform,
    from_sea: bool,
    out: &Path,
) -> Result<String> {
    let (w, h) = PANORAMA_TILE;
    let rows = ELEVATIONS.len().div_ceil(2) as u32;
    let frame = Frame::new(gpu, w * 2, h * rows);
    let view = camera_view(w, h);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    frame.clear(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let mut readings = Vec::new();
    for (i, &elevation) in ELEVATIONS.iter().enumerate() {
        let scene = SkyScene::north_atlantic(elevation, SUN_AZIMUTH);
        sky.apply(&scene);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        sky.update(&gpu.queue, &mut encoder, &view, 0.0);
        let sun = sky.sun_direction();
        let azimuth = sun.x.atan2(sun.z);
        let tile = Tile {
            x: (i as u32 % 2) * w,
            y: (i as u32 / 2) * h,
            width: w,
            height: h,
        };
        diagnostics.panorama(
            &gpu.device,
            &mut encoder,
            sky,
            &frame.view,
            tile,
            azimuth,
            from_sea,
        );
        let directions: Vec<[f32; 4]> = PROBES
            .iter()
            .map(|&(_, e, offset)| {
                let e = e.unwrap_or(elevation.max(0.0) + 5.0).to_radians() as f32;
                let a = azimuth + (offset as f32).to_radians();
                [
                    a.sin() * e.cos(),
                    e.sin(),
                    a.cos() * e.cos(),
                    if from_sea { 1.0 } else { 0.0 },
                ]
            })
            .collect();
        let buffer = diagnostics.probe(&gpu.device, &mut encoder, sky, &directions);
        gpu.queue.submit([encoder.finish()]);
        let values = read_probes(gpu, &buffer)?;
        let sea = sky.sea_level();
        let probes: Vec<String> = PROBES
            .iter()
            .zip(&values)
            .map(|((name, _, _), v)| format!("\"{name}\": [{:.4}, {:.4}, {:.4}]", v[0], v[1], v[2]))
            .collect();
        readings.push(format!(
            "  {{\"elevation\": {elevation}, \"light\": {{\"sun\": [{:.3}, {:.3}, {:.3}], \"moon\": [{:.3}, {:.3}, {:.3}], \"sky\": [{:.3}, {:.3}, {:.3}]}}, \"probes\": {{{}}}}}",
            sea.sun.x, sea.sun.y, sea.sun.z, sea.moon.x, sea.moon.y, sea.moon.z, sea.sky.x, sea.sky.y, sea.sky.z,
            probes.join(", ")
        ));
    }
    let name = if from_sea { "sea" } else { "camera" };
    frame.save(
        gpu,
        display,
        true,
        &out.join(format!("panorama-{name}.png")),
    )?;
    Ok(format!("[\n{}\n]\n", readings.join(",\n")))
}

/// One still of the dome as the game draws it, from 30 m up, looking along `forward`.
fn dome_still(
    gpu: &Gpu,
    sky: &mut Sky,
    display: &DisplayTransform,
    forward: Vec3,
    fov: f32,
    path: &Path,
) -> Result<()> {
    let (w, h) = (960, 540);
    let frame = Frame::new(gpu, w, h);
    let position = Vec3::new(0.0, 30.0, 0.0);
    let view = SkyView::looking_at(position, position + forward, fov, [w, h]);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    sky.update(&gpu.queue, &mut encoder, &view, 0.0);
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
    frame.save(gpu, display, false, path)
}

/// A unit vector at compass `azimuth` (radians) and `pitch` (degrees) above the horizon.
fn heading(azimuth: f32, pitch: f32) -> Vec3 {
    let pitch = pitch.to_radians();
    Vec3::new(
        azimuth.sin() * pitch.cos(),
        pitch.sin(),
        azimuth.cos() * pitch.cos(),
    )
}

fn dome(gpu: &Gpu, sky: &mut Sky, display: &DisplayTransform, out: &Path) -> Result<()> {
    for &elevation in &ELEVATIONS {
        sky.apply(&SkyScene::north_atlantic(elevation, SUN_AZIMUTH));
        for (facing, turn) in [("sun", 0.0f32), ("away", 180.0)] {
            // The horizon a third of the way up the frame (pitched 8 degrees up).
            let sun = sky.sun_direction();
            let forward = heading(sun.x.atan2(sun.z) + turn.to_radians(), 8.0);
            let path = out.join(format!("dome-{elevation}-{facing}.png"));
            dome_still(gpu, sky, display, forward, 52.0, &path)?;
        }
    }
    // Deep night under a crescent: stars and the Milky Way, facing away from the set sun, 35 degrees up.
    let mut night = SkyScene::north_atlantic(-30.0, SUN_AZIMUTH);
    night.moon_phase = 0.08;
    sky.apply(&night);
    let sun = sky.sun_direction();
    let forward = heading(sun.x.atan2(sun.z) + std::f32::consts::PI, 35.0);
    dome_still(gpu, sky, display, forward, 52.0, &out.join("night-sky.png"))?;
    // The first-quarter moon through 30x glasses (the view of `sky-celestial.html`).
    let mut quarter = SkyScene::north_atlantic(-30.0, 250.0);
    quarter.moon_phase = 0.25;
    sky.apply(&quarter);
    let fov = (2.0 * (26f32.to_radians().tan() / 30.0).atan()).to_degrees();
    let moon = sky.moon_direction();
    dome_still(gpu, sky, display, moon, fov, &out.join("night-moon.png"))?;
    Ok(())
}

fn environment(
    gpu: &Gpu,
    sky: &mut Sky,
    diagnostics: &Diagnostics,
    display: &DisplayTransform,
    out: &Path,
) -> Result<()> {
    let roughness = [0.0, 0.25, 0.5, 0.75, 1.0];
    let (w, h) = (512, 256);
    let frame = Frame::new(gpu, w, h * roughness.len() as u32);
    let view = camera_view(w, h);
    for &elevation in &ELEVATIONS {
        sky.apply(&SkyScene::north_atlantic(elevation, SUN_AZIMUTH));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        sky.update(&gpu.queue, &mut encoder, &view, 0.0);
        frame.clear(&mut encoder);
        let sun = sky.sun_direction();
        let azimuth = sun.x.atan2(sun.z);
        for (row, &r) in roughness.iter().enumerate() {
            let tile = Tile {
                x: 0,
                y: row as u32 * h,
                width: w,
                height: h,
            };
            diagnostics.environment(
                &gpu.device,
                &mut encoder,
                sky.environment(),
                &frame.view,
                tile,
                azimuth,
                r,
            );
        }
        gpu.queue.submit([encoder.finish()]);
        frame.save(
            gpu,
            display,
            false,
            &out.join(format!("environment-{elevation}.png")),
        )?;

        let layout = sky.environment().layout;
        let atlas = Frame::new(gpu, layout.width * 2, layout.height * 2);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        atlas.clear(&mut encoder);
        let tile = Tile {
            x: 0,
            y: 0,
            width: atlas.width,
            height: atlas.height,
        };
        diagnostics.atlas(
            &gpu.device,
            &mut encoder,
            sky.environment(),
            &atlas.view,
            tile,
        );
        gpu.queue.submit([encoder.finish()]);
        atlas.save(
            gpu,
            display,
            false,
            &out.join(format!("atlas-{elevation}.png")),
        )?;
        write_atlas(
            gpu,
            sky.environment(),
            &out.join(format!("atlas-{elevation}.f16")),
        )?;
    }
    Ok(())
}

/// The atlas's raw half floats, row 0 first, for `tools/compare.py` to set against the web's.
fn write_atlas(gpu: &Gpu, environment: &naval_sky::EnvironmentMap, path: &Path) -> Result<()> {
    let layout = environment.layout;
    let row = layout.width * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("atlas read-back"),
        size: u64::from(padded * layout.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        environment.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(layout.height),
            },
        },
        environment.texture.size(),
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    let data = slice.get_mapped_range().context("atlas range")?;
    let mut bytes = Vec::with_capacity((row * layout.height) as usize);
    for chunk in data.chunks(padded as usize) {
        bytes.extend_from_slice(&chunk[..row as usize]);
    }
    drop(data);
    buffer.unmap();
    std::fs::write(path, bytes)?;
    Ok(())
}

/// GPU time (ms, median of `runs`) of what an update encodes after a change of the air (every table and the
/// environment), of the sun (the sky views, ambient and environment) and of nothing but the environment, from
/// the sky's own pass timestamps (`Sky::set_timestamps`) when the adapter has them, else by wall clock.
fn timings(gpu: &Gpu, sky: &mut Sky, runs: usize) -> Result<String> {
    let view = camera_view(960, 540);
    let timestamps = gpu
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY);
    let resolve = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sky timing resolve"),
        size: 32,
        usage: wgpu::BufferUsages::QUERY_RESOLVE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sky timing read-back"),
        size: 32,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let period = f64::from(gpu.queue.get_timestamp_period());
    // The query set, borrowed back from the sky only to resolve it.
    let query = timestamps.then(|| {
        gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("sky timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 4,
        })
    });
    if let Some(query) = &query {
        sky.set_timestamps(Some(query.clone()));
    }
    let mut measure = |change: &dyn Fn(&mut Sky, usize)| -> Result<(f64, f64)> {
        let mut tables = Vec::new();
        let mut environment = Vec::new();
        for run in 0..runs {
            change(sky, run);
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            encoder.clear_buffer(&resolve, 0, None);
            sky.update(&gpu.queue, &mut encoder, &view, 0.0);
            if let Some(query) = &query {
                encoder.resolve_query_set(query, 0..4, &resolve, 0);
                encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, 32);
            }
            gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
            let start = std::time::Instant::now();
            gpu.queue.submit([encoder.finish()]);
            gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
            let wall = start.elapsed().as_secs_f64() * 1000.0;
            if query.is_some() {
                let slice = readback.slice(..);
                slice.map_async(wgpu::MapMode::Read, |_| {});
                gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
                let ticks: [u64; 4] = {
                    let data = slice.get_mapped_range().context("timestamps")?;
                    let values = bytemuck::cast_slice::<u8, u64>(&data);
                    [values[0], values[1], values[2], values[3]]
                };
                readback.unmap();
                let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * period / 1e6;
                tables.push(ms(ticks[0], ticks[1]));
                environment.push(ms(ticks[2], ticks[3]));
            } else {
                tables.push(wall);
                environment.push(0.0);
            }
        }
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        Ok((median(tables), median(environment)))
    };
    let air = measure(&|sky, run| {
        let mut scene = SkyScene::north_atlantic(20.0, SUN_AZIMUTH);
        scene.atmosphere.turbidity = 2.2 + 0.01 * (run % 2) as f64;
        sky.apply(&scene);
    })?;
    let sun = measure(&|sky, run| {
        sky.apply(&SkyScene::north_atlantic(
            20.0 + 0.1 * (run % 2) as f64,
            SUN_AZIMUTH,
        ))
    })?;
    let environment = measure(&|sky, _| sky.refresh_environment())?;
    sky.set_timestamps(None);
    // JSON members, for `main` to gather into one object.
    if timestamps {
        Ok(format!(
            " \"update\": {{\"method\": \"GPU timestamps at the start and end of each of the update's compute passes, median of {runs}\",\n  \"airChange\": {{\"tablesMs\": {:.3}, \"environmentMs\": {:.3}}},\n  \"sunChange\": {{\"tablesMs\": {:.3}, \"environmentMs\": {:.3}}},\n  \"environmentOnly\": {{\"environmentMs\": {:.3}}}}}",
            air.0, air.1, sun.0, sun.1, environment.1
        ))
    } else {
        // Without timestamps `measure` holds the whole update's wall clock in its first element.
        Ok(format!(
            " \"update\": {{\"method\": \"wall clock of the whole update, submit to idle, median of {runs}\",\n  \"airChange\": {{\"updateMs\": {:.3}}},\n  \"sunChange\": {{\"updateMs\": {:.3}}},\n  \"environmentOnly\": {{\"updateMs\": {:.3}}}}}",
            air.0, sun.0, environment.0
        ))
    }
}

/// Time (ms) per dome pass at 1920 x 1080: `batch` passes in one submission, wall clock from submit to idle,
/// divided by the batch; median of `runs`. A daylight sky facing the sun, and a night sky facing the stars and
/// the Milky Way. (Render-pass timestamps read back zero on this Metal adapter.)
fn dome_timings(gpu: &Gpu, sky: &mut Sky, runs: usize) -> Result<String> {
    let (w, h) = (1920, 1080);
    let batch = 40;
    let frame = Frame::new(gpu, w, h);
    let mut measure = |scene: SkyScene, turn: f32, pitch: f32| -> Result<f64> {
        sky.apply(&scene);
        let sun = sky.sun_direction();
        let forward = heading(sun.x.atan2(sun.z) + turn, pitch);
        let position = Vec3::new(0.0, 30.0, 0.0);
        let view = SkyView::looking_at(position, position + forward, 52.0, [w, h]);
        // Settle the tables and environment first.
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        sky.update(&gpu.queue, &mut encoder, &view, 0.0);
        gpu.queue.submit([encoder.finish()]);
        gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        let mut samples = Vec::new();
        for _ in 0..runs {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            for _ in 0..batch {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("dome timing"),
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
            let start = std::time::Instant::now();
            gpu.queue.submit([encoder.finish()]);
            gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0 / batch as f64);
        }
        samples.sort_by(f64::total_cmp);
        Ok(samples[samples.len() / 2])
    };
    let day = measure(SkyScene::north_atlantic(20.0, SUN_AZIMUTH), 0.0, 8.0)?;
    let mut night_scene = SkyScene::north_atlantic(-30.0, SUN_AZIMUTH);
    night_scene.moon_phase = 0.08;
    let night = measure(night_scene, std::f32::consts::PI, 35.0)?;
    Ok(format!(
        " \"dome1920x1080\": {{\"method\": \"{batch} passes per submission, wall clock / {batch}, median of {runs}\", \"dayMs\": {day:.3}, \"nightMs\": {night:.3}}}"
    ))
}

/// A headless device with timestamp queries when the adapter has them.
fn device() -> Result<Gpu> {
    let base = Gpu::headless()?;
    let wanted = wgpu::Features::TIMESTAMP_QUERY;
    if !base.adapter.features().contains(wanted) {
        return Ok(base);
    }
    let (device, queue) =
        pollster::block_on(base.adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sky stills"),
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

/// The stars over two places of the war on a moonless night, the camera on a star a sailor there steered by: Acrux
/// from Savo Island at 20:00 on 8 August 1942, Betelgeuse from 50°N 30°W at midnight on 15 January 1942.
fn stars(gpu: &Gpu, sky: &mut Sky, display: &DisplayTransform, out: &Path) -> Result<()> {
    use naval_sky::state::{CelestialModel, Place};
    let mut night = SkyScene::north_atlantic(-40.0, SUN_AZIMUTH).cloudless();
    night.moon_phase = 0.0;
    sky.apply(&night);
    for (name, latitude, longitude, start, utc_offset, [ra, dec]) in [
        (
            "savo",
            -9.1,
            159.8,
            "1942-08-08T20:00",
            11.0,
            [186.65f64, -63.10f64],
        ),
        (
            "atlantic",
            50.0,
            -30.0,
            "1942-01-15T00:00",
            -2.0,
            [88.79, 7.41],
        ),
    ] {
        let sidereal = naval_sim::theatre::sun::sidereal_deg(start, utc_offset, 0.0, longitude)
            .context("the still's start")?;
        let place = Some(Place { latitude, sidereal });
        sky.set_place(place);
        // Where the star stands there: the same frame the sky draws by.
        let mut model = CelestialModel::default();
        model.set_place(place);
        let (ra, dec) = (ra.to_radians(), dec.to_radians());
        let star = glam::DVec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        let forward = (model.star_rotation.transpose() * star).as_vec3();
        println!(
            "{name}: the star {:.1} degrees up, bearing {:.0}",
            forward.y.asin().to_degrees(),
            (180.0 - forward.x.atan2(forward.z).to_degrees()).rem_euclid(360.0)
        );
        let path = out.join(format!("stars-{name}.png"));
        dome_still(gpu, sky, display, forward, 60.0, &path)?;
    }
    sky.set_place(None);
    Ok(())
}

fn main() -> Result<()> {
    let gpu = device()?;
    let info = gpu.adapter.get_info();
    println!("adapter: {} ({:?})", info.name, info.backend);
    let out = native_offscreen::build_dir("sky").join("native");
    std::fs::create_dir_all(&out)?;
    let quality = SkyQuality::High;
    let mut sky = Sky::new(&gpu.device, &gpu.queue, quality, SkyTargets::default());
    let diagnostics = Diagnostics::new(&gpu.device, &sky, HDR);
    let display = DisplayTransform::new(&gpu.device, CAPTURE_FORMAT);
    if std::env::args().nth(1).as_deref() == Some("stars") {
        stars(&gpu, &mut sky, &display, &out)?;
        println!("wrote {}", out.display());
        return Ok(());
    }

    let camera = panorama(&gpu, &mut sky, &diagnostics, &display, false, &out)?;
    std::fs::write(out.join("probes-camera.json"), &camera)?;
    let sea = panorama(&gpu, &mut sky, &diagnostics, &display, true, &out)?;
    std::fs::write(out.join("probes-sea.json"), &sea)?;
    dome(&gpu, &mut sky, &display, &out)?;
    stars(&gpu, &mut sky, &display, &out)?;
    environment(&gpu, &mut sky, &diagnostics, &display, &out)?;
    let timing = format!(
        "{{\n{},\n{}\n}}\n",
        timings(&gpu, &mut sky, 21)?,
        dome_timings(&gpu, &mut sky, 21)?
    );
    std::fs::write(out.join("timings.json"), &timing)?;
    println!("{timing}");
    println!("wrote {}", out.display());
    Ok(())
}
