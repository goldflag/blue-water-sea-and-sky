//! The sky's mean radiance for the renderer core's eye adaptation (`native/docs/render-core.md`, "The sky mean
//! seam"): a one-invocation compute pass writes `env_radiance(up, 1)` of the sky's environment, as the web's
//! `EyeAdaptation.ts` reads `pmremTexture(environment, up, 1)`, into the renderer's `exposure.sky_mean` buffer every
//! frame. Put the layer after the sky's own (its prepare refreshes the environment) and set the renderer's sky mean
//! to `SkyMean::Gpu`. Moved here from the harness's `sky` scene, so any host meters the real sky without glue.

use naval_render::{
    DrawContext, Phase, PipelineId, PrepareContext, RenderContext, SKY_MEAN_BUFFER, SceneLayer,
    ShaderError, compute_recipe, shader_file,
};

use crate::EnvironmentMap;

/// Writes the sky's mean radiance for the eye adaptation in `prepare`.
pub struct SkyMeanLayer {
    pipeline: PipelineId,
    environment: wgpu::BindGroup,
    output: wgpu::BindGroup,
}

impl SkyMeanLayer {
    /// A layer metering `environment` (the sky's, or the game sky's `sky.sky.environment()`).
    pub fn new(
        ctx: &mut RenderContext<'_>,
        environment: &EnvironmentMap,
    ) -> Result<Self, ShaderError> {
        // This crate's environment module, under the path its own composer uses, with its disk path (hot reload),
        // unless the sky registered it.
        if ctx
            .library
            .text(crate::shaders::ENVIRONMENT_FILE.path)
            .is_none()
        {
            ctx.add_shader(crate::shaders::ENVIRONMENT_FILE)?;
        }
        let shader = shader_file!("shaders/sky_mean.wgsl");
        ctx.add_shader(shader)?;
        let output_layout = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sky mean"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let pipeline = ctx.add_pipeline(
            "sky mean",
            shader.path,
            Vec::new(),
            compute_recipe(
                vec![environment.bind_group_layout.clone(), output_layout.clone()],
                "main",
            ),
        )?;
        let buffer = ctx
            .resources
            .buffer(SKY_MEAN_BUFFER)
            .expect("the renderer registers the sky mean buffer");
        let output = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky mean"),
            layout: &output_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.buffer.as_entire_binding(),
            }],
        });
        Ok(Self {
            pipeline,
            environment: environment.bind_group.clone(),
            output,
        })
    }

    /// Meter another environment (the sky rebuilt its atlas on a quality change).
    pub fn set_environment(&mut self, environment: &EnvironmentMap) {
        self.environment = environment.bind_group.clone();
    }
}

impl SceneLayer for SkyMeanLayer {
    fn label(&self) -> &str {
        "sky mean"
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let mut pass = ctx
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky mean"),
                // In the frame's GPU timings beside the renderer's own passes.
                timestamp_writes: ctx.timer.compute_pass("sky mean"),
            });
        pass.set_pipeline(ctx.pipelines.compute(self.pipeline));
        pass.set_bind_group(0, &self.environment, &[]);
        pass.set_bind_group(1, &self.output, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    fn draw(&self, _phase: Phase, _pass: &mut wgpu::RenderPass<'_>, _ctx: &DrawContext<'_>) {}
}
