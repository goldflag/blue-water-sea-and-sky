//! A stand-in sky behind the sea for the ocean lane's stills: the environment atlas's sharpest level along each view
//! ray (`shaders/backdrop.wgsl`), drawn in the renderer core's Sky phase. The sky lane draws the real dome.

use naval_render::{DrawContext, Phase, PrepareContext, SceneLayer};

use crate::{HDR_FORMAT, shaders};

/// A `SceneLayer` drawing the environment behind everything else.
pub struct StandInSky {
    pipeline: wgpu::RenderPipeline,
    environment: wgpu::BindGroup,
}

impl StandInSky {
    /// With the renderer's camera layout (group 0) and the environment's layout and bind group (group 1).
    pub fn new(
        device: &wgpu::Device,
        camera: &wgpu::BindGroupLayout,
        environment_layout: &wgpu::BindGroupLayout,
        environment: &wgpu::BindGroup,
    ) -> Self {
        let module = shaders::shader_module(device, shaders::BACKDROP, &[]);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stand-in sky"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("stand-in sky"),
                    bind_group_layouts: &[Some(camera), Some(environment_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(naval_render::scene_depth_state(false)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            environment: environment.clone(),
        }
    }

    /// Show another environment (a new atlas or scene).
    pub fn set_environment(&mut self, environment: &wgpu::BindGroup) {
        self.environment = environment.clone();
    }
}

impl SceneLayer for StandInSky {
    fn label(&self) -> &str {
        "stand-in sky"
    }

    fn prepare(&mut self, _ctx: &mut PrepareContext<'_>) {}

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, _ctx: &DrawContext<'_>) {
        if phase == Phase::Sky {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, &self.environment, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
