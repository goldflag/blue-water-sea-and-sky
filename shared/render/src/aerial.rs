//! The aerial-perspective volume every material reads: what the air between the eye and a point adds and takes away.
//!
//! A froxel volume over the frame's camera (Hillaire 2020, "A Scalable and Production Ready Sky and Atmosphere
//! Rendering Technique"): the screen in [`AERIAL_SIZE`]`.0` × `.1` columns and each column's ray in `.2` slices of
//! radial distance from the eye, exponentially spaced from [`AERIAL_NEAR`] to [`AERIAL_FAR`]. Each cell holds the
//! light the air scatters toward the eye along the ray up to its distance (in-scatter, linear HDR radiance) and the
//! share of the light from beyond that the air lets through (transmittance), so a surface at that distance is seen as
//! `color × transmittance + inscatter`. One 3D texture holds both: the in-scatter in the first `.2` layers, the opacity
//! (1 − transmittance) in the next `.2` (a material reads each half within itself, so trilinear filtering never mixes
//! them). Zeros are clear air, so a new volume, or a host's stand-in, draws as without air until the sky's layer fills it
//! each frame (naval-clouds' aerial pass: the atmosphere's tables and the weather's haze).
//!
//! It is bound at group 0 beside the camera (bindings 6 and 7), so every scene pipeline samples it by its fragment's
//! world position (`naval::aerial`: `aerial_apply`, `aerial_apply_additive`). [`AerialVolume::sky`] is the dome's slice
//! beside it: the haze along each column's ray to the end of the air (rgb in-scatter, alpha opacity).

/// Columns across, rows down and slices along the view of the volume.
pub const AERIAL_SIZE: (u32, u32, u32) = (32, 32, 32);
/// Radial distance (m) from the eye of the first slice and of the last; the slices between are spaced exponentially
/// (each 38 % farther than the last). The last is the far plane of a view that draws the land beyond a theatre window
/// ([`crate::camera::HORIZON_FAR`]); it was 160 km (each slice a third farther) until 2026-10-09.
/// `shaders/naval/aerial_volume.wgsl` holds the same numbers.
pub const AERIAL_NEAR: f32 = 20.0;
pub const AERIAL_FAR: f32 = 450_000.0;
/// The volume's and the sky slice's format: linear HDR radiance and transmittance in half floats.
pub const AERIAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Bytes of one texel of [`AERIAL_FORMAT`].
pub const AERIAL_TEXEL_BYTES: u32 = 8;
/// Group 0 bindings: the volume and its sampler.
pub const AERIAL_BINDINGS: [u32; 2] = [6, 7];

/// The radial distance (m) of slice `k` (0 to 31, fractional between slices).
pub fn aerial_slice_distance(k: f32) -> f32 {
    AERIAL_NEAR * (AERIAL_FAR / AERIAL_NEAR).powf(k / (AERIAL_SIZE.2 as f32 - 1.0))
}

/// Bytes of the volume and of its sky slice, laid out as a buffer copy of them lays them (rows of
/// [`AERIAL_SIZE`]`.0` texels, 256 bytes).
pub const AERIAL_VOLUME_BYTES: u64 =
    (AERIAL_SIZE.0 * AERIAL_SIZE.1 * AERIAL_SIZE.2 * 2 * AERIAL_TEXEL_BYTES) as u64;
pub const AERIAL_SKY_BYTES: u64 = (AERIAL_SIZE.0 * AERIAL_SIZE.1 * AERIAL_TEXEL_BYTES) as u64;

/// The volume's textures: owned by the renderer, written by the sky's layer, read by every material.
pub struct AerialVolume {
    /// [`AERIAL_SIZE`]`.0` × `.1` × 2 `.2`: rgb in-scatter in the first `.2` layers, rgb opacity in the next.
    pub volume: wgpu::Texture,
    /// The dome's slice, `AERIAL_SIZE.0` × `AERIAL_SIZE.1`: rgb the haze's in-scatter along each column's ray to the
    /// end of the air, a the share of the sky behind it that the haze takes away.
    pub sky: wgpu::Texture,
    pub volume_view: wgpu::TextureView,
    pub sky_view: wgpu::TextureView,
    /// Trilinear, clamped to the edges.
    pub sampler: wgpu::Sampler,
}

impl AerialVolume {
    /// The volume, clear air (wgpu zeroes a new texture).
    pub fn new(device: &wgpu::Device) -> Self {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        let (w, h, d) = AERIAL_SIZE;
        let texture = |label: &str, depth: u32, dimension| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: depth,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension,
                format: AERIAL_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let volume = texture("aerial volume", 2 * d, wgpu::TextureDimension::D3);
        let sky = texture("aerial sky", 1, wgpu::TextureDimension::D2);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("aerial"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            volume_view: volume.create_view(&Default::default()),
            sky_view: sky.create_view(&Default::default()),
            volume,
            sky,
            sampler,
        }
    }

    /// The group-0 layout entries (bindings 6 and 7).
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
        let stages = wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE;
        [
            wgpu::BindGroupLayoutEntry {
                binding: AERIAL_BINDINGS[0],
                visibility: stages,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: AERIAL_BINDINGS[1],
                visibility: stages,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }

    /// The group-0 entries for [`AerialVolume::layout_entries`].
    pub fn group_entries(&self) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding: AERIAL_BINDINGS[0],
                resource: wgpu::BindingResource::TextureView(&self.volume_view),
            },
            wgpu::BindGroupEntry {
                binding: AERIAL_BINDINGS[1],
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ]
    }

    /// Copies the volume, then its sky slice, into `buffer` from `offset`, as [`AERIAL_VOLUME_BYTES`] and
    /// [`AERIAL_SKY_BYTES`] lay them out (the sky's own passes read the air through such a copy).
    pub fn copy_to_buffer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        buffer: &wgpu::Buffer,
        offset: u64,
    ) {
        let (w, h, d) = AERIAL_SIZE;
        let copy = |encoder: &mut wgpu::CommandEncoder,
                    texture: &wgpu::Texture,
                    offset: u64,
                    depth: u32| {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(w * AERIAL_TEXEL_BYTES),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: depth,
                },
            );
        };
        copy(encoder, &self.volume, offset, 2 * d);
        copy(encoder, &self.sky, offset + AERIAL_VOLUME_BYTES, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_run_from_near_to_far() {
        assert!((aerial_slice_distance(0.0) - AERIAL_NEAR).abs() < 1e-3);
        let last = AERIAL_SIZE.2 as f32 - 1.0;
        assert!((aerial_slice_distance(last) / AERIAL_FAR - 1.0).abs() < 1e-5);
        // Each slice a fixed ratio beyond the last: 38 % farther.
        let ratio = aerial_slice_distance(1.0) / aerial_slice_distance(0.0);
        assert!((1.37..1.39).contains(&ratio), "{ratio}");
        assert!((aerial_slice_distance(20.0) / aerial_slice_distance(19.0) - ratio).abs() < 1e-4);
    }

    #[test]
    fn the_wgsl_holds_the_same_slices() {
        let source = crate::shaders::builtin::AERIAL_VOLUME.source;
        assert!(source.contains(&format!("const AERIAL_NEAR: f32 = {AERIAL_NEAR:.1};")));
        assert!(source.contains(&format!("const AERIAL_FAR: f32 = {AERIAL_FAR:.1};")));
        assert!(source.contains(&format!(
            "const AERIAL_SLICES: f32 = {:.1};",
            AERIAL_SIZE.2 as f32
        )));
    }

    #[test]
    fn a_copy_fits_whole_rows() {
        // Buffer copies of textures need rows of 256 bytes.
        assert_eq!(AERIAL_SIZE.0 * AERIAL_TEXEL_BYTES % 256, 0);
        assert_eq!(AERIAL_VOLUME_BYTES % 256, 0);
    }
}
