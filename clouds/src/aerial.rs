//! The aerial perspective for everything in the world: one compute pass a frame fills the renderer's froxel volume
//! (`naval_render::aerial`, which every material reads at group 0) for the frame's camera, from the sky's atmosphere
//! (takram's tables, as the dome and the clouds take them) and the weather's haze, lit by the sun, the moon, the sky's
//! environment and the clouds' ground shadow (`shaders/aerial.wgsl`). The sky's own passes (the dome, the clouds'
//! march) read the copy [`naval_sky::aerial::AerialCopies`] holds, made right after.
//!
//! The haze is the sky's (`naval_sky::haze::Haze`, [`Sky::weather_haze`]): a marine boundary layer whose extinction at
//! sea level the visibility sets (`naval_sky::haze::SIGHTING_DEPTH` over it, the atmosphere's own extinction there
//! taken off). A view sees the atmosphere's `aerial_scale` share of the tables' air (`naval_sky::model::aerial`: half
//! on a fair day), so the land seen from the air keeps its colour; the sea-level visibility is the same.

use bytemuck::{Pod, Zeroable};
use naval_render::{
    AERIAL_SIZE, AerialVolume, CameraUniforms, GpuTimer, MakePipelines, PipelineId,
    PipelineRegistry, RenderContext, ShaderError,
};
use naval_sky::Sky;

use crate::gpu;
use crate::layer::CloudShadow;
use crate::shaders;

/// `AerialParams` in `shaders/aerial.wgsl`, 128 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct AerialUniforms {
    inverse_view_projection: [f32; 16],
    eye: [f32; 4],
    haze: [f32; 4],
    phase: [f32; 4],
    view: [f32; 4],
}

/// The pass's pipeline on the renderer's registry ([`AerialPerspective::register_pipelines`]).
#[derive(Clone, Copy, Debug)]
pub struct AerialPipelines {
    pass: PipelineId,
}

/// The aerial-perspective pass.
pub struct AerialPerspective {
    /// Off, the volume holds clear air (the materials draw as without air).
    pub enabled: bool,
    buffer: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    /// Groups 1 to 3 (the sky's, the environment's, the clouds' shadow), for the registry's recipe.
    layouts: [wgpu::BindGroupLayout; 3],
    /// `None` while the registry's build is pending (a deferred sky): the pass then waits.
    pipeline: Option<wgpu::ComputePipeline>,
    /// Group 0 over the renderer's volume (its textures never change).
    group: Option<wgpu::BindGroup>,
}

fn pass_layouts(
    layout: &wgpu::BindGroupLayout,
    layouts: &[wgpu::BindGroupLayout; 3],
) -> Vec<wgpu::BindGroupLayout> {
    std::iter::once(layout.clone())
        .chain(layouts.iter().cloned())
        .collect()
}

impl AerialPerspective {
    /// The pass over the sky's group layout, the environment's and the clouds' shadow's.
    pub fn new(
        device: &wgpu::Device,
        sky: &Sky,
        shadow: &CloudShadow,
        make: MakePipelines<'_>,
    ) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aerial perspective"),
            size: std::mem::size_of::<AerialUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let format = naval_render::AERIAL_FORMAT;
        let layout = gpu::layout(
            device,
            "aerial perspective",
            &[
                gpu::uniform_entry(0),
                gpu::storage_entry(1, format, wgpu::TextureViewDimension::D3),
                gpu::storage_2d_entry(2, format),
            ],
        );
        let layouts = [
            sky.bind_group_layout().clone(),
            sky.environment().bind_group_layout.clone(),
            shadow.bind_group_layout.clone(),
        ];
        let pipeline = make.is_now().then(|| {
            let module =
                shaders::shader_module(device, shaders::AERIAL_WGSL, shaders::AERIAL_PATH, &[]);
            let all = pass_layouts(&layout, &layouts);
            let refs: Vec<Option<&wgpu::BindGroupLayout>> = all.iter().map(Some).collect();
            gpu::compute_pipeline(device, "aerial perspective", &module, "aerial_main", &refs)
        });
        Self {
            enabled: true,
            buffer,
            layout,
            layouts,
            pipeline,
            group: None,
        }
    }

    /// Builds the pass's pipeline through the renderer's registry. The clouds' files must be registered first.
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<AerialPipelines, ShaderError> {
        let pass = ctx.add_pipeline(
            "aerial perspective",
            shaders::AERIAL_PATH,
            Vec::new(),
            naval_render::compute_recipe(pass_layouts(&self.layout, &self.layouts), "aerial_main"),
        )?;
        Ok(AerialPipelines { pass })
    }

    /// Takes the registry's current pipeline; whether it changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &AerialPipelines) -> bool {
        let new = registry.compute(ids.pass);
        if self.pipeline.as_ref() != Some(new) {
            self.pipeline = Some(new.clone());
            return true;
        }
        false
    }

    /// Fills `volume` for `camera` (the frame's, as the materials draw with it), then copies it into the sky's group
    /// for the dome and the clouds. After the sky's update and the clouds' shadows, before the scene pass.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        volume: &AerialVolume,
        sky: &Sky,
        shadow: &CloudShadow,
        shadow_heights: [f32; 2],
        timer: Option<&GpuTimer>,
    ) {
        let Some(pipeline) = &self.pipeline else {
            return;
        };
        let haze = sky.weather_haze();
        // Under the sea the water's own view takes over: the air there is left clear.
        let on = self.enabled && camera.position[1] > 0.0;
        let uniforms = AerialUniforms {
            inverse_view_projection: camera.inverse_view_projection,
            eye: [
                camera.position[0],
                camera.position[1],
                camera.position[2],
                if on { 1.0 } else { 0.0 },
            ],
            haze: [
                haze.sighting_extinction() as f32,
                haze.top.max(0.0) as f32,
                haze.scale_height.max(1.0) as f32,
                haze.taper.max(1.0) as f32,
            ],
            phase: [
                haze.asymmetry.clamp(-0.95, 0.95) as f32,
                haze.albedo.clamp(0.0, 1.0) as f32,
                shadow_heights[0],
                shadow_heights[1],
            ],
            view: [sky.air().aerial_scale.clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniforms));
        let group = self.group.get_or_insert_with(|| {
            gpu::bind_group(
                device,
                "aerial perspective",
                &self.layout,
                &[
                    (0, self.buffer.as_entire_binding()),
                    (1, wgpu::BindingResource::TextureView(&volume.volume_view)),
                    (2, wgpu::BindingResource::TextureView(&volume.sky_view)),
                ],
            )
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("aerial perspective"),
                timestamp_writes: timer.and_then(|timer| timer.compute_pass("aerial perspective")),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &*group, &[]);
            pass.set_bind_group(1, sky.bind_group(), &[]);
            pass.set_bind_group(2, &sky.environment().bind_group, &[]);
            pass.set_bind_group(3, &shadow.bind_group, &[]);
            // Workgroups of 2 × 2 columns, each the column's 32 slices.
            pass.dispatch_workgroups(AERIAL_SIZE.0 / 2, AERIAL_SIZE.1 / 2, 1);
        }
        sky.aerial().copy_from(encoder, volume);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_match_the_wgsl() {
        assert_eq!(std::mem::size_of::<AerialUniforms>(), 128);
        let module = shaders::compose(shaders::AERIAL_WGSL, shaders::AERIAL_PATH, &[])
            .unwrap_or_else(|report| panic!("{report}"));
        let params = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("AerialParams"))
            .expect("AerialParams in the pass");
        assert_eq!(params.1.inner.size(module.to_ctx()), 128);
    }
}
