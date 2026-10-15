//! The detail noise on the GPU: the startup fill writes the texels the CPU reference gives (the lattice is hashed with
//! integers, so only float rounding separates one GPU from another), and its mips are the means of the texels under
//! them.

use anyhow::Result;
use native_offscreen::Gpu;
use naval_clouds::detail_noise::{self, DetailFill, MIPS, SIZE};
use naval_render::MakePipelines;

/// The texels of one mip, RGBA, x fastest.
fn read_mip(gpu: &Gpu, texture: &wgpu::Texture, mip: u32) -> Result<Vec<u8>> {
    let size = SIZE >> mip;
    let row = size * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("detail noise read-back"),
        size: u64::from(padded) * u64::from(size) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("detail noise copy"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d::ZERO,
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
            depth_or_array_layers: size,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    let mapped = slice
        .get_mapped_range()
        .map_err(|e| anyhow::anyhow!("mapping the detail noise: {e:?}"))?;
    let mut texels = Vec::with_capacity((row * size * size) as usize);
    for chunk in mapped.chunks(padded as usize) {
        texels.extend_from_slice(&chunk[..row as usize]);
    }
    Ok(texels)
}

#[test]
fn the_fill_writes_the_reference_and_its_mips() -> Result<()> {
    let gpu = Gpu::headless()?;
    let texture = detail_noise::texture(&gpu.device);
    let mut fill = DetailFill::new(&gpu.device, &texture, MakePipelines::Now);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("detail noise fill"),
        });
    fill.encode_if_stale(&mut encoder);
    let started = std::time::Instant::now();
    gpu.queue.submit([encoder.finish()]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    println!(
        "detail noise fill: {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let texels = read_mip(&gpu, &texture.texture, 0)?;
    let at = |x: u32, y: u32, z: u32| {
        let i = ((z * SIZE + y) * SIZE + x) as usize * 4;
        [texels[i], texels[i + 1], texels[i + 2], texels[i + 3]]
    };
    // A lattice of texels, the volume's corners and edges among them, against the CPU's.
    let mut worst = 0;
    for z in (0..SIZE).step_by(21).chain([SIZE - 1]) {
        for y in (0..SIZE).step_by(13).chain([SIZE - 1]) {
            for x in (0..SIZE).step_by(9).chain([SIZE - 1]) {
                let gpu_texel = at(x, y, z);
                let reference = detail_noise::reference_texel(x, y, z);
                for channel in 0..4 {
                    let difference =
                        (i32::from(gpu_texel[channel]) - i32::from(reference[channel])).abs();
                    worst = worst.max(difference);
                    assert!(
                        difference <= 2,
                        "texel ({x}, {y}, {z}) channel {channel}: {} on the GPU, {} on the CPU",
                        gpu_texel[channel],
                        reference[channel]
                    );
                }
            }
        }
    }
    println!("worst difference from the reference: {worst} of 255");

    // Every mip is the mean of the mip above it; the last (4³) holds each channel's mean to a few levels.
    let mut above = texels;
    for mip in 1..MIPS {
        let size = SIZE >> mip;
        let wide = size * 2;
        let this = read_mip(&gpu, &texture.texture, mip)?;
        for (x, y, z) in [
            (0, 0, 0),
            (size - 1, size / 2, 1),
            (size / 3, size - 1, size - 1),
        ] {
            for channel in 0..4usize {
                let mut sum = 0u32;
                for corner in 0..8u32 {
                    let (cx, cy, cz) = (
                        x * 2 + (corner & 1),
                        y * 2 + ((corner >> 1) & 1),
                        z * 2 + ((corner >> 2) & 1),
                    );
                    sum += u32::from(above[((cz * wide + cy) * wide + cx) as usize * 4 + channel]);
                }
                let mean = sum as f32 / 8.0;
                let stored = f32::from(this[((z * size + y) * size + x) as usize * 4 + channel]);
                assert!(
                    (stored - mean).abs() <= 1.0,
                    "mip {mip} texel ({x}, {y}, {z}) channel {channel}: {stored} against {mean}"
                );
            }
        }
        above = this;
    }
    Ok(())
}
