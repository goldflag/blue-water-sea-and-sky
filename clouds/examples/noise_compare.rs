//! Runs the procedural texture generators (`shaders/noise.wgsl`) and compares their output with takram's shipped
//! textures: `cargo run -p naval-clouds --example noise_compare --release -- <takram packages/clouds/assets dir>`.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("assets dir"));
    let gpu = native_offscreen::Gpu::headless()?;
    let device = &gpu.device;
    let source = include_str!("../shaders/noise.wgsl");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("noise"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let make = |label: &str, size: (u32, u32, u32), dimension: wgpu::TextureDimension| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: size.2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension,
            format,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let shape = make("shape", (128, 128, 128), wgpu::TextureDimension::D3);
    let detail = make("detail", (32, 32, 32), wgpu::TextureDimension::D3);
    let weather = make("weather", (512, 512, 1), wgpu::TextureDimension::D2);
    let turbulence = make("turbulence", (128, 128, 1), wgpu::TextureDimension::D2);
    let views =
        [&shape, &detail, &weather, &turbulence].map(|t| t.create_view(&Default::default()));
    let storage = |binding: u32, dim: wgpu::TextureViewDimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: dim,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage(0, wgpu::TextureViewDimension::D3),
            storage(1, wgpu::TextureViewDimension::D3),
            storage(2, wgpu::TextureViewDimension::D2),
            storage(3, wgpu::TextureViewDimension::D2),
        ],
    });
    let entries: Vec<wgpu::BindGroupEntry<'_>> = views
        .iter()
        .enumerate()
        .map(|(i, v)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(v),
        })
        .collect();
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_bind_group(0, &group, &[]);
        pass.set_pipeline(&pipeline("shape_main"));
        pass.dispatch_workgroups(32, 32, 32);
        pass.set_pipeline(&pipeline("detail_main"));
        pass.dispatch_workgroups(8, 8, 8);
        pass.set_pipeline(&pipeline("weather_main"));
        pass.dispatch_workgroups(64, 64, 1);
        pass.set_pipeline(&pipeline("turbulence_main"));
        pass.dispatch_workgroups(16, 16, 1);
    }
    gpu.queue.submit([encoder.finish()]);
    let read = |texture: &wgpu::Texture| -> Vec<u8> {
        let size = texture.size();
        let row = size.width * 4;
        let padded = row.div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * size.height * size.depth_or_array_layers),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        gpu.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().expect("mapped");
        let mut out = Vec::with_capacity((row * size.height * size.depth_or_array_layers) as usize);
        for r in 0..(size.height * size.depth_or_array_layers) {
            let start = (r * padded) as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        out
    };
    let stats = |name: &str, ours: &[u8], theirs: &[u8], channels: usize, flip: Option<usize>| {
        for c in 0..channels {
            let mut sum = 0f64;
            let mut max = 0u8;
            let mut ours_mean = 0f64;
            let mut theirs_mean = 0f64;
            let n = theirs.len() / channels;
            for i in 0..n {
                let j = match flip {
                    Some(size) => {
                        let (x, y) = (i % size, i / size);
                        (size - 1 - y) * size + x
                    }
                    None => i,
                };
                let a = ours[j * 4 + c];
                let b = theirs[i * channels + c];
                ours_mean += f64::from(a);
                theirs_mean += f64::from(b);
                let d = a.abs_diff(b);
                sum += f64::from(d);
                max = max.max(d);
            }
            println!(
                "{name} channel {c}: mean |diff| {:.3}, max {max}, means ours {:.2} theirs {:.2}",
                sum / n as f64,
                ours_mean / n as f64,
                theirs_mean / n as f64
            );
        }
    };
    let shape_ours = read(&shape);
    let detail_ours = read(&detail);
    let weather_ours = read(&weather);
    let turbulence_ours = read(&turbulence);
    let shape_theirs = std::fs::read(dir.join("shape.bin"))?;
    let detail_theirs = std::fs::read(dir.join("shape_detail.bin"))?;
    stats("shape", &shape_ours, &shape_theirs, 1, None);
    stats("detail", &detail_ours, &detail_theirs, 1, None);
    let weather_theirs = image::open(dir.join("local_weather.png"))?
        .to_rgba8()
        .into_raw();
    let turbulence_theirs = image::open(dir.join("turbulence.png"))?
        .to_rgba8()
        .into_raw();
    stats("weather", &weather_ours, &weather_theirs, 4, None);
    stats(
        "weather flipped",
        &weather_ours,
        &weather_theirs,
        4,
        Some(512),
    );
    stats("turbulence", &turbulence_ours, &turbulence_theirs, 4, None);
    stats(
        "turbulence flipped",
        &turbulence_ours,
        &turbulence_theirs,
        4,
        Some(128),
    );
    Ok(())
}
