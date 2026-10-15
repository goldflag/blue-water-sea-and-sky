//! The clouds' textures. takram's shape volume (128³) and local weather (512², a channel per layer) are its shipped
//! assets, embedded as lossless PNGs: their generators hash value noise with `sin`, which differs between GPUs, so
//! generating them here would not give takram's texels (`examples/noise_compare.rs` measures it). The turbulence is
//! generated on the GPU at startup (`noise.wgsl`'s `turbulence_main`), which reproduces takram's to the bit. The detail
//! noise is the crate's own, generated at startup too (`crate::detail_noise`; takram's 32³ shape detail, which it
//! replaced, stopped at 20–40 m). The blue noise is the clouds' own (`examples/blue_noise.rs`), standing in for
//! NVIDIA's STBN, which takram ships but whose licence allows non-commercial use only.

use crate::gpu::{self, Texture};

pub const SHAPE_PNG: &[u8] = include_bytes!("../assets/shape.png");
pub const LOCAL_WEATHER_PNG: &[u8] = naval_sim::clouds::LOCAL_WEATHER_PNG;
pub const BLUE_NOISE_PNG: &[u8] = include_bytes!("../assets/blue_noise.png");

pub const SHAPE_SIZE: u32 = 128;
pub const LOCAL_WEATHER_SIZE: u32 = 512;
pub const TURBULENCE_SIZE: u32 = 128;
pub const BLUE_NOISE_SIZE: u32 = 128;

/// A volume's texels from its PNG (`size` wide, `size²` tall: layer after layer), x fastest.
pub fn volume_bytes(png: &[u8], size: u32) -> Vec<u8> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("an embedded volume decodes")
        .to_luma8();
    assert_eq!(
        (image.width(), image.height()),
        (size, size * size),
        "volume layout"
    );
    image.into_raw()
}

/// The local weather's RGBA texels as three uploads it: rows flipped (three's `flipY` on loaded images), so texture
/// row 0 is the PNG's last.
pub fn local_weather_bytes() -> Vec<u8> {
    let image = image::load_from_memory_with_format(LOCAL_WEATHER_PNG, image::ImageFormat::Png)
        .expect("the local weather decodes")
        .to_rgba8();
    let size = LOCAL_WEATHER_SIZE as usize;
    assert_eq!(
        (image.width() as usize, image.height() as usize),
        (size, size)
    );
    let bytes = image.into_raw();
    let row = size * 4;
    let mut flipped = vec![0u8; bytes.len()];
    for y in 0..size {
        flipped[y * row..(y + 1) * row]
            .copy_from_slice(&bytes[(size - 1 - y) * row..(size - y) * row]);
    }
    flipped
}

pub fn blue_noise_bytes() -> Vec<u8> {
    let image = image::load_from_memory_with_format(BLUE_NOISE_PNG, image::ImageFormat::Png)
        .expect("the blue noise decodes")
        .to_luma8();
    assert_eq!(
        (image.width(), image.height()),
        (BLUE_NOISE_SIZE, BLUE_NOISE_SIZE)
    );
    image.into_raw()
}

fn upload(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    level: u32,
    size: (u32, u32, u32),
    texel: u32,
    bytes: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * texel),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: size.2,
        },
    );
}

/// The shape volume (`R8Unorm`), uploaded.
pub fn shape(device: &wgpu::Device, queue: &wgpu::Queue) -> Texture {
    let size = SHAPE_SIZE;
    let texture = gpu::texture(
        device,
        "cloud shape",
        (size, size, size),
        wgpu::TextureDimension::D3,
        1,
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    upload(
        queue,
        &texture.texture,
        0,
        (size, size, size),
        1,
        &volume_bytes(SHAPE_PNG, size),
    );
    texture
}

/// The local weather (`Rgba8Unorm`, a full mip chain of 2 × 2 boxes as three's `generateMipmaps`), uploaded.
pub fn local_weather(device: &wgpu::Device, queue: &wgpu::Queue) -> Texture {
    let size = LOCAL_WEATHER_SIZE;
    let mips = size.ilog2() + 1;
    let texture = gpu::texture(
        device,
        "cloud local weather",
        (size, size, 1),
        wgpu::TextureDimension::D2,
        mips,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    let mut level_bytes = local_weather_bytes();
    let mut level_size = size as usize;
    for level in 0..mips {
        upload(
            queue,
            &texture.texture,
            level,
            (level_size as u32, level_size as u32, 1),
            4,
            &level_bytes,
        );
        if level_size > 1 {
            level_bytes = gpu::box_mip(&level_bytes, level_size);
            level_size /= 2;
        }
    }
    texture
}

/// The blue noise (`R8Unorm`, 128²), uploaded.
pub fn blue_noise(device: &wgpu::Device, queue: &wgpu::Queue) -> Texture {
    let size = BLUE_NOISE_SIZE;
    let texture = gpu::texture(
        device,
        "cloud blue noise",
        (size, size, 1),
        wgpu::TextureDimension::D2,
        1,
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    upload(
        queue,
        &texture.texture,
        0,
        (size, size, 1),
        1,
        &blue_noise_bytes(),
    );
    texture
}

/// The turbulence's texture (`Rgba8Unorm`, 128²), for `noise.wgsl`'s `turbulence_main` to fill.
pub fn turbulence(device: &wgpu::Device) -> Texture {
    gpu::texture(
        device,
        "cloud turbulence",
        (TURBULENCE_SIZE, TURBULENCE_SIZE, 1),
        wgpu::TextureDimension::D2,
        1,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_textures_decode() {
        let shape = volume_bytes(SHAPE_PNG, SHAPE_SIZE);
        assert_eq!(shape.len(), 128 * 128 * 128);
        // takram's shape.bin: texels 112–253, mean 203.36.
        let mean = shape.iter().map(|&b| f64::from(b)).sum::<f64>() / shape.len() as f64;
        assert!((mean - 203.36).abs() < 0.01, "{mean}");
        assert_eq!(local_weather_bytes().len(), 512 * 512 * 4);
    }

    /// The blue noise is a dither mask: every level held by the same number of texels, and little low-frequency
    /// energy (each 8 × 8 block's mean stays near the whole mask's).
    #[test]
    fn the_blue_noise_is_even_and_fine_grained() {
        let bytes = blue_noise_bytes();
        let mut counts = [0usize; 256];
        for &b in &bytes {
            counts[usize::from(b)] += 1;
        }
        assert!(counts.iter().all(|&c| c == 64), "levels {:?}", &counts[..8]);
        let size = BLUE_NOISE_SIZE as usize;
        let mut worst: f64 = 0.0;
        let mut total = 0.0;
        for by in 0..size / 8 {
            for bx in 0..size / 8 {
                let mut sum = 0.0;
                for y in 0..8 {
                    for x in 0..8 {
                        sum += f64::from(bytes[(by * 8 + y) * size + bx * 8 + x]);
                    }
                }
                let stray = (sum / 64.0 - 127.5).abs();
                worst = worst.max(stray);
                total += stray;
            }
        }
        let mean = total / ((size / 8) * (size / 8)) as f64;
        // The same levels shuffled into white noise stray 7.6 levels on average and 32 at worst; the mask 2.6 and 9.5.
        assert!(
            mean < 4.0 && worst < 16.0,
            "block means stray {mean} on average, {worst} at worst"
        );
    }
}
