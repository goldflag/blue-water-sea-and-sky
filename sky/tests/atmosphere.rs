//! The sky on the GPU: takram's tables against the CPU model (the transmittance the precompute integrates, the
//! sun's light the clouds read), the sky finite and lit as it should be from the sea and from aircraft by day and
//! night, the celestial light continuous through sunset, and the environment a consumer reads oriented and finite.
//! Skips when the machine has no GPU adapter.

use glam::Vec3;
use native_offscreen::Gpu;
use naval_sky::diagnostics::Diagnostics;
use naval_sky::model::{self, PLANET_RADIUS};
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

/// A sky for `scene`, updated once for a camera `height` metres up (its tables computed), and its probes.
struct Probed {
    sky: Sky,
    diagnostics: Diagnostics,
}

impl Probed {
    fn new(gpu: &Gpu) -> Self {
        let sky = Sky::new(
            &gpu.device,
            &gpu.queue,
            SkyQuality::High,
            SkyTargets::default(),
        );
        let diagnostics = Diagnostics::new(&gpu.device, &sky, wgpu::TextureFormat::Rgba16Float);
        Self { sky, diagnostics }
    }

    fn update(&mut self, gpu: &Gpu, scene: &SkyScene, height: f32) {
        self.sky.apply(scene);
        let eye = Vec3::new(0.0, height, 0.0);
        let view = SkyView::looking_at(eye, eye + Vec3::NEG_Z, 52.0, [64, 64]);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.sky.update(&gpu.queue, &mut encoder, &view, 0.0);
        gpu.queue.submit([encoder.finish()]);
    }

    fn probe(&self, gpu: &Gpu, probes: &[[f32; 4]]) -> Vec<[f32; 3]> {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let buffer = self
            .diagnostics
            .probe(&gpu.device, &mut encoder, &self.sky, probes);
        gpu.queue.submit([encoder.finish()]);
        read(gpu, &buffer)
            .into_iter()
            .map(|v| [v[0], v[1], v[2]])
            .collect()
    }
}

fn direction(elevation: f64, azimuth: f64) -> [f32; 3] {
    naval_sky::state::direction_from_angles(elevation, azimuth)
        .as_vec3()
        .to_array()
}

/// The transmittance the precompute integrated into its table matches the CPU's integral of the same air (optical
/// depth within 0.5%),
/// from sea level toward the zenith and down to a degree above the horizon, for a clear and two hazy maps; and the
/// table itself at the bottom of the air, where a float32 sample once dropped under it (`get_profile_density`).
#[test]
fn transmittance_matches_the_cpu_integral() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut probed = Probed::new(&gpu);
    for turbidity in [1.0, 2.2, 6.0] {
        let mut scene = SkyScene::north_atlantic(30.0, 270.0).cloudless();
        scene.atmosphere.turbidity = turbidity;
        probed.update(&gpu, &scene, 30.0);
        let air = model::air_coefficients(&scene.atmosphere);
        let elevations = [90.0, 45.0, 20.0, 10.0, 5.0, 3.0, 2.0, 1.0];
        let probes: Vec<[f32; 4]> = elevations
            .iter()
            .map(|&e| {
                let d = direction(e, 0.0);
                [d[0], d[1], d[2], 2.0]
            })
            .collect();
        let gpu_values = probed.probe(&gpu, &probes);
        for (&elevation, value) in elevations.iter().zip(&gpu_values) {
            // The GPU reads from 2 m above the sea.
            let mu = elevation.to_radians().sin();
            let depth = model::optical_depth(&air, PLANET_RADIUS + 0.002, mu, 4000);
            for c in 0..3 {
                let gpu_depth = -f64::from(value[c]).ln();
                assert!(
                    (gpu_depth - depth[c]).abs() < 0.005 * depth[c] + 0.002,
                    "turbidity {turbidity}, elevation {elevation}: optical depth {gpu_depth} against {} (channel {c})",
                    depth[c]
                );
            }
        }
        // The table at the bottom of the air and just above it, straight up.
        for altitude in [0.0, 0.0076, 0.0152, 0.03] {
            let r = PLANET_RADIUS + altitude;
            let value = probed.probe(&gpu, &[[r as f32, 1.0, 0.0, 6.0]])[0];
            let depth = model::optical_depth(&air, r, 1.0, 4000);
            for c in 0..3 {
                let gpu_depth = -f64::from(value[c]).ln();
                assert!(
                    (gpu_depth - depth[c]).abs() < 0.003 * depth[c],
                    "turbidity {turbidity}, {altitude} km up: {gpu_depth} against {} (channel {c})",
                    depth[c]
                );
            }
        }
    }
}

/// The sky is finite and never negative from the sea, an aircraft and orbit's edge, by day, at sunset, in twilight,
/// at night and in fog, in every direction including straight down; by day the zenith is blue.
#[test]
fn the_sky_is_finite_day_and_night() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut probed = Probed::new(&gpu);
    let mut fog = SkyScene::north_atlantic(15.0, 270.0).cloudless();
    fog.atmosphere.turbidity = 9.0;
    let mut night = SkyScene::north_atlantic(-30.0, 270.0).cloudless();
    night.moon_phase = 0.5;
    let scenes = [
        ("noon", SkyScene::north_atlantic(60.0, 270.0).cloudless()),
        ("sunset", SkyScene::north_atlantic(0.0, 270.0).cloudless()),
        (
            "twilight",
            SkyScene::north_atlantic(-6.0, 270.0).cloudless(),
        ),
        ("night", night),
        ("fog", fog),
    ];
    let mut probes = Vec::new();
    for elevation in [90.0, 45.0, 10.0, 2.0, 0.5, 0.0, -0.2, -1.0, -10.0, -89.0] {
        for azimuth in [0.0, 90.0, 180.0, 270.0] {
            let d = direction(elevation, azimuth);
            probes.push([d[0], d[1], d[2], 0.0]);
            probes.push([d[0], d[1], d[2], 1.0]);
        }
    }
    for (name, scene) in scenes {
        for height in [30.0, 2600.0, 9000.0, 40000.0] {
            probed.update(&gpu, &scene, height);
            let values = probed.probe(&gpu, &probes);
            for (probe, value) in probes.iter().zip(&values) {
                assert!(
                    value.iter().all(|v| v.is_finite() && *v >= 0.0),
                    "{name} at {height} m along {probe:?}: {value:?}"
                );
            }
            if name == "noon" && height < 100.0 {
                let zenith = values[0];
                assert!(zenith[2] > zenith[0] * 1.5, "noon zenith {zenith:?}");
                // Brighter toward the horizon than at the zenith, as a hazy sea sky is.
                assert!(
                    values[3 * 8][1] > zenith[1],
                    "{:?} {zenith:?}",
                    values[3 * 8]
                );
            }
        }
    }
}

/// The clouds' light (`sun_sky_scalar_irradiance`): by day the sun's direct light at cloud height is the scene's
/// intensity through the air above, and the sky's is a fraction of it; under the planet's shadow the sun's is gone.
#[test]
fn the_clouds_light_follows_the_sun() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut probed = Probed::new(&gpu);
    let scene = SkyScene::north_atlantic(60.0, 270.0).cloudless();
    probed.update(&gpu, &scene, 30.0);
    let base = [0.0, PLANET_RADIUS as f32 + 1.5, 0.0];
    let values = probed.probe(
        &gpu,
        &[
            [base[0], base[1], base[2], 4.0],
            [base[0], base[1], base[2], 5.0],
        ],
    );
    let (sun, sky) = (values[0], values[1]);
    let luminance = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // The scene's sun is 6.6 above the air, on the dome's scale (× `sky_grade::GAIN`, 1.4: 9.24); at 1.5 km under a
    // 60° sun some tenths are lost on the way.
    let top = 6.6 * naval_sky::model::sky_grade::GAIN as f32;
    assert!(
        luminance(sun) > 4.5 * 1.4 && luminance(sun) < top,
        "{sun:?}"
    );
    // White above the air, warmer through it.
    assert!(sun[0] >= sun[2], "{sun:?}");
    assert!(
        luminance(sky) > 0.2 && luminance(sky) < luminance(sun),
        "{sky:?}"
    );
    let dusk = SkyScene::north_atlantic(-8.0, 270.0).cloudless();
    probed.update(&gpu, &dusk, 30.0);
    let low = [0.0, PLANET_RADIUS as f32 + 0.5, 0.0];
    let values = probed.probe(&gpu, &[[low[0], low[1], low[2], 4.0]]);
    assert!(luminance(values[0]) < 1e-3, "{:?}", values[0]);
}

/// The celestial light: the sun by day, dimming and reddening to the horizon without jumps, the moon once it
/// outshines the setting sun.
#[test]
fn the_light_is_continuous_through_sunset() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut sky = Sky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::Low,
        SkyTargets::default(),
    );
    let mut previous: Option<(f32, bool)> = None;
    let mut nights = 0;
    for step in 0..=160 {
        let elevation = 30.0 - f64::from(step) * 0.25;
        sky.apply(&SkyScene::north_atlantic(elevation, 270.0));
        let light = sky.light();
        let level = light.intensity * light.color.max_element();
        assert!(level.is_finite() && level >= 0.0, "{elevation}: {light:?}");
        if let Some((last, last_night)) = previous {
            if light.night == last_night && !light.night {
                assert!(
                    level <= last + 1e-4 && last - level < 0.35,
                    "{elevation}: {level} after {last}"
                );
            }
            if light.night != last_night {
                nights += 1;
            }
        }
        previous = Some((level, light.night));
    }
    assert_eq!(nights, 1, "the moon takes over once");
    assert!(sky.light().night);
}

/// A sky updated for the North Atlantic at sun 20°, and its atlas read back (RGB per texel, row 0 first).
fn baked_atlas(gpu: &Gpu) -> (Sky, Vec<[f32; 3]>) {
    let mut sky = Sky::new(
        &gpu.device,
        &gpu.queue,
        SkyQuality::High,
        SkyTargets::default(),
    );
    sky.apply(&SkyScene::north_atlantic(20.0, 270.0));
    let view = SkyView::looking_at(Vec3::new(0.0, 30.0, 0.0), Vec3::NEG_Z, 52.0, [64, 64]);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    sky.update(&gpu.queue, &mut encoder, &view, 0.0);
    gpu.queue.submit([encoder.finish()]);
    let environment = sky.environment();
    let layout = environment.layout;
    let row = layout.width * 8;
    let padded = row.div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
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
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = slice.get_mapped_range().unwrap();
    let mut texels = Vec::with_capacity((layout.width * layout.height) as usize);
    for y in 0..layout.height {
        for x in 0..layout.width {
            let start = (y * padded + x * 8) as usize;
            let half: &[u16] = bytemuck::cast_slice(&data[start..start + 8]);
            texels.push([0, 1, 2].map(|c| f16_to_f32(half[c])));
        }
    }
    drop(data);
    buffer.unmap();
    (sky, texels)
}

#[test]
fn environment_is_oriented_and_finite() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (sky, texels) = baked_atlas(&gpu);
    let layout = sky.environment().layout;
    assert!(sky.environment().generation >= 1);
    let texel = |x: u32, y: u32| texels[(y * layout.width + x) as usize];
    let s = layout.face_size;
    // Level 0, native orientation: the +Y face (column 1, first row) is sky, the −Y face (column 1, second
    // row) is sea, and in the +X face the rows run from the nadir side down to the zenith side.
    let up = texel(s + s / 2, s / 2);
    let down = texel(s + s / 2, s + s / 2);
    assert!(up.iter().all(|v| v.is_finite() && *v > 0.02), "sky {up:?}");
    assert!(down[2] < up[2] * 0.2, "sea {down:?} vs sky {up:?}");
    assert!(
        texel(s / 2, 2)[2] < texel(s / 2, s - 3)[2],
        "+X face rows run from below to above"
    );
    for y in 0..2 * s {
        for x in 0..3 * s {
            assert!(texel(x, y).iter().all(|v| v.is_finite()));
        }
    }
}

/// A sky put on screen in place of another (the theatre's scene swaps) takes that sky's tables when they hold the same
/// air, and reads as one that computed its own; it takes none from a sky that has not computed them, nor under other
/// air.
#[test]
fn a_sky_takes_the_tables_of_another_of_the_same_air() {
    let Ok(gpu) = Gpu::headless() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let scene = SkyScene::north_atlantic(30.0, 270.0).cloudless();
    let mut computed = Probed::new(&gpu);
    computed.update(&gpu, &scene, 30.0);
    let take = |sky: &mut Sky, from: &Sky| {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let took = sky.take_tables_from(&gpu.queue, &mut encoder, from);
        gpu.queue.submit([encoder.finish()]);
        took
    };

    let mut copied = Probed::new(&gpu);
    copied.sky.apply(&scene);
    assert!(take(&mut copied.sky, &computed.sky), "the same air");
    copied.update(&gpu, &scene, 30.0);
    assert_eq!(
        (computed.sky.precomputes(), copied.sky.precomputes()),
        (1, 0)
    );
    let mut probes = Vec::new();
    for elevation in [90.0, 30.0, 5.0, 1.0] {
        for azimuth in [0.0, 120.0, 240.0] {
            let d = direction(elevation, azimuth);
            for mode in [0.0, 1.0, 2.0] {
                probes.push([d[0], d[1], d[2], mode]);
            }
        }
    }
    assert_eq!(
        copied.probe(&gpu, &probes),
        computed.probe(&gpu, &probes),
        "a sky on copied tables reads as the one that computed them"
    );

    let mut fresh = Probed::new(&gpu);
    fresh.sky.apply(&scene);
    let mut other = Probed::new(&gpu);
    other.sky.apply(&scene);
    assert!(!take(&mut other.sky, &fresh.sky), "tables not computed yet");
    let mut hazy = scene;
    hazy.atmosphere.turbidity = 6.0;
    other.sky.apply(&hazy);
    assert!(!take(&mut other.sky, &computed.sky), "other air");
    assert!(
        !take(&mut computed.sky, &copied.sky),
        "tables of its own already"
    );
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}
