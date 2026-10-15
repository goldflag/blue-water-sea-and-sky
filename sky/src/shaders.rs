//! The sky's WGSL, composed with naga_oil: the shared modules (`naval_sky::bruneton`, `common`, `noise`,
//! `atmosphere`, `celestial`, `environment`) and the passes that import them.

use std::borrow::Cow;

use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, ComposerError, NagaModuleDescriptor, ShaderLanguage,
    ShaderType,
};

pub const BRUNETON_WGSL: &str = include_str!("../shaders/bruneton.wgsl");
pub const COMMON_WGSL: &str = include_str!("../shaders/common.wgsl");
pub const NOISE_WGSL: &str = include_str!("../shaders/noise.wgsl");
pub const ATMOSPHERE_WGSL: &str = include_str!("../shaders/atmosphere.wgsl");
pub const CELESTIAL_WGSL: &str = include_str!("../shaders/celestial.wgsl");
pub const AERIAL_WGSL: &str = include_str!("../shaders/aerial.wgsl");
pub const PRECOMPUTE_WGSL: &str = include_str!("../shaders/precompute.wgsl");
pub const MILKY_WAY_WGSL: &str = include_str!("../shaders/milky_way.wgsl");
pub const DOME_WGSL: &str = include_str!("../shaders/dome.wgsl");
pub const ENVIRONMENT_BAKE_WGSL: &str = include_str!("../shaders/environment_bake.wgsl");
pub const PREFILTER_WGSL: &str = include_str!("../shaders/prefilter.wgsl");
pub const DISPLAY_WGSL: &str = include_str!("../shaders/display.wgsl");
pub const DIAGNOSTICS_WGSL: &str = include_str!("../shaders/diagnostics.wgsl");

/// The sky's WGSL for the renderer core's shader library (with the files' disk paths, for hot reload): the shared
/// modules in dependency order, then the passes. [`register`] adds them; the keys match the paths the sky composes
/// with here.
/// The environment module (`naval_sky::environment`) as a library file with its disk path: the other crates whose
/// shaders sample the sky's environment register this one (when no layer has), so it hot-reloads for all of them.
pub const ENVIRONMENT_FILE: naval_render::ShaderFile =
    naval_render::shader_file!("shaders/environment.wgsl");

pub const FILES: [naval_render::ShaderFile; 12] = [
    naval_render::shader_file!("shaders/bruneton.wgsl"),
    naval_render::shader_file!("shaders/common.wgsl"),
    naval_render::shader_file!("shaders/noise.wgsl"),
    naval_render::shader_file!("shaders/atmosphere.wgsl"),
    naval_render::shader_file!("shaders/celestial.wgsl"),
    naval_render::shader_file!("shaders/aerial.wgsl"),
    ENVIRONMENT_FILE,
    naval_render::shader_file!("shaders/precompute.wgsl"),
    naval_render::shader_file!("shaders/milky_way.wgsl"),
    naval_render::shader_file!("shaders/environment_bake.wgsl"),
    naval_render::shader_file!("shaders/prefilter.wgsl"),
    naval_render::shader_file!("shaders/dome.wgsl"),
];

/// Library keys of the passes in [`FILES`].
pub const PRECOMPUTE_PATH: &str = "naval-sky/shaders/precompute.wgsl";
pub const MILKY_WAY_PATH: &str = "naval-sky/shaders/milky_way.wgsl";
pub const ENVIRONMENT_BAKE_PATH: &str = "naval-sky/shaders/environment_bake.wgsl";
pub const PREFILTER_PATH: &str = "naval-sky/shaders/prefilter.wgsl";
pub const DOME_PATH: &str = "naval-sky/shaders/dome.wgsl";

/// Adds the sky's WGSL to the renderer core's library (before any crate's pipeline that imports it).
pub fn register(
    ctx: &mut naval_render::RenderContext<'_>,
) -> Result<(), naval_render::ShaderError> {
    for file in FILES {
        ctx.add_shader(file)?;
    }
    Ok(())
}

/// Every shared module the sky's passes import, in dependency order (the renderer core's aerial-perspective volume
/// layout first: `naval_sky::aerial` reads the sky's copy of it).
const MODULES: [(&str, &str); 8] = [
    (
        naval_render::shaders::builtin::AERIAL_VOLUME.path,
        naval_render::shaders::builtin::AERIAL_VOLUME.source,
    ),
    ("naval-sky/shaders/bruneton.wgsl", BRUNETON_WGSL),
    ("naval-sky/shaders/common.wgsl", COMMON_WGSL),
    ("naval-sky/shaders/noise.wgsl", NOISE_WGSL),
    ("naval-sky/shaders/atmosphere.wgsl", ATMOSPHERE_WGSL),
    ("naval-sky/shaders/celestial.wgsl", CELESTIAL_WGSL),
    ("naval-sky/shaders/aerial.wgsl", AERIAL_WGSL),
    (
        "naval-sky/shaders/environment.wgsl",
        crate::environment::ENVIRONMENT_WGSL,
    ),
];

/// Registers the sky's shared modules with a composer: another lane's shader can then import the atmosphere
/// (`naval_sky::atmosphere::{sky_radiance, aerial, ...}`) over the group-1 bindings of `naval_sky::common` (group 0 stays the camera).
pub fn add_sky_modules(composer: &mut Composer) -> Result<(), Box<ComposerError>> {
    for (file_path, source) in MODULES {
        // The renderer core's module may be there already (a composer holding the core's modules), and adding it
        // again would drop every module that imports it.
        if file_path == naval_render::shaders::builtin::AERIAL_VOLUME.path
            && composer.contains_module("naval::aerial_volume")
        {
            continue;
        }
        composer.add_composable_module(ComposableModuleDescriptor {
            source,
            file_path,
            language: ShaderLanguage::Wgsl,
            ..Default::default()
        })?;
    }
    Ok(())
}

/// A pass's naga module, composed over the sky's modules.
pub fn compose(source: &str, file_path: &str) -> Result<wgpu::naga::Module, String> {
    compose_with(source, file_path, &[], &[])
}

/// A pass's naga module, composed over the sky's modules, then `extra` composable modules (file path, source, in
/// dependency order: another crate's, such as the clouds' hooks), with shader defs (`CLOUDS`, `#{NAME}` values).
pub fn compose_with(
    source: &str,
    file_path: &str,
    extra: &[(String, String)],
    defs: &[(&str, naga_oil::compose::ShaderDefValue)],
) -> Result<wgpu::naga::Module, String> {
    let mut composer = Composer::default();
    add_sky_modules(&mut composer).map_err(|error| error.emit_to_string(&composer))?;
    let shader_defs: std::collections::HashMap<String, naga_oil::compose::ShaderDefValue> = defs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), *value))
        .collect();
    for (path, module) in extra {
        let added = composer.add_composable_module(ComposableModuleDescriptor {
            source: module,
            file_path: path,
            language: ShaderLanguage::Wgsl,
            shader_defs: shader_defs.clone(),
            ..Default::default()
        });
        if let Err(error) = added {
            return Err(error.emit_to_string(&composer));
        }
    }
    composer
        .make_naga_module(NagaModuleDescriptor {
            source,
            file_path,
            shader_type: ShaderType::Wgsl,
            shader_defs,
            ..Default::default()
        })
        .map_err(|error| error.emit_to_string(&composer))
}

/// A pass's shader module. Panics with naga_oil's report if the sky's own WGSL does not compose: that is a
/// bug in this crate, caught by its tests.
pub fn shader_module(device: &wgpu::Device, source: &str, file_path: &str) -> wgpu::ShaderModule {
    shader_module_with(device, source, file_path, &[], &[])
}

/// [`shader_module`] over extra modules and shader defs (see [`compose_with`]).
pub fn shader_module_with(
    device: &wgpu::Device,
    source: &str,
    file_path: &str,
    extra: &[(String, String)],
    defs: &[(&str, naga_oil::compose::ShaderDefValue)],
) -> wgpu::ShaderModule {
    let module =
        compose_with(source, file_path, extra, defs).unwrap_or_else(|report| panic!("{report}"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(file_path),
        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pass_composes_and_validates() {
        for (source, path) in [
            (PRECOMPUTE_WGSL, "precompute.wgsl"),
            (MILKY_WAY_WGSL, "milky_way.wgsl"),
            (DOME_WGSL, "dome.wgsl"),
            (ENVIRONMENT_BAKE_WGSL, "environment_bake.wgsl"),
            (PREFILTER_WGSL, "prefilter.wgsl"),
            (DISPLAY_WGSL, "display.wgsl"),
            (DIAGNOSTICS_WGSL, "diagnostics.wgsl"),
        ] {
            let module = compose(source, path).unwrap_or_else(|report| panic!("{path}: {report}"));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{path}: {error:?}"));
        }
    }
}
