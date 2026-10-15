# Sky: an overview for re-implementers

This crate is the game's sky: a WGSL port of takram three-geospatial's atmosphere (Bruneton's precomputed atmospheric
scattering) with the game's grade over it, a dome drawn at the far plane, the sun, moon, stars and Milky Way, and a
prefiltered environment atlas that ships and the sea read for reflections and ambient light. The code is copied
verbatim from the game and does not build here. Read this page first, then the files in "Reading order".

**Path mapping.** The sources and the docs still use the game's monorepo paths:

| In the sources | In this repo |
| --- | --- |
| `native/crates/naval-sky` | `sky/` |
| `native/crates/naval-ocean` | `ocean/` |
| `native/crates/naval-clouds` | `clouds/` |
| `native/crates/naval-render` | `shared/render/` (only `frame.rs`, `aerial.rs`, `shaders.rs` and a few shader modules) |
| `crates/naval-sim/src/clouds` | `shared/sim-clouds/` |
| `crates/naval-sim/src/theatre/sun.rs` | `shared/sim/sun.rs` |
| `native/docs/*.md` | `docs/` |

Everything else they name (`src/game/sky/*`, `src/game/ocean/*`, the "web game", three.js nodes, `naval-client`,
`naval-ships`, `naval-terrain`, `.build/` captures, the theatre and its scenarios) is not in this repo. The web game
was removed; references to it are history. Where a doc says "the web's" it usually means "what the code was ported
from".

**Conventions.** Y up. World units are metres; the atmosphere works in kilometres. Azimuth 0 is toward +Z, 90 toward
+X; the theatre's axes are x east, z south, so north is −Z. Depth is reversed-Z (`Depth32Float`, clear 0, far plane at
0). The scene's HDR target is `Rgba16Float`. RGB channels of the atmosphere stand for 680, 550 and 440 nm. "The sea's
units": linear radiance in which the sun above the air has luminance `sun_intensity` (about 6) and a daylight sky pixel
is about 0.3 to 1.0.

## 1. What it produces

| Output | Where | Who reads it |
| --- | --- | --- |
| The dome: graded sky radiance plus sun disc, moon, stars, Milky Way, a night glow, and lightning airlight | `shaders/dome.wgsl` (`dome_fragment`), one full-screen triangle at depth 0, `GreaterEqual`, no depth write | The scene pass, `Phase::Sky` (after opaque geometry and the water) |
| The atmosphere's tables | `src/tables.rs`, `shaders/precompute.wgsl` | Dome, environment bake, clouds, aerial volume, rain/spray |
| An atmosphere API in WGSL (`naval_sky::atmosphere`) | `shaders/atmosphere.wgsl` | The clouds (`clouds/shaders/march.wgsl`, `hooks.wgsl`), the aerial pass, rain, splashes |
| Environment atlas: prefiltered radiance in three.js's PMREM "CubeUV" 2D atlas layout, `Rgba16Float`, alpha = clear-sky luminance | `src/environment.rs`, `shaders/environment_bake.wgsl`, `shaders/prefilter.wgsl`; sampled via `shaders/environment.wgsl` | Ships (specular + diffuse IBL), the sea's reflection and water body (`ocean/shaders/surface.wgsl`), the aerial pass's sky light, the sky mean |
| The scene's directional light (`CelestialLight`: direction, colour with max channel 1, intensity, night flag) and `SeaLevel { sun, moon, sky }` | `Sky::light`, `Sky::sea_level` in `src/sky.rs`, computed on the CPU in `model::sea_level_light` and `model::sky_irradiance` | Every lit mesh, the sea's glitter, cloud shadows |
| Sky mean for eye adaptation: `env_radiance(up, roughness 1)` in rgb, clear-sky luminance in w | `src/sky_mean.rs`, `shaders/sky_mean.wgsl` | The renderer's exposure (not in this repo) |
| The weather's haze model (CPU side) and a copy of the aerial-perspective volume the sky's own passes read | `src/haze.rs`, `src/aerial.rs`, `shaders/aerial.wgsl` | The aerial pass (`clouds/shaders/aerial.wgsl`), the dome, the cloud march |

The aerial-perspective volume itself is owned by the renderer (`shared/render/src/aerial.rs`) and filled by the
clouds crate (`clouds/src/aerial.rs`, `clouds/shaders/aerial.wgsl`); see section 6.

### Frame order

`clouds/src/game_sky.rs` (`GameSky::prepare`, `update_timed`) drives the game's frame:

1. Weather update (sets lightning on the sky).
2. `Sky::update_tables`: advance time and cloud drift, write the 544-byte `SkyUniforms`, bake the Milky Way once,
   run the atmosphere precompute only if the air changed.
3. Clouds prepare (their light tables, which the environment bake reads).
4. `Sky::update_environment`: bake level 0 of the atlas and prefilter, when needed (see section 5).
5. Cloud shadow maps.
6. Aerial-perspective volume fill (needs the tables, the environment and the cloud shadow), then a buffer copy into
   the sky's group 1 (binding 12).
7. Scene pass: Opaque, then (after the opaque depth copy) the cloud march, Water, Sky (dome, then cloud composite),
   Transparent.

Without clouds, `SkyLayer` (`src/layer.rs`) is the minimal `SceneLayer`: `prepare` calls `Sky::update`, `draw` calls
`draw_dome` in `Phase::Sky`. `SceneLayer` and `Phase` are in `shared/render/src/frame.rs`.

### Bind groups

Group 0 is the renderer's camera (plus the aerial volume at bindings 6 and 7). The sky's group 1 (`shaders/common.wgsl`):
0 `SkyUniforms`, 1 transmittance (`Rgba32Float`, loaded, filtered by hand), 2 scattering, 3 single Mie, 4 higher order
(3D), 5 irradiance, 6 linear clamp sampler, 7 star grid, 8 star colour ramp, 9 Milky Way, 10 its sampler,
11 `AtmosphereParameters` (304 bytes), 12 aerial volume copy (storage buffer). Eight sampled textures: any pass that
binds group 1 has eight left of WebGPU's 16 per stage. The environment is its own group (texture, sampler, 80-byte
`SkyEnvironment` uniform) that a consumer puts at any group index from 1 up.

## 2. The atmosphere model

Bruneton 2017 as takram ships it (`shaders/bruneton.wgsl` ports takram's `definitions/common/precompute/runtime.glsl`
at commit b012ad06; every function takes the tables as arguments). CPU mirror in f64: `src/model.rs`.

| Constant | Value | Source |
| --- | --- | --- |
| Planet radius / top of the air | 6360 km / 6460 km (100 km thick) | `model::PLANET_RADIUS`, `ATMOSPHERE_TOP` |
| Rayleigh at sea level (Earth) | (5.802, 13.558, 33.1) × 1e-3 per km, scale height 8 km | `EARTH_RAYLEIGH`, `RAYLEIGH_DENSITY` |
| Aerosol (clear sky) | 3.996e-3 per km scattering, albedo 0.9 (extinction 4.44e-3), scale height 1.2 km | `CLEAR_MIE`, `MIE_ALBEDO` |
| Ozone | (2.4, 3.0, 0.136) × 1e-3 per km at peak; tent peaking at 25 km, zero 15 km above and below | `OZONE_ABSORPTION`, `ABSORPTION_DENSITY` |
| Solar irradiance at top | (1.474, 1.8504, 1.91198) | `SOLAR_IRRADIANCE` |
| Sun angular radius | 0.004675 rad (the tables' only; the drawn disc is larger, section 4) | `SUN_ANGULAR_RADIUS` |
| Ground albedo, lowest sun | 0.1; mu_s_min = −0.5 (120° from zenith) | `GROUND_ALBEDO`, `MU_S_MIN` |
| Phase functions | Rayleigh 3/(16π)(1+ν²); Mie Cornette–Shanks with `mie_g` | `rayleigh_phase`, `mie_phase` |

### Tables

| Table | Size | Format | Holds |
| --- | --- | --- | --- |
| transmittance | 256 × 64 | `Rgba32Float` | **optical depth** to the top of the air (not transmittance; see changes) |
| scattering | 256 × 128 × 32 | `Rgba16Float` 3D | Rayleigh single + all higher orders (divided by Rayleigh phase); single Mie red in alpha |
| single Mie | 256 × 128 × 32 | `Rgba16Float` 3D | single Mie scattering (takram's separate table) |
| higher order | 256 × 128 × 32 | `Rgba16Float` 3D | orders 2+ alone (so light shafts can occlude only single scattering) |
| irradiance | 64 × 16 | `Rgba16Float` | sky irradiance on a horizontal surface |

The 4D scattering parametrisation is Bruneton's: width 256 = ν (8) × μ_s (32), height 128 = μ, depth 32 = r
(`SCATTERING_TEXTURE_*` constants at the top of `bruneton.wgsl`).

**Precompute** (`src/tables.rs`, `Precompute::encode`; entry points in `shaders/precompute.wgsl`): transmittance,
direct irradiance, single scattering, then for orders 2, 3 and 4: scattering density, indirect irradiance, multiple
scattering; then two resolve passes write all runtime tables at once, so a recompute swaps in one frame. Deltas are
half-float textures; sums are float32 storage buffers. Scratch is about 75 MB, created per run and dropped. Everything
is in radiance (takram's `luminanceFromRadiance` is the identity here).

**When it runs:** the first update, and whenever the air changes (`Sky::apply` with different `AirCoefficients`, or a
hot-reloaded precompute pipeline). A sun or moon move runs nothing: the tables cover every sun angle. Cost: about 77 ms
of GPU under heavy load (74 to 108 ms measured in theatre scene swaps). A new sky with the same air can copy another's
tables instead (`Sky::take_tables_from`, about 25 MB).

### Changes from takram/Bruneton (`docs/sky.md`, "Changes from takram")

- **Optical depth, not transmittance, in float32.** The runtime takes Bruneton's ratio of two transmittances as a
  difference of optical depths. Along a horizon ray in a hazy map the transmittance underflowed float32 and the ratio
  became 0/0.
- **96 quadratically growing intervals** for single and multiple scattering (Bruneton: 50 equal). Equal 15 to 20 km
  steps along a horizon ray through haze that is opaque in a few km overstated the horizon several times in fog.
  `INTEGRATION_INTERVALS` in `bruneton.wgsl`; the CPU's `optical_depth` uses the same `(i/n)²` spacing.
- **Altitude clamped at 0** in the density profiles: a sea-level sample could round below 0 in float32, pick the
  profile's lower (zero) layer and drop the densest sample (the sea-level transmittance row read 4% short in fog).
- **Single scattering held at ≥ 0** where `scattering − higher order` rounds negative in half floats.
- **Top of the air at 100 km**, not takram's 60: with the sun 8° down the planet's shadow reaches about 60 km over the
  camera, so a 60 km sky had no lit air overhead in the blue hour.
- **Ozone** raised (takram: 0.65, 1.881, 0.085 × 1e-3). The red channel stands for broadband red, which the Chappuis
  band absorbs far more than at 680 nm; at takram's value the blue hour turned magenta.
- **The moon** is a second light through the same tables (takram's WebGPU `moonScattering` idea).

## 3. Authored air, units, exposure and grade

### Authored atmosphere → coefficients (`model::air_coefficients`)

A scene authors `AuthoredAtmosphere { rayleigh, turbidity, mie, mie_g, multiple }` (`src/model.rs`; presets in
`src/state.rs`: `DEFAULT_SKY_SCENE` is the port's, rayleigh 0.42, turbidity 3.2, mie 0.25, mie_g 0.6, multiple 1.4;
`SkyScene::north_atlantic` is rayleigh 0.38, turbidity 2.2, mie 0.225, mie_g 0.72, multiple 1.0).

**The rayleigh parameter is on an inherited scale where 0.4 is physical Earth** (the scale of the commercial sky asset
the game first used; `authored::RAYLEIGH_EARTH = 0.4`). It maps to two things:

```
blue       = rayleigh / 0.4
molecules  = blue ^ 0.6                  -> rayleigh coefficients = EARTH_RAYLEIGH * molecules   (tables)
chroma     = blue ^ 0.5                  -> dome saturation = 1.35 * chroma                      (grade only)
```

So 0.4 gives Earth's coefficients exactly; 0.38 gives 0.970 × Earth and slightly less saturation. The rest:

```
excess          = max(turbidity - 1, 0)
aerosol (1/km)  = 0.003996 + 0.027 * excess * (1 + (excess / 5)^2)     // turbidity 1 = takram's clear sky; 6 -> 0.27/km
mie_extinction  = aerosol / 0.9
mie_g           = clamp(mie_g, 0, 0.85)                                  // used in the precompute's phase
mie_gain        = (mie / 0.25) ^ 0.7                                     // dome's aureole strength only
multiple        = authored, scales the higher-order table in the dome
aerial_scale    = lerp(0.5, 1, smoothstep(2.5, 4.5, turbidity))         // share of this air a view ray sees (section 6)
```

Any change of these values reruns the precompute. `mie_gain`, `multiple` and `chroma` are applied at runtime and do
not need a recompute, but they live in the same struct so the comparison triggers it anyway.

### Units (`model::game_scale`, `white_balance`, `Sky::refresh`)

The tables hold radiance for takram's solar spectrum. takram converts with radiance-to-luminance factors divided by
the sun's unit luminance (`sun_relative_luminance`, `sky_relative_luminance`). The game then:

- scales so the sun above the air has luminance `sun_intensity`: `game_scale = sun_intensity / lum(solar_luminance)`;
- white-balances so that sun is white (`white_balance`: about −8.5% red, +4% blue), because the scene light, glints
  and ships all take it as white;
- reads light the aerosol scattered (single Mie and the higher orders) with the **sun's** factors, not the sky's
  (`SUN_TINT` in `atmosphere.wgsl`, the ratio sun/sky). The sky's factors assume a λ⁻³ spectrum; with them the fog
  weather's haze read pink.

Per-frame uniforms (`Sky::refresh`): `sun_scale = sky_factors × balance × game_scale × lift × 1.4`;
`direct_scale` and `diffuse_scale` (the clouds' sun and sky light) use the same gain; `moon_scale = sky_factors ×
moon_irradiance / solar × 0.25 × 1.4 × moon_share`.

### The dome's grade (`model::sky_grade`, `shoulder`/`grade` in `atmosphere.wgsl`)

1. **Gain 1.4** over the physics (`GAIN`). Since 2026-10-03 the sun and moon on the world take the same gain, so the
   dome, clouds and lit meshes share one brightness scale.
2. **Phases.** The dome draws single Mie with an authored aureole: `(1 − 0.35) × CornetteShanks(g) × mie_gain + 0.35
   × HG(0.92) × mie_gain` (`phase_terms`, `aerosol_phase`), and multiplies the higher orders by `multiple`.
3. **Chroma**: `mix(luminance, color, 1.35 × chroma)`.
4. **Luminance shoulder**: above a knee of 0.45, `over / (over / (0.95 − 0.45) + 1) + 0.45`, ceiling 0.95.
5. **Night floor** added after the shoulder: `(0.006, 0.0105, 0.02) × (1 + 0.5 (1 − |y|)⁴)` (airglow, starlight).
6. **Below the horizon** the dome holds the horizon's colour: the direction is lifted to 0.002 (cosine) above the
   geometric horizon of the camera's altitude (`held`), with phases still taken toward the true direction so the
   aureole fades.

### Twilight and night (`model::TWILIGHT`, `twilight_lift`, `Sky::apply`)

The sky's exposure rises as the sun sets, interpolated in log, keyed on the sun's elevation above the **camera's**
geometric horizon (so it shifts with altitude): 3° → 1, 0° → 3.5, −2° → 8.4, −4° → 20.2, −6° → 78.4, −8° → 291,
−10° → 672. These are the game's old table lifted an eighth below the horizon because takram's sky is darker there.
The lift multiplies the sky and the clouds' light, so dusk stays readable at a fixed display exposure.

- The sun lights the sky until −14° (`SUN_SETS`). The moon lights it while above −6° (`MOON_SETS`), fading in as the
  sun goes from 8° to 2° (`MOON_FADE`).
- Camera altitude is tracked in steps of 2% (or 5 m), up to 90 km; a step updates the lift and re-bakes the
  environment.
- The scene light is whichever of sun and moon has the larger max channel at the sea (`state::choose_light`).

### Light at the sea (CPU, `model::sea_level_light`, `sky_irradiance`, `Sky::light_the_sea`)

- Sun: transmittance from the CPU's optical-depth integral (500 quadratic samples) of the same air, luminance equal to
  the tables' sun; hue is `(T_sun / T_zenith)^0.6`, tinted (1, 0.95, 0.85); never under 12% of the zenith sun as it
  sets; faded out between −1° and 2°. Then × 1.4 and × the weather haze's beam transmittance.
- Moon: `0.8 × tint #b4c9f0 (linear) × (1 − cos 2πφ)/2`, × 0.8 transmittance once 6° up, × 1.4, × haze.
- Sky irradiance on the sea: a CPU single-scattering estimate (4 rings × 8 sectors × 16 steps) with a `1 + 0.5 ×
  multiple` fill, the lift, the gain and the night floor. The tables are never read back (the crate is portable).

### Display (`src/display.rs`, `shaders/display.wgsl`)

A capture-only transform: exposure × 0.9, contrast 1.25 about 0.18, then three.js r185 AgX, into an sRGB target. The
game's real display pass (bloom, eye adaptation, a night grade that lifts world shadows by up to 1.25 stops and dims
the sky by a quarter stop as the clear-sky mean falls from log2 −3 to −5.5) lives in the renderer, not here; it is
described in `docs/lighting.md`, "Night readability" and "Realism".

## 4. Celestial bodies (`shaders/celestial.wgsl`, `src/state.rs`, `src/stars.rs`)

All are radiance above the air; the dome multiplies by `transmittance_to_space` (faded over ±0.0047 cosine at the
horizon).

- **Placement.** Without a place (`CelestialModel::set`), sun and moon ride one arc: the meridian at the authored sun's
  elevation, the pole opposite; the moon is `2π × phase` behind the sun, tilted 28° off the arc. With a place
  (`Sky::set_place(Place { latitude, sidereal })`), the pole is `(0, sin lat, −cos lat)` and right ascension 0 lies
  the sidereal angle west of the meridian; the theatre computes sidereal time in `shared/sim/sun.rs`
  (`sidereal_deg`, a low-precision Astronomical Almanac series) and sets it every frame (no bake). A theatre's moon is
  the real one (its almanac code is not in this repo); `SkyScene::moon` places it.
- **Sun disc** (`sun_disc`): drawn radius 0.01225 rad (about 0.7°, 2.6× the real sun; the tables use the real
  0.004675). Limb darkening `1 − (0.5, 0.6, 0.72)(1 − μ)`, corona `0.045 × exp(−Δθ / 0.2°)`, × 1.35 × `sun_intensity`,
  anti-aliased over 0.8 px, occluded by the moon.
- **Moon** (`moon_disc`): same radius. A sphere lit by the true sun direction: `lit / (lit + z + 1e-3) × 1.7 + 0.15 lit`
  (a Lommel–Seeliger-like term), opposition surge `0.38 + 0.62 ((1 − sun·moon)/2)^1.5`, earthshine `0.016 (1 − lit)`
  tinted (0.8, 0.87, 1). Procedural near side: 14 elliptical maria, 8 named craters, ray systems and ejecta from MaterialX
  noise, detail fading in with the disc's size on screen. A tight halo `exp(−Δθ / 0.12°)`. Surface radiance =
  `tint × lum(moon irradiance) × 2.8 / lit` so the disc's total tracks the moonlight.
- **Stars**: the Yale Bright Star Catalogue (`data/bright_stars.tsv`, 8,952 stars, J2000, V magnitude and B−V; built by
  `data/build.ts`). Stored in a cube-face grid over the galactic sphere, 80 × 80 cells a face with a tangent warp
  (`WARP` 0.31157, 0.08442), one star per cell, four grids shifted by half a cell along neither, both, x or y axis
  so every star fits one at its true place, cells four layers deep for close groups (Pleiades); 486 × 1296 half-float
  texture. The shader reads four texels per pixel, at night only (sun below 1°). Flux `2^(−0.731 m)` (a 0.55
  compression of the magnitude scale); limiting magnitude `−1.5 + 0.55 (−2 − sun_elev) − 1.4 × moonlight`; Gaussian
  image of 0.8 px (wider for bright stars); colour from a 64-entry B−V ramp, desaturated for faint stars; zoom gain
  `min(sqrt(magnification vs a 52° field), 3)`. Not drawn below `−sqrt(2h/R) − 0.02`.
- **Twinkling**: two sines seeded by the star's magnitude (so a star split across a face edge flickers as one),
  amplitude 0.1 at the zenith rising to 0.6 at the horizon, floor 0.15.
- **Milky Way** (`shaders/milky_way.wgsl`): baked once into a 2048 × 512 `Rgba8Unorm` equal-area band in galactic
  coordinates (longitude across, sin latitude down), storing `sqrt(radiance / 2)`, 12 box-filtered mips. Procedural:
  blobs, ridged noise, dust lanes. Brightest parts 0.04 (`MILKY_WAY_RADIANCE`) × `(1 − smoothstep(−16°, −9°, sun)) ×
  (1 − 0.45 moonlight)`. Equatorial → galactic matrix: `state::EQUATORIAL_TO_GALACTIC`.
- **Night glow** (`NIGHT_GLOW` (0.75, 1.15, 1.9) × the Milky Way share): a flat blue about the Milky Way's brightest,
  about ten times the real airglow, so a moonless night (half of all night hours with a real moon) is darker than a
  moonlit one but a ship still shows.
- **Lightning airlight** (`flash_airlight` in `dome.wgsl`): closed-form single scattering of a point flash along the
  view ray, `∫dt/r² = (π/2 + atan(along/offset)) / offset`, with the local air density and phases. Optional.
- Not modelled: precession to the 1940s (0.8°), proper motion, refraction, planets.

## 5. Environment bake and prefilter

**Layout** (`CubeUvLayout`, `src/environment.rs`): three.js's PMREM CubeUV atlas in one 2D texture. Face size
`S = 2^floor(log2(w / 4))` for a bake width `w` of 256/384/512/768 (Low to Ultra): 64 or 128. Atlas `3 × max(S, 112)`
by `4 S` (384 × 512 on High). A level is a 3 × 2 block of faces (+X +Y +Z / −X −Y −Z), each face with a one-texel
border. Sharp levels from `max_mip` down to 4 (16² faces), then six extra blurs at 16² with Gaussian sigma 0.125,
0.215, 0.35, 0.446, 0.526, 0.582 rad. **Orientation**: texel row 0 is the top and stored +Y holds the sky; three.js's
render-target atlas is y-flipped relative to this (`docs/sky.md`, "Layout"). If you are not matching three.js, any
prefiltered cubemap with GGX-roughness → mip lookup will do.

**Bake** (`environment_bake.wgsl`, `bake_main`, 8 × 8 workgroups): each level-0 texel's direction is evaluated directly
(no equirectangular step), from sea level (2 m) under the camera.

- Above the horizon: `sky_radiance(dir, from_sea)` plus the moon as a blurred blob (`moon_glow`) and the Milky Way at
  the matching mip (`celestial_diffuse_radiance`). **No sun disc, no stars**: consumers draw their own glints. With
  clouds attached, `color = sky × clouds.a + clouds.rgb` from the clouds' short march (`clouds_bake` in
  `clouds/shaders/hooks.wgsl`).
- Below the horizon: the sea as seen from above. Mirrored sky × Schlick (F0 0.02); the scene light mirrored into a
  Cox–Munk glitter lobe (Beckmann with slope variance 0.039, a 7 m/s wind, Smith masking), faded in over the first 15°
  below the horizon so the sea's own reflection does not count a low sun twice; and a water body returning
  `(0.003, 0.015, 0.035)` of the light into it (about 1.4% in luminance). The sky irradiance and the cloud
  transmittance of the light are averaged from 64 shared samples per workgroup.
- Alpha: the luminance the texel would have without clouds. The eye adaptation meters against it.

**Prefilter** (`prefilter.wgsl`, `blur_main`): three.js's separable Gaussian chain. Each level is blurred from the
previous one by the Gaussian that widens it to its own sigma, latitudinally about a pole then longitudinally, up to 20
taps per side; the pole turns from level to level. Known artefact: faint face seams around roughness 0.5.

**When it refreshes** (`Sky::encode_environment`): a whole bake and prefilter after any `apply` (sun, moon, air), a 2%
camera-altitude step, the first frame, or `refresh_environment()`. With clouds attached it also sweeps a quarter of
the rows every fourth frame and prefilters after the last band (16 frames per full refresh), and re-bakes at once
after a 2 km camera jump. `generation` bumps per prefilter. Cost about 0.10 ms (M5 Pro).

**Sampling** (`shaders/environment.wgsl`, mirrored for the ocean as `ocean/shaders/sky_environment.wgsl`):

- `env_radiance(atlas, sampler, env, dir, roughness)`: three's `roughnessToMip`, `mip = −2 log2(1.16 r)` below
  r = 0.21, then linear through (0.21, 4), (0.305, 3), (0.4, 2), (0.8, −1), (1, −2); one or two bilinear taps.
- `env_irradiance(n) = env_radiance(n, 1) × π`. This is a 0.582 rad Gaussian of the sky, **not** a cosine convolution:
  3% under the dome's true irradiance on a level surface and 5 to 20% over on upright surfaces and undersides.
- `env_reflection_direction(n, v, roughness)`: three's mirror direction bent toward the normal by roughness⁴.
- The sea reflects `normalize(vec3(r.x, max(r.y, 0), r.z))` at its own roughness; the water body reads
  `env_radiance(up, 1) × π`.
- `SkyEnvironment` (80 bytes) also carries the active light (direction, intensity, colour), the night flag, the sun and
  moon directions, moon phase and illumination, so the sea can draw its own glints.

## 6. Aerial perspective and haze (`docs/haze.md`)

One model for the air between the eye and everything: ships, sea, land, effects, the dome and the clouds.

**Two media along each ray.**

1. The atmosphere's tables, via `sky_radiance_to_point` (graded as the dome is, the night floor scaled by opacity).
   A view sees only `aerial_scale` of it: 0.5 up to turbidity 2.5, all of it from 4.5. The opacity uses that share and
   the in-scatter is rescaled to keep the colour (`thinned` in `clouds/shaders/aerial.wgsl`:
   `(1 − T^k) / (1 − T)`). The dome, the lighting and the environment keep the whole air. This was an art choice
   (2026-10-09) after measuring that the tables were physically right but land seen from 1.5 to 3 km altitude was
   frosted.
2. The weather's haze (`src/haze.rs`, `Haze`): a marine boundary layer, density `exp(−h / 250 m)` up to the layer's
   top (the cloud base clamped to 400 to 1500 m), then `exp(−Δh / 250 m)` above it. Albedo 0.95, Cornette–Shanks
   g = 0.75. Extinction at sea (550 nm) = `SIGHTING_DEPTH / V − aerial_scale × atmosphere's own`, never negative,
   where V is the game's visibility and **`SIGHTING_DEPTH = 2.42` = ln(0.45 / 0.04)**: a hull's 0.45 contrast falls to
   4% at V, so a ship disappears at about the range the simulation's lookouts lose her. Koschmieder's 3.912 (2%) then
   lies at 1.62 V. Path length to space uses a Chapman grazing limit `sqrt(2H / (π r))` (`Haze::path`).

**Lighting of the haze**: sun and moon through the tables' transmittance (with the twilight lift and gain), through the
haze above delta-Eddington scaled (`(1 − ω g) × extinction`), and through the clouds' ground shadow map (beyond its
40 km, the map's mean). Sky light from the environment atlas weighted by the haze phase: a quadrature of a cone within
25° of the ray from a 12° level, rings at 50° and 105°, and the back from the roughest level. The haze dims the
atmosphere's in-scatter by the haze in front of it, and vice versa. Under clouds, the cloud shadow removes the sun's
share of the atmosphere's in-scatter and the environment lights that air instead (a closed deck gives grey distance,
not a sunlit horizon band).

**The volume** (`shared/render/src/aerial.rs`, Hillaire 2020 camera froxels): 32 × 32 screen columns, 32 radial slices
`d_k = 20 m × (450 km / 20 m)^(k/31)` (each 38% farther). One `Rgba16Float` 3D texture 32 × 32 × 64: in-scatter in
layers 0 to 31, **opacity** (1 − transmittance) in 32 to 63, so zeros mean clear air. A 32 × 32 dome slice beside it
holds the haze to the end of the air (rgb in-scatter, a opacity). Fill: workgroups of 2 × 2 columns × 32 slices, each
thread one slice interval in 4 steps, then a scan down the column. Columns whose ray meets the sea are held at the
horizon (as the dome is), else the band over the horizon flickered with camera pitch. Under the sea the volume is
clear air. Cost about 0.12 ms, independent of resolution.

**Reading it.** Materials: `aerial_apply(color, world) = color × T + inscatter` and `aerial_apply_additive` (dim only,
for glows and tracers), in `shared/render/shaders/aerial.wgsl`, sampled trilinearly by screen uv and radial distance;
nearer than 20 m the first slice is scaled by `d / 20`. The dome and the cloud march cannot spare a texture slot, so
they read a storage-buffer copy at group 1 binding 12 with manual bilinear + linear filtering and a hand-written
half-float decode (`sky/shaders/aerial.wgsl`; `unpack2x16float` needs a capability not all devices grant). The dome
draws `color × (1 − a) + rgb` from the dome slice; the march calls `aerial_along(uv, front_depth)`.

## 7. Couplings with the ocean and clouds

- **Ocean** reads: the environment atlas group (reflection, water body, the sky mean for spindrift), the
  `SkyEnvironment` light fields for its own sun/moon glitter, the aerial volume at group 0, and `CelestialLight`.
  The atlas's below-horizon sea is for ships and aircraft; the ocean clamps its reflected rays to the horizon and must
  not re-read that half.
- **Clouds** read the atmosphere API (planet frame `planet_km(p) = ((p.x − cam.x)/1000, p.y/1000 + 6360,
  (p.z − cam.z)/1000)`): `sun_sky_scalar_irradiance` (sun through the air and planet shadow, sky over the sphere × 2π),
  `sun_sky_irradiance`, `sky_radiance_to_point`, `sky_radiance_along`, and the `direct_scale`/`diffuse_scale` uniforms.
  They supply hooks back: `clouds_shadow_length(pixel)` (km of the view ray in cloud shadow, fed to takram's light
  shafts in the dome), `clouds_bake(origin, dir)` and `clouds_bake_light` for the environment. `Sky::attach_clouds`
  recomposes the dome and bake with shader def `CLOUDS` and binds the clouds at group 2 (the bake's atlas at group 3).
- **Cloud drift**: the sky owns `wind_offset` (`update` adds wind × dt; `seek(t)` sets wind × t).
- **Lit meshes** take `SeaLevel`/`CelestialLight` each frame; the cloud ground shadow marches toward the active body
  (the moon once it outshines the sun).
- **naga_oil gotcha**: a struct field named `sky` gets rewritten in any module that imports the `sky` uniform, hence
  `SunSky { sun, skylight }`.

## 8. Lessons and pitfalls (from `docs/sky.md`, `docs/haze.md`, `docs/lighting.md`)

Precision and the precompute:

- Store optical depth in float32 and subtract; transmittance ratios go 0/0 in haze at the horizon.
- Use non-uniform (quadratic) integration steps along long rays through dense low air, or the horizon is overstated
  several times.
- Clamp altitude ≥ 0 in density lookups and scattering ≥ 0 after half-float subtractions.
- A 60 km atmosphere leaves the zenith unlit in the blue hour; use 100 km.
- 77 ms precompute in one frame is a visible hitch on weather changes; spread it over frames or copy tables between
  skies of the same air. The dome reading the 4D tables per pixel costs 0.50 ms at 1080p by day against 0.34 ms for a
  sky-view LUT; a per-frame Hillaire sky-view table would recover that.

Colour:

- Read aerosol-scattered light with the sun's spectrum, not the sky's λ⁻³ factors (pink fog otherwise).
- Physical ozone at "680 nm" leaves the blue hour magenta; the red channel is broadband.
- White-balance so the sun above the air is white if meshes treat the light as white.
- AgX turns saturated reds pink in its shoulder; the hinomaru red went pink at 2.7 × albedo exposure.

Brightness and balance (measured values worth keeping):

- One gain for sky, sun and clouds (1.4). The sky had stood 1.4 × over the sunlit world; a separate cloud gain of 1.9
  washed the sea.
- The world's sun must go through the same air as the sky: a readability ramp that held the full sun from 18° up made
  lit surfaces 12 to 27% too bright and ignored haze.
- With the sun 61° up in the clear weather: sun on a facing surface 7.61 (sea units), sky on a level surface about 0.57,
  direct/sky ratio on a level surface 11.7 against a physical 5.7 to 7.8. The clear weather is clearer than real
  maritime air (aerosol optical depth 0.024 against about 0.11); the authored `mie_gain` of 0.45 makes the aureole put
  back less than half the aerosol's light, so the dome's sky is 13% under the tables' own irradiance.
- Exposure is about 0.8 stop brighter than a camera's (one unit about 13.8 kcd/m²).
- The sea in the environment originally returned 1% of the light; a real sea returns about 4.6% under a 61° sun (Payne
  1972), more as the sun sinks. Airframe undersides were a fifth as lit as they should be.
- Eye adaptation: meter against the clear-sky luminance (atlas alpha) so overcast noon looks near clear noon (at most
  2 stops up), never lift the brightest tenth past 1.5 stops over the clear sky mean, fade out from day to night, and
  close up to 1 stop for glare. Bloom strength 0.1; at 0.3 the glitter around hulls veiled them.

Night:

- Ships glowed at night (200 to 440 × the sea beside them) from a 1.5 × moon boost, cloud shadows that stopped only the
  sun, and a constant hemisphere fill. Fixed by giving meshes the moon as the sea takes it, marching cloud shadows
  toward the active body, and no fill at all (only lightning fills).
- A real moonless sky is black for gameplay purposes; the night glow is about 10 × the real airglow.

Haze:

- The old distance fog ramp faded to a colour that was not the sky behind, so ships stayed visible past the
  visibility. Applying one physically based medium to everything fixed it, at the cost of every ship being fainter at
  every range.
- From altitude most of the veil was the tables' molecules and aerosol, not the weather haze; fixed by showing views
  half the atmosphere on fair days and moving the haze to a 250 m surface layer.
- Open: the environment holds no haze, so reflections and ambient see a slightly bluer horizon than the hazed dome; the
  haze has only delta-Eddington forward multiple scattering, which shows as a grey-brown band at the layer top from
  aloft; the clouds' environment bake still lights the air under a deck with the sun.

Layout and binding:

- WebGPU's 16 sampled textures per stage forced the aerial volume into a storage buffer for the sky's passes and
  removed the old far-fog map.
- The stars' old two-grid scheme could not hold about a quarter of the sky's positions; random stars hid it,
  catalogue stars exposed it. Four shifted grids and layered cells fix it.

## 9. Reading order

1. `docs/sky.md`: the lane doc; the atmosphere, its changes, the interface, environment layout, stars.
2. `sky/src/model.rs`: every constant, the authored-to-physical mapping, twilight, grade, sea-level light (CPU, f64).
3. `sky/shaders/atmosphere.wgsl`: how the tables are read in game units and graded; the clouds' contract.
4. `sky/src/sky.rs`: `Sky::apply`, `refresh`, `prepare_tables`, `encode_tables`, `encode_environment`: the per-frame
   state machine and uniform scales.
5. `sky/shaders/bruneton.wgsl`: takram's Bruneton functions (`get_sky_radiance`, `get_sky_radiance_to_point`,
   `get_sun_and_sky_irradiance`, the precompute integrals).
6. `sky/src/tables.rs` with `sky/shaders/precompute.wgsl`: table sizes, formats, pass sequence.
7. `sky/shaders/dome.wgsl` and `sky/shaders/celestial.wgsl`: the backdrop, discs, stars, Milky Way.
8. `sky/shaders/environment_bake.wgsl`, `sky/shaders/prefilter.wgsl`, `sky/shaders/environment.wgsl`: the IBL.
9. `docs/haze.md` with `sky/src/haze.rs`: visibility calibration and the haze layer.
10. `clouds/shaders/aerial.wgsl` and `shared/render/shaders/aerial_volume.wgsl`: the froxel fill and read.
11. `sky/src/state.rs`: scene presets, celestial placement, star and Milky Way visibility curves, moon lighting.
12. `docs/lighting.md`, "Realism", "Into the sun", "Sun and sky at the sea": measured balance and eye adaptation.

Tests worth reading for expected behaviour: `sky/tests/atmosphere.rs` (transmittance within 0.5% of the CPU integral
from zenith to 1° for turbidity 1, 2.2 and 6; sky finite from 30 m to 40 km at noon, sunset, night and fog; light
continuous through sunset; atlas orientation) and `sky/tests/light_balance.rs` (the table in section 8).

## 10. Game-specific parts you can ignore

- three.js compatibility: the exact CubeUV atlas layout, its orientation notes, the Gaussian prefilter chain and the
  Gaussian (not cosine) irradiance exist to match the removed web game's materials. Use your engine's reflection probe
  or a GGX-prefiltered cubemap plus SH irradiance.
- `fog_radiance`, `overcast`, `Sky::set_haze(grey, shade)` and `SceneFog` terms: legacy, no longer drawn.
- Pipeline registry, hot reload, deferred pipelines and placeholders (`new_deferred`, `adopt_pipelines`,
  `register_pipelines`, `shaders.rs` registration), `set_timestamps`, `Diagnostics`, the examples (they compare against
  a sky that no longer exists), `SkyQuality` tied to a "Clouds" graphics setting.
- `SkyScene` fields for clouds and weather (only the wind drift is read here), the `port` flag, the map presets.
- The theatre plumbing: `Place`, sidereal time, the real moon's almanac, `take_tables_from` for scene swaps.
- The `SIGHTING_DEPTH` calibration is tied to the game's lookout rules; pick your own visibility definition (Koschmieder
  3.912 is the meteorological one).
- Readability choices you may or may not want: the 2.6 × sun/moon disc, twilight lift, night floor, night glow,
  readability floor on the setting sun, the half-atmosphere aerial share on fair days, the dome grade's shoulder and
  chroma, the 1.4 gain.
- Lightning airlight, the procedural moon's maria table, the moon's earthshine constants: polish, not structure.
