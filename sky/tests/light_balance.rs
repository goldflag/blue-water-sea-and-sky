//! A measurement, not a check: the sun's and the sky's light at the sea as the game has it (luminance, the sea's units),
//! for the North Atlantic's clear weather (the photo-match view's air: `TURBIDITY` and `MIE`, the authored aureole after
//! the client's `SUN_HAZE`, override it) at sun elevations of 10° to 75°: the direct light on a surface facing the sun
//! (the world's, `SeaLevel::sun`, and the tables' own), and the sky's on a level surface (the CPU's estimate, the
//! tables' irradiance, the environment's level 0 integrated over the hemisphere, and `env_irradiance` as meshes read it:
//! the roughest level times π), then the environment's on upright surfaces and the underside. Skips without a GPU.
//! `native/docs/lighting.md`, "Sun and sky at the sea", has its readings. Run with
//! `cargo test -p naval-sky --test all -- light_balance:: --ignored --nocapture`.

use glam::Vec3;
use native_offscreen::Gpu;
use naval_sky::diagnostics::{Diagnostics, Tile};
use naval_sky::haze::Haze;
use naval_sky::model::{self, AuthoredAtmosphere, PLANET_RADIUS};
use naval_sky::{Sky, SkyQuality, SkyScene, SkyTargets, SkyView};

fn read(gpu: &Gpu, buffer: &wgpu::Buffer) -> Vec<[f32; 4]> {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = slice.get_mapped_range().unwrap();
    let values = bytemuck::cast_slice::<u8, [f32; 4]>(&data).to_vec();
    drop(data);
    buffer.unmap();
    values
}

fn lum(c: [f32; 3]) -> f64 {
    0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
}

const W: u32 = 720;
const H: u32 = 360;

/// The environment at `roughness`, unwrapped (azimuth across, compass 0 in the middle; elevation 90° to −90° down).
fn env_image(gpu: &Gpu, sky: &Sky, diagnostics: &Diagnostics, roughness: f32) -> Vec<[f32; 4]> {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    diagnostics.environment(
        &gpu.device,
        &mut encoder,
        sky.environment(),
        &view,
        Tile {
            x: 0,
            y: 0,
            width: W,
            height: H,
        },
        0.0,
        roughness,
    );
    let row = W * 16;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        texture.size(),
    );
    gpu.queue.submit([encoder.finish()]);
    read(gpu, &buffer)
}

fn pixel_direction(x: u32, y: u32) -> Vec3 {
    let azimuth = ((x as f32 + 0.5) / W as f32 - 0.5) * std::f32::consts::TAU;
    let elevation = (0.5 - (y as f32 + 0.5) / H as f32) * std::f32::consts::PI;
    Vec3::new(
        azimuth.sin() * elevation.cos(),
        elevation.sin(),
        azimuth.cos() * elevation.cos(),
    )
}

/// Irradiance (luminance) on a surface facing `normal` from an unwrapped radiance image: ∫ L cos dω over its hemisphere.
fn irradiance(image: &[[f32; 4]], normal: Vec3) -> f64 {
    let d_phi = std::f64::consts::TAU / f64::from(W);
    let d_theta = std::f64::consts::PI / f64::from(H);
    let mut sum = 0.0;
    for y in 0..H {
        for x in 0..W {
            let d = pixel_direction(x, y);
            let c = f64::from(d.dot(normal));
            if c <= 0.0 {
                continue;
            }
            let solid = f64::from((d.y).asin().cos()) * d_phi * d_theta;
            let p = image[(y * W + x) as usize];
            sum += lum([p[0], p[1], p[2]]) * c * solid;
        }
    }
    sum
}

fn sample(image: &[[f32; 4]], direction: Vec3) -> f64 {
    let d = direction.normalize();
    let elevation = d.y.asin();
    let azimuth = d.x.atan2(d.z);
    let x = (((azimuth / std::f32::consts::TAU + 0.5) * W as f32) as u32).min(W - 1);
    let y = (((0.5 - elevation / std::f32::consts::PI) * H as f32) as u32).min(H - 1);
    let p = image[(y * W + x) as usize];
    lum([p[0], p[1], p[2]])
}

fn atmosphere(turbidity: f64) -> AuthoredAtmosphere {
    AuthoredAtmosphere {
        rayleigh: 0.38,
        turbidity,
        mie: 0.18 * 0.45,
        mie_g: 0.72,
        multiple: 1.0,
    }
}

#[test]
#[ignore]
fn light_balance_table() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let turbidity: f64 = std::env::var("TURBIDITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.5);
    let mie: f64 = std::env::var("MIE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.18 * 0.45);
    let mut sky = Sky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::High,
        SkyTargets::default(),
    );
    let diagnostics = Diagnostics::new(&gpu.device, &sky, wgpu::TextureFormat::Rgba32Float);
    let air = model::air_coefficients(&AuthoredAtmosphere {
        mie,
        ..atmosphere(turbidity)
    });
    let zenith_t = model::sea_transmittance(&air, 90.0);
    let aod = air.mie_extinction[1] * model::MIE_HEIGHT;
    let rod = air.rayleigh[1] * model::RAYLEIGH_HEIGHT;
    println!(
        "turbidity {turbidity}: aerosol optical depth (550) {aod:.4}, Rayleigh {rod:.4}, zenith T {:?}, mie gain {:.3}",
        zenith_t.map(|t| (t * 1000.0).round() / 1000.0),
        air.mie_gain
    );
    println!(
        "elev | DNI_world DNI_tables | DHI_cpu DHI_table DHI_env0 DHI_envR1 | Dh/DHI(env R1) Dh/DHI(table) | zenith L, sky 90 deg from sun"
    );
    for elevation in [10.0, 30.0, 45.0, 61.0, 75.0] {
        let mut scene = SkyScene::north_atlantic(elevation, 0.0).cloudless();
        scene.atmosphere = AuthoredAtmosphere {
            mie,
            ..atmosphere(turbidity)
        };
        sky.apply(&scene);
        sky.set_weather_haze(Haze::new(35_000.0, 2300.0));
        let eye = Vec3::new(0.0, 30.0, 0.0);
        let view = SkyView::looking_at(eye, eye + Vec3::NEG_Z, 52.0, [64, 64]);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        sky.update(&gpu.queue, &mut encoder, &view, 0.0);
        gpu.queue.submit([encoder.finish()]);

        let sea = [0.0, PLANET_RADIUS as f32 + 0.002, 0.0];
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let buffer = diagnostics.probe(
            &gpu.device,
            &mut encoder,
            &sky,
            &[
                [sea[0], sea[1], sea[2], 4.0],
                [sea[0], sea[1], sea[2], 7.0],
                [sea[0], sea[1], sea[2], 8.0],
            ],
        );
        gpu.queue.submit([encoder.finish()]);
        let probes = read(&gpu, &buffer);
        let p = |i: usize| [probes[i][0], probes[i][1], probes[i][2]];
        let dni_tables = lum(p(0));
        let dhi_table = lum(p(2));

        let level = sky.sea_level();
        let dni_world = lum(level.sun.as_vec3().to_array());
        let dhi_cpu = lum(level.sky.as_vec3().to_array());

        let sharp = env_image(&gpu, &sky, &diagnostics, 0.0);
        let rough = env_image(&gpu, &sky, &diagnostics, 1.0);
        let up = Vec3::Y;
        let dhi_env0 = irradiance(&sharp, up);
        let dhi_env_r1 = std::f64::consts::PI * sample(&rough, up);
        let sin = elevation.to_radians().sin();
        let sun_dir = sky.sun_direction();
        let away = Vec3::new(-sun_dir.x, 0.0, -sun_dir.z).normalize();
        let side = Vec3::new(sun_dir.z, 0.0, -sun_dir.x).normalize();
        let ninety = (up * (elevation as f32).to_radians().cos()
            - Vec3::new(sun_dir.x, 0.0, sun_dir.z).normalize()
                * (elevation as f32).to_radians().sin())
        .normalize();
        println!(
            "{elevation:>4} | {dni_world:>9.3} {dni_tables:>10.3} | {dhi_cpu:>7.3} {dhi_table:>9.3} {dhi_env0:>8.3} {dhi_env_r1:>9.3} | {:>14.2} {:>13.2} | {:.3} {:.3}",
            dni_world * sin / dhi_env_r1,
            dni_world * sin / dhi_table,
            sample(&sharp, up),
            sample(&sharp, ninety),
        );
        // Vertical surfaces: toward the sun's azimuth, away from it, across it; and the underside.
        let toward = Vec3::new(sun_dir.x, 0.0, sun_dir.z).normalize();
        for (name, n) in [
            ("toward", toward),
            ("away", away),
            ("side", side),
            ("down", -up),
        ] {
            println!(
                "       {name:>6}: env cos-integral (level 0) {:.3}, env R1 x pi {:.3}",
                irradiance(&sharp, n),
                std::f64::consts::PI * sample(&rough, n)
            );
        }
    }
}
