//!
//! | `naval::motion`    | `surface_motion`, `motion_response` (temporal AA's motion pass) |
//! | `naval::shadow`    | `sun_shadow`, `water_sun_shadow`, `shadow_map_visibility`, `shadow_near_share` (group 0 bindings 1 to 4) |The shader library: WGSL files composed with `naga_oil`.
//!
//! Every WGSL file any crate draws with is registered here under a stable path
//! (`<crate>/shaders/<file>.wgsl`). A file that starts with
//! `#define_import_path` is a module other files `#import`; any other file is an
//! entry shader that pipelines are built from. The renderer's own modules are
//! registered by [`ShaderLibrary::new`]:
//!
//! | Import path        | Gives                                                                 |
//! | ------------------ | --------------------------------------------------------------------- |
//! | `naval::camera`    | `Camera`, `camera` (group 0 binding 0), `world_to_clip`, `ndc_from_pixel`, `view_ray`, `view_depth` |
//! | `naval::color`     | `luminance`, `srgb_oetf`, `srgb_eotf`                                  |
//! | `naval::display`   | `display_grade`, `agx_tone_mapping`, `agx_hdr_tone_mapping`            |
//! | `naval::output`    | `dither_encoded` (group 0 bindings 8 and 9), `perceptual`, `perceptual_inverse` |
//! | `naval::shadow`    | `sun_shadow`, `water_sun_shadow`, `shadow_map_visibility`, `shadow_near_share` (group 0, bindings 1 to 4) |
//! | `naval::fullscreen`| `FullscreenVertex`, `fullscreen_vertex`                               |
//!
//! Sources are embedded with `include_str!` (see [`shader_file!`]), so the
//! library works on the web; the `hot-reload` feature re-reads them from disk.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, NagaModuleDescriptor, ShaderLanguage, ShaderType,
    get_preprocessor_data,
};

pub use naga_oil::compose::ShaderDefValue;

/// Shader defines for one pipeline, such as `("ENCODE_SRGB", ShaderDefValue::Bool(true))`.
pub type ShaderDefs = Vec<(String, ShaderDefValue)>;

/// A WGSL file embedded in a crate. Make one with [`shader_file!`].
#[derive(Clone, Copy, Debug)]
pub struct ShaderFile {
    /// The library key: `<crate name>/<path in the crate>`.
    pub path: &'static str,
    /// The text compiled into the binary.
    pub source: &'static str,
    /// Where the file was at build time, for hot reload. Never read without the
    /// `hot-reload` feature.
    pub disk: Option<&'static str>,
}

/// A [`ShaderFile`] for a WGSL file relative to the calling crate's manifest:
/// `naval_render::shader_file!("shaders/ocean.wgsl")` is keyed
/// `<crate>/shaders/ocean.wgsl`.
///
/// ```
/// let file = naval_render::shader_file!("shaders/naval/camera.wgsl");
/// assert_eq!(file.path, "naval-render/shaders/naval/camera.wgsl");
/// assert!(file.source.contains("#define_import_path naval::camera"));
/// ```
#[macro_export]
macro_rules! shader_file {
    ($relative:literal) => {
        $crate::ShaderFile {
            path: concat!(env!("CARGO_PKG_NAME"), "/", $relative),
            source: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $relative)),
            disk: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/", $relative)),
        }
    };
}

/// The renderer's own WGSL files.
pub mod builtin {
    use super::ShaderFile;

    pub const CAMERA: ShaderFile = crate::shader_file!("shaders/naval/camera.wgsl");
    pub const COLOR: ShaderFile = crate::shader_file!("shaders/naval/color.wgsl");
    pub const DISPLAY: ShaderFile = crate::shader_file!("shaders/naval/display.wgsl");
    pub const FULLSCREEN: ShaderFile = crate::shader_file!("shaders/naval/fullscreen.wgsl");
    pub const SHADOW: ShaderFile = crate::shader_file!("shaders/naval/shadow.wgsl");
    pub const MOTION: ShaderFile = crate::shader_file!("shaders/naval/motion.wgsl");
    pub const LIGHTS: ShaderFile = crate::shader_file!("shaders/naval/lights.wgsl");
    pub const OUTPUT: ShaderFile = crate::shader_file!("shaders/naval/output.wgsl");
    pub const AERIAL_VOLUME: ShaderFile = crate::shader_file!("shaders/naval/aerial_volume.wgsl");
    pub const AERIAL: ShaderFile = crate::shader_file!("shaders/naval/aerial.wgsl");
    pub const DISPLAY_PASS: ShaderFile = crate::shader_file!("shaders/display_pass.wgsl");
    pub const TRANSPARENT_COVER: ShaderFile = crate::shader_file!("shaders/transparent_cover.wgsl");

    /// The shared modules, registered by every library.
    pub const MODULES: &[ShaderFile] = &[
        CAMERA,
        COLOR,
        DISPLAY,
        FULLSCREEN,
        SHADOW,
        MOTION,
        LIGHTS,
        OUTPUT,
        AERIAL_VOLUME,
        AERIAL,
    ];
}

/// A shader that did not compose, or a pipeline that did not build, with the
/// message naga_oil or wgpu gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ShaderError {}

struct Source {
    text: String,
    disk: Option<String>,
    /// `#define_import_path`, for a module.
    module: Option<String>,
    /// What it `#import`s, as written.
    imports: Vec<String>,
}

/// Registered WGSL and the `naga_oil` composer built from its modules.
pub struct ShaderLibrary {
    sources: BTreeMap<String, Source>,
    composer: Composer,
    /// Modules whose registered text is not in the composer yet, because a
    /// rebuild failed since they changed. They go live with the next good rebuild.
    pending: BTreeSet<String>,
    generation: u64,
    /// Modules were registered since the composer was built: the next composition rebuilds it once. Registration
    /// is lazy so that a load registering dozens of modules builds the composer once rather than once per module
    /// (each build parses every module); hot reload's [`ShaderLibrary::update`] rebuilds at once, to report a
    /// broken module while the last good composer stands.
    stale: bool,
    /// Each module's text (by hash) as last composed on its own: a lazy rebuild probes only modules not among them.
    probed: BTreeMap<String, u64>,
}

impl Default for ShaderLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl ShaderLibrary {
    /// A library holding the renderer's shared modules.
    pub fn new() -> Self {
        let mut library = Self {
            sources: BTreeMap::new(),
            composer: Composer::default(),
            pending: BTreeSet::new(),
            generation: 0,
            stale: false,
            probed: BTreeMap::new(),
        };
        for file in builtin::MODULES
            .iter()
            .chain([&builtin::DISPLAY_PASS, &builtin::TRANSPARENT_COVER])
        {
            library
                .add(*file)
                .expect("the renderer's own shaders compose");
        }
        library
    }

    /// Registers an embedded file, replacing any file at the same path. A module that does not compose is
    /// reported by the next composition (registration builds nothing; see [`ShaderLibrary::compose`]).
    pub fn add(&mut self, file: ShaderFile) -> Result<(), ShaderError> {
        self.insert(
            file.path.to_owned(),
            file.source.to_owned(),
            file.disk.map(str::to_owned),
            false,
        )
        .map(drop)
    }

    /// Registers WGSL made at run time (generated code, tests) under `path`.
    pub fn add_source(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), ShaderError> {
        self.insert(path.into(), text.into(), None, false).map(drop)
    }

    /// Replaces the text of a registered file, as hot reload does, and returns
    /// the paths whose new text is now live: none when the text is unchanged,
    /// the file itself, and any module edits held back by an earlier failed
    /// rebuild that go live with this one. When a module no longer composes, the
    /// composer keeps the last good modules and the error is returned; the new
    /// text is kept, so the next edit is tried from it.
    pub fn update(&mut self, path: &str, text: String) -> Result<Vec<String>, ShaderError> {
        let Some(source) = self.sources.get(path) else {
            return Err(ShaderError {
                path: path.to_owned(),
                message: "not a registered shader".to_owned(),
            });
        };
        if source.text == text {
            return Ok(Vec::new());
        }
        let disk = source.disk.clone();
        // The composer of the files as registered so far is the last good one an edit that fails falls back on.
        let _ = self.ensure_composer();
        self.insert(path.to_owned(), text, disk, true)
    }

    /// The registered text of `path`.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.sources.get(path).map(|source| source.text.as_str())
    }

    /// Stores a file; a module rebuilds the composer now (`eager`, hot reload) or at the next composition.
    fn insert(
        &mut self,
        path: String,
        text: String,
        disk: Option<String>,
        eager: bool,
    ) -> Result<Vec<String>, ShaderError> {
        if !eager
            && let Some(existing) = self.sources.get(&path)
            && existing.text == text
            && existing.disk == disk
        {
            // The same file again (another layer registering a shared module): nothing changes.
            return Ok(vec![path]);
        }
        let (module, imports, _) = get_preprocessor_data(&text);
        let is_module = module.is_some();
        let previous = self.sources.insert(
            path.clone(),
            Source {
                text,
                disk,
                module,
                imports: imports.into_iter().map(|import| import.import).collect(),
            },
        );
        self.generation += 1;
        if !is_module && !previous.is_some_and(|source| source.module.is_some()) {
            return Ok(vec![path]);
        }
        if !eager {
            self.stale = true;
            self.pending.insert(path.clone());
            return Ok(vec![path]);
        }
        match self.build_composer(&BTreeMap::new()) {
            Ok((composer, probed)) => {
                self.composer = composer;
                self.probed = probed;
                self.stale = false;
                let mut live: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
                if !live.contains(&path) {
                    live.push(path);
                }
                Ok(live)
            }
            Err(error) => {
                self.pending.insert(path);
                Err(error)
            }
        }
    }

    /// Rebuilds the composer from every module, adding each after the modules
    /// it imports, and composes each module on its own unless `probed` holds its
    /// text already. Returns the modules' text hashes, all probed now.
    fn build_composer(
        &self,
        probed: &BTreeMap<String, u64>,
    ) -> Result<(Composer, BTreeMap<String, u64>), ShaderError> {
        let mut composer = Composer::default();
        let modules: Vec<(&String, &Source)> = self
            .sources
            .iter()
            .filter(|(_, source)| source.module.is_some())
            .collect();
        let mut added: BTreeSet<&str> = BTreeSet::new();
        while added.len() < modules.len() {
            let ready = modules.iter().find(|(path, source)| {
                !added.contains(path.as_str())
                    && source.imports.iter().all(|import| {
                        match self.module_path(import) {
                            Some(dependency) => added.contains(dependency),
                            // An import of nothing registered fails in naga_oil with its own message.
                            None => true,
                        }
                    })
            });
            let Some((path, source)) = ready else {
                let stuck: Vec<&str> = modules
                    .iter()
                    .map(|(path, _)| path.as_str())
                    .filter(|path| !added.contains(path))
                    .collect();
                return Err(ShaderError {
                    path: stuck.join(", "),
                    message: "these modules import each other in a cycle".to_owned(),
                });
            };
            let added_module = composer
                .add_composable_module(ComposableModuleDescriptor {
                    source: &source.text,
                    file_path: path,
                    language: ShaderLanguage::Wgsl,
                    ..Default::default()
                })
                .map(|_| ());
            if let Err(error) = added_module {
                return Err(ShaderError {
                    path: (*path).clone(),
                    message: plain(&error.emit_to_string(&composer)),
                });
            }
            added.insert(path.as_str());
        }
        // naga_oil parses a module only when a shader uses one of its items, so
        // compose each module's own text once as a shader (its
        // `#define_import_path` blanked, keeping line numbers): a module that no
        // longer parses is caught now, while the last good composer stands.
        let mut hashes = BTreeMap::new();
        for (path, source) in &modules {
            let hash = text_hash(&source.text);
            hashes.insert((*path).clone(), hash);
            if probed.get(path.as_str()) == Some(&hash) {
                continue;
            }
            let probe: String = source
                .text
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("#define_import_path") {
                        ""
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let composed = composer.make_naga_module(NagaModuleDescriptor {
                source: &probe,
                file_path: path,
                shader_type: ShaderType::Wgsl,
                ..Default::default()
            });
            if let Err(error) = composed {
                return Err(ShaderError {
                    path: (*path).clone(),
                    message: plain(&error.emit_to_string(&composer)),
                });
            }
        }
        Ok((composer, hashes))
    }

    /// The path of the module an `#import` names (`naval::camera` or an item in it).
    fn module_path(&self, import: &str) -> Option<&str> {
        self.sources
            .iter()
            .filter_map(|(path, source)| Some((path, source.module.as_deref()?)))
            .filter(|(_, module)| {
                import == *module
                    || import
                        .strip_prefix(module)
                        .is_some_and(|rest| rest.starts_with("::"))
            })
            .max_by_key(|(_, module)| module.len())
            .map(|(path, _)| path.as_str())
    }

    /// Builds the composer when modules were registered since it was last built.
    fn ensure_composer(&mut self) -> Result<(), ShaderError> {
        if self.stale {
            let (composer, probed) = self.build_composer(&self.probed)?;
            self.composer = composer;
            self.probed = probed;
            self.stale = false;
            self.pending.clear();
        }
        Ok(())
    }

    /// Composes the entry shader at `path` into a naga module. The first composition after modules were registered
    /// builds the composer, and reports a registered module that does not compose.
    pub fn compose(
        &mut self,
        path: &str,
        defs: &ShaderDefs,
    ) -> Result<wgpu::naga::Module, ShaderError> {
        self.ensure_composer()?;
        let source = self.sources.get(path).ok_or_else(|| ShaderError {
            path: path.to_owned(),
            message: "not a registered shader".to_owned(),
        })?;
        let shader_defs: HashMap<String, ShaderDefValue> = defs.iter().cloned().collect();
        self.composer
            .make_naga_module(NagaModuleDescriptor {
                source: &source.text,
                file_path: path,
                shader_type: ShaderType::Wgsl,
                shader_defs,
                additional_imports: &[],
            })
            .map_err(|error| ShaderError {
                path: path.to_owned(),
                message: plain(&error.emit_to_string(&self.composer)),
            })
    }

    /// A wgpu shader module for the entry shader at `path`.
    pub fn create_module(
        &mut self,
        device: &wgpu::Device,
        label: &str,
        path: &str,
        defs: &ShaderDefs,
    ) -> Result<wgpu::ShaderModule, ShaderError> {
        let module = self.compose(path, defs)?;
        Ok(device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        }))
    }

    /// `path` and every file it imports, directly or through other modules.
    pub fn dependencies(&self, path: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![path.to_owned()];
        while let Some(next) = pending.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            if let Some(source) = self.sources.get(&next) {
                pending.extend(
                    source
                        .imports
                        .iter()
                        .filter_map(|import| self.module_path(import))
                        .map(str::to_owned),
                );
            }
        }
        seen
    }

    /// Registered paths and where each was on disk, for hot reload.
    pub fn disk_files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.sources
            .iter()
            .filter_map(|(path, source)| Some((path.as_str(), source.disk.as_deref()?)))
    }

    /// Every registered path.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.sources.keys().map(String::as_str)
    }

    /// Counts every registration and update; a pipeline built at an older
    /// generation may be stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// naga_oil's report without terminal color codes.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.trim_end().to_owned()
}

/// A hash of a module's text, to know it was composed on its own before.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_modules_compose_into_the_display_pass() {
        let mut library = ShaderLibrary::new();
        let module = library
            .compose(builtin::DISPLAY_PASS.path, &Vec::new())
            .expect("display pass composes");
        assert!(
            module
                .entry_points
                .iter()
                .any(|entry| entry.name == "fragment")
        );
        let encoded = library
            .compose(
                builtin::DISPLAY_PASS.path,
                &vec![("ENCODE_SRGB".to_owned(), ShaderDefValue::Bool(true))],
            )
            .expect("display pass composes with ENCODE_SRGB");
        assert!(encoded.functions.len() >= module.functions.len());
    }

    #[test]
    fn dependencies_follow_imports_through_modules() {
        let library = ShaderLibrary::new();
        let deps = library.dependencies(builtin::DISPLAY_PASS.path);
        for file in [builtin::DISPLAY, builtin::COLOR, builtin::FULLSCREEN] {
            assert!(deps.contains(file.path), "{deps:?} lacks {}", file.path);
        }
        assert!(!deps.contains(builtin::CAMERA.path));
    }

    #[test]
    fn another_crate_imports_the_camera() {
        let mut library = ShaderLibrary::new();
        library
            .add_source(
                "ocean/shaders/surface.wgsl",
                "#import naval::camera::{camera, world_to_clip}\n\
                 @vertex fn vertex(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {\n\
                     return world_to_clip(p + vec3<f32>(0.0, sin(camera.time), 0.0));\n\
                 }\n",
            )
            .unwrap();
        library
            .compose("ocean/shaders/surface.wgsl", &Vec::new())
            .expect("an importing shader composes");
    }

    #[test]
    fn a_broken_module_is_reported_and_the_last_good_composer_kept() {
        let mut library = ShaderLibrary::new();
        let error = library
            .update(
                builtin::COLOR.path,
                "#define_import_path naval::color\nfn luminance(".to_owned(),
            )
            .expect_err("a broken module does not compose");
        assert_eq!(error.path, builtin::COLOR.path);
        assert!(!error.message.contains('\u{1b}'));
        // The display pass still composes from the last good color module.
        library
            .compose(builtin::DISPLAY_PASS.path, &Vec::new())
            .expect("last good composer kept");
    }

    #[test]
    fn an_edit_held_back_by_a_broken_module_goes_live_with_the_fix() {
        let mut library = ShaderLibrary::new();
        library
            .update(
                builtin::COLOR.path,
                "#define_import_path naval::color\nfn luminance(".to_owned(),
            )
            .expect_err("broken");
        // A good camera edit cannot go live while color is broken.
        let camera = builtin::CAMERA.source.replace(
            "fn world_to_clip",
            "fn world_to_clip_edited_marker() {}\nfn world_to_clip",
        );
        library
            .update(builtin::CAMERA.path, camera)
            .expect_err("the composer still fails on color");
        // Fixing color brings both live.
        let mut live = library
            .update(builtin::COLOR.path, builtin::COLOR.source.to_owned())
            .expect("fixed");
        live.sort();
        assert_eq!(live, [builtin::CAMERA.path, builtin::COLOR.path]);
    }
}
