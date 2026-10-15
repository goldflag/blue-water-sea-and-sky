//! GPU check of the sea's heights about hulls (`naval_ocean::hull_heights`, the wet band's sea): with the wave field
//! flat and the wake calm, every texel of a coupled hull's patch holds the sea around hulls `coupled_height` gives on the
//! CPU at that texel's world point (`hullSea.ts`'s blend of combat's two sines), so the patches land where they say and
//! read the coupling as the surface does; with the sea drawn everywhere, `everywhere_height`. Needs a GPU adapter.

use std::sync::mpsc;

use bytemuck::Zeroable;
use glam::Vec3;
use native_offscreen::Gpu;
use naval_ocean::field::WaveUniforms;
use naval_ocean::hull_heights::{
    HULL_HEIGHT_SLOTS, HullHeights, HullPatch, PATCH_ACROSS, PATCH_ALONG,
};
use naval_ocean::hull_sea::{
    HullFootprint, HullSeaUniforms, HullSeaWave, coupled_height, everywhere_height, hull_sea_slot,
    hull_sea_wavelength,
};
use naval_ocean::{QualityTier, WaveField};
use naval_render::{Camera, CameraUniforms, FrameTime, MakePipelines};

/// One layer of the heights as metres, row by row.
fn read_layer(gpu: &Gpu, texture: &wgpu::Texture, layer: u32) -> Vec<f32> {
    let row = PATCH_ALONG * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hull heights read-back"),
        size: u64::from(padded) * u64::from(PATCH_ACROSS),
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
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(PATCH_ACROSS),
            },
        },
        wgpu::Extent3d {
            width: PATCH_ALONG,
            height: PATCH_ACROSS,
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
    let mut values = Vec::with_capacity((PATCH_ALONG * PATCH_ACROSS) as usize);
    for chunk in mapped.chunks(padded as usize) {
        values.extend(bytemuck::cast_slice::<u8, f32>(&chunk[..row as usize]));
    }
    values
}

#[test]
fn patches_hold_the_coupled_sea_at_their_texels() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let device = &gpu.device;
    let field = WaveField::new(device, &QualityTier::High.cascades());
    let layout = field.surface_layout();

    // A flat field: no cascades (the tiles set, so the long waves' read stays finite), zero textures.
    let uniforms = WaveUniforms {
        tile: [1024.0, 181.0, 31.0, 1.0],
        ..WaveUniforms::default()
    };
    let waves = [
        HullSeaWave {
            amplitude: 2.86,
            wavelength: 249.0,
            direction: 0.6,
            phase: 1.3,
        },
        HullSeaWave {
            amplitude: 1.23,
            wavelength: 142.0,
            direction: 1.4,
            phase: 3.3,
        },
    ];
    let hulls = [
        HullFootprint {
            x: 1120.0,
            z: -840.0,
            bearing: 0.3,
            length: 250.0,
            beam: 36.0,
            contact: 1.0,
        },
        HullFootprint {
            x: 1300.0,
            z: -700.0,
            bearing: 2.2,
            length: 112.0,
            beam: 12.0,
            contact: 0.6,
        },
    ];
    let time = 437.25;
    let origin = (1150.0, -800.0);
    let mut hull_sea = HullSeaUniforms::default();
    hull_sea.set(&waves, time, time, 0.0, &hulls, origin);

    use wgpu::util::DeviceExt;
    let buffer = |label, contents: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })
    };
    let wave_buffer = buffer("flat waves", bytemuck::bytes_of(&uniforms));
    let hull_buffer = buffer("hull sea", bytemuck::bytes_of(&hull_sea));
    let flat = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("flat field"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    // No coast: the open sea's placeholders, which the shader does not read.
    let shore = naval_ocean::shore::ShoreTextures::open(device);
    let view = wgpu::BindingResource::TextureView(&flat);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flat waves"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: view.clone(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: view.clone(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: view,
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wave_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: hull_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&shore.heights),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&shore.exposure),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let mut heights = HullHeights::new(device, layout, MakePipelines::Now);
    heights.place(&hulls);
    assert_eq!(heights.placed().len(), hulls.len());
    let wavelength = hull_sea_wavelength(&waves);
    let slots: Vec<_> = hulls.iter().map(|h| hull_sea_slot(h, wavelength)).collect();
    // A camera without a projection sees every patch at full resolution; one 6 km off, both, thinned.
    let far = Camera::look_at(
        Vec3::new(1200.0, 40.0, 5200.0),
        Vec3::new(1200.0, 0.0, -800.0),
    )
    .uniforms(1920, 1080, FrameTime::fixed(0, 1.0 / 60.0));
    for (everywhere, camera, full) in [
        (false, CameraUniforms::zeroed(), true),
        (false, far, false),
        (true, CameraUniforms::zeroed(), true),
    ] {
        hull_sea.set_everywhere(everywhere);
        gpu.queue
            .write_buffer(&hull_buffer, 0, bytemuck::bytes_of(&hull_sea));
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(heights.encode(
            device,
            &mut naval_render::UploadSink::Queue(&gpu.queue),
            &mut encoder,
            &camera,
            &group,
            None
        ));
        gpu.queue.submit([encoder.finish()]);
        assert_eq!(heights.uniforms().count(), hulls.len());
        let texture = heights.texture().clone();
        assert_eq!(texture.depth_or_array_layers(), HULL_HEIGHT_SLOTS as u32);
        for (layer, hull) in hulls.iter().enumerate() {
            let reach = heights.uniforms().patches[layer * 2 + 1];
            let texels = (reach[2] as u32, reach[3] as u32);
            assert_eq!(
                texels == (PATCH_ALONG, PATCH_ACROSS),
                full,
                "patch {layer}: {texels:?}"
            );
            let got = read_layer(&gpu, &texture, layer as u32);
            let patch = HullPatch::about(hull);
            let (mut worst, mut largest) = (0.0f64, 0.0f64);
            for j in (0..texels.1).step_by(3) {
                for i in (0..texels.0).step_by(5) {
                    let (x, z) = patch.texel_of(i, j, texels);
                    let want = if everywhere {
                        everywhere_height(0.0, 0.0, &waves, time, x, z)
                    } else {
                        coupled_height(0.0, 0.0, &slots, &waves, time, x, z)
                    };
                    let value = f64::from(got[(j * PATCH_ALONG + i) as usize]);
                    worst = worst.max((value - want).abs());
                    largest = largest.max(want.abs());
                }
            }
            assert!(largest > 1.0, "patch {layer} reads a flat sea: {largest}");
            // f32 phases about the origin and sines on the GPU: millimetres.
            assert!(worst < 5e-3, "patch {layer}: worst {worst} m of {largest}");
        }
    }
}
