# Sky lane (`native/crates/naval-sky`)

The native client's sky: takram's atmosphere (Bruneton's precomputed scattering, as takram's three-geospatial
`packages/atmosphere` ships it) with the game's grade over it, the dome at the far plane, the sun, moon, stars and
Milky Way, and the environment lighting that ships and the sea read in place of three's PMREM. The crate is
portable (`cargo check -p naval-sky --target wasm32-unknown-unknown` passes). Outputs go to `.build/native/sky/`.

It no longer ports the web's sky (`src/game/sky`): on 2026-10-02 Bill asked for takram's clouds in the native client,
and for its atmosphere too where it is better, with "their scattering model and sky, with our twilight lift, night
floor and golden-hour grade re-applied so battles stay as readable as now". The celestial bodies, the environment
atlas and its interface, the far fog map and the CPU's scene light are the game's as before.

Clouds and weather live in `naval-clouds` (`native/docs/clouds.md`), which hooks into this crate (see "Clouds and
weather in this crate" at the end). **A game frame uses `naval_clouds::GameSky`**, the sky, clouds and weather
together; `naval_sky::SkyLayer` is the sky alone.

**Status (2026-10-02):** takram's tables computed on the GPU and read by the dome, the environment, the fog map and
the clouds' contract; the grade retuned against the sky it replaced from the bridge and from aircraft (900 m to
20 km) over a day, a night and the overcast and fog weathers' air; tests against the CPU's integral of the same
air. Branch `goldflag/takram-native-sky`.

## The atmosphere

### takram's model, ported

`shaders/bruneton.wgsl` ports takram's `src/shaders/bruneton/{definitions,common,precompute,runtime}.glsl`
(three-geospatial at b012ad06; Bruneton's BSD notice and takram's MIT notice in its header and in
`THIRD_PARTY.md`). Every function takes the tables (and the sampler for the filtered ones) as arguments, so the
precompute and the runtime share them. takram's changes are kept: a separate single-Mie table and a higher-order
table (light shafts occlude only single scattering), SafeSqrt toward the top of the air, rays clipped at the bottom
of the air, the closest-point test and the horizon epsilon of GetSkyRadianceToPoint, the guard on single Mie with
the sun below the horizon. Lengths are kilometres; the channels stand for 680, 550 and 440 nm.

| Table | Size | Format | Holds |
| --- | --- | --- | --- |
| transmittance | 256 × 64 | `Rgba32Float` | optical depth to the top of the air (read by four loads and a manual bilinear filter) |
| scattering | 256 × 128 × 32 | `Rgba16Float` 3D | Rayleigh single scattering and every higher order (divided by the Rayleigh phase); single Mie's red in alpha |
| single Mie | 256 × 128 × 32 | `Rgba16Float` 3D | single Mie scattering |
| higher order | 256 × 128 × 32 | `Rgba16Float` 3D | the orders after the first alone |
| irradiance | 64 × 16 | `Rgba16Float` | the sky's irradiance on a horizontal surface |

**The precompute** (`shaders/precompute.wgsl`, `src/tables.rs`) is takram's `PrecomputedTexturesGenerator` as
compute passes, in radiance (its `luminanceFromRadiance` is the identity): transmittance, direct irradiance, single
scattering, then for orders 2, 3 and 4 the scattering density, indirect irradiance and multiple scattering. The
deltas are half-float textures; the sums are float32 storage buffers; a resolve writes the runtime's tables in one
go at the end, so a recompute swaps them within one frame. It runs in the first update after construction and after
any change of the air (`Sky::apply` with other authored air, or a precompute pipeline rebuilt by hot reload), and
makes its scratch (about 75 MB: three float32 sums and three half-float deltas) each time and drops it once
encoded. A change of the sun or moon runs nothing: the tables hold every sun angle.

**A sky taking another's tables** (`Sky::take_tables_from(queue, encoder, from)`, `GameSky::take_tables_from`): a sky
put on screen in place of another whose air is the same copies that sky's tables (about 25 MB, a fraction of a
millisecond) instead of running the precompute in its first update. The theatre's scene swaps do (each new scene
brings a sky of its own; the precompute was 74 to 108 ms of GPU in the swap's first frame, `native/docs/theatre.md`).
It copies only into a sky that has computed none yet, from one that has; a deferred sky's first adoption of the
registry's pipelines (`new_deferred`, then `adopt_pipelines`) takes them in place of its placeholders and keeps tables
it took, which were computed with the registry's own. `Sky::precomputes()` counts the precomputes a sky ran.

**Changes from takram, all in the precompute's accuracy or the game's units** (takram's thin default air agrees
either way; our maps' haze did not):

- The transmittance table holds float32 optical depth, and the runtime takes Bruneton's ratio of two
  transmittances as a difference of optical depths: along a horizon ray through a hazy map's air the transmittance
  passes float32's exponent range, and the ratio became 0/0.
- Single and multiple scattering integrate over 96 intervals that grow quadratically from the viewpoint (Bruneton:
  50 equal ones). Equal intervals of 15–20 km along a horizon ray, through haze that is opaque within a few
  kilometres, overstated the horizon several times in the fog weather.
- The density profiles hold the altitude at 0: a sample at the bottom of the air could round a hair below it in
  float32 and pick the profile's lower layer, which is zero for molecules and aerosol, dropping the densest sample
  of every sea-level ray (the sea-level row of the transmittance table read 4% short in the fog weather).
- Single scattering is held at zero where `scattering − higher order` rounds negative in half floats.
- The top of the air is 100 km up (the game's), not takram's 60 km: with the sun 8° down the planet's shadow
  reaches about 60 km over the camera, so a 60 km sky had no lit air overhead in the blue hour.

### The game's units and grade over it

`shaders/atmosphere.wgsl` reads the tables as every other part does (`naval_sky::atmosphere`, over group 1).

- **Units.** takram reads radiance through its radiance-to-luminance factors divided by the sun's unit luminance
  (`model::sun_relative_luminance`, `sky_relative_luminance`). The game scales that so the sun's luminance above the
  air is the scene's `sun_intensity` (`model::game_scale`; a daylight sky pixel is about 0.3–1.0), and white-balances
  it to the sun (`model::white_balance`, about −8.5% red and +4% blue): in the game the sun above the air is white,
  as the scene light, the sea's glints and the ships' light take it. Light the aerosol scatters (single Mie, and in
  a hazy map most of the higher orders) keeps the sun's spectrum, so it is read with the sun's factors rather than
  the sky's (`SUN_TINT`); with the sky's, which assume Rayleigh's λ⁻³, the fog weather's haze read pink.
- **The authored air** (`model::AuthoredAtmosphere`) maps onto takram's: molecules by the authored blue (0.4 is
  takram's Earth), the aerosol as takram's clear-sky 0.004 per km plus the game's maritime haze for the turbidity
  (1 adds none, the fog weather's 6 makes 0.27 per km), single-scattering albedo 0.9 (takram's), the authored
  asymmetry for the precompute's Mie phase. A change recomputes the tables.
- **The grade** (`model::sky_grade`, the game's `SKY_GRADE`): a gain of 1.4 over the physics, the chroma lift of the
  authored blue, the luminance shoulder (knee 0.45, ceiling 0.95) and the navy night floor; the dome draws the
  aureole with the authored lobe and narrow core (`model::phase_terms`) in place of the tables' Cornette–Shanks,
  and scales the higher orders by the authored `multiple`.
- **Twilight**: the sky's exposure lifts as the sun sets (`model::TWILIGHT`, read for the sun's elevation above the
  camera's horizon): the game's table, lifted an eighth more below the horizon (8.4 at −2°, 78 at −6°, 291 at −8°),
  where takram's scattering leaves the sky away from the sun that much darker.
- **Ozone** is the game's (2.4e-3, 3e-3, 0.136e-3 per km at the peak, not takram's 0.65e-3, 1.881e-3, 0.085e-3): the
  red channel stands for broadband red, which the Chappuis band absorbs far more than at 680 nm, and at takram's value
  the blue hour turned magenta (more so under its red-weighted sky factors).
- **The moon** scatters through the same tables as a second light (takram's WebGPU `moonScattering` idea), at the
  grade's quarter of the game's moonlight, its colour over the sun's per channel; the moon takes the sky from the sun
  as before (`SUN_SETS` −14°, `MOON_SETS` −6°, faded in from 8° of sun to 2°).
- **Below the horizon** the dome holds the horizon's colour (the sky read just above the geometric horizon, the
  phases toward the actual direction so the aureole fades below it), as the game's sky did, so it meets the far fog.
- **Lightning in the air** (2026-10-03, `dome.wgsl`, `flash_airlight`): while a flash lights the scene the dome adds
  the air's single scattering of it, the channel a point of the sky's `lightning_intensity` at `lightning_position`
  (the irradiance `intensity × (1 km / r)²` in the sea's units): the inverse square integrated along the ray in closed
  form, the air's coefficients as dense as where the ray passes the flash, the Rayleigh and aerosol phases at the angle
  the flash makes with the ray from the eye (the aureole about a flash seen through haze), through the air between the
  eye and the flash. Not in the environment's bake (a flash outlasts none of its sweeps). The clouds take the same
  flash through their march (`clouds.md`, "Night: the moon and lightning").

### The interface

The clouds' contract (with `naval-clouds`; the planet frame: the planet's centre straight under the camera, world
axes, `planet_km(p) = ((p.x − camera.x) / 1000, p.y / 1000 + 6360, (p.z − camera.z) / 1000)`):

| WGSL (`naval_sky::atmosphere`) | Is |
| --- | --- |
| `BOTTOM_RADIUS_KM` 6360, `TOP_RADIUS_KM` 6460 | the air's bottom and top |
| `SunSky { sun, skylight }`, `SkyLight { radiance, transmittance }` | results (the field is `skylight`, not `sky`: naga_oil rewrites every `sky` token, field accessors included, in a module that imports the `sky` uniform) |
| `sun_sky_scalar_irradiance(point_km) -> SunSky` | takram's GetSunAndSkyScalarIrradiance for the sun: its direct light through the air and the planet's shadow (no cosine) and the sky's over the whole sphere (×2π), game units, the twilight lift in both, the night floor in the sky's |
| `sun_sky_irradiance(point_km, normal) -> SunSky` | GetSunAndSkyIrradiance on a surface |
| `sky_radiance_to_point(camera_km, point_km, shadow_length_km) -> SkyLight` | GetSkyRadianceToPoint with light shafts: the air's light between the two points as the dome shows the sky (sun, moon at night, grade, the night floor in proportion to the air's opacity), so distant clouds fade into the dome's horizon; and the transmittance between them |
| `sky_radiance_along(camera_km, direction, shadow_length_km) -> SkyLight` | GetSkyRadiance: the graded sky along a ray (no discs or stars) and the transmittance to space (0 toward the ground) |

The game's own reads, on the new tables: `sky_radiance(direction, from_sea)`, `dome_sky(direction, from_sea,
shadow_length)`, `fog_radiance(direction)` (the ocean's), `transmittance_to_space(direction, from_sea)`,
`overcast(color)`, `sun_transmittance(position)` and `moon_transmittance(position)` (world metres),
`ambient(altitude)` (rain), `aerial(direction, distance, from_sea)`.

`naval_sky::bruneton` exports takram's functions themselves (`get_sky_radiance`, `get_sky_radiance_to_point`,
`get_sun_and_sky_irradiance`, `get_transmittance_to_top_atmosphere_boundary`, …) over any tables passed in; the sky's
group 1 binds the runtime's (`naval_sky::common`: `transmittance_table`, `scattering_table`, `single_mie_table`,
`higher_order_table`, `irradiance_table`, `linear_clamp`, and the air as `air: AtmosphereParameters` at binding 11).
The group holds eight sampled textures and two samplers, as before.

## Environment lighting (the interface ships and the ocean read)

The shape has been stable since it was published on 2026-09-29. The atlas is now filled.

### What it is

It is one 2D texture, `Rgba16Float`, holding prefiltered sky radiance in **three's PMREM (CubeUV) atlas
layout**. That is exactly what `src/game/sky/environment/Environment.ts` produces and what every
`pmremTexture(env, direction, roughness)` in the web game reads: ship materials through `scene.environment`,
the sea's reflections in `OceanSurfaceMaterial.ts`/`physicalReflection.ts`, the water body's up-welling in
`waterBody.ts`, and `EyeAdaptation.ts`. Keeping three's layout means the web's lookups port one for one. The texture
is not stored the way the web's is, though: see the orientation note under Layout.

The contents are linear HDR radiance in the sea's units, around a sea-level point under the camera. A
daylight sky pixel is about 0.3–1.0; the sun's irradiance above the air is `sun.intensity`, about 6.

- Above the horizon: the dome (sky, moon and Milky Way). There is **no sun disc and no stars**; the sea and
  ships draw their own glints.
- Below the horizon: the sea as ships and airframes see it from above (2026-10-09), lit as the scene is: the sky
  mirrored with water's Fresnel; the scene's light (`SkyUniforms.light_direction` and `light_color`: the sun, or the
  moon once it outshines it) mirrored off the waves into a glitter lobe about its mirror direction (Cox and Munk's
  slopes for a 7 m/s breeze as a Beckmann microfacet lobe with Smith masking); and a blue-green water body sending
  back about 1.4% (luminance) of what the light and the sky put into it. The glitter fades in over the first 15°
  below the horizon, because the sea's own reflection reads the horizon through the blurred levels and would take a
  low sun's glitter a second time over its own. The sea returns about 4.6% of the light on it under a 61° sun (it
  returned 1.0% before), more as the sun sinks (Payne 1972), while the light itself falls; at night the moon does
  the same at its own brightness. Measured on a Wildcat's underside in the photo view (`E_down / (E_sky + E_sun)`):
  0.046 at 14:00, 0.065 at 16:30, 0.11 at 17:15, the light from below falling from 0.12 to 0.063 (`E_down / π`).
  Ships take it on their lower hulls and overhangs (0.5 to 4% brighter at noon); the sea and its glitter path change
  by 1% at dusk and 3% under the moon; the marine haze (`naval_clouds` aerial), which scatters the environment's
  light, the sea's included, brightens a little at the horizon under a low sun or moon. The bake's pass costs about
  0.01 ms more a slice (M5 Pro, p50 0.047 to 0.055 ms).

With clouds attached (`Sky::attach_clouds`, which `naval_clouds::GameSky` does) the bake also holds, above the horizon,
the clouds' short march over the dome (`clouds_bake`): an overcast sky greys and dims reflections and ambient light.
Below it, the glitter and the light into the water body take the share of the scene's light the clouds let through to
64 sea points within 1.5 km of the camera (`clouds_bake_light`, marched as the ground shadow map is), averaged across
each workgroup so every texel takes the same.

### Sampling it (WGSL)

`naval_sky::add_environment_module(&mut composer)` registers `shaders/environment.wgsl` with naga_oil as
`naval_sky::environment`. Every function takes the atlas, sampler and uniforms as arguments, so you can bind
them in any group:

```wgsl
#import naval_sky::environment::{SkyEnvironment, env_radiance, env_irradiance, env_reflection_direction}

@group(2) @binding(0) var sky_atlas: texture_2d<f32>;
@group(2) @binding(1) var sky_sampler: sampler;
@group(2) @binding(2) var<uniform> sky_env: SkyEnvironment;

// Specular: three's radiance context (mirror direction bent toward the normal by roughness^4).
let r = env_reflection_direction(n, v, roughness);            // v: surface -> eye, unit
let specular_light = env_radiance(sky_atlas, sky_sampler, sky_env, r, roughness);
// Diffuse: three's irradiance context, roughest level along the normal, times pi.
let irradiance = env_irradiance(sky_atlas, sky_sampler, sky_env, n);
let diffuse = albedo / PI * irradiance;                         // = albedo * env_radiance(n, 1.0)
```

- `env_radiance(atlas, sampler, env, direction, roughness) -> vec3<f32>` is `pmremTexture(env, dir, r)`.
  - The direction is in world space and need not be unit length.
  - Roughness is GGX roughness from 0 to 1 (three's `roughness`, not squared).
  - It samples level 0 explicitly (`textureSampleLevel`), so it works in any stage, including compute and
    vertex.
  - It costs one bilinear tap, or two when the roughness falls between levels.
- `env_irradiance(atlas, sampler, env, normal)` is `env_radiance(normal, 1.0) * PI`, three's
  `EnvironmentNode` convention, which the web's ships use. It is the web's 0.582-radian Gaussian of the sky,
  not a true cosine convolution; it is kept that way so ships shade as they do on the web.
- `env_reflection_direction(normal, view, roughness)` is three's lookup direction for specular.
- For the sea: the web reflects `normalize(vec3(ray.x, max(ray.y, 0), ray.z))` at its own roughness, and the
  water body reads `env_radiance(up, 1.0) * PI`. Do the same.

**Roughness to level** uses three's `roughnessToMip`, unchanged: `mip = clamp(f(r), -2, max_mip)`.

- `f(r) = -2 log2(1.16 r)` below r = 0.21, then linear through (0.21, 4), (0.305, 3), (0.4, 2), (0.8, −1)
  and (1, −2).
- A level `m >= 4` is the sharp level with faces of `2^m` texels; `m = max_mip` is the unblurred sky. A sharp
  level `m` below the base carries sigma `1 / 2^m` radians.
- Levels `m < 4` are the six extra blurs at 16² per face, with total Gaussian sigma 0.125, 0.215, 0.35,
  0.446, 0.526 and 0.582 radians for m = 3 … −2.

### Binding it (Rust)

```rust
let env: &naval_sky::EnvironmentMap = sky.environment();       // or sky_layer.sky.environment()
// env.bind_group is laid out by env.bind_group_layout (== naval_sky::environment_bind_group_layout):
//   binding 0  texture_2d<f32> (filterable float, D2)  env.view
//   binding 1  sampler (filtering, linear, clamp)       env.sampler
//   binding 2  uniform SkyEnvironment, 80 bytes         env.uniforms
// visible to VERTEX | FRAGMENT | COMPUTE. Put it at any group >= 1 (group 0 is the render core's camera).
pass.set_bind_group(2, &env.bind_group, &[]);
let light: naval_sky::CelestialLight = sky.light(); // the scene's directional light
```

- `EnvironmentMap { layout: CubeUvLayout, texture, view, sampler, uniforms, bind_group_layout, bind_group,
  generation }`. (Binding 3, the far fog's colour map baked from `fog_radiance`, went on 2026-10-03 with the fog ramp:
  the air in front of every surface is the aerial perspective, `haze.md`.)
  - The texture object stays the same until `Sky::set_quality` changes the atlas size. When `layout`
    changes, rebuild any bind group of yours that holds `view`.
  - `generation` bumps on every refilter.
  - The sky writes `uniforms` in every `update`.
- `EnvironmentUniforms` (WGSL `SkyEnvironment`, 80 bytes) holds `texel_size` (1/width, 1/height),
  `max_mip`, `generation`, `light_direction`, `light_intensity`, `light_color`, `night` (0/1),
  `sun_direction`, `moon_phase`, `moon_direction` and `moon_illumination`.
- `CelestialLight { direction, color, intensity, night }` is `CelestialLight` in
  `src/game/sky/contracts.ts`, taken from the atmosphere's sea-level sun and moon (`SkyState.chooseLight`).
  - The sun is the light by day; the moon takes over once it outshines the sun.
  - `color` has its brightest channel at 1, and `intensity` is that channel in the sea's units: the scene's
    `sun_intensity` above the air (about 6) through it, before the gain below (5.7 of the North Atlantic's 6.6 with
    the sun 61° up). `radiance()` returns color × intensity.
  - Since 2026-10-03 the sun and moon at the sea take the dome's grade gain (`sky_grade::GAIN`), as the clouds'
    sunlight does (`direct_scale`): one brightness scale for sky and world (`docs/lighting.md`, "Realism"). Meshes
    take the light as it is (no moonlight share, no hemisphere fill but a lightning flash's).
  - The web's `flash` and `bolt` are `Sky::lightning()` here (`LightningLight { flash, bolt }`, beside this struct so its
    four fields stay as other crates build them): lightning's extra diffuse light (a multiple of the ambient) and its
    direct light (`BoltLight { position, intensity }`, irradiance `intensity × (1 km / r)²`), as the weather last set
    them with `Sky::set_lightning`; 0 without weather.

### Layout (for tools and debugging only; sample through the functions)

- `CubeUvLayout::from_bake_width(w)` gives faces of `S = 2^floor(log2(w / 4))`: 64 on Low/Medium (bake 256
  or 384) and 128 on High/Ultra (512 or 768).
- The atlas is `3·max(S, 112)` × `4·S` (384 × 512 on High).
- A level is a 3 × 2 block of faces: +X +Y +Z on the first row, −X −Y −Z on the second. Each face has a
  one-texel border holding directions just past its edge.
- The sharp level of face `s` sits at (0, 4·(S − s)); extra level k (1…6) sits at (48·k, 4·(S − 16)).
- **Orientation (native):** texel row 0 is the top of the texture. Within each face, rows follow three's
  `getDirection` uv with v increasing down the rows. So the stored +Y face holds the sky, the −Y face the sea,
  and each side face runs from below the horizon (top rows) to above it (bottom rows). `env_radiance` reads
  exactly this, and the sky's prefilter writes it (`tests/atmosphere.rs`, `environment_is_oriented_and_finite`).
- **The web's atlas as stored differs.** three renders its atlas into a render target and so looks it up with
  the direction's y negated (`PMREMNode`: `vec3(x, -y, z)` for render-target textures). Read back as stored,
  every face is mirrored top to bottom and ±Y are swapped relative to the native atlas; that is, the stored
  +Y face holds the sea. naval-ships' `web_atlas_to_native` (PR #660) mirrors each face's rows, border
  included, and swaps ±Y, level by level. After that the web atlas's layout is this one (while the native sky
  ported the web's, their contents agreed within 0.05–0.14% on the sharp levels and 0.4–2.6% on the blurred ones;
  that test went with the web's sky). The prefilter's blur poles are y-negated for the same reason, so it turns
  about the same world axes as the web's.

### When it refreshes

- The native sky re-bakes and refilters the whole atlas in `update` after:
  - any `apply` (new sun, moon or air);
  - a camera altitude step of 2% (the sun's twilight exposure follows the camera's horizon, as on the web);
  - the first frame;
  - `refresh_environment()`.
- The bake is evaluated straight into the atlas's level 0 per cube texel, with no equirectangular
  intermediate. The web's Gaussian chain (`SkyPMREMGenerator`) then refilters the other levels.
- The web also re-bakes the whole thing on any sun or light change. For drifting clouds it sweeps the bake in
  four bands every 16 frames. With clouds attached the native sky does too: a quarter of the atlas's rows every
  fourth frame and the prefilter after the last quarter (so reflections follow the clouds within 16 frames), and
  a whole bake at once after any of the changes above or a 2 km jump of the camera.

## Using the sky

```rust
// Standalone:
let mut sky = naval_sky::Sky::new(&device, &queue, SkyQuality::High, SkyTargets::default());
sky.apply(&SkyScene::north_atlantic(20.0, 270.0));          // on a scene change
sky.update(&queue, &mut encoder, &sky_view, dt);             // each frame, before the scene pass
sky.draw_dome(&mut scene_pass);                              // after opaque geometry, reversed-Z depth bound

// In the renderer core (native/docs/render-core.md):
let mut sky_layer = naval_sky::SkyLayer::new(&device, &queue, SkyQuality::High);
sky_layer.sky.apply(&scene);
renderer.render(input, &mut [&mut ships, &mut ocean, &mut sky_layer], output)?; // prepare + Phase::Sky
```

- **`Sky::update`** encodes compute passes only when needed:
  - the Milky Way's bake, once;
  - takram's tables when the air changes (the first update computes them; a deferred sky's only once its pipelines
    are the registry's);
  - the environment when anything it shows changes (the sun, moon or air, a 2% step of the camera's altitude, with
    clouds the sweep).
  - A steady sky costs one 544-byte uniform write and an 80-byte one.
- **`Sky::draw_dome`** draws one full-screen triangle at clip depth 0 (`GreaterEqual`, no depth write), in
  `SkyTargets` formats (default `Rgba16Float` + `Depth32Float`).
  - It sets only group 1 (and the clouds' group 2 when attached). Its pipeline leaves group 0 empty, so the render
    core's camera stays bound.
  - It takes its view rays from `SkyView` (camera-to-world and projection), which `SkyLayer` copies from the
    camera block.
- **`Sky::bind_group_layout()` / `bind_group()`** are group 1: the 544-byte `SkyUniforms` (`shaders/common.wgsl`),
  takram's tables and their air, and the celestial textures. With `naval_sky::add_sky_modules(&mut composer)`,
  another part's shader can `#import naval_sky::atmosphere::{…}` (the table above), `naval_sky::bruneton::{…}` and
  `naval_sky::celestial::{...}` over that group.
- **`Sky::light()`, `sea_level()` (sun, moon, sky irradiance at the sea), `sun_direction()` and
  `moon_direction()`** are for the scene's lights; `Sky::air()` is the air the tables hold, for CPU reads of its
  transmittance (the clouds' key light, `naval_clouds::key_light`). The CPU integrates the same air the tables hold
  (`model::optical_depth`, takram's profiles and the authored coefficients) for the sun at the sea: its luminance is
  the tables' sun through the air (since 2026-10-09; before, a readability ramp held it at the whole sun above the air
  from 18° up: `docs/lighting.md`, "Sun and sky at the sea"), its hue the game's `SUN_GRADE` (tint, desaturation),
  never under the readability floor as it sets, faded out at the horizon; the sky's irradiance at the sea is the CPU's
  single-scattering estimate (the tables are not read back: the crate is portable).
- **`Sky::set_timestamps(query_set)`** is for profiling: timestamps 0 and 1 bracket the tables' pass (the precompute,
  or the Milky Way's bake), and 2 and 3 the environment's.
- **`naval_sky::display::DisplayTransform`** is a *temporary* game look plus AgX for this lane's captures.
- **`naval_sky::diagnostics::Diagnostics`** provides the panorama, probes (the sky's radiance from the camera or
  the sea, transmittance to space, the clouds' scalar irradiance, the transmittance table at a radius and angle) and
  environment unwraps through the consumer's own `env_radiance`.

### Running it

From `native/`:

```sh
cargo test -p naval-sky                                         # model, layouts, and GPU tests (tests/atmosphere.rs)
cargo run -p naval-sky --example takram --release -- <tag>      # bridge and aircraft stills + probes: .build/native/sky/takram/<tag>/
cargo run -p naval-sky --example stills --release               # panoramas, domes, environment + timings.json
cargo run -p naval-sky --example render_core --release          # through naval_render: .build/native/sky/render-core/
cargo check -p naval-sky --target wasm32-unknown-unknown
```

The GPU tests: the transmittance table against the CPU's integral of the same air (optical depth within 0.5% from the
zenith to a degree above the horizon, for turbidity 1, 2.2 and 6, and the table at the bottom of the air); the sky
finite and never negative from 30 m, 2.6 km, 9 km and 40 km at noon, sunset, twilight, night and in fog, in every
direction, with a blue zenith by day; the clouds' light (`sun_sky_scalar_irradiance`) at cloud height by day and
gone in the planet's shadow; the celestial light continuous through sunset with one hand-over to the moon; the
environment atlas oriented and finite. The web's tools (`tools/web-compare.ts`, `tools/compare.py`) still render the
web's sky beside this one; they no longer match by design.

## Against the sky it replaced

`examples/takram.rs` renders the dome from the bridge (30 m, facing the sun, away and to the side, the zenith) and
from aircraft (900 m, 2.6 km, 5 km and 9 km, a dive from 4 km, the chart from 20 km) over the North Atlantic at sun
60°, 20°, 5°, 0°, −4° and −8°, a full-moon and a crescent night, the overcast and fog weathers' air (turbidity 3.8 and
6) and the port, with radiance probes, the celestial light and the sea-level light. The old sky's stills come from
the same example run on master `bf196ce40`. Images under `.build/native/sky/takram/` (`before/` and the run's tag,
`sheet-*.png` side by side, old on the left); measured 2026-10-02, Apple M5 Pro.

Luminance of the probes, new over old, from the bridge facing away from the sun:

| Sky | Zenith | 10° toward the sun | Horizon toward the sun | Horizon to the side | Horizon away | 30° away |
| --- | --- | --- | --- | --- | --- | --- |
| Noon (sun 60°) | 1.00 | 1.12 | 1.14 | 1.14 | 1.20 | 0.97 |
| Golden hour (20°) | 0.98 | 1.01 | 1.01 | 1.13 | 1.17 | 1.05 |
| Sunset (0°) | 0.97 | 1.08 | 0.99 | 0.99 | 1.05 | 1.05 |
| Twilight (−4°) | 0.95 | 1.19 | 1.27 | 0.74 | 0.89 | 1.04 |
| Blue hour (−8°) | 0.89 | 1.21 | 1.27 | 0.73 | 0.97 | 0.87 |
| Full-moon night | 1.00 | 1.05 | 1.09 | 1.04 | 1.05 | 1.01 |
| Overcast air (turbidity 3.8) | 0.91 | 1.02 | 1.07 | 1.18 | 1.22 | 0.99 |
| Fog air (turbidity 6) | 0.84 | 1.00 | 1.05 | 0.96 | 0.84 | 0.94 |

From aircraft the sky above the horizon is within 0–10% the same way. The hues: by day takram's sky is a touch less
cyan than the old one near the horizon (its red-weighted sky factors); dusk, the blue hour and moonlit nights read as
before. The celestial light is unchanged by day, at sunset and at night (the CPU's air is the tables'), 7% dimmer with
the sun 5° up (takram's clear-sky aerosol under the map's haze); the sea-level sky irradiance is within 5%.

Below the horizon the dome holds the horizon's colour as before, but the colour is takram's: from altitude the old
dome showed a saturated blue there and the new one the pale haze of the horizon itself. The sea covers it in the game.

## GPU cost

Apple M5 Pro (Metal), 2026-10-02, with other agents building (load average 175–185 over the runs): the stills
example's timings for this sky and for master `bf196ce40` back to back.

| | This sky | The sky it replaced |
| --- | --- | --- |
| A change of the air (the tables) | 77 ms, once | 0.96 ms |
| A change of the sun or moon (the tables) | nothing | 0.63 ms |
| Environment bake and prefilter | 0.10 ms | 0.33 ms |
| Dome at 1920 × 1080, by day | 0.50 ms | 0.34 ms |
| Dome at 1920 × 1080, at night (the moon's light) | 0.19 ms | 0.19 ms |

The precompute is the price of takram's tables: it runs at construction and when a scene's air changes (a map or
weather is applied, the developer console's air rows), not per frame; under this load 77 ms of GPU time in one frame.
The dome reads takram's tables per pixel (the transmittance's four loads and six trilinear reads per light, the moon's
too at dusk) where the old dome read a sky-view atlas.

## The stars (2026-10-12)

The stars are the real ones. `src/stars.rs` reads the Yale Bright Star Catalogue (5th revised edition, CDS V/50) from
`data/bright_stars.tsv`: 8,952 stars at their J2000 places with their V magnitudes and B−V colours, brightest first.
`data/build.ts` writes the table from the catalogue (`bun native/crates/naval-sky/data/build.ts`); it leaves out the
fourteen entries the catalogue withdrew and folds components closer than 90″ into one star of their summed light
(Alpha Centauri, Castor, Mizar: 144 of them). Until this change the sky drew 9,000 seeded random stars, the web
game's.

- **The grids.** The dome finds a star by its cell: each cube face of the galactic sphere is 80 × 80 cells and a
  cell holds one star, whose disc (0.19° in radius) must lie inside it. The web's two grids, one shifted half a cell
  both ways, could not hold a star near one cell's edge on one axis and near the middle on the other (about a
  quarter of the sky), which a random catalogue avoided by drawing again. There are now four grids, shifted half a
  cell along neither axis, both, the first and the second (`STAR_GRIDS`), so every star is drawn at its true place;
  `stars()` in `celestial.wgsl` reads four texels a pixel instead of two, at night only. A star whose disc crosses
  a cube face's edge (about one in a hundred) is written in a cell of each face, each face drawing its own part,
  and a star flickers by its magnitude, not its place in a cell, so the two parts flicker as one. Stars too close
  for a cell each (the Pleiades, Mizar and Alcor: 219 of them) share one: a cell is four texels deep
  (`STAR_LAYERS`), a star's B−V raised by 10 flagging another a layer down, which the shader reads only then. The
  texture is 486 × 1296 half-float texels (5 MB; the two grids were 0.6 MB). `stars::tests` reads every slot back
  as the shader does and holds it to its star's direction, and finds a cell for every star of the catalogue.
- **Where they stand.** `Sky::set_place(Some(Place { latitude, sidereal }))` stands the stars and the Milky Way by
  a place on the Earth and the local sidereal time there: the pole the latitude above the north point, right
  ascension zero the sidereal time west of the meridian (the scene's axes are the theatre's, x east and z south).
  The theatre sets it every frame (`theatre::Theatre::light`, from `conditions::place`: the subject's latitude and
  `naval_sim::theatre::sun::sidereal_deg` of the scenario's clock and its longitude), which costs no bake, so the sky
  turns with the clock and agrees with the theatre's sun, reckoned from the same sidereal time. The Southern Cross
  stands over Savo on the evening of 8 August 1942 and Orion over a January Atlantic (`state::tests`). A scene
  with no place (the port, a staged battle) keeps the old frame: the pole a quarter turn from the authored sun.
- **The moon (2026-10-12).** A theatre's moon is the real one of its date: `naval_sim::theatre::moon::position`
  (the Astronomical Almanac's low-precision series, seen from the place itself: parallax lowers it by up to a
  degree) gives where it stands and its phase, `theatre::conditions::moon` hands them to the scene's conditions
  (`BattleConditions::moon`) and `MapSelection::resolve` to `SkyScene::moon` and `moon_phase`. It moves with the
  sun's steps (`Theatre::light`), since its light is baked as the sun's is. Its tests hold it on the sun at the
  eclipse of August 1999, opposite it at the eclipse of January 2000, and under the horizon when Mikawa attacked
  off Savo. A battle and the port have no date: their moon is still full and opposite the sun, or the console's
  `moonPhase` along the sun's arc. Nothing in the simulation reads the moon: sighting takes its dark from the sun.
- **The night's own light.** With the real moon about half of all night hours have none, and a sky lit by nothing
  drew a black sea and an unseen ship. The dome and the environment's bake now add `NIGHT_GLOW` (`celestial.wgsl`):
  airglow and the stars too faint to draw, a flat blue about as bright as the Milky Way's brightest, fading
  through twilight and under a moon with `sky.milky_way`. That is some ten times the real figure, chosen so a
  moonless night is plainly darker than a moonlit one yet a ship still shows against the sea
  (`.build/native/moon/`, a Bismarck at night under a full moon and under none). It costs the Milky Way some
  contrast.
- **Left as they were.** Places are J2000's with no precession to the 1940s (0.8°), no proper motion
  and no refraction. The pole stands over the scene's −Z, which is the chart's grid north, as the theatre's sun is
  drawn by its true bearing on the same axes: far east or west of a high-latitude scenario's origin both are a
  degree or two off true north together (Codex review). There are no planets. The Milky Way is the procedural
  band, in the same galactic frame as the stars. The environment's bake holds the Milky Way where it stood at the
  last bake (a sun step), not each frame.

## Open problems

- **The precompute's hitch**: a change of the air spends about 77 ms of GPU (under load) in one frame. A weather
  changing in play (`naval_clouds::GameSky::blend`, `docs/clouds.md`, "Weather") changes the air once, halfway through
  its drift, so it pays that once; spread it over frames (the scratch already lives outside the runtime's tables, and
  the resolve swaps them at once) to drift the air too. A new sky of the same air as the one on screen takes its tables
  instead (`Sky::take_tables_from`, above). The sun moving with a battle's clock (0.25° at a time) computes nothing.
- **The dome's cost** by day is about 1.5 times the old dome's. A sky-view table per frame (Hillaire's), filled from
  takram's tables, would bring it back if it matters.
- **Below the horizon from altitude** the held horizon colour is the haze's (above). Only visible where the sea does
  not cover the dome.
- **The sky mean for eye adaptation** is `naval_sky::SkyMeanLayer`: put it after the sky's layer and set the
  renderer's sky mean to `SkyMean::Gpu`.
- **Hot reload through the registry:** `shaders::register(ctx)` adds the sky's WGSL to the renderer's library
  (`shaders::FILES`), `Sky::register_pipelines(ctx)` builds the precompute, milky way, environment bake and prefilter
  and the dome there, and `Sky::adopt_pipelines(registry, ids)` takes the registry's current ones each frame (a
  rebuilt precompute pass computes the tables again). A sky built without a renderer keeps composing its own.
- **Quality changes** replace the atlas texture. Consumers must watch `layout` (or re-fetch the bind group).
- **The blurred atlas levels** keep three's latitudinal/longitudinal Gaussian chain and its faint face seams around
  roughness 0.5.

## The weather's haze and the aerial perspective (2026-10-03)

The haze lane (`haze.md`) draws the air between the eye and everything in the world from one volume, the atmosphere's
tables and the weather's haze. What lives here:

- `haze::Haze` and `Sky::set_weather_haze` / `weather_haze`: the marine boundary layer's aerosol the visibility asks for
  over the tables' own air (`haze::SIGHTING_DEPTH` over the visibility at 550 nm, the one calibration:
  `Haze::sighting_extinction`), its layer, phase and albedo; setting it takes the sun's and the
  moon's light at the sea (`SeaLevel`, so `light()`) again through it, delta-Eddington scaled, so a low sun dims on
  every lit mesh as it does in the haze. Not to be confused with `set_haze(grey, shade)`, the old overcast greying of
  `fog_radiance` (both now unread by the scene).
- `aerial::AerialCopy`: a buffer copy of the renderer's volume and its dome slice in group 1 (binding 12, read through
  `naval_sky::aerial`: `aerial_along(uv, distance)` for the clouds' march, `aerial_over_sky(color, uv)` for the dome).
  A buffer because the passes that bind group 1 sample as many textures as a stage may.
- The dome draws the dome slice's haze over its radiance (`dome.wgsl`), so its horizon is the hazed sea's.
- The environment's far fog map (binding 3, `fog_main`, `fog_fill.wgsl`, `env_fog`) is gone.

## Clouds and weather in this crate

What `naval-clouds` (`native/docs/clouds.md`) uses here. Changing any of it means changing both crates.

- **The contract above** (`sun_sky_scalar_irradiance`, `sun_sky_irradiance`, `sky_radiance_to_point`,
  `sky_radiance_along`, the planet frame) lights takram's clouds and their aerial perspective.
- **`SkyScene.clouds` and `SkyScene.weather`** are the scene's. The sky itself reads only the cloud wind: `update`
  drifts `SkyUniforms.wind_offset` by it and `seek(t)` stands it at wind × t; `set_wind_offset` places it exactly.
  `SkyScene::cloudless()` clears them for sky-only stills. (`CloudGenus` and `HighClouds` remain as scene data.)
- **`SkyUniforms`** keeps `camera_position`, `sun_direction`, `moon_direction`, `time`, `wind_offset`,
  `inverse_projection`, `view_to_world`, `viewport`, `flash`, `lightning_position`, `lightning_intensity`, `haze`
  and the scene light (`light_direction`, `light_color`, `sun_irradiance`, `moon_irradiance`): 544 bytes, the layout
  test checks the Rust mirror. The clouds set lightning with `set_lightning(position, intensity, flash, bolt)` and the
  haze with `set_haze(grey, shade)`.
- **`Sky::attach_clouds(device, CloudHooks { modules, bind_group_layout, bind_group })`** recomposes the dome and the
  environment bake with the shader def `CLOUDS` over the clouds' naga_oil modules, binding theirs at group 2 (the
  bake's own atlas and parameters at group 3). The hook module `naval_clouds::hooks` provides
  `clouds_shadow_length(pixel: vec2<f32>) -> f32` (km of the view ray through that pixel in the clouds' shadow, 0
  without shafts; the dome passes it to takram's sky radiance, as takram's sky reads its shadow-length buffer) and
  `clouds_bake(origin, direction) -> vec4<f32>` (origin in world metres at the sea under the camera; premultiplied
  radiance in game units with aerial perspective, transmittance in a). The cirrus hooks are gone with the old clouds.
  `has_clouds()` says whether they are attached; `set_cloud_bind_group` swaps the group.
- **`Sky::update` splits** into `update_tables` (time, drift, uniforms, the tables) and `update_environment` (the
  bake, swept with clouds attached), so whatever the clouds fill for the bake is filled between them.
- WebGPU's 16 sampled textures per stage: the sky's group holds eight, so a pass that binds it has eight left.

## Re-sync

The native sky no longer ports `src/game/sky`'s atmosphere, dome grade or environment colours: a re-sync of the web's
sky has nothing to bring over but the celestial bodies (`celestial/`; the stars are no longer the web's), the environment's layout and
prefilter, and the scene light's grade (`seaLevelLight`, `SUN_GRADE`, `MOON_GRADE`, `TWILIGHT` as a starting point).

## Needs from naval-sim

Nothing: the sky is presentation only. The theatre's sidereal time (`theatre::sun::sidereal_deg`) and its moon
(`theatre::moon`) were added for the stars and the moon on 2026-10-12.
