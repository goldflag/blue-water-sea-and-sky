# Clouds: overview for re-implementers

This directory is the game's volumetric cloud renderer: a WGSL/Rust port of takram's `@takram/three-clouds`
(packages/clouds of three-geospatial, MIT), lit through the sky crate's Bruneton atmosphere (`sky/`). The game
changed a good deal of it afterwards. This page is meant to let you write your own clouds without reading all 15k
lines. Every claim cites a file and function so you can check it. The reasons and measurements behind the choices
are in the dated development log `docs/clouds.md`, and the haze is described in `docs/haze.md`.

**Path names in the sources.** The comments and docs use the game's monorepo paths. In this repository:

| In the sources | Here |
| --- | --- |
| `native/crates/naval-clouds` | `clouds/` |
| `native/crates/naval-sky` | `sky/` |
| `native/crates/naval-ocean` | `ocean/` |
| `native/crates/naval-render` | `shared/render/` |
| `crates/naval-sim/src/clouds` | `shared/sim-clouds/` |
| `native/docs/*.md` | `docs/` |

Anything else is not included: `src/game/*` (the removed web game, including its old `sky/weather` that the rain and
lightning were ported from), `naval-protocol`, `naval-client` (where `weather.rs` drifts weather and runs the battle
clock), the harness and the simulation beyond `shared/`.

Units: metres, and a "planet frame" whose centre is straight below the camera at radius 6,360 km
(`layer.rs::PLANET_RADIUS`). The atmosphere functions take kilometres. Radiance is on the sky dome's scale, meaning
the sun's light includes the dome's grade gain (`direct_scale × sky_grade::GAIN`) and no separate cloud gain is
applied.

---

## 1. What it produces, and the pass order

Outputs each frame:

- a full-resolution premultiplied cloud colour (radiance, opacity), composited over the scene after the dome;
- a light-shaft length per pixel (km of the view ray in cloud shadow), which the sky dome takes out of its in-scatter;
- Beer shadow maps (BSM) in up to 3 cascades, used by the march;
- a 40 km ground shadow map (sun transmittance down to the sea), which the ocean, ships and land multiply into their
  direct light;
- clouds inside the sky's environment bake, so the sea reflects them;
- the aerial-perspective froxel volume that every material reads (filled here because it needs the cloud shadow);
- weather: near-camera rain streaks, splashes on the sea, lightning bolts and flashes, a thunder cue, and heavy
  rain's screen veil.

### Textures

| Texture | Size, format | Origin | File |
| --- | --- | --- | --- |
| Local weather | 512², `Rgba8Unorm`, full box-filtered mip chain, tiles every 100 km | takram's shipped PNG, embedded | `shared/sim-clouds/local_weather.png`, `textures.rs::local_weather` |
| Shape | 128³, `R8Unorm` (Perlin–Worley), tiles every 3.33 km (`SHAPE_REPEAT` 0.0003/m) | takram's `shape.bin` as PNG (128 × 16384) | `assets/shape.png`, `textures.rs::shape` |
| Detail | 128³, `Rgba8Unorm`, 6 mips (128³ to 4³), tiles every 100 m (0.78 m texels) | **in-house**, generated on the GPU at startup (49 ms, 9.6 MB) | `shaders/detail_noise.wgsl`, `src/detail_noise.rs` |
| Turbulence | 128², `Rgba8Unorm`, 20 repeats per weather tile (5 km) | generated at startup, matches takram's bit for bit | `shaders/noise.wgsl::turbulence_main` |
| Blue noise | 128², `R8` | **in-house** void-and-cluster | `assets/blue_noise.png`, `examples/blue_noise.rs` |

takram's shape and weather generators hash value noise with `sin`, and `sin` gives different results on different
GPUs, so the port embeds the shipped textures rather than regenerating them (`examples/noise_compare.rs` shows the
difference). `noise.wgsl` still contains the ported generators, but at runtime only the turbulence is generated.

### Render targets (`layer.rs::ScreenTargets`, `ShadowTargets`)

| Target | Resolution | Format | Contents |
| --- | --- | --- | --- |
| March colour | ¼ × ¼ of the screen | `Rgba16Float` | premultiplied radiance and opacity |
| March depth and velocity | ¼ × ¼ | `Rgba32Float` | front depth, screen motion, opaque scene view-z |
| March shaft length | ¼ × ¼ | `Rgba16Float` | shadow length (km) |
| History colour ×2 | full | `Rgba16Float` | each pixel's running mean (ping-pong) |
| History state ×2 | full | `Rgba16Float` | r shaft length (km), g sample count, b view depth of the opaque surface seen (km, 0 for sky), a prior weight |
| BSM current, motion, history ×2 | `map_size`², one array layer per cascade | `Rgba16Float` | r front depth (km), g mean extinction, b max optical depth, a its tail |
| Ground shadow | `ground_size`² over 40 km | `Rgba8Unorm` | r = exp(−optical depth) toward the key light |

### Frame order (`game_sky.rs::GameSky::prepare` → `update_timed`, then `layer.rs`)

1. One-time fills of the turbulence and detail noise (`encode_noise_fill`).
2. Cut detection (`update_timed`): the camera moving more than 250 m or turning more than 0.5 rad in a frame, a zoom
   change over 20 %, the sun moving 2°, or coverage changing by 0.05. A cut drops every history.
3. Weather (`weather.rs::Weather::update`): lightning scheduler, rain boxes, splashes. The strike is handed to the sky
   (`set_lightning`).
4. Sky tables (`Sky::update_tables`).
5. Clouds CPU half (`CloudLayer::prepare`): choose the key light (sun or moon), fit the cascades, write the uniforms,
   pick this update's Bayer cell, plan which BSM cascade and which ground-map texels to march.
6. Environment bake (`Sky::update_environment`), which calls the hook `clouds_bake` (`shaders/hooks.wgsl`).
7. Shadows (`encode_shadows`): BSM march for **one cascade per update** (round robin, `shadow_pass.wgsl`), then the BSM
   temporal pass (`shadow_resolve.wgsl`), then a slice of the ground map (`ground.wgsl`, 16,384 texels per update,
   so a 1024² map is fully refreshed about every 64 frames).
8. Aerial-perspective volume (`aerial.rs`, `shaders/aerial.wgsl`), which uses the ground shadow.
9. At the renderer's opaque copy point, once the opaque depth exists: the **march** (`march_pass.wgsl::march_main`,
   8×8 workgroups over the quarter-resolution buffer, one pixel per 4×4 block) and then the **resolve**
   (`resolve.wgsl`, full resolution). On the first update after a cut, and only when time is standing still (a
   capture, a paused frame), `march_full_main` marches every pixel straight into the history and the resolve is
   skipped.
10. `Phase::Sky`: the dome (which reads the shaft length through `hooks.wgsl::clouds_shadow_length`), then the
    composite (`composite.wgsl`), a full-screen triangle with no depth test that blends
    `scene × (1 − a) + rgb`.
11. `Phase::Transparent`: splashes, bolt, rain (`splashes.wgsl`, `bolt.wgsl`, `rain.wgsl`).
12. After the scene: heavy rain's veil (`post.rs`, `shaders/haze.wgsl`; this is the rain veil, not the aerial haze).

The march reaches each pixel once every 16 updates in Bayer order (`layer.rs::BAYER_INDICES`
`[0,8,2,10,12,4,14,6,3,11,1,9,15,7,13,5]`). A held or paused picture keeps marching for `HOLD_UPDATES` = 240 updates
(15 full cycles, 16 samples per pixel) and then stops, which makes captures deterministic.

### The primary march (`march.wgsl::march_clouds`, entry `march_pass.wgsl::march_pixel`)

- **Ray setup**: the direction comes from the lens and the view rotation only, not from a near-plane point minus the
  camera. f32 steps in the camera position (2.4e-4 m at 2.6 km altitude) would otherwise read as motion. The near and
  far limits are the intersections with the layer spheres (`clouds_near_far`: below, inside or above the layers),
  clipped by the opaque scene's depth and by the sea sphere. A ray that starts under water (from the ocean's probe)
  marches nothing.
- **Steps**: the first step is `min_step + (1.01 − 1) × near`, and every step grows ×1.01 (`perspective_step_scale`).
  Steep rays are capped so they take at least `SHELL_SAMPLES` = 16 samples through the shadow-casting layers' depth.
  The start is jittered by `2 × jitter × step` (takram: "spatial aliasing remains unless the jitter is doubled").
- **Empty space**: in gaps between layers (`inside_layer_intervals`) and where the weather sample is below
  `min_density`, the step is `mix(step, max_step 1000 m, min(1, mip))`. In the fog/rain band that starts below the
  sea, empty air is crossed in strides of 6 % of the distance, and the band is skipped beyond 12 km
  (`SURFACE_STRIDE`, `SURFACE_REACH`).
- **In cloud**: sample the full medium (`field.wgsl::sample_media_at`) and light it (`sample_radiance`). Integrate
  with Frostbite's energy-conserving form: `S_int = (S − S·T)/σt`, `L += T_acc · S_int`, `T_acc *= T`.
- **Adaptive steps near the camera**: a cloud within 800 m uses steps of at most `NEAR_STEP` = 20 m, growing back to
  normal by 1.5 km. In slabs (decks, fog) within 600 m, each step covers at most 0.35 optical depth (`FINE_DEPTH`) and
  is at least 8 m. Thin parts of a slab may take up to 2× the step, and rain up to 4×.
- **Early-out**: when transmittance falls to `min_transmittance` (1e-2, or 1e-1 on Low). The output opacity is
  `remap(T, 1, min_T)`.
- **Front depth**: the transmittance-weighted mean distance (Frostbite 5.9.1). It is used for aerial perspective,
  motion vectors, and the end of the shaft ray.
- **After the march**: aerial perspective from the shared froxel volume at the front depth
  (`aerial_along(uv, front_depth)`), and motion from the front's world position minus this update's wind drift
  (`drift_step`).

---

## 2. Density model

### Layers (`shared/sim-clouds/layers.rs`)

There are four layer slots, packed into `vec4`s so the shader evaluates all four at once. Each `Layer` has:
`channels` (weights on the weather texture's RGBA), `altitude`, `height`, `density_scale`, `shape_amount`,
`shape_detail_amount`, `weather_exponent`, `shape_altering_bias`, `coverage_filter_width`, a `DensityProfile`
(`exp_term·e^(exponent·h) + linear·h + constant`, takram's default being `0.75h + 0.25`), `coverage`, `shadow` (0–1)
and `slab` (0–1). Compared with takram, the channel mix, the per-layer coverage and the fractional `shadow`/`slab`
are additions. They exist so that two weathers can be interpolated parameter by parameter (`Layers::lerp`). `pack`
also computes the up to three empty gaps between layers, which the march strides over.

takram's default layers (`Layers::takram`), used for fair weather:

| Slot | Height | Channel | density_scale | Notes |
| --- | --- | --- | --- | --- |
| 0 | 750–1400 m | R | 0.2 | casts shadows |
| 1 | 1000–2200 m | G | 0.2 | casts shadows |
| 2 | 7500–8000 m | B | 0.003 | shape 0.4, no detail, filter 0.5 |
| 3 | empty | A | | |

Default values: bias 0.35, filter width 0.6, shape and detail amount 1. The medium uses scattering 1.0 and absorption
0 (`layer.rs::SCATTERING_COEFFICIENT`), so extinction (per metre) equals the summed layer density.

### Weather sample (`field.wgsl::sample_weather`)

```
h        = remap_clamped(height, layer_min, layer_max)            // per layer
local    = pow(texel · channels, weather_exponent)
shape_h  = 1 − (clamp(2·h^bias − 1, −1, 1))²                       // rounds tops; 0 at base and top
factor   = 1 − coverage · shape_h
density  = remap_clamped(mix(local, 1, filter), factor, factor + filter)
if shadow pass: density *= shadow_layer_mask                        // after coverage (takram masks before)
```

The weather texture's mip comes from the pixel footprint at the ray start (`weather_mip_level`, scaled by
`MIP_LEVEL_SCALE` 0.25).

### Medium (`field.wgsl::sample_media_at`)

1. **Turbulence** (High and Ultra only): the position is displaced by
   `350 m × (turb·2 − 1) × dot(density, remap(h, 0.3, 0))`, that is, only near a layer's base (`TURBULENCE_DISPLACEMENT`).
2. **Shape**: `density = remap_clamped(density, (1 − shape) · shape_amount, 1)`, where the shape is read at
   `(pos + evolution + turbulence) × 0.0003`. The evolution shifts the shape vertically by
   `|wind drift| / 100 km × 2e4 m`, so clouds change shape as the weather moves.
3. **Detail** (not on Low): applied only when `mip·0.5 + (jitter − 0.5)·0.5 < 0.5`, which is a stochastic LOD
   cutoff. `carve_detail`: `remap_clamped(density·2, erosion·0.5, 1)`. The erosion mixes wisps (r→g by depth
   `density·0.5`) at a layer's bottom with billows (b→a by `density·0.6`) at its top (`billowy = remap(h, 0.2, 0.4)`).
   It is shaped to takram's statistics: `wisps = 0.17 + (bumps − 0.35)·0.47` and
   `billows = 0.29 + (0.35 − bumps)·0.47`.
4. **Profile**: `density = saturate(density · density_scale · profile(h))`. The four layers are summed, and
   `media.weight` records each layer's share, which the lighting uses.

**Detail mip (`detail_lod`)**: texels no finer than ⅛ of the march step and no finer than ½ pixel, **and never finer
than mip 3** (16³ texels of 6.25 m, 16 KB). Reading mip 2 (131 KB) cost the march 50 % more at sea level because of
texture-cache misses, and mips 0–1 at Ultra's 10 m steps cost +0.7 to +3.5 ms. Light marches always use mip 3, so
the lighting does not shift as the camera moves.

### Detail noise (in-house, `shaders/detail_noise.wgsl`)

Each channel is three octaves of "alligator" noise (cells ×2, weight ×½, normalised by 1.75). In each lattice cell
one bump of random height (0.5–1) sits at a random place, with a smoothstep falloff of radius one cell. The noise
value is *tallest bump − second tallest*, which gives rounded bumps with creases between them. The lattice is hashed
with integers (Wellons' lowbias32), so every GPU generates the same field. `reference_texel` is a CPU copy that the
test `tests/detail_noise.rs` compares against to within 1/255.

| Channel | Cells | Lookup | Scale |
| --- | --- | --- | --- |
| r (low wisps) | 4 | curl-displaced: `curl(3)·0.045 + curl(7)·0.018` | ×1.55 |
| g (high wisps) | 12 | `curl(5)·0.02 + curl(13)·0.008` | ×1.75 |
| b (low billows) | 5 | plain | ×2.1 |
| a (high billows) | 14 | plain | ×1.75 |

The curl comes from three periodic value noises with quintic fade. Mips are 2×2×2 box means. Wisps carve away the
bumps, leaving a web of creases. Billows carve away the creases, leaving lobes. Guerrilla's Nubis³ noise pack carries
no licence and was not used.

### Weather presets → layers (`shared/sim-clouds/weathers.rs::layers_for`)

When no genus is named, coverage alone decides. Fair weather is takram's layers. The fair layers blend into a deck
by `smoothstep(0.66, 0.8, c)` and into a storm by `smoothstep(0.87, 0.94, c)`. Overcast uses 0.82 and storm 0.94.

| Slot | Fair | Deck (overcast) | Storm | Fog (`fog > 0`) |
| --- | --- | --- | --- | --- |
| 0 | cumulus 750–1400 m | stratocumulus `deck_layer` 800 m, 700 m deep, coverage 0.55, filter 0.35, density 0.1, channels B 0.35 + A 0.65, slab | nimbostratus 250–2250 m, density 0.12, bias 1.0, profile `0.4h+0.6`, slab | broken stratus 600–950 m blended in by `fog` |
| 1 | cumulus 1–2.2 km | altostratus 2 km above the deck, 600 m, density 0.008, slab | cumulus heads base+1 km, 4.5 km deep, density 0.12, coverage 0.3 | emptied |
| 2 | high sheet 7.5–8 km | as fair | 8.5 km, 800 m, density 0.004, coverage 0.85 | as weather |
| 3 | empty | empty | rain shafts if precipitation > 0.3 and the tier allows them | fog banks |

- **Rain shafts** (`rain`): from −100 m up to the lowest cloud base, channels B/A 0.5 each, no shape or detail,
  density `0.003·p²`, coverage `0.22 + 0.08p`. High and Ultra only (about 0.5 ms on Medium).
- **Fog banks** (`fog_banks`): from −50 m, 300 m deep, with the bias chosen so the shape function peaks at the sea
  surface (`bias = ln 0.5 / ln(50/300)`), profile `1 − 0.6h`, density 0.02, coverage `0.3 + 0.06·amount`, slab.
- **Genera** (`genus_layers`): cumulus (1.1 km base, 1.9 km deep), cumulonimbus (towers 800 m–10.8 km, density 0.12),
  stratocumulus, stratus, nimbostratus, altocumulus (3.5 km), altostratus (4 km). Each has a default base and depth
  that the scene can override. **High sheets** (`high_sheet`) replace slot 2: cirrus (takram's), cirrostratus (700 m
  deep, density 0.002, shape 0.15) and cirrocumulus (channel A, 300 m, density 0.006). This snapshot has no separate
  cumulus "heap" shape model. Cumulus are takram's two layers. A later attempt at per-column bases, tops and anvils
  (PR #987) was reverted the same day because it looked worse and marched slower.
- `clouds/src/weathers.rs` adapts the sky scene to `CloudWeather`. `CloudLayer::blend` interpolates two weathers.

**Why decks are separate layers**: at overcast coverage, takram's layers saturate. Over the weather texture, takram's
low layer at 0.82 coverage is saturated over 27 % of the sky and clear nowhere, so it looks like a flat grey sheet
from below. The deck at 0.55 with a 0.35 filter is saturated over 10 % of the sky, clear over 3 %, and has structure
everywhere else.

---

## 3. Lighting

All of this is in `march.wgsl`. `sample_radiance` lights one sample, and `ray_light` holds the per-ray constants,
because the angle to the sun is the same along a whole ray.

**Sun and sky irradiance.** These come from the sky's Bruneton tables: `sun_sky_scalar_irradiance(point_km)`. When
"accurate sun/sky light" is on (High and Ultra), they are evaluated per sample. On Low and Medium they are
interpolated between the bottom and top of the layers, sampled once per ray (`frame_light`,
`clouds_sun_sky_irradiance`). After dark the **key light** becomes the moon (`src/key_light.rs`). The switch happens
when the moon's direct light at the top of the clouds outshines the sun's. For a full moon and an 8 km cloud top
that is 2.5° after sunset, where the two beams are equal (0.217 against 0.212), so the change is continuous. The
moon's light goes through the same tables (`key_scalar_irradiance`). Light shafts are off under the moon.

**Phase.** takram's dual Henyey–Greenstein uses g = 0.7 and −0.2, mixed 50/50 (`SCATTER_ANISOTROPY`).

**Broken cloud** (cumulus, storm towers, high sheets) uses the game's model, not takram's (`sun_scattering`):

```
first  = forward_lobe · mix(0.5, 1, body) + body · (back_lobe + max(0.7·HG(0.8) − forward_lobe, 0))
later  = Σ_{i=1..2} 0.8^i · phase(g·0.6^i) · exp(−τ_sun · 0.7^i)          // 3 octaves total
later *= mix(1, edge_ramp(opacity), 1 − cos⁸θ⁺)                             // edge term, none toward the sun
S_sun  = E_sun · (first · exp(−τ_sun) + body · later)
```

- Three octaves on every tier (`OCTAVE_CONTRIBUTION` 0.8, `OCTAVE_ATTENUATION` 0.7, `OCTAVE_PHASE_ATTENUATION`
  0.6). With takram's eight octaves at ½ each, cloud tops looked evenly white and a cloud against a low sun looked
  dull. Two octaves came out 4–7 % darker and cost the same.
- A silver forward lobe on the first octave (Guerrilla 2017: `max(lobe, 0.7·HG(0.8))`). Within about 28° of the
  sun, thin edges get up to 3.3× takram's forward light.
- An **edge term in place of powder**: later octaves ramp from 0 at the ray's entry into cloud to full at 0.8 of
  accumulated opacity (`EDGE_DEPTH`). It is integrated over each sample's stretch of opacity (`edge_ramp`) so that it
  does not depend on step length. A powder ramp on local density darkens the whole cloud evenly, because the shell
  the eye sees is thin everywhere.
- **Thin layers** (`layer_bodies`: optical depth at the thickest point / 8, clamped to 0–1) scatter once. They lose
  the back lobe, the silver lobe and the later octaves in proportion, and keep half the forward lobe
  (`VEIL_FORWARD`), standing in for the diffraction peak of ice crystals and rain drops.

**Slabs** (decks, storm deck, fog) keep takram's eight octaves (`later_slab_octaves`: ½ⁱ contribution, ½ⁱ optical
depth, ½ⁱ anisotropy) and takram's **powder**, `1 − 0.8·exp(−σt·150)` (`POWDER_SCALE`, `POWDER_EXPONENT`). The deck
light was derived against these, and without powder a deck seen from above is featureless.

**Optical depth to the sun** (`sun_optical_depth`): a short march of `max_iteration_count_to_sun` steps (1 on Low, 2
otherwise), starting at 100 m and doubling each step (`march_optical_depth`; zero iterations returns takram's fudge of
0.5). Beyond that, the BSM: `min(maxOD + tail, meanExt · distance_to_front)` (`read_shadow_optical_depth`). PCF with
a Vogel disk (8 taps, 6 texels) is used only when the sun is near the horizon. Cascades fade into each other over a
margin chosen by the jitter (`faded_cascade_index`). Under a closed deck deeper than 60 optical depths, the sun is not
marched at all (`SPENT_DEPTH`).

**Sky light** (`sky_light`): takram's gradient (½ at a layer's base to 1 at its top) × `sky_occlusion`, where
`sky_occlusion = mix(0.4, 1, sqrt(1 − profile))` (Guerrilla 2022). The profile is the weather density before
shape and detail carving, so hollows receive less sky light than billows. Under or inside a closed slab this is
replaced by **two-stream diffusion** (`deck_skylight`):

```
τ        = column_above(uv, height)   // weather alone, 2 heights per layer, × SHAPE_FILL 0.5
diffused = 1 / (1 + 0.1125·τ)         // 0.1125 = ¾(1 − g), g = 0.85
E_down   = (E_sky + E_sun·μ)·diffused − E_sun·μ·exp(−τ/μ)
```

This is weighted by `column.closed`, the largest slab weight among the layers above. Fair weather skips it entirely.

**Ground and sea bounce** (`radiance_from_ground`): albedo 0.3 under `E_sky + (1 − coverage)·E_sun` at the camera,
attenuated by the cloud below the sample. That cloud is estimated from the weather alone,
`0.1 · Σ σt · depth_below` (`GROUND_FILL`), with no marched steps on any tier. This comes within one level of a
marched step in fair weather and under a deck, and 1.3 levels under a storm.

**Lightning** (`flash_radiance`): a point light at the sky's `lightning_position` with irradiance
`I·(1 km/r)²` (r ≥ 60 m), attenuated through the coarse weather extinction over 3 jittered samples, scattered by
takram's octaves, with a floor of slab diffusion `1/(4π(1 + 0.1125τ))`. The tint is (0.8, 0.87, 1.0). While a flash
is lit, the resolve clips history as for a moving view so that the flash appears and disappears in sync with the
bolt.

**Shadows onto the scene** (`shaders/ground.wgsl`, consumer API `shaders/shadow.wgsl`): the map covers 40 km in the
clouds' drifting frame and wraps, each texel standing for its repeat nearest the camera. Each texel marches the
shadow-casting layers toward the key light in 12/16/24/32 even steps (Low to Ultra) and stores `exp(−τ)`. Consumers
call `cloud_shadow` (a 4-tap cubic B-spline, because the 40 m texels would otherwise show as a grid) or
`cloud_shadow_fast` (1 tap). The shadow fades in as the sun rises from 1° to 7° (`SHADOW_SUN`), fades out over the
outer 15 % of the map, and goes to 0 under the thickest deck. **Multiply it into the direct sun or moon only.** The
light under a cloud is the sky's, which the environment probe already provides. The ocean does exactly this
(`ocean/shaders/surface.wgsl`: `water_sun_shadow(...) * cloud_shadow(...)`).

**Environment bake** (`hooks.wgsl::clouds_bake`): the bake runs its own march (12/16/24/32 steps by tier) with 6 even
sun steps up to the top of the shadow-casting layers (`BAKE_SUN_STEPS`). The screen BSM only covers the camera's
frustum, and without these sun steps the baked clouds were nearly unshaded. A slab step is lit at its mean
scattering depth (`light_depth`) rather than its midpoint. When lit at the midpoint, an overcast night's sea came out
brighter than a clear night's (sea 9.7 → 5.8, against 5.7 for a 16×-finer reference).

---

## 4. Temporal reprojection, noise, tiers, cost

### Blue noise (in-house)

takram ships NVIDIA's STBN, which is licensed for non-commercial use only, so it is **not used**.
`examples/blue_noise.rs` generates a 128² mask with Ulichney's void-and-cluster method (Gaussian σ 1.9 on the torus,
seeded, each of the 256 levels holding exactly 64 texels). The march reads it **at the full-resolution pixel**, not
at the march texel, and advances by the golden ratio once per march of that pixel:
`fract(level + sample·0.618…)` (`field.wgsl::march_jitter`). Reading one 2D pattern at the march texel would give all
16 cells of a block the same value. Advancing it every update over a 64-frame cycle (`stbn`, still used for shadow
jitter) gave each pixel only 4 jitters, all within a third of [0, 1), and the grain never averaged out.

### Reconstruction (`resolve.wgsl`)

takram replaces each pixel with its newest sample. This port keeps a **running mean per pixel**:

- Up to `MOST_SAMPLES` = 16 samples where the view has not moved the pixel, and `MOST_MOVED_SAMPLES` = 8 where it has.
  A moved history is resampled every update, and a longer span only collects blur.
- **Motion**: the mean of the motion of the 3×3 march neighbours, weighted by a quadratic B-spline of 1.5 march
  texels. takram's nearest-fragment motion steps from one 4×4 block to the next and shears long histories. The march
  motion includes the wind's drift since the last update.
- **History read**: its own texel when motion < 0.01 px, bilinear below about 0.3 px, blending to Catmull–Rom by
  0.7 px.
- **Clipping**: variance clipping (Salvi; Playdead's AABB clip) to the spline-weighted moments of the march, on the
  radiance only (opacity follows). Gamma is 8 for a still pixel (`STILL_GAMMA`) and 2 once it moves 1 px. A history
  more than 1.5 half-widths outside the box keeps proportionally fewer samples, and none beyond 2.5.
- **Prior**: a pixel with few samples of its own takes the neighbourhood mean at 0.25 sample-weight per update, up to
  2 samples' worth, fading out by 4 own samples (`PRIOR_*`). This is what stops the clouds boiling when the view
  turns.
- **Silhouette**: where the 3×3 neighbourhood straddles an opaque edge, only neighbours that saw the same surface
  count (view depths within 20 %). A history that saw a different surface restarts. Without this, masts left
  cloud-free halos.
- **Sliding view**: where the march neighbourhood moves 0.25–1.5 px per update relative to the view's own turn (the
  parallax of near cloud while the camera travels), the pixel blends 25 % of a Gaussian (1.25 texels) of the current
  march each update and keeps at most 2 samples.

**Composite spatial filter** (`composite.wgsl`): a pixel holding fewer than 8 samples is drawn through a Gaussian
over same-surface neighbours. The deviation is 2.5 px at one sample and shrinks as `1/√n`, over a 5×5 window, with
neighbours weighted by their own sample count. Prior-only pixels count as 0.25 of a sample. The history itself is
never filtered. The composite also clamps cloud opacity to the haze opacity where the opaque scene is nearer than the
lowest cloud base, and draws nothing where the ray starts under water.

**BSM temporal pass** (`shadow_resolve.wgsl`): blends the fresh march in at `TEMPORAL_ALPHA` 0.01 with variance gamma
1. Only one cascade is marched per update. The other cascades' history carries over unchanged, and each cascade is
sampled with the matrix it was marched with (`drawn_shadow_matrices`). A cascade is re-marched immediately after a
cut, or when the view has left it (`cascades.rs::Cascade::escapes`). Cascades use practical splits with λ 0.6, and
the light camera stands back `1e6 + (1e3 − 1e6)·sin(elevation)` m (`light_distance`). The BSM march itself uses
volsample's structured sampling: planes chosen by icosahedral normals and fixed in a world-anchored frame.

### Quality tiers (`src/quality.rs::tier`)

These are takram's presets with three trims (marked †). The march always runs at ¼ × ¼ resolution with a 16-update
reconstruction.

| | Low | Medium (default) | High | Ultra |
| --- | --- | --- | --- | --- |
| Shafts / shape detail / turbulence | no / no / no | no / yes / no | yes / yes / yes | yes / yes / yes |
| Sun and sky light | interpolated | interpolated | per sample | per sample |
| Primary march | 200 it., min 100 m, 100 km, min T 0.1 | 500 it., 50 m, 200 km, min T 0.01 | as Medium | 500 it., **10 m**, 200 km |
| Max step, step growth | 1000 m, ×1.01 | same | same | same |
| min density / extinction | 1e-4 | 1e-4 | 1e-5 | 1e-5 |
| Sun steps (from 100 m, ×2) | 1 | 2 | 2 | 2 |
| Ground steps | 0† | 0† | 0† | 0† |
| Octaves (broken cloud) | 3 | 3 | 3 | 3 |
| Shafts march | | | 500 it. from **200 m**† to 200 km | same |
| BSM | 2 × 256², 25 it. | 3 × 256², 50 it. | 3 × 512², 50 it. | 3 × **768²**†, 50 it. |
| BSM step | 100–1000 m | same | same | same |
| Ground map, steps | 512², 12 | 1024², 16 | 1024², 24 | 1024², 32 |
| Env bake steps | 12 | 16 | 24 | 32 |
| Rain drops, shafts | 3000, no | 8000, no | 12000, yes | 20000, yes |

takram's thin haze (`approximate_haze`) is off on every tier. The aerial-perspective volume replaces it.

### Measured cost

All on an Apple M5 Pro at 2400 × 1350, in ms per frame (`examples/timings.rs`; details in `docs/clouds.md` "GPU
cost").

| Tier | Clouds total, noon / sunset / overcast / 2.6 km / dive (2026-10-02) | Full march on a cut frame |
| --- | --- | --- |
| Low | 1.16 / 1.31 / 0.82 / 1.34 / 1.08 | 2.4–7.3 |
| Medium | 1.63 / 1.97 / 1.02 / 1.97 / 1.39 | 5.4–13.8 |
| High | 2.61 / 3.50 / 1.59 / 3.14 / 2.24 | 7.2–18.7 |
| Ultra | 3.59 / 5.26 / 2.23 / 4.90 / 3.40 | 10.9–30.6 |

- The march dominates. From High upward the BSM comes next, rising to about 45 % of an Ultra sunset. The resolve is
  about 0.8 ms on every tier after the 2026-10-03 rewrite (a single 3×3 read).
- The game's weathers (decks, storm, fog) cost about as much as fair cumulus seen from the air: Medium march 1.3–2.3
  ms, Ultra storm with rain shafts 3.9–5.6 ms. takram's decks were cheap only because rays stopped at an opaque base.
- Moving the BSM to one cascade per update cut High from 4.1 to 3.4 ms and Ultra from 8.1 to 4.5 ms. The three trims
  cut High 3.3 → 2.6 ms and Ultra 5.3 → 3.9 ms on average.
- The near-camera fixes cost 1.02 → 1.26 ms among cumulus, plus 0.05–0.15 ms in the composite while pixels are young.

---

## 5. Couplings with the other systems

| Coupling | Direction | Where |
| --- | --- | --- |
| Atmosphere tables (transmittance, irradiance, scattering) and `sky_radiance_to_point` | sky → clouds | `#import naval_sky::bruneton`, `naval_sky::atmosphere` in `march.wgsl` |
| Hooks into the dome and environment bake: `clouds_shadow_length(pixel)`, `clouds_bake(origin, dir)` | clouds → sky | `hooks.wgsl`. `Sky::attach_clouds` recomposes the dome and bake with the `CLOUDS` shader def and binds the clouds at group 2 (`docs/sky.md` "Clouds and weather in this crate") |
| Wind drift (`wind_offset`), lightning position and intensity, moon irradiance | sky uniforms | `Sky::set_lightning`, `SkyUniforms` |
| Aerial perspective at the clouds' front | shared volume → clouds | `aerial_along(uv, front_depth)` in `march_pass.wgsl`. Volume: 32×32 columns × 32 slices from 20 m to 450 km, one `Rgba16Float` 32×32×64 3D texture (in-scatter, then opacity). Filled by `clouds/src/aerial.rs` and `shaders/aerial.wgsl` using the cloud ground shadow. Model in `docs/haze.md` |
| Ocean underwater probe (each pixel's near-plane depth below the waves) | ocean → clouds | `CloudLayer::set_underwater_probe`, `common.wgsl::ray_starts_under_water`, read by the march and the composite |
| Cloud shadow multiplied into the sun on sea, ships and land | clouds → everyone | `CloudLayer::shadow()` → `CloudShadow`. WGSL module `naval_clouds::shadow` (`shadow.wgsl`), 64-byte uniform |
| Sea heights for rain splashes | ocean → clouds | `Weather::set_sea_heights` (`naval_ocean::rain_heights`) |
| Cloud cover in front of transparents (smoke fades behind cloud) | clouds → effects | `CloudLayer::cover`, `naval_sky::CloudCover` |
| Opaque depth (rays stop at hulls) | renderer → clouds | `SceneCopies` opaque depth, `after_copies(Opaque)` |

---

## 6. Changes relative to takram's original, and why

| Change | Why |
| --- | --- |
| World-fixed planar texture coordinates (east, height, north) drifted by the wind, on a curved planet frame centred under the camera | Flat-sea game world. Clouds stay put as the camera moves |
| Shape and weather embedded as PNGs, not regenerated | takram's `sin` hash gives different textures on different GPUs |
| In-house 128³ RGBA alligator detail noise, mip ≥ 3, replacing takram's 32³ `shape_detail.bin` | Licence-free, finer structure available. The mip floor keeps the texture in cache and keeps takram's look and cost |
| In-house blue noise, golden ratio per march of a pixel, read at full resolution | STBN is non-commercial. The earlier stepping never converged |
| Per-layer coverage, channel mix, `shadow`/`slab` weights; shadow mask applied after coverage | Weathers blend continuously. Non-shadowing layers no longer shade at high coverage |
| Weather layers: decks, storms, fog banks, rain shafts, genera, high sheets | takram's overcast is a flat saturated sheet |
| Deck light (`deck_skylight`, `column_above`) | takram lights every point with the whole clear sky, so a deck's underside was uniformly lit |
| Broken-cloud light: 3 octaves, silver lobe, edge ramp instead of powder, sky occlusion, single scatter for thin layers | Flat white tops, dull clouds against a low sun. Relief from 2.6 km at noon went 9.63 → 10.18, and the 99th percentile toward a dusk sun went 200.7 → 225.4 |
| Ground bounce estimated from weather, no marched steps | Same picture within one level, cheaper. Every tier now matches |
| Moon as key light, lightning as a point light in the march | takram is sunlit only |
| Reconstruction rewritten (running mean, spline motion, drift in motion, prior, silhouette, sliding view, composite filter) | takram's sample replacement left visible grain, block edges and halos round masts |
| Full march on still frames after a cut; convergence over 240 updates | Deterministic captures with no 4×4 mosaic |
| Steep-ray minimum of 16 samples, 20 m near steps, slab steps capped at 0.35 optical depth, band strides | Grain from the chart view, near cumulus and fog |
| One BSM cascade per update, with drawn matrices and an escape test | Saves about a third of the shadow cost with no visible change (captures match to 0.03 of a level) |
| Trims: shafts from 200 m steps, no ground step, Ultra BSM at 768² | Not visible side by side, about 1 ms saved |
| takram's haze and aerial pass replaced by the shared froxel volume; shafts only in the dome | One haze model for every material |
| Ground shadow map instead of a deferred BSM read | The game's scene is forward lit |
| No `CLOUD_GAIN` (previously 1.9); clouds use the dome's grade gain | One radiance scale for sky, clouds and world |

---

## 7. Lessons and pitfalls (from `docs/clouds.md`)

**How to measure.** "Grain" is the mean absolute difference between adjacent pixels' luma, in 8-bit levels, over the
cloud region. The reference ("truth") is either the same frame marched at 32 golden-ratio jitters and averaged, or
the same pose held from a cut for 300 frames with time nearly frozen (`dt` 1e-6). Use `wind: 0`, because drift alone
moves a near cloud between the frame and its truth. Take the truth at the frame's own instant: a truth taken one frame
late made a flight's kept detail read 0.42 → 0.29. One truth still carries 0.4–0.65 levels of noise, so average four
truths. Check crops by eye as well, because the near-cloud "snow" covers only a few percent of a frame.

- **Grain (2026-10-03).** Noon chase 2.16 → 0.99, dusk 1.29 → 0.45, binoculars ×24 1.93 → 0.31, over a deck 2.13 →
  0.59. The 32-jitter truth is 0.29–0.93, and the results now sit on it. The causes were the jitter sequence (§4) and
  takram keeping roughly one sample per pixel.
- **f32 precision.** Build rays from the lens and rotation, and motion from the camera's world position. Planet-frame
  heights (half-metre f32 steps at the planet radius) moved a still view's history a third of a pixel per update.
- **Drift in motion vectors.** Binoculars magnify a 12 m/s wind 20 km away to a fifth of a pixel per update. Without
  drift in the motion, error against the truth was 3.65 levels; with it, 2.27.
- **Pan grain (2026-10-04).** The march reaches each pixel once in 16 updates. A second into a 45°/s turn, 13 % of
  pixels had no sample of their own. The prior brought frame-to-frame change down from 0.81 to 0.64 (45°/s) and from
  1.19 to 0.76 (90°/s). What a turn still loses is detail: frames hold 0.35–0.39 of the converged fine detail.
- **Near-camera grain (2026-10-05).** Flying 100 m/s past a cumulus 300 m away, the march's own noise, temporal AA,
  history rejection and motion noise were all ruled out. The cause is parallax inside a volume: no single motion
  vector carries the history. The fix is three changes together (20 m steps, the sliding view, the wider composite
  filter): error 2.97 → 1.87, grain 1.62 → 1.13. Steps of 8 m and 12.5 m measured the same as 20 m.
- **Paused field (retired 2026-10-08).** A sharper "near field" drawn only in paused or held pictures turned a cumulus
  into a mottled web of dark creases the moment the game paused. The paused frame differed from the previous frame by
  14 levels on average; after removal, by under 1. **A paused picture must converge to what play drew.**
  `tests/gpu.rs::a_paused_picture_from_the_air_is_plays` holds this.
- **Texture cache.** See the detail mip floor in §2. The 100 m detail tile exists so that mip 3 has about takram's
  texel size.
- **Aircraft views.** Even a converged flight keeps only 0.34–0.61 of the fine detail: near and far cloud move
  differently along one ray, and the history is a single picture resampled every update. Halving the step size cuts
  the converged noise by a third from above, at roughly double the march cost. Not taken.
- **A still camera in play is softer than converged** (0.65 of the fine detail from 2.6 km with wind, 0.90 without),
  because drift resamples the history every update. Keeping whole-pixel history with sub-pixel remainders kept more
  detail (0.74) but showed raw noise (1.07 → 1.88).
- **Accelerated time.** Clouds at 8× time scale held only 0.66 of the converged edge contrast. The theatre now keeps
  the sky at 1× (`GameSky::time_scale`).
- **Bake bias.** Long bake steps lit at their midpoint made overcast scenes too bright (overcast night sky mean
  0.0232 → 0.0138, reference 0.0142). `tests/gpu.rs::an_overcast_nights_ambient_is_the_same_on_every_tier` keeps
  Medium and Ultra within 15 %.
- **Lightning in a ¼-resolution march.** Use gamma 2 clipping while a flash is lit, or the still-pixel gamma of 8
  keeps the history dark through the flash.
- **Things that measured as no improvement**: Catmull–Rom history under slow drift (it brings in other pixels' noise
  and shows a 4×4 lattice), a 2-sample history while clouds drift, a 7×7 composite filter (+0.2 ms), five octaves,
  takram's interpolated light (no saving), and stepping back to find a cloud's edge (crisper, +0.6 ms).
- **Open problems**: the inside of a deck is flat grey (its light is the same at every sample, so it needs a light
  volume, not more density detail); there is no billow-to-billow self-shadow beyond about 150 m; sky light has no
  direction; the sea bounce is a guess; rain shafts on the horizon look like land; the weather texture repeats every
  100 km when seen from the chart; only one key light (between 1.4° and 2.5° below the horizon the low clouds receive
  neither beam).

---

## 8. Reading order

1. `docs/clouds.md`: read "The light of broken cloud", "Weather", "Near detail", "Quality tiers", "Where it departs
   from takram" and "Open problems". Skip the rest at first.
2. `shared/sim-clouds/layers.rs`: the layer parameters, takram's defaults, packing and interval gaps.
3. `shared/sim-clouds/weathers.rs`: how coverage, genus, fog and rain become four layers.
4. `clouds/src/quality.rs`: every tier's step counts and switches.
5. `clouds/shaders/field.wgsl`: weather, shape, turbulence, detail carving, profile, `column_above`.
6. `clouds/shaders/march.wgsl`: phase, octaves, deck light, bounce, BSM reads, the primary march, shafts.
7. `clouds/shaders/march_pass.wgsl`: per-pixel ray setup, scene depth, aerial perspective, motion.
8. `clouds/shaders/resolve.wgsl`: the temporal reconstruction (its header comment is the design).
9. `clouds/shaders/composite.wgsl`: the spatial filter and blending over the scene.
10. `clouds/shaders/shadow_pass.wgsl`, `shadow_resolve.wgsl`, `clouds/src/cascades.rs`: the Beer shadow maps.
11. `clouds/shaders/ground.wgsl`, `shadow.wgsl`, `hooks.wgsl`: what other systems receive.
12. `clouds/src/layer.rs` (`prepare`, `encode_shadows`, `encode_march`) and `clouds/src/game_sky.rs`
    (`update_timed`): orchestration and frame order.

`clouds/shaders/detail_noise.wgsl` is self-contained. Read it when you need the noise.

---

## 9. What is game-specific and can be ignored

- `CloudClearing` (no cloud shadow at the port's berth) and `clear_berth` in `shadow.wgsl`.
- The simulation side of `shared/sim-clouds` (`CloudField`, sight rays with `SIGHT_DEPTH` 3.912, `Catalog`,
  `ScenarioSky`, `WorldChart`) and `GameSky`'s `authoritative_clouds`. Spotting uses these, not rendering. Keep the
  layer and weather functions.
- Theatre and battle plumbing: `time_scale`, `start_in_play`, `take_tables_from`, `blend_from`, `hold`, the battle
  clock's sun and the `weather::change` drift (in `naval-client`, not included). Keep the idea that a weather change
  blends layers without cutting the history.
- Lightning scheduling and bolt geometry (`lightning.rs`, `bolt.rs`, `bolt.wgsl`), the thunder cue, the rain veil
  (`post.rs`, `haze.wgsl`, `post.wgsl`), rain streaks and splashes. These are the game's own effects, ported from its
  web version, and are independent of the cloud renderer.
- `CloudHooks`/naga_oil recomposition, the `PipelineRegistry` hot reload, `UploadSink`, `GpuTimer` and
  `RenderContext`: engine plumbing.
- `examples/stills.rs`, `examples/timings.rs` and the shot/harness commands in `docs/clouds.md`: the game's
  measurement tools. The views and metrics they use are worth copying.
- References in the docs to the web game, `src/game/sky`, re-syncing, `CLOUD_GAIN` and PR history.
