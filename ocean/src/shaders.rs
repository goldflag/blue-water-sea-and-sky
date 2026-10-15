//! Shader composition: every WGSL file under `shaders/` goes through one naga_oil composer, which holds the composable
//! modules (the renderer core's `naval::camera`, `naval::shadow`, `naval::motion` and `naval::lights`, the clouds' `naval_clouds::shadow`, `naval_ocean::noise`, `naval_ocean::reflection`,
//! `naval_ocean::screen_reflection`, `naval_ocean::water_body`, `naval_ocean::foam`, `naval_ocean::point_lights`, `naval_ocean::mx_noise`, `naval_ocean::wake`, `naval_ocean::rain`, `naval_ocean::spray_haze`, and the mirror of the sky
//! lane's `naval_sky::environment`) and turns
//! entry files into naga modules for wgpu.

use std::borrow::Cow;

use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, ComposerError, NagaModuleDescriptor, ShaderDefValue,
    ShaderLanguage, ShaderType,
};

/// A composable module: import path is declared inside the source with `#define_import_path`.
struct Composable {
    path: &'static str,
    source: &'static str,
}

/// In dependency order: a module is registered after everything it imports.
const COMPOSABLE: &[Composable] = &[
    Composable {
        path: "naval-ocean/shaders/water_properties.wgsl",
        source: include_str!("../shaders/water_properties.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/noise.wgsl",
        source: include_str!("../shaders/noise.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/sky_environment.wgsl",
        source: include_str!("../shaders/sky_environment.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/reflection.wgsl",
        source: include_str!("../shaders/reflection.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/water_body.wgsl",
        source: include_str!("../shaders/water_body.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/foam.wgsl",
        source: include_str!("../shaders/foam.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/point_lights.wgsl",
        source: include_str!("../shaders/point_lights.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/mx_noise.wgsl",
        source: include_str!("../shaders/mx_noise.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/wake.wgsl",
        source: include_str!("../shaders/wake.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/waves.wgsl",
        source: include_str!("../shaders/waves.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/screen_reflection.wgsl",
        source: include_str!("../shaders/screen_reflection.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/underwater_ray.wgsl",
        source: include_str!("../shaders/underwater_ray.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/rain.wgsl",
        source: include_str!("../shaders/rain.wgsl"),
    },
    Composable {
        path: "naval-ocean/shaders/spray_haze.wgsl",
        source: include_str!("../shaders/spray_haze.wgsl"),
    },
];

/// An entry file: compute or render stages.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub path: &'static str,
    pub source: &'static str,
}

pub const FFT: Entry = Entry {
    path: "naval-ocean/shaders/fft.wgsl",
    source: include_str!("../shaders/fft.wgsl"),
};
pub const RESOLVE: Entry = Entry {
    path: "naval-ocean/shaders/resolve.wgsl",
    source: include_str!("../shaders/resolve.wgsl"),
};
pub const MIPS: Entry = Entry {
    path: "naval-ocean/shaders/mips.wgsl",
    source: include_str!("../shaders/mips.wgsl"),
};
pub const SURFACE: Entry = Entry {
    path: "naval-ocean/shaders/surface.wgsl",
    source: include_str!("../shaders/surface.wgsl"),
};
pub const ENVIRONMENT_FILL: Entry = Entry {
    path: "naval-ocean/shaders/environment_fill.wgsl",
    source: include_str!("../shaders/environment_fill.wgsl"),
};
pub const BACKDROP: Entry = Entry {
    path: "naval-ocean/shaders/backdrop.wgsl",
    source: include_str!("../shaders/backdrop.wgsl"),
};
pub const STAMPS: Entry = Entry {
    path: "naval-ocean/shaders/stamps.wgsl",
    source: include_str!("../shaders/stamps.wgsl"),
};
pub const UNDERWATER_PROBE: Entry = Entry {
    path: "naval-ocean/shaders/underwater_probe.wgsl",
    source: include_str!("../shaders/underwater_probe.wgsl"),
};
pub const HULL_HEIGHTS: Entry = Entry {
    path: "naval-ocean/shaders/hull_heights.wgsl",
    source: include_str!("../shaders/hull_heights.wgsl"),
};
pub const UNDERWATER: Entry = Entry {
    path: "naval-ocean/shaders/underwater.wgsl",
    source: include_str!("../shaders/underwater.wgsl"),
};
pub const UNDERWATER_MOTES: Entry = Entry {
    path: "naval-ocean/shaders/underwater_motes.wgsl",
    source: include_str!("../shaders/underwater_motes.wgsl"),
};
pub const REFLECTION_TILES: Entry = Entry {
    path: "naval-ocean/shaders/reflection_tiles.wgsl",
    source: include_str!("../shaders/reflection_tiles.wgsl"),
};
pub const WAKE_FIELD: Entry = Entry {
    path: "naval-ocean/shaders/wake_field.wgsl",
    source: include_str!("../shaders/wake_field.wgsl"),
};
pub const RAIN_HEIGHTS: Entry = Entry {
    path: "naval-ocean/shaders/rain_heights.wgsl",
    source: include_str!("../shaders/rain_heights.wgsl"),
};
pub const CAUSTICS: Entry = Entry {
    path: "naval-ocean/shaders/caustics.wgsl",
    source: include_str!("../shaders/caustics.wgsl"),
};
pub const SPINDRIFT: Entry = Entry {
    path: "naval-ocean/shaders/spindrift.wgsl",
    source: include_str!("../shaders/spindrift.wgsl"),
};

/// Every file the ocean's pipelines are built from, with its path on disk (hot reload), for the renderer's library:
/// the modules in dependency order, then the entries. The stand-in environment's fill and the backdrop (tools only)
/// are not among them.
pub const FILES: [naval_render::ShaderFile; 28] = [
    naval_render::shader_file!("shaders/water_properties.wgsl"),
    naval_render::shader_file!("shaders/noise.wgsl"),
    naval_render::shader_file!("shaders/sky_environment.wgsl"),
    naval_render::shader_file!("shaders/reflection.wgsl"),
    naval_render::shader_file!("shaders/water_body.wgsl"),
    naval_render::shader_file!("shaders/foam.wgsl"),
    naval_render::shader_file!("shaders/point_lights.wgsl"),
    naval_render::shader_file!("shaders/mx_noise.wgsl"),
    naval_render::shader_file!("shaders/wake.wgsl"),
    naval_render::shader_file!("shaders/waves.wgsl"),
    naval_render::shader_file!("shaders/screen_reflection.wgsl"),
    naval_render::shader_file!("shaders/underwater_ray.wgsl"),
    naval_render::shader_file!("shaders/rain.wgsl"),
    naval_render::shader_file!("shaders/spray_haze.wgsl"),
    naval_render::shader_file!("shaders/underwater_probe.wgsl"),
    naval_render::shader_file!("shaders/hull_heights.wgsl"),
    naval_render::shader_file!("shaders/rain_heights.wgsl"),
    naval_render::shader_file!("shaders/caustics.wgsl"),
    naval_render::shader_file!("shaders/underwater.wgsl"),
    naval_render::shader_file!("shaders/underwater_motes.wgsl"),
    naval_render::shader_file!("shaders/fft.wgsl"),
    naval_render::shader_file!("shaders/resolve.wgsl"),
    naval_render::shader_file!("shaders/mips.wgsl"),
    naval_render::shader_file!("shaders/surface.wgsl"),
    naval_render::shader_file!("shaders/stamps.wgsl"),
    naval_render::shader_file!("shaders/wake_field.wgsl"),
    naval_render::shader_file!("shaders/spindrift.wgsl"),
    naval_render::shader_file!("shaders/reflection_tiles.wgsl"),
];

/// Registers the ocean's files with the renderer's library (and the clouds' shadow functions the surface imports, when
/// no other layer has), so its pipelines can be built on the registry and hot-reload.
pub fn register(
    ctx: &mut naval_render::RenderContext<'_>,
) -> Result<(), naval_render::ShaderError> {
    if ctx
        .library
        .text(naval_clouds::shaders::SHADOW_FILE.path)
        .is_none()
    {
        ctx.add_shader(naval_clouds::shaders::SHADOW_FILE)?;
    }
    for file in FILES {
        ctx.add_shader(file)?;
    }
    Ok(())
}

/// Integer shader defs as the library takes them.
pub fn library_defs(defs: &[(&str, u32)]) -> naval_render::ShaderDefs {
    defs.iter()
        .map(|(name, value)| (name.to_string(), ShaderDefValue::UInt(*value)))
        .collect()
}

/// A composer holding every composable module.
pub fn composer() -> Result<Composer, Box<ComposerError>> {
    let mut composer = Composer::default();
    // The renderer core's camera, its sun shadow maps (group 0 beside the camera), its motion block and the scene's
    // point lights, and the clouds' shadow functions (bindings of the caller's choosing).
    let builtin = &naval_render::shaders::builtin::MODULES;
    for module in builtin.iter().filter(|m| {
        [
            "camera.wgsl",
            "shadow.wgsl",
            "motion.wgsl",
            "lights.wgsl",
            "aerial_volume.wgsl",
            "aerial.wgsl",
        ]
        .iter()
        .any(|n| m.path.ends_with(n))
    }) {
        composer
            .add_composable_module(ComposableModuleDescriptor {
                source: module.source,
                file_path: module.path,
                language: ShaderLanguage::Wgsl,
                ..Default::default()
            })
            .map_err(Box::new)?;
    }
    composer
        .add_composable_module(ComposableModuleDescriptor {
            source: naval_clouds::shaders::SHADOW_WGSL,
            file_path: "naval-clouds/shaders/shadow.wgsl",
            language: ShaderLanguage::Wgsl,
            ..Default::default()
        })
        .map_err(Box::new)?;
    for module in COMPOSABLE {
        composer
            .add_composable_module(ComposableModuleDescriptor {
                source: module.source,
                file_path: module.path,
                language: ShaderLanguage::Wgsl,
                ..Default::default()
            })
            .map_err(Box::new)?;
    }
    Ok(composer)
}

/// Compose `entry` into a naga module with integer shader defs, e.g. `("CASCADES", 3)`.
pub fn naga_module(
    composer: &mut Composer,
    entry: Entry,
    defs: &[(&str, u32)],
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    composer
        .make_naga_module(NagaModuleDescriptor {
            source: entry.source,
            file_path: entry.path,
            shader_type: ShaderType::Wgsl,
            shader_defs: defs
                .iter()
                .map(|(name, value)| (name.to_string(), ShaderDefValue::UInt(*value)))
                .collect(),
            ..Default::default()
        })
        .map_err(Box::new)
}

/// Compose `entry` and create the wgpu shader module; a composition error names the file and line.
pub fn shader_module(
    device: &wgpu::Device,
    entry: Entry,
    defs: &[(&str, u32)],
) -> wgpu::ShaderModule {
    let mut composer =
        composer().unwrap_or_else(|error| panic!("{}", error.emit_to_string(&Composer::default())));
    let module = naga_module(&mut composer, entry, defs).unwrap_or_else(|error| {
        panic!(
            "composing {}: {}",
            entry.path,
            error.emit_to_string(&composer)
        )
    });
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(entry.path),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry composes and validates without a GPU.
    #[test]
    fn every_shader_composes_and_validates() {
        for entry in [
            FFT,
            RESOLVE,
            MIPS,
            SURFACE,
            ENVIRONMENT_FILL,
            BACKDROP,
            WAKE_FIELD,
            STAMPS,
            UNDERWATER_PROBE,
            HULL_HEIGHTS,
            UNDERWATER,
            UNDERWATER_MOTES,
            SPINDRIFT,
            RAIN_HEIGHTS,
            CAUSTICS,
            REFLECTION_TILES,
        ] {
            let mut composer = composer()
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&Composer::default())));
            // The surface also with the scene's point lights compiled in.
            let lit: &[&[(&str, u32)]] = if entry.path == SURFACE.path {
                &[&[], &[("POINT_LIGHTS", 1)]]
            } else {
                &[&[]]
            };
            for extra in lit {
                let defs: Vec<(&str, u32)> = [("CASCADES", 3), ("REFLECTION_STEPS", 16)]
                    .into_iter()
                    .chain(extra.iter().copied())
                    .collect();
                let module = naga_module(&mut composer, entry, &defs).unwrap_or_else(|error| {
                    panic!("{}: {}", entry.path, error.emit_to_string(&composer))
                });
                wgpu::naga::valid::Validator::new(
                    wgpu::naga::valid::ValidationFlags::all(),
                    wgpu::naga::valid::Capabilities::default(),
                )
                .validate(&module)
                .unwrap_or_else(|error| panic!("{}: {error:?}", entry.path));
            }
        }
    }
}
