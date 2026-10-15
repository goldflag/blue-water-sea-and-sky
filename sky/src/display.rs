//! A display transform for sky captures (`shaders/display.wgsl`): the game's look and
//! AgX, from a linear HDR texture to an sRGB target.

use crate::gpu;
use crate::shaders;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniforms {
    exposure: f32,
    grade: f32,
    padding: [f32; 2],
}

pub struct DisplayTransform {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
}

impl DisplayTransform {
    /// For an output of `format` (an sRGB format, so the hardware encodes what AgX leaves linear).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = shaders::shader_module(
            device,
            shaders::DISPLAY_WGSL,
            "naval-sky/shaders/display.wgsl",
        );
        let layout = gpu::layout(
            device,
            "sky display",
            &[gpu::texture_entry(0, false), gpu::uniform_entry(1, false)],
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky display"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky display"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("display_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("display_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky display"),
            size: std::mem::size_of::<DisplayUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            uniforms,
        }
    }

    /// Grades `radiance` (unless `graded` says it already is) and tone maps it into `output`, same size.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        radiance: &wgpu::TextureView,
        output: &wgpu::TextureView,
        exposure: f32,
        graded: bool,
    ) {
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&DisplayUniforms {
                exposure,
                grade: if graded { 0.0 } else { 1.0 },
                padding: [0.0; 2],
            }),
        );
        let group = gpu::bind_group(
            device,
            "sky display",
            &self.layout,
            &[
                (0, wgpu::BindingResource::TextureView(radiance)),
                (1, self.uniforms.as_entire_binding()),
            ],
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sky display"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
