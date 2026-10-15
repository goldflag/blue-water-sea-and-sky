//! GPU check of the caustics under the sea (`naval_ocean::caustics`): over a real wind sea every level holds a net of
//! light, brighter and darker than a flat sea's about a mean near it, its contrast greatest a few metres down; and a
//! flat sea gives flat light. Needs a GPU adapter.

use std::sync::mpsc;

use native_offscreen::Gpu;
use naval_ocean::calibration::{NORTH_ATLANTIC, scene_parameters};
use naval_ocean::caustics::{BRIGHTEST, Caustics, LEVELS, TEXELS};
use naval_ocean::{QualityTier, WaveField};
use naval_render::{Camera, FrameTime, MakePipelines};

/// One level's gains (its layer's red channel), row by row.
fn read_level(gpu: &Gpu, texture: &wgpu::Texture, level: u32) -> Vec<f32> {
    let size = texture.width();
    let row = size * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("caustics read-back"),
        size: u64::from(padded) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: level,
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
    let mut values = Vec::with_capacity((size * size) as usize);
    for chunk in mapped.chunks(padded as usize) {
        for texel in chunk[..row as usize].as_chunks::<8>().0 {
            values.push(half::f16::from_bits(u16::from_le_bytes([texel[0], texel[1]])).to_f32());
        }
    }
    values
}

/// (mean, deviation, share at the brightest) of a level's gains.
fn statistics(values: &[f32]) -> (f64, f64, f64) {
    let count = values.len() as f64;
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
    let variance = values
        .iter()
        .map(|&v| (f64::from(v) - mean).powi(2))
        .sum::<f64>()
        / count;
    let brightest = values.iter().filter(|&&v| v > BRIGHTEST * 0.98).count() as f64 / count;
    (mean, variance.sqrt(), brightest)
}

#[test]
fn the_waves_draw_a_net_of_light_at_every_depth() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let device = &gpu.device;
    let camera = Camera::look_at(
        glam::Vec3::new(300.0, -8.0, -120.0),
        glam::Vec3::new(300.0, -8.0, -220.0),
    )
    .uniforms(1920, 1080, FrameTime::fixed(0, 1.0 / 60.0));
    for wind in [5.0, 9.0, 15.0] {
        let mut field = WaveField::new(device, &QualityTier::High.cascades());
        let parameters = scene_parameters(&NORTH_ATLANTIC, wind, false);
        let mut caustics = Caustics::new(
            device,
            field.surface_layout(),
            QualityTier::High.cascades().len(),
            MakePipelines::Now,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        for step in 0..3 {
            field.update(
                &gpu.queue,
                &mut encoder,
                &parameters,
                40.0 + f64::from(step) / 30.0,
                1.0 / 30.0,
            );
        }
        caustics.encode(
            device,
            &mut naval_render::UploadSink::Queue(&gpu.queue),
            &mut encoder,
            &camera,
            field.surface_bind_group(),
            None,
        );
        gpu.queue.submit([encoder.finish()]);
        let texture = caustics.texture().clone();
        assert_eq!(
            (texture.width(), texture.depth_or_array_layers()),
            (TEXELS, LEVELS)
        );
        let mut contrasts = Vec::new();
        for level in 0..LEVELS {
            let gains = read_level(&gpu, &texture, level);
            assert!(gains.iter().all(|v| v.is_finite() && *v > 0.0));
            let (mean, deviation, brightest) = statistics(&gains);
            println!(
                "wind {wind} m/s, level {level} ({} m): mean {mean:.3}, deviation {deviation:.3}, {:.2} % at the brightest",
                naval_ocean::caustics::depth(level),
                brightest * 100.0
            );
            // A net about the flat sea's light (the waves move the light, they do not make it): neither flat nor
            // all focus.
            assert!((0.93..1.07).contains(&mean), "level {level}: mean {mean}");
            assert!(
                (0.15..0.7).contains(&deviation),
                "level {level}: deviation {deviation}"
            );
            assert!(
                brightest < 0.02,
                "level {level}: {brightest} at the brightest"
            );
            contrasts.push(deviation);
        }
        // The net is sharpest under the surface and softest at the deepest level.
        assert!(
            contrasts[LEVELS as usize - 1] < contrasts[0] * 0.9,
            "{contrasts:?}"
        );
    }
}
