//! GPU checks of the wave field, as `scripts/diagnostics/ocean-waves.html` checks the web's: the evolved, inverse-
//! transformed displacement against a CPU inverse DFT of the same spectrum, the mip chain's box average, and foam
//! building under a 9 m/s wind. Needs a GPU adapter.

use std::f64::consts::PI;
use std::sync::mpsc;

use native_offscreen::Gpu;
use naval_ocean::calibration::{NORTH_ATLANTIC, scene_parameters};
use naval_ocean::sea_state::{drawn_sea, sea_state_cascades};
use naval_ocean::spectrum::{FOLD_PERIOD, build_spectrum};
use naval_ocean::{QualityTier, WaveField};

fn half_to_f32(bits: u16) -> f32 {
    half::f16::from_bits(bits).to_f32()
}

/// One layer and mip level of an `Rgba16Float` array texture as RGBA floats.
fn read_layer(gpu: &Gpu, texture: &wgpu::Texture, layer: u32, level: u32) -> (u32, Vec<f32>) {
    let size = texture.width() >> level;
    let row = size * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("field read-back"),
        size: u64::from(padded) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    receiver.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut values = Vec::with_capacity((size * size * 4) as usize);
    for chunk in mapped.chunks(padded as usize) {
        for pair in chunk[..row as usize].as_chunks::<2>().0 {
            values.push(half_to_f32(u16::from_le_bytes([pair[0], pair[1]])));
        }
    }
    (size, values)
}

#[test]
fn transform_matches_a_cpu_inverse_dft() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let tier = QualityTier::High.cascades();
    let mut field = WaveField::new(&gpu.device, &tier);
    let parameters = scene_parameters(&NORTH_ATLANTIC, 9.0, false);
    let time = 123.456;
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    assert!(field.update(&gpu.queue, &mut encoder, &parameters, time, 1.0 / 30.0));
    gpu.queue.submit([encoder.finish()]);

    let sea = drawn_sea(&parameters.waves, true);
    let cascades = sea_state_cascades(&tier, sea.peak_wavelength);
    let spectrum = build_spectrum(&cascades, &sea);
    let phase = f64::from(((time % FOLD_PERIOD) / FOLD_PERIOD) as f32);
    for (c, cascade) in spectrum.cascades.iter().enumerate() {
        let (n, gpu_values) = read_layer(&gpu, &field.textures()[0], c as u32, 0);
        let n = n as usize;
        let dk = 2.0 * PI / cascade.size;
        // Evolved modes: H = a·e^{−2πi·m·phase}, displacement i·λ·k̂·H horizontally.
        let modes: Vec<(f64, f64, f64, f64)> = cascade
            .amplitudes
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, a)| a[2] != 0.0)
            .map(|(texel, a)| {
                let turn = 2.0 * PI * (f64::from(a[2]) * phase).rem_euclid(1.0);
                let (s, co) = turn.sin_cos();
                let (re, im) = (f64::from(a[0]), f64::from(a[1]));
                let signed = |i: usize| {
                    if i < n / 2 {
                        i as f64
                    } else {
                        i as f64 - n as f64
                    }
                };
                (
                    signed(texel % n),
                    signed(texel / n),
                    re * co + im * s,
                    im * co - re * s,
                )
            })
            .collect();
        let mut worst: f64 = 0.0;
        let mut largest: f64 = 0.0;
        for &(x, z) in &[
            (0usize, 0usize),
            (17, 3),
            (100, 200),
            (255, 128),
            (64, 64),
            (201, 9),
        ] {
            let (mut dx, mut dy, mut dz) = (0.0, 0.0, 0.0);
            for &(nx, nz, hr, hi) in &modes {
                let angle = 2.0 * PI * (nx * x as f64 + nz * z as f64) / n as f64;
                let (s, co) = angle.sin_cos();
                // Re(H e^{iθ}) and Re(i H e^{iθ}).
                let real = hr * co - hi * s;
                let imaginary = -(hr * s + hi * co);
                let (kx, kz) = (nx * dk, nz * dk);
                let k = (kx * kx + kz * kz).sqrt().max(1e-6);
                dy += real;
                dx += sea.choppiness * kx / k * imaginary;
                dz += sea.choppiness * kz / k * imaginary;
            }
            let texel = (z * n + x) * 4;
            let got = [
                gpu_values[texel],
                gpu_values[texel + 1],
                gpu_values[texel + 2],
            ];
            for (g, want) in got.iter().zip([dx, dy, dz]) {
                worst = worst.max((f64::from(*g) - want).abs());
                largest = largest.max(want.abs());
            }
        }
        assert!(largest > 1e-3, "cascade {c} is flat: {largest}");
        // Half floats keep about three decimal digits.
        assert!(
            worst <= 2e-3 * largest.max(0.05),
            "cascade {c}: worst {worst} of {largest}"
        );
    }
}

#[test]
fn mips_average_and_foam_builds() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let tier = QualityTier::High.cascades();
    let mut field = WaveField::new(&gpu.device, &tier);
    let parameters = scene_parameters(&NORTH_ATLANTIC, 15.0, false);
    for step in 0..300 {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        field.update(
            &gpu.queue,
            &mut encoder,
            &parameters,
            50.0 + f64::from(step) / 30.0,
            1.0 / 30.0,
        );
        gpu.queue.submit([encoder.finish()]);
    }
    let extras = &field.textures()[2];
    let (n, level0) = read_layer(&gpu, extras, 1, 0);
    let (m, level1) = read_layer(&gpu, extras, 1, 1);
    assert_eq!(m, n / 2);
    let n = n as usize;
    for &(x, z) in &[(0usize, 0usize), (10, 20), (63, 127)] {
        let at = |x: usize, z: usize| level0[(z * n + x) * 4 + 2];
        let mean = (at(2 * x, 2 * z)
            + at(2 * x + 1, 2 * z)
            + at(2 * x, 2 * z + 1)
            + at(2 * x + 1, 2 * z + 1))
            / 4.0;
        let got = level1[(z * (n / 2) + x) * 4 + 2];
        assert!(
            (got - mean).abs() <= 2e-3 * mean.abs().max(1e-3),
            "mip {got} against {mean}"
        );
    }
    // A gale breaks: foam covers part of the middle cascade, never above a fresh sheet's 1.
    let foam: Vec<f32> = level0.as_chunks::<4>().0.iter().map(|t| t[1]).collect();
    let covered = foam.iter().filter(|&&f| f > 0.1).count() as f64 / foam.len() as f64;
    assert!(covered > 0.002 && covered < 0.5, "foam covers {covered}");
    assert!(foam.iter().all(|&f| (0.0..=1.001).contains(&f)));
}
