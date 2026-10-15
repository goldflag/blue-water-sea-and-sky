# Clouds and weather lane (`native/crates/naval-clouds`)

The clouds are a port of **@takram/three-clouds** (packages/clouds of
https://github.com/takram-design-engineering/three-geospatial, MIT; notices in `naval-clouds/THIRD_PARTY.md`) to
wgpu: four cloud layers over the curved planet, takram's weather and shape textures (the detail noise is the crate's
own since 2026-10-04, "Near detail" below), a ray march lit through
the sky's atmosphere (takram's own Bruneton model, `native/docs/sky.md`), Beer shadow maps (BSM) in cascades, temporal
upscaling, light shafts and haze. They replaced the native port of the web game's clouds (2026-10-02); the web game
keeps its own clouds. The weather is still the game's, ported from `src/game/sky/weather`: near rain, splashes,
lightning (the bolt, the scene flash, thunder) and heavy rain's veil. The crate is portable
(`cargo check -p naval-clouds --target wasm32-unknown-unknown`). Outputs go to `.build/native/clouds/`.

Bill's brief for the port (2026-10-02): takram's clouds as they are (their features only: no cloud genera, storm towers,
rain shafts, lightning glowing in the clouds or moonlit clouds), the native client only, and whatever Ultra takes. Then
(2026-10-03) weather that changes the sky: the weather's own layers within takram's march (decks with structure under
them, storms with a low ragged base, towers and rain shafts, fog banks on the sea, the genus, base and depth a scene
names), weather changes that drift over minutes, and a sun that moves through a battle. See "Weather" below; fair
weather keeps takram's layers. Since 2026-10-03 (Bill's night-lighting brief, light only) the moon and lightning light
them too: "Night: the moon and lightning" below. Since 2026-10-04 (Bill: "we have planes in the air, we basically want
clouds as good as those in flight sims"; the plan's first stage) the light of broken cloud is the game's, not takram's:
"The light of broken cloud" below. The plan's sixth stage (shapes with heights and types: per-column bases and tops, a
cloud-type table, non-repeating weather maps, storm towers with anvils; PR #987) was merged and taken out the same
day: Bill found the shapes much worse than takram's and the march slower.

## The light of broken cloud (2026-10-04)

From an aircraft at noon takram's cloud tops were evenly white and flat, and toward a low sun a cloud was one dull mass
without bright rims. The causes, from the research behind the plan (`.build/cloud-research`, not in the repository):
eight octaves of multiple scattering at a half each (the film paper's defaults: the first octave, the only one with the
true shadow and phase, is half the sum, and a quarter of it lies in octaves whose optical depth is cut 8 to 128 times),
a forward lobe tuned for midday, and the whole clear sky's light on every sample. Now, for broken cloud (cumulus, a
storm's towers, the high sheets), in `shaders/march.wgsl` (`ray_light`, `sun_scattering`, `sky_light`,
`sample_radiance`):

- **Three octaves** (every tier; shipped renderers use one to three): octave i carries 0.8^i of the light through
  0.7^i of the optical depth toward the sun, by a phase whose anisotropy is cut to 0.6^i (Dagor's form; its values are
  0.7, 0.3 and 0.6, "at noon 0.5 to 0.6 and 0.45"). The later octaves light a cloud's first few optical depths; from
  about ten its core takes only the sky's light. The contribution holds a sunlit top where takram's was (its 99th
  percentile from 2.6 km at noon is 208 of 255, as it was). The attenuation is steeper than Dagor's because the tops'
  relief grows with it at the same brightness: 9.7 levels at 0.45, 10.1 at 0.6, 10.3 at 0.75, against takram's 9.6
  (relief: the RMS of luma less its 12 px blur over the clouds). A ray takes the octaves' phases once (the angle to the
  sun is one along it), so a sample costs one exponential of a vector where takram's cost eight exponentials and
  sixteen powers.
- **A narrow forward lobe on the first octave** (Guerrilla 2017's form, max(lobe, 0.7 × HG(0.8))): a cloud's thin edges
  within about 28° of the sun, where the optical depth to the sun and to the eye are both small, take up to 3.3 times
  takram's forward lobe (2.3 times at 10°, 1.4 at 20°). The first octave's full extinction keeps it to the edges.
- **An edge term in place of powder**: the later octaves are light scattered more than once, which gathers with the
  depth of cloud about a sample. They ramp from none at a view ray's entry into cloud to all from 0.8 of opacity gathered
  along the ray (Dagor keys its edge albedo the same way: a ramp on the local density, which takram's powder was, is one
  darkening everywhere, since the shell the eye sees is thin throughout), and not at all looking toward the sun (cos⁸),
  so the silver lining stays. The ramp is taken as its mean over each sample's stretch of the ray (`edge_ramp`): taken
  at the stretch's start it left the first sample of a dense cloud, most of what the eye sees of it, without its later
  octaves, by an amount that changed with the step (a tier's, the bake's). The sun's light only: darkening the sky's
  light lowers a cloud's brightness without adding contrast.
- **The sky's light less what the cloud about a sample hides** (`sky_occlusion`; Guerrilla 2022's pow(1 − profile,
  0.5), floored at 0.4): the profile is the sample's layer's weather before the shape and detail noise carve it
  (`WeatherSample.density`). Checked per layer: it is 0 at the envelope's edge, top and base and rises into the cell (to
  0.63 at takram's cover of 0.38, to 1 from a cover of about 0.6, where the floor holds); the shape noise puts the
  surface the eye sees where the profile equals what it carves, so hollows, carved deep into the envelope, take less of
  the sky than billows. It is a smooth field: it shades a cloud's hollows and core broadly and gives no billow-scale
  relief by itself (that is the sun's). Slabs do not take it (their light from above is `deck_skylight`).
- **Thin layers scatter once** (`layer_bodies`: a layer's optical depth at its thickest over 8; the high sheet 0.19, a
  rain shaft under 0.3, every cumulus and deck 1): in proportion to its thinness a layer has neither the back lobe (which
  stands for the light a body returns toward the sun), the narrow lobe nor the later octaves, and half its forward lobe
  (ice crystals and rain drops send half of what they scatter into a diffraction peak too narrow to tell from the sun).
  takram's powder had left the high sheet about a third of its light at every angle, and its density was chosen with
  that: without this the high sheet doubles in brightness and whites out the sky round a low sun.
- **Slabs keep takram's light** (decks, the storm's deck, fog: `Layer::slab`): its eight octaves and its powder. Under a
  higher sheet (an optical depth of a few, which stops the three octaves) takram's long tail is what reaches a deck's
  top, the deck's own light from above (`deck_skylight`, "less the octaves' share") was derived against it, and the
  creases on a deck's top turned out to be takram's powder on that light. With the new light alone a deck from above is
  a smooth sheet. The light volume (the plan's second stage) gives slabs their true optical depths; their light is its
  to rework. **Lightning** keeps takram's eight octaves too (`approximate_multiple_scattering`), and the moon lights the
  clouds by the same functions as the sun.
- **The sea's bounce on every tier, with no step marched** (`radiance_from_ground`; `quality.rs`): takram marches
  toward the ground for the cloud under a sample (three steps; one on Medium here, none on High and Ultra from
  2026-10-02, which drew no bounce). Without the sun's deep octaves the bounce is about half a cloud base's light, and
  the tiers without it drew bases darker than Medium's (11 levels at the 5th percentile from the sea at noon, 7 before).
  Now every tier takes it through the cloud its own layers' weather puts under the sample (a tenth of the weather at
  the sample over the depth down to the layers' bases: nothing at a base or a side, everything inside a cloud), by
  the frame's light at the camera. From the sea it is within a level of the marched step's in fair weather and under a
  deck, 1.3 under the storm.
- **The environment's bake** (`hooks.wgsl`) takes the same functions (`ray_light`, `sun_scattering`, `sky_light`), so
  the sea reflects the clouds the sky shows.

One lit sample's light is `sample_radiance` (position, weather, medium, the opacity the view ray has gathered before and
after the sample's stretch, the ray's `RayLight` and `FrameLight`): the far march calls it, and a near pass can.

### How it measures

Converged stills in the real client (the lane's `.build/native/look-L/look.ts`: a custom battle, the view held from a cut
with time all but standing, Medium clouds, FXAA, eye adaptation off, 1600 × 900), master `d7f721e16` against this, Apple
M5 Pro, 2026-10-04. Over each view's cloud region (where the sky alone differs with and without the clouds): mean luma,
its 99th percentile (the sunlit tops) and 5th, and the relief (the RMS of luma less its 12 px blur), levels of 255.

| View | Mean | 99th | 5th | Relief |
| --- | --- | --- | --- | --- |
| 2.6 km, noon, away from the sun | 182.2 → 180.5 | 208.2 → 208.1 | 117.9 → 121.5 | 9.63 → 10.18 |
| 2.6 km, noon, across the sun | 190.4 → 186.1 | 212.0 → 211.1 | 156.0 → 147.9 | 5.99 → 7.10 |
| 2.6 km, noon, toward the sun | 192.1 → 189.7 | 212.0 → 212.0 | 148.3 → 149.8 | 6.65 → 7.56 |
| 5 km, noon, 45° down | 179.4 → 176.7 | 208.3 → 207.3 | 111.4 → 114.1 | 10.85 → 11.30 |
| 2.6 km, morning | 163.7 → 160.5 | 204.7 → 206.4 | 89.0 → 89.4 | 10.80 → 11.58 |
| 2.6 km, dusk, toward the sun | 96.7 → 102.0 | 200.7 → 225.4 | 54.6 → 61.9 | 4.14 → 5.77 |
| 2.6 km, dusk, away from the sun | 80.0 → 78.0 | 121.9 → 118.2 | 55.0 → 58.4 | 4.76 → 4.82 |
| A cumulus 300 m ahead, 1.15 km | 183.4 → 181.9 | 209.2 → 211.2 | 140.9 → 141.1 | 3.51 → 3.79 |
| Sea, noon | 177.2 → 175.0 | 210.0 → 213.3 | 152.0 → 146.5 | 4.92 → 5.73 |
| Sea, noon, up toward the sun | 162.5 → 157.5 | 202.1 → 202.2 | 127.8 → 123.0 | 4.38 → 5.01 |
| Sea, dusk, toward the sun | 98.9 → 100.4 | 199.6 → 200.4 | 66.3 → 68.6 | 2.01 → 1.97 |
| Sea, overcast | 168.4 → 168.0 | 180.3 → 180.4 | 158.4 → 157.0 | 1.46 → 1.63 |
| Sea, storm | 77.3 → 75.7 | 96.4 → 94.4 | 67.4 → 65.4 | 2.62 → 2.69 |
| Sea, fog | 162.0 → 162.5 | 217.0 → 220.7 | 110.1 → 110.3 | 2.93 → 2.90 |
| 2.6 km over the overcast's deck | 166.9 → 167.5 | 176.7 → 177.6 | 148.4 → 145.6 | 3.01 → 3.01 |
| Inside the deck | 163.6 → 164.0 | 167.7 → 168.4 | 160.4 → 160.7 | 0.46 → 0.46 |
| 3 km over the storm | 158.8 → 158.9 | 173.7 → 173.7 | 121.8 → 123.7 | 2.86 → 2.84 |
| Sea, moonlit night, toward the moon | 34.1 → 31.2 | 51.7 → 51.8 | 22.0 → 15.7 | 3.96 → 4.69 |
| 2.6 km, moonlit night | 38.8 → 35.8 | 55.7 → 55.7 | 19.0 → 15.0 | 5.96 → 7.04 |

From above at noon the hollows between billows shade and the rims read against them (a fifth more relief across the
sun), at the brightness the tops had; toward a dusk sun the rims glow (the 99th percentile up 25 levels) over cores
that are smooth and dark; from the sea the bases are greyer under lit edges. Decks, the storm and fog are as they were
(within a level, 1.5 under the storm: the bounce). A moonlit night is 8% darker in the mean, with darker cores under
lit rims. Two octaves at the same constants are 4% darker than three at noon (173.4 against 180.5 from 2.6 km) and a
fifth darker under the moon, with less relief (9.4 against 10.2), and cost the same: every tier takes three. The tiers
now agree: from the sea at noon the mean is 175.0, 174.9 and 174.7 on Medium, High and Ultra (177.2, 174.9, 174.4
before), the 5th percentile 146.5, 146.7, 146.7 (152.0, 144.7, 143.8); from 2.6 km 180.5, 180.2, 181.0.

The sea under a partly cloudy sky is a deeper blue: it reflects the bake and takes its water's light from it, and the
clouds' bases are darker there too. In the open-sea check views (`sea-near`, `sea-wide`) its mean goes from (97, 113,
132) to (82, 104, 127); fourteen check references changed with the clouds and the sea and were blessed.

Stills (today, two octaves, three octaves side by side; before and after pairs; crops) are in the lane's
`.build/native/look-L/stage1/`. Measured again over master with the near detail (`c49c204f8`, "The near detail"), the
changes hold: from 2.6 km at noon 180.5 → 178.9 in the mean, relief 9.46 → 10.07; toward a dusk sun the 99th
percentile 200.7 → 225.4; the overcast and storm from the sea within 1.6 levels; Medium's march 0.06 to 0.20 ms
cheaper (load 9 to 18).

### What it costs

`examples/timings` (2400 × 1350, Apple M5 Pro, 2026-10-04), master's crate and this one built side by side and run
alternately, three rounds each, load average 5 to 8; the march's tenth percentile, the lowest round, milliseconds:

| Tier | Noon | Sunset | Overcast | 2.6 km | Storm |
| --- | --- | --- | --- | --- | --- |
| Low | 0.62 → 0.61 | 0.67 → 0.65 | 0.84 → 0.94 | 0.82 → 0.78 | 0.50 → 0.55 |
| Medium | 1.17 → 1.00 | 1.30 → 1.11 | 1.92 → 1.71 | 1.54 → 1.37 | 0.99 → 0.86 |
| High | 1.61 → 1.55 | 1.76 → 1.72 | 1.93 → 2.10 | 2.04 → 1.99 | 1.20 → 1.30 |
| Ultra | 2.03 → 2.00 | 2.24 → 2.22 | 2.12 → 2.29 | 3.21 → 3.13 | 2.66 → 2.93 |

Medium, the default, is 0.13 to 0.21 ms cheaper everywhere: the octaves (their phases once a ray), and the bounce
without its marched step. Fair weather is a little cheaper on every tier; under a deck Low, High and Ultra pay 0.05 to
0.27 ms for the bounce they did not draw. The other passes are untouched. (A marched step on every tier instead costs
High 0.1 to 0.5 ms and Ultra up to 0.8; the sea's own light under each sample, takram's accurate form, another 0.4 ms
under a deck.)

### Open

- **Slabs** (decks, the storm's deck, fog) keep takram's light: flat grey inside a deck, no long shadows across a deck
  at a low sun. The light volume is the place for it.
- **Self-shadow still reaches 150 m** (two marched samples, then the Beer shadow maps' one slab a texel): billows do not
  shade their neighbours yet. The relief gained here is the octaves' and the edge term's.
- **The sky's light has no direction**: a cloud's shadowed side takes the whole sky's irradiance (less the local
  occlusion), so at dusk the cores away from the sun stay about as bright as they were (the 5th percentile did not
  fall).
- **The sea's bounce is a guess** on every tier (takram's too): the clear sky's light at the camera on a sea of albedo
  0.3, the sun's by the uncovered share of the sky, which under a closed deck is far too much.
- **The high sheet near a low sun** is brighter than it was (the forward lobe a veil keeps is half of takram's, where
  powder had left a third of everything).
- **The interpolated tiers' light at dusk**: Low and Medium take the bounce's sun as the scalar irradiance at the camera,
  not on the sea's surface, as before.

## Night: the moon and lightning (2026-10-03)


At night the clouds were dark shapes (takram's are lit by the sun alone) and lightning lit hulls, land and rain but
not the clouds it strikes from. Now:

- **The moon is the key light after dark** (`src/key_light.rs`). The body whose beam the march, the Beer shadow maps,
  the light shafts and the environment's bake follow (`CloudUniforms::sun`) is the sun by day and the moon once the
  moon's direct light at the top of the clouds (`heights.y`, where the sun lingers longest) outshines the sun's: the
  clouds' sunlight as the march takes it (the sun's luminance above the air, the twilight lift, the dome's gain)
  against the world's moonlight (`SkyUniforms::moon_irradiance`, the dome's gain), each through the air (`model::
  optical_depth` on the CPU). For a full moon opposite the sun and the clouds' 8 km top that is 2.5° after the sun
  sets at the sea, the two beams equal there (0.217 against 0.212: the test holds them within a tenth), so the direct
  light passes from one body to the other without a step; the sky's light on them is continuous through it. Before
  the switch the low clouds have already lost the sun's beam (it leaves them at about 1.4° below the horizon); after it
  the high ones keep the moon's.
- **The moon's light on them** (`shaders/march.wgsl`, `key_scalar_irradiance`, `key_irradiance`): its beam through the
  same tables (`get_sun_and_sky_scalar_irradiance` toward the moon) at the world's moonlight on the dome's scale
  (`key_light.xyz`: the dome's gain × the moon above the air over the tables' solar irradiance, per channel), no lift
  of its own; and the sky's light: the moonlit sky at the dome's grade (`sky.moon_scale`), the sun's twilight while it
  still lights the sky (`sun_active`), the night floor. By day `sun_sky_scalar_irradiance` as before, so day and dusk
  draw the same bytes. The haze, the ground bounce and the bake take the same light. The light shafts are off under
  the moon (`shafts.w`): the dome takes their length out of the sun's light.
- **Lightning lights the clouds** (`flash_radiance`). The flash the weather strikes (the sky's `lightning_position`, the
  channel's brightest point inside the cloud, and `lightning_intensity`, an irradiance of `intensity × (1 km / r)²`
  in the sea's units; set the same frame as the bolt) is a point light in the march: its light reaches each sample
  through the cloud between, the weather's extinction (the layers' coverage and profiles before shape and detail carve
  them, smooth at a stroke's reach) over three jittered samples, scattered by the sun's octaves and phase; deep in a
  deck, where the octaves' fall-off leaves nothing, by the light a non-absorbing cloud diffuses (the two-stream slab
  transmission 1 / (1 + ¾(1 − g)τ), g = 0.85 for cloud droplets; takram's dual lobe is fitted to the look, not the
  transmission). A ground stroke lights the deck's base about its channel and the cloud about its top from within; an
  intra-cloud flash glows through a deck's top and, faintly, its base. The march costs more only while a flash lights
  the clouds (a uniform branch).
- **The reconstruction follows the flash** (`resolve.wgsl`, `CloudUniforms::flash`): while a flash lights the clouds
  (this update's or the last's), every pixel clips its history to the march as a moving view's does (variance gamma 2),
  so the flash shows the frame its stroke does and goes with it (a still pixel's gamma 8 kept the history dark through
  the flash); held or paused clouds march again.
- **Cost** (Apple M5 Pro, 2026-10-03, High, 2400 × 1350, master against this alternating, 12 rounds a side, load
  7–20): under the moon the Beer shadow maps take 0.04–0.06 ms more (they march toward a moon above the clouds, where
  master marched toward a sun below the horizon) and the march 0.04–0.17 ms less (no shafts); with a flash lighting a
  partly cloudy sky the march stays within master's (0.98 against 1.02 ms median); by day nothing moves (the water pass
  4.217 against 4.221 ms, the march 1.060 against 1.048).
- **Strike placement**: the weather places a strike's channel from the scene's cloud base and depth
  (`StrikeClouds`), which the layers follow only where the scene chose them (`weathers.rs`; otherwise their heights are
  the weather's own); an intra-cloud flash placed above the layers lights only their top. The stills' intra-cloud flash
  is seed 15, 1.8 km up, inside the deck.

## Run it

From `native/` with `~/.cargo/bin` on `PATH`:

```sh
cargo run -p naval-clouds --example stills --release -- [--only bridge,air2600] [--tier ultra] \
    [--time morning|noon|evening|sunset|night] [--cover 0.38] [--frames 64] [--tag name] [--moving] [--bob] [--hdr]
cargo run -p naval-clouds --example timings --release -- [--size 2400x1350] [--frames 120] [--tiers high,ultra]
cargo run -p naval-clouds --example blue_noise --release          # rewrites assets/blue_noise.png (same bytes)
cargo run -p naval-clouds --example noise_compare --release -- <three-geospatial>/packages/clouds/assets
cargo test -p naval-clouds --release
```

- `stills` draws the sky alone (no sea or ships: the clouds stop at the sea sphere) from fixed views (the bridge three
  ways and looking up, 900 m, 1.35 km, 2.6 km twice, 5 km, 9 km, a dive, the chart from 40 km) through the renderer
  core, held and converged, into `.build/native/clouds/takram/<tag>/<view>.png`; `--moving` lets time run, `--bob`
  also moves the camera as a ship's chase camera does (forward at 12 m/s, pitching ±1.5° over 8 s), and `--hdr` writes
  the scene's linear radiance before the display grade (`<view>.rgbf`, f32 RGB).
- The real client: `bun native/tools/shot.ts --battle "bismarck;;fletcher:static" --time noon
  --weather partly-cloudy --world-eye=0,2600,4000 --world-target=0,2300,-10000` (see `native/docs/harness.md`).
- `timings` is described under "GPU cost".

## What is ported, and where

| Native | takram | Notes |
| --- | --- | --- |
| `src/layers.rs` | `CloudLayer.ts`, `CloudLayers.ts`, `DensityProfile.ts`, `updateCloudLayerUniforms` | takram's four default layers: 750–1400 m (red channel, shadows), 1000–2200 m (green, shadows), 7500–8000 m (blue, density 0.003, no detail), and an empty fourth; packed into vec4s with the gaps the march strides over. Each layer also has a coverage of its own, a weighted mix of the weather's channels, a shadow weight and a slab weight (takram: one coverage, one channel, a shadow flag), so two sets of layers blend ([`Layers::lerp`]). |
| `src/weathers.rs` | (none) | The layers each weather draws (see "Weather"). |
| `src/quality.rs` | `qualityPresets.ts` | Low, Medium, High (takram's defaults) and Ultra onto the tiers of the same names (table below). |
| `src/cascades.rs` | `CascadedShadowMaps.ts`, `splitFrustum.ts`, `FrustumCorners.ts` (after three-csm) | Practical splits (λ 0.6), light-space fitting, texel snapping, the fade margin. |
| `src/textures.rs`, `assets/`, `crates/naval-sim/src/clouds` | `shape.bin`, `local_weather.png` | takram's shipped textures embedded as lossless PNGs: their generators hash value noise with `sin`, which differs between GPUs, so generating them would not give takram's texels (`examples/noise_compare.rs`). |
| `src/detail_noise.rs`, `shaders/detail_noise.wgsl` | (`shape_detail.bin`, replaced) | The detail noise, the crate's own: 128³, four channels, generated at startup ("Near detail"). |
| `shaders/noise.wgsl` | `turbulence.frag`, `perlin.glsl`, `tileableNoise.glsl` | The turbulence is generated at startup and matches takram's to the bit. |
| `shaders/common.wgsl`, `field.wgsl` | `parameters.glsl`, `clouds.glsl` | Uniforms, the weather (coverage, shape-altering function, coverage filter), the media (shape, turbulence, detail, density profiles). |
| `shaders/march.wgsl`, `march_pass.wgsl` | `clouds.frag` | The primary march (perspective steps, empty-space strides, sun march + BSM, multiple-scattering octaves over a dual-lobe phase, ground bounce, sky gradient, powder, Frostbite's energy-conserving integration), shadow length, haze, aerial perspective. Broken cloud's light is the game's since 2026-10-04 ("The light of broken cloud"); slabs and lightning keep takram's octaves and powder. |
| `shaders/shadow_pass.wgsl`, `shadow_resolve.wgsl` | `shadow.frag`, `shadowResolve.frag` | BSM per cascade with structured volume sampling (front depth, mean extinction, max optical depth, its tail), and their temporal pass. |
| `shaders/resolve.wgsl` | `cloudsResolve.frag` | Temporal upscaling: one pixel of each 4 × 4 block marched per update in Bayer order; every pixel keeps the running mean of its own samples (up to 16), its history reprojected with the clouds' drift and clipped to the variance of the march about it; a pixel with few of its own leans on the march about it (the prior). |
| `shaders/composite.wgsl` | `cloudsEffect.frag` | Premultiplied clouds over the scene; a pixel holding few samples drawn through a small spatial filter. |
| `shaders/ground.wgsl` | (none) | The ground shadow map the forward-lit ships and sea read (below). |
| `shaders/hooks.wgsl` | (none) | What the sky's dome and environment bake take: the shafts' length per pixel, a short march for the bake. |
| `src/weather.rs`, `lightning.rs`, `bolt.rs`, `post.rs`, `shaders/rain.wgsl`, `splashes.wgsl`, `bolt.wgsl`, `haze.wgsl`, `post.wgsl` | the game's `src/game/sky/weather` | Unchanged from the web port, less what lit the old clouds. The splashes stand on the drawn sea since 2026-10-03, as the web's do: `Weather::set_sea_heights` takes the ocean's heights about the camera (`naval_ocean::rain_heights`, wired by naval-client's `scene.rs`; `ocean.md`, "Weather at sea"), the flat mean sea without them. |

### Where it departs from takram, and why

- **The world is a flat sea under a curved sky.** The planet frame (takram's ECEF) has its centre straight under the
  camera (`naval-sky`'s convention), so heights and the layers' spheres curve away from the camera, while every
  texture is read in world-fixed planar coordinates (east, height, north) drifted by the wind: the weather tiles every
  100 km (takram's 100 repeats over a cube-sphere face), the shape every 3.3 km, the detail every 100 m (takram's 32³
  tiled every 167 m). Clouds stay
  where they are as the camera moves.
- **The blue noise is the crate's own** (`examples/blue_noise.rs`: Ulichney's void-and-cluster, 128², animated over
  frames by golden-ratio offsets). takram ships NVIDIA's STBN, licensed for non-commercial use only. takram reads its
  STBN at the march buffer's texel, which works because each of STBN's 64 frames is a fresh pattern; one pattern
  stepped by the golden ratio would give a block's 16 cells one value, offset only by the update each is marched in,
  and print the same 4-pixel pattern into every block, so the march reads it at the drawing buffer's pixel. It steps
  by the golden ratio once per march of that pixel (`march_jitter`: one update in 16), so the samples a pixel gathers
  spread evenly over the jitter. Until 2026-10-03 it stepped every update over a cycle of 64, which gave a pixel only
  four jitters, all within a third of [0, 1): its samples never averaged to the clouds, and their grain stood in every
  still (`stbn`, the shadow maps' jitter, still steps so).
- **No cloud gain** (from 2026-10-03; `CLOUD_GAIN` was 1.9 on the clouds' whole radiance): the clouds' sunlight takes
  the dome's grade gain instead (`direct_scale` × `sky_grade::GAIN`), as the sun on the sea and ships does, so clouds,
  sky and world share one scale (`docs/lighting.md`, "Realism").
- **Reconstruction** (rewritten 2026-10-03 to take the grain out): takram replaces a pixel with each new sample, and
  until 2026-10-03 the port blended it half into the pixel's history, about three samples' worth, so one sample's
  noise stood in every still as a sandy grain. Now every pixel of the drawing buffer keeps the running mean of its own
  samples and how many it holds (the shafts' history's green channel): up to 16 (16 updates each, about four seconds)
  where the view has not moved the pixel, 8 where it has, whose history is resampled every update and gathered the
  blur of those resamplings and of the clouds' evolving shapes over the longer span (against the clouds marched at 32
  jitters the shorter span is as near or nearer in every view, and keeps edges a tenth sharper).
  - **Motion**: the history is reprojected by the motion of the march pixels about it (their mean weighted by a
    quadratic B-spline of 1.5 march texels; takram's nearest fragment steps from one 4 × 4 block to the next, which
    sheared a long history wherever the motion varies over the view), and the march's motion of a cloud includes the
    wind's drift since the previous update (`CloudUniforms::drift_step`): binoculars magnify a 12 m/s wind 20 km off to
    a fifth of a pixel an update, and without it a long history smeared along the wind (against the 32-jitter clouds:
    3.65 levels from them at 2 km among cumulus, 2.27 with it).
  - **History read**: a pixel the view did not move reads its own history texel; one moved under 0.3 px, bilinearly,
    blending to takram's Catmull–Rom read (`textureCatmullRom`, which takram leaves commented out) by 0.7 px. Under a
    slow drift a Catmull–Rom read kept the noise of samples carried in from other pixels and a bias of up to a level
    and a half (the nine-tap form also printed bright single pixels), where the bilinear read is the nearest to the
    32-jitter clouds in every view measured.
  - **Clipping**: to the march about the pixel, the box's moments weighted by the same spline (the nearest march
    texel's box stepped at the blocks' edges, which binoculars printed as soft 4 × 4 blocks under a pitching ship);
    variance gamma 8 for a pixel the view did not move, whose detail clipping to the quarter-resolution
    neighbourhood would drag back toward that picture, and takram's 2 from a pixel of motion up; on the radiance, the
    opacity following (a box over all four channels has no width inside a thick cloud, whose opacity is 1 throughout);
    at least a hundredth of the mean wide. A history the box finds more than one and a half half-widths out keeps
    proportionally fewer samples, none from two and a half, so a changed cloud takes the new samples at once.
  - **The prior** (from 2026-10-04, for a view that turns): a pixel with few samples of its own leans on the march
    about it, the spline-weighted mean the clipping box is built on. It joins the pixel's mean a quarter of a sample's
    weight an update until it holds two samples' worth (`PRIOR_WEIGHT`), so a new pixel's prior is the mean of eight
    updates' marches, each on another cell of the block; each sample of the pixel's own then lowers the weight the
    prior may hold (none from four, `PRIOR_SAMPLES`), so later samples weigh the more and the prior fades out of the
    mean. The state's alpha holds its weight. Before, a pixel without a sample followed the march buffer upsampled
    bilinearly, half a new estimate every update (a 4 px blotch that changed every frame), and then took its first
    sample whole. A still view is as it was (its pixels hold 8 or 16 samples and no prior); see "How it looks" for a
    view that turns.
- **The scene's silhouette** (from 2026-10-03): the march writes where each ray met the opaque scene (the view depth,
  0 for none) beside its motion, and the reconstruction reads the opaque depth too. Where a pixel's 3 × 3 march
  neighbourhood straddles the silhouette, only the march pixels that saw the pixel's surface give its sample, its
  motion and its clipping box. takram takes them all, so a ray stopped at a ship (no cloud, the ship's motion) left a
  pale cloud-free halo a few pixels wide round masts, directors and funnels against the clouds, widest while the view
  turned. The history also keeps the view depth (km) of the surface each pixel saw (the shafts' history's blue
  channel, 0 for sky or sea), and a reprojected history that saw another surface starts again from this update's
  estimate: a hull steaming across the clouds leaves no trail (before, the stale history behind a mast stood until
  its cell came round, a pale streak up to 16 updates long).
- **On a frame that leaves time standing** (a capture, a frame drawn again, a paused picture, the first frame), the
  first update after a cut marches every pixel (`march_full_main`) into the history the composite reads, so the picture
  is whole rather than the march buffer upsampled (the 4 × 4 mosaic a shot taken the frame its camera arrives showed).
  It gives every pixel one sample, which the composite's spatial filter (below) draws without its grain. It costs
  about ten quarter-resolution marches (see "GPU cost"); in play a cut fills in over the next 16 updates, as
  takram's does, rather than spend that long frame (every pixel the prior first, then its own samples). A sky put on screen in place of another in play
  (`GameSky::start_in_play`: the theatre's scene swaps, `native/docs/theatre.md`) takes its first frame as one of play,
  so the frame that swaps the scene in does not pay the full march.
- **The spatial filter** (from 2026-10-03, `composite.wgsl`): a pixel whose mean holds between one and seven samples
  (a capture's full march, the first seconds after a cut, sky a hull has just uncovered) is drawn through a Gaussian
  over its neighbours that saw the same surface, 1 px wide for one sample and narrower as the samples gather (1/√n),
  5 × 5 or 3 × 3. The march's jitter is blue noise, whose error a few pixels across cancels: on a capture taken the
  frame its camera arrives it brings the clouds from 1.8–2.9 levels off the 32-jitter clouds to 0.7–1.5. A pixel with
  no sample of its own (the prior alone, smooth already) or with eight or more is drawn as it is, and the history
  is never filtered.
- **Precision**: rays come from the lens and the view's rotation (as takram's vertex stage) and motion from the
  camera's world position, never from points near the camera or the planet frame's heights, whose f32 steps (2.4e-4 m
  at 2.6 km, half a metre at the planet's radius) moved a still view's history a third of a pixel an update.
- **Steep rays** take at least 16 samples through the shadow-casting layers' depth (takram's step grows with the
  distance to the layers, which left a steep ray from the chart four).
- **Scene shadows**: takram shades a scene through the BSM in its deferred aerial-perspective pass; the game's ships,
  islands and sea are lit forward and read the ground shadow map instead, through the interface they always had.
- **Light shafts** show in the sky's dome (the hooks), not over the sea or ships, whose haze is the game's aerial
  perspective rather than takram's aerial-perspective pass.
- **Haze** (2026-10-03, `haze.md`): takram's thin haze is off in every tier; the march takes the air in front of the
  clouds' front from the aerial-perspective volume (`naval_sky::aerial::aerial_along`, the same air every material
  takes) in place of `apply_aerial_perspective`, which the environment bake keeps. `GameSky` runs the volume's pass
  (`crate::aerial`) after the clouds' shadows.

## Using it

```rust
let mut sky = naval_clouds::GameSky::new(&device, &queue, SkyQuality::Medium, naval_clouds::Targets::default());
// or GameSky::new_in(ctx, quality, targets) to build on the renderer's registry (hot reload)
sky.apply(&queue, &scene);                       // naval_sky::SkyScene: sun, moon, air, cloud coverage, weather
renderer.render(input, &mut [&mut ships, &mut ocean, &mut sky], output)?;
sky.post_process(&device, &queue, &mut encoder, &hdr_copy, &scene_depth, &scene_hdr, [w, h]); // or post_after_copies
let light = sky.light();                         // CelestialLight: the sun or the moon
let flash = sky.lightning();                     // LightningLight { flash, bolt }
for thunder in sky.thunder() { /* distance, delay, loudness for the audio */ }
```

- **Frame order** (`GameSky::prepare`): cut detection (250 m, 0.5 rad, a 20% zoom, a 2° sun move, a 0.05 coverage
  change) → weather (lightning, rain boxes, splashes) → the strike to the sky → the sky's tables → the clouds' CPU half
  (cascades, uniforms, the update's Bayer cell) → the BSM, their temporal pass and a slice of the ground shadow map →
  the environment bake. At render core II's opaque copy point (`after_copies(Opaque)`, `scene.opaque.depth`) the
  march and the reconstruction: rays stop at the opaque scene and the sea sphere. In `Phase::Sky` the dome (with the
  shafts) then the composite over everything drawn; splashes, the bolt and rain in `Phase::Transparent`.
  `GameSky::prepare` publishes what the composite draws (`CloudLayer::cover`, `naval_sky::CloudCover`) for the
  transparents drawn after it: the combat effects fade their smoke by the clouds in front (`combat-effects.md`).
- **Under water** (`ray_starts_under_water`): a ray that starts under the waves meets the sea's surface before any
  cloud, so the march gives it no clouds, haze or shaft and the composite draws nothing there. Which rays do comes from
  the ocean's underwater probe (`GameSky::set_underwater_probe`, the client hands it over: each pixel's near-plane
  point's depth below the waves and the wake), so a camera in a trough keeps the sky above the wave and one under a
  crest loses it; without an ocean no ray starts under water. The composite is not depth-tested and knows only the
  opaque depth, taken before the sea is drawn, so until 2026-10-03 it painted the clouds over the sea's underside at
  any depth. The sky, clouds and all, shows under water only through Snell's window, which the sea's underside takes
  from the environment (`sea-sky.md`, "The underwater view").
- **Scene**: the clouds read the weather from `SkyScene` (`naval_clouds::weathers::CloudWeather`: the cover, the genus,
  the base and depth when chosen, the high sheet, fog banks, rain) and the wind's drift, and draw its layers ("Weather").
  `GameSky::apply` puts a scene on at once (a cover change over 0.05 cuts, as before); `GameSky::blend(from, to, t)`
  draws the weather `t` of the way between two scenes without a cut, for a weather change drifting in.
- **Held skies and paused pictures**: `hold(Some(t))` stands sky time and drift at `t`, restarts the sequence on each
  new view (a full march first) and stops marching after `HOLD_UPDATES` (240: every pixel marched 15 more times, its
  mean 16 samples; 48 before 2026-10-03); a paused frame converges the same way and then stands, so captures are
  deterministic.
- **Sky time at a rate** (from 2026-10-03): `GameSky::time_scale` (1 by default) steps the sky's time, the clouds'
  drift and their evolving shapes that many seconds a frame second; the rain and the lightning keep the frame's time.
  Since 2026-10-06 the theatre leaves it at 1× at every speed, so the camera riding an accelerated ship travels
  through the cloud field while the clouds keep their natural wind drift. Previously cruise scenes accelerated
  them with the ships up to 8×, making them appear to move alongside the camera. Pausing still holds their drift
  and shape evolution. A frame costs the same at any rate.
- **Diagnostics**: `force_strike` (the web's `forceStrike`), `GameSky::enabled` (profiling), `stills --hdr`.

### What other lanes read

- **Cloud shadows** (ships, islands, the sea): unchanged. `sky.clouds.shadow()` is a `CloudShadow { view, sampler,
  uniforms, bind_group_layout, bind_group }`; `naval_clouds::shadow::cloud_shadow(map, sampler, params, position)`
  (cubic B-spline, four reads) or `cloud_shadow_fast` (one read): 1 in full light, 0 under the thickest deck (0.15
  before 2026-10-03), faded in with its body's elevation and at the map's edge, clear at the port's berth
  (`CloudClearing::port`). The map (`shaders/ground.wgsl`) is the transmittance of the scene's light, the sun's or the
  moon's once it outshines the sun (`CloudUniforms::ground_light`, from 2026-10-02: before, nothing shaded moonlight),
  through takram's shadow-casting layers above each point of the sea, 40 km across in the clouds' drifting frame,
  marched with the clouds' own field, 16,384 texels an update (1024² on High and Ultra, a full pass about every 64
  frames; 512² on Low).
- **The environment** (`naval_sky::EnvironmentMap`) holds the clouds through `clouds_bake` (the tier's bake steps, the
  sky gradient, and the sun's light through six even steps up to the top of the shadow-casting layers, swept as
  before; a deck's steps lit at its base, below). The march on screen takes the bulk of that optical depth from the
  Beer shadow maps, which cover only the camera's frustum; with takram's 100 m march toward the sun alone the bake's
  clouds came out nearly unshaded. The sea reflects the bake and takes its water's light from it, so a cloudy sky now
  reads paler and greyer on the sea than the old clouds' did: under the partly cloudy chase view its mean is (53, 72,
  92) against (19, 50, 75) before (eye adaptation off, 2026-10-02). About half of that was the clouds' gain
  (`CLOUD_GAIN`, gone since 2026-10-03) and the rest is takram's brighter and fuller cover. The bake's alpha is the
  luminance the texel would have without clouds, prefiltered with the radiance: the eye adaptation meters against it
  (`docs/lighting.md`, "Realism").
  **A deck is lit at its base (2026-10-13).** The bake's steps are hundreds of metres long (the layers' whole depth in
  12 to 32 steps), more than a deck is deep, and each was lit at its middle: a deck seen from the sea took the light of
  a point most of the way up it, where little cloud is left overhead, so the sea, the hulls and the haze, which take
  the bake's light, were lit by a deck brighter than the one on screen, and brighter on a tier with fewer steps. A
  step of a slab (a deck, fog) is now lit where its light comes from (`light_depth`: the mean depth of what the step
  scatters back to its start, half of a thin step, one mean free path into an opaque one); broken cloud keeps its
  middle, so fair weather draws the same bytes. Found when Bill asked whether the night was too bright: an overcast
  night's sea was brighter than a clear night's. Measured in the client (Bismarck, the North Atlantic, a full moon 28°
  up at night, Medium clouds, 1728 × 1030, one binary with the bake swapped by hot reload), before → after, and in
  brackets the old bake marched 16 times as finely with its light in 48 steps:

  | Scene | Environment's sky mean | Sea beside the hull | Near sea, toward the moon or sun | Hull |
  | --- | --- | --- | --- | --- |
  | Overcast night | 0.0232 → 0.0138 (0.0142) | 9.7 → 5.8 (5.7) | 22.4 → 7.9 (8.8) | 14.2 → 8.4 (9.4) |
  | Overcast noon | 0.435 → 0.340 (0.340) | 103 → 100 (99) | 77 → 72 (72) | 73 → 73 (73) |
  | Storm, noon | 0.122 → 0.120 (0.117) | 74 → 74 (72) | 58 → 58 (58) | 67 → 64 (64) |
  | Fog, noon | 0.787 → 0.701 (0.694) | 136 → 135 (138) | 120 → 119 (120) | 125 → 125 (124) |

  Sea and hull are 8-bit medians. A clear night reads sea 21, near sea 12 (with the night sky's glow, 2026-10-12).
  The overcast noon's eye opens 0.4 stop where it had not, as the reference's does. `tests/gpu.rs`,
  `an_overcast_nights_ambient_is_the_same_on_every_tier`, holds Medium's and Ultra's deck ambient within 15% (0.0301
  against 0.0179 before, 0.0131 against 0.0139 after) and under a clear night's. Left as it was: the six light
  steps (doubling them from the sample left the deck brighter), and broken cloud's steps, which carry the same bias
  (a partly cloudy night's sky mean 0.0175 against 0.0149 by the fine march, 2026-10-12). Stills in
  `.build/native/night-look/`.
- **Lightning on meshes, thunder, eye adaptation**: as before; lightning lights the clouds again since 2026-10-03, through
  the march ("Night: the moon and lightning").
- **The hooks** the sky composes (`naval_sky::CloudHooks`): `clouds_shadow_length(pixel)` and
  `clouds_bake(origin, direction)`.

### Post passes

`GameSky::post_process` (or `post_after_copies` at the backdrop copy point) now draws only heavy rain's veil; the
screen-space sun shafts went with the old clouds, and takram's light shafts take their place in the dome.

## Aircraft spotting and shared cloud weather

The renderer-free layer types, weather profiles and local weather PNG live in `crates/naval-sim/src/clouds`.
`naval-sky` re-exports the genus types; `naval-clouds` re-exports the layers and adapts `SkyScene` to their common
weather profiles. Aircraft-to-ship sight rays integrate the shared map's coarse density with a mean shape fill of 0.5,
using a sight threshold of optical depth 3.912 (2% contrast). This is gameplay tuning, not a historical calibration.
The GPU continues drawing its fine shape/detail noise, so cloud fringes are approximate in spotting. The native-only
`RenderPacket.clouds` gives visual battles their authoritative layers and drift, including theatre coordinates and
clock after a handoff or re-form. Cloud layer 3's rain and sea-level fog stay the renderer's; simulation visibility
already limits sight in those conditions. Graphics quality, disabled map-view clouds and camera position never
change spotting. See [air operations](../../docs/air-operations.md#reconnaissance-reports-missions).

`naval-client`'s `theatre_clouds` regression compares the drawn wind offset with the simulation's cloud field at
1x, 8x and 64x requested speed, then checks pause and resume while the camera moves.

## Weather

What the weather does to the clouds, the sky's light and the sun (2026-10-03): `src/weathers.rs` (the layers),
`CloudLayer::apply` and `blend`, `GameSky::blend`, `shaders/field.wgsl` (`column_above`) and `march.wgsl`
(`deck_skylight`, the fine steps), and naval-client's `weather.rs` (the drift, the battle's clock).

### The layers each weather draws

Every weather fills takram's four slots, so two weathers blend parameter by parameter. With no genus named the cover
decides: takram's layers up to 0.66 (clear, partly cloudy, the maps' own skies), turning into a deck from 0.66 to 0.8 and
a storm from 0.87 to 0.94, continuously.

| Slot | Fair (takram) | Deck (overcast) | Storm | Fog |
| --- | --- | --- | --- | --- |
| 0 | cumulus, 750–1400 m | stratocumulus from 800 m, 700 m deep | nimbostratus, 250–2250 m, solid from about 600 m | stratus, 600–950 m, broken |
| 1 | cumulus, 1–2.2 km | altostratus, 3.5–4.1 km | cumulus heads, 1.25–5.75 km | none |
| 2 | high layer, 7.5–8 km | as fair | a thicker sheet at 8.5 km | as fair |
| 3 | empty | empty (a drizzle draws no shafts) | rain shafts under the cells (High and Ultra) | fog banks on the sea |

- **Decks** are slabs (`Layer::slab`): the broad weather channel (B, streaky) under the finer one (A, which takram leaves
  unused) for rolls and cells, a layer coverage of 0.55 with a narrow filter (0.35) so the deck is not saturated (takram's
  layers at the overcast's 0.82 have no gap and no thin place: over the weather texture, takram's low layer at 0.82 is
  saturated over 27 % of the sky at its widest height and nowhere clear, where the deck at 0.55 is saturated over 10 %,
  clear over 3 % and in between everywhere else), half a cumulus's density, and a base that undulates with the cells.
- **The storm's** deck is twice as dense (an optical depth in the hundreds under its deepest cells) and widest at half
  height, so the cells' bottoms hang ragged from the solid base about 600 m down to 250 m.
- **Fog banks** start under the sea: takram's shape-altering function thins every layer to nothing at its bottom, so
  the layer runs from −50 m with its bias set to peak at the surface, thinning to its top at 250 m. The broad channel
  makes banks kilometres across, the finer one breaks their edges; about half the sea lies in them, a few hundred metres
  of visibility in the thick of one. Rain shafts start under the sea too and rise to the lowest cloud's base, from the
  deck's own channels without shape noise so they fall under its thickest cells, a fifth of the sea under them in a
  downpour, their density with the square of the rain; none for a drizzle (0.3 or less: the overcast's 0.15), and only
  from High up (`CloudTier::rain_shafts`: about half a millisecond on Medium).
- **A genus** the scene names (the battle rail, the console) draws its own layer at the base and depth the scene asks
  (`CloudScene::base_chosen`, `depth_chosen`: a genus brings its own, the rail's base or the console's depth set them);
  a base chosen without a genus moves takram's fair layers there. **A high sheet** named replaces slot 2 (cirrus:
  takram's high layer; cirrostratus: a smooth veil; cirrocumulus: grains).

### The light under a deck

takram lights every point of a cloud with the whole clear sky's light (a gradient from half at a layer's base to all of
it at its top) and the sun through octaves of multiple scattering, which fade exponentially with the optical depth to the
sun. Under a closed deck neither holds: the sky over a point is the deck, and sunlight reaches the base by diffusion, not
by octaves (at an optical depth of 30 the octaves pass a few thousandths of the sun where diffusion passes a quarter).
So the base was one evenly lit sheet of the clear sky's light.

Under and inside slabs the sky light is the light falling on the deck's top (the sky's and the sun's) diffused down
through the cloud over the point, less the sun's beam that crosses unscattered (the octaves' share): two-stream
diffusion through a conservative slab, `1 / (1 + 3/4 (1 − g) τ)` with the droplets' g = 0.85 (`deck_skylight`). τ is the
optical depth of every layer over the point, estimated from the weather alone at two heights of each layer's span above
it, with half the weather's density (what the shape and detail noise leave on average) (`column_above`). Thin patches
and gaps pass more light, thick cells less: the deck's structure from below, and a storm's base dark where it is deep.
The weight is the largest slab weight of the layers above the point, so broken cloud keeps takram's light: fair weather
draws exactly as before, and skips the estimate.

### Steps in slabs and the band

- **Near the camera**, in a slab within 600 m (fog it stands in, a deck it flies through), a step takes at most 0.35 of
  optical depth (and at least 8 m), and the ray goes on by it: takram's 50 m steps there left every pixel a different
  few samples of a dense medium, grain the reconstruction could not settle. The ray ends after a few such steps (it is
  opaque), so the cost stays bounded.
- **Thin parts** of a slab (a deck's ragged base) take up to two steps' length, rain up to four, 0.35 of optical depth
  each.
- **The fog and rain band** (the fourth layer when it starts under the sea) is crossed for tens of kilometres by a level
  ray, and every ray toward the sea now marches it: its empty air goes by in strides of 6 % of the distance, and past
  12 km the rest of the band is skipped.
- **Deep under a closed deck** (an optical depth of 60 over the point) the sun is not marched: its octaves there are
  under a fiftieth of the light the deck diffuses down.

takram's own layers keep all of takram's steps: fair weather marches as before.

### GPU cost

`cargo run -p naval-clouds --example timings --release` with two scenes added (the storm's deck and rain from the sea,
12° up; the fog's banks, 4° up), 2400 × 1350, Apple M5 Pro, 2026-10-03, two rounds each of master (`0875c604f`) and this
branch alternating, load average 10–28 (other agents rendering; single frames swing by up to a millisecond). The march
pass, milliseconds a frame:

| Tier | Scene | Master | Weather |
| --- | --- | --- | --- |
| Medium | noon (fair) | 0.98–1.00 | 1.12–1.41 (the same march: the picture is identical, the spread is load) |
| Medium | 2.6 km up (fair) | 1.45–1.67 | 1.56–1.65 |
| Medium | overcast | 0.41–0.43 | 1.70–2.17 |
| Medium | storm | 0.32–0.35 | 1.28–1.99 |
| Medium | fog | 0.35–0.50 | 2.00–2.34 |
| Ultra | noon (fair) | 1.92–1.97 | 2.37–2.49 |
| Ultra | overcast | 0.80–1.40 | 2.92–2.94 |
| Ultra | storm (with rain shafts) | 0.71–0.75 | 3.92–5.63 |
| Ultra | fog | 0.77–1.18 | 3.08–3.59 |

Fair weather costs what it did. A deck, a storm or fog bank costs about what fair weather's clouds do from the air: takram's
decks were cheap because a ray stopped at their opaque base, where the weather's let the eye into their structure, and fog
and rain are marched from the camera. That is 1–1.8 ms more than master's overcast and storm on Medium, over the lane's
0.3 ms budget for those weathers; the rain shafts, the dearest single part (about 0.5 ms), are drawn from High up only.
Levers left: a lower march resolution for the band, fewer octaves in slabs, the deck's base less ragged.

### Weather changes

`GameSky::blend(from, to, t)` draws the weather `t` of the way between two scenes: the clouds blend the two weathers'
layers on every call and keep every history (the march's reconstruction, the shadow maps and the ground map follow the
clouds as they follow drifting ones), the rain and lightning follow, and the sky's light (the sun's strength, the
environment's bake) takes the blended scene in steps of 1/48 of the change. The air is `from`'s until halfway and `to`'s
after (`SkyScene::lerp`), so its tables (77 ms of GPU under load, `docs/sky.md`) are computed once, not for every step.

naval-client's `weather::change(scene, queue, seconds)` starts a drift from the look on screen (the sky, the fog every
material takes, the water's light, the meshes' ambient: `weather::Look`) to the weather the scene's selection and
overrides resolve to, eased over `seconds` of the scene's clock (`weather::DRIFT_SECONDS`, four minutes, by default);
`weather::advance` moves it every frame (not on a paused one). The sea and its wind change at once, as before (the
simulation's sea answers a wind over its own seconds). The harness's `weather` command changes a battle's weather this way
(`docs/harness.md`); the developer console still applies its overrides at once (`weather::apply`).

### The sun on a battle's clock

A battle's sun moves with its clock, as the theatre's does with the theatre's: `weather::follow_clock` puts the sun of
`BattleClock::sun_at(seconds)` on the scene once it has moved 0.25° (about a minute), re-resolving the light as the
theatre's `relight` does (the sky and its bake, the clouds' light, the fog's colour, the water's light, the meshes'
ambient, the moon with the sun). The day is `battleEnvironment`'s (`catalog::sun_at_hour`: up at 6, 70° at noon, down at
18, 15° an hour round the sky); a battle at an hour starts there, and a time preset's or a map's own sun starts on the
arc through it (`catalog::hour_of_sun`), keeping its azimuth's offset from the day's. A battle at 16:00 sees the sun set
two hours in. The console's hour, while set, holds the sun; the reading shows the hour.

## Near detail (2026-10-04, the flight-grade clouds' stage 3)

Bill's brief (2026-10-04): "we have planes in the air, we basically want clouds as good as those in flight sims". The
problem this stage takes: a cloud within about a kilometre is a soft blob with no fine structure even when the picture
has settled, because the finest data was takram's 32³ detail volume (5.2 m texels, features of 20 to 40 m).

- **The detail noise** (`src/detail_noise.rs`, `shaders/detail_noise.wgsl`) is one 128³ four-channel volume with five
  mips, tiling every 100 m (0.78 m texels): low- and high-frequency wisps in r and g, low- and high-frequency billows in
  b and a. Every channel is layered "alligator" noise, written here from the kind's published description (a cellular
  noise: each lattice cell holds one bump of a random height at a random place, and the noise is the tallest bump at
  a point less the second tallest, so bumps stand rounded with creases between them; Guerrilla's Nubis³ names it for
  billows where inverted Worley noise reads as packed spheres). The wisps' lookups are displaced by curl noise first,
  which swirls the creases into filaments. Stored bumps bright: the field carves the bumps away for wisps (the web of
  creases stays) and the creases away for billows (the bumps stay). It is filled on the GPU at startup (49 ms on the
  M5 Pro, once; 9.6 MB of GPU memory), on a lattice hashed with integers, so every GPU places the same bumps: the
  texels differ between GPUs by float rounding alone (`reference_texel` is the same noise on the CPU; a test holds the
  fill to it, within a level of 255). No asset is shipped; takram's `shape_detail.png` is gone. Guerrilla's own noise
  pack carries no licence and was not used or read.
- **The up-rez** (`field.wgsl`, "The detail"; after Nubis³): the frequency by the depth into the cloud (the low
  frequency at the edge, more of the high toward the core: the direction the Nubis³ slides print; the other way round,
  which the speaker's photographs suggest, the edge is all fine grain here and no lobe shows), wisps at a layer's bottom
  and billows at its top (takram's rule), and the mip chosen from the march's step and the pixel (`detail_lod`: a texel
  no finer than an eighth of the step, which a pixel's 8 to 16 jittered samples resolve, nor than half a pixel), never
  finer than mip 3.
- **One field.** Every march reads the noise at mip 3 or coarser (6.25 m texels, about takram's), carved to takram's
  statistics (its wisps carved 0.17 of the edge on average and its billows 0.29, with its spread), so the clouds keep
  their size, their soft edge, their noise and their cost. A paused or held picture reads the same field as play and
  converges to the picture play drew the moment before (`gpu.rs`, `a_paused_picture_from_the_air_is_plays`).
- **The near field, retired 2026-10-08.** Until then a picture that stood (held, or paused) over a camera in the air
  read a second field within 600 m of it (fading out by 1,200 m), walked in steps of 0.35 of optical depth, 8 m at the
  least: the finer mips, the carving at 1.6 contrast (0.47 in the far field), a twice-folded copy of the fetch inside
  150 m, the edge sharpened by a power of 0.3 to 0.6, the inside uneven (the low-frequency wisps taking up to 0.6 of
  the density anywhere) and the density thinned around the camera. Play never read it: its 50 m steps cannot resolve
  it (short steps cost 1.6 ms more among cumulus on Medium), and the near pass meant to carry it into play (stage 4,
  draft #976) was shelved. Bill saw the result: pausing on an aircraft turned the cumulus beside it from play's soft
  cloud into a mottled web of dark creases ("the clouds start looking horrible"), and play brought the soft cloud
  back. A paused frame and the frame before it differed by 14 levels on average over a cumulus 350 m below the camera;
  they now differ by under 1 (the samples' own noise).

**The trap: the texture cache.** Reading mip 2 of the four-channel volume (32³ × 4 bytes, 131 KB) in every media
sample cost the march half as much again at sea level (1.16 → 1.77 to 2.1 ms at noon, Medium), where takram's 32³
single channel (32 KB) stays in cache; mip 3 (16 KB) costs 0.07 ms. Hence the 100 m tile (mip 3 holds 6.25 m texels,
about takram's) and the floor at mip 3 on every tier (without it Ultra's 10 m steps read mips 0 and 1 from
everywhere: +0.7 ms at noon from the sea, +2.0 to +3.5 ms in the air).

**Measured** (2026-10-04, Apple M5 Pro, Medium clouds, the High preset; before is origin/master d7f721e16, which read
takram's 32³ volume): the sea-level views are not noisier and cost 0.07 ms more of march (1.15 → 1.22 ms at noon,
1.28 → 1.35 at sunset, 1.91 → 2.04 under an overcast; 2400 × 1350); High +0.05 to +0.10, Ultra +0.0 to +0.2, Low
unchanged (it reads no detail). The air views cost the same 0.05 to 0.09 ms more. From 2,600 m the settled picture
loses 6% of its fine detail (mip 3's 6.25 m texels against takram's 5.2 m) and a tenth of its noise. The inside of a
deck is one flat grey: the light there (`deck_skylight`) is the same at every sample, so no density structure shows,
whatever it is; it needs light that varies inside the deck, not more density detail.

**Open.** Fine structure in near clouds, in play and paused alike: it needs short steps near the camera every frame
(the near pass, with a short history and a light volume), not a field only a standing picture reads. A step back to a
cloud's edge on the first sample of it. The run-time turbulence on High and Ultra was not retired: it displaces the
shape (30 to 300 m) at the cloud bases, a scale the baked curl in the wisps (5 to 40 m) does not reach. The 100 m tile
repeats along a long grazing view of a deck's top no more than takram's 167 m did; not seen in the views checked.

## Quality tiers

takram's presets, onto the tiers of the same names (Graphics → Clouds), less three trims (below). The march renders a
quarter of the drawing buffer each axis and reconstructs the rest over 16 updates on every tier.

| Tier | Light shafts | Shape detail | Turbulence | Sun / sky light | Primary march | Shadow maps | Ground map |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Low | no | no (and no detail noise read) | no | interpolated | 200 steps of 100 m+, 100 km, ends at 10% transmittance, 1 sun step | 2 × 256², 25 steps | 512² |
| Medium | no | yes | no | interpolated | 500 steps of 50 m+, 200 km, 2 sun steps | 3 × 256², 50 steps | 1024² |
| High | yes, from 200 m steps | yes | yes | accurate | 500 steps of 50 m+, 200 km, 2 sun steps | 3 × 512², 50 steps | 1024² |
| Ultra | as High | yes | yes | accurate | as High, from 10 m steps | 3 × 768², 50 steps | 1024² |

**The trims** (2026-10-02), each held against takram's settings on six views at High and Ultra (the bridge at noon and
toward a dusk sun, 2.6 km at noon and at dusk, a dive, the chart), none of them visible side by side:

- **The light shafts' march starts from 200 m steps** (takram's 50 m). Its steps still grow 1% each to the same 200 km,
  so the longest ray takes about 240 steps instead of 370. Capping it at 128 steps instead stops it near 51 km and
  lightens a strip along the dusk horizon.
- **No step toward the ground** (takram marches three for the sea's light on the cloud bases; from 2026-10-02 High and
  Ultra drew no bounce, their bases a few levels darker, and Medium marched one step). Since 2026-10-04 every tier takes
  the bounce through the cloud its weather puts under a sample ("The light of broken cloud"), and three octaves of
  multiple scattering in broken cloud where takram's presets have eight.
- **Ultra's shadow maps at 768²** (takram's 1024²): a dusk sun's shafts over a low bank keep their number and length,
  slightly softer. At 512², High's size, most of them go.

## How it looks

The clouds were checked from the bridge (morning, noon, evening toward and away from the sun, looking up), from
aircraft (900 m inside the low layer, 1.35 km among the clouds, 2.6 km over the deck toward and away from the sun, 5 km,
9 km over the high layer, a dive) and the chart, in the stills and in the real client
(`bun native/tools/shot.ts`, the views of the lead's capture script). Measured on noon stills (Ultra): sunlit tops
p99 about 223 of 255 through the game's display, the zenith from 2.6 km about 0.16 in linear radiance, the horizon
about 0.42.

- **Grain** (2026-10-03, the real client: a custom battle on the manual clock, the camera set, then four seconds of
  play before the capture unless it says otherwise; Medium clouds, FXAA, 1600 × 900). The neighbour difference (mean
  absolute difference of adjacent pixels' luma, 8-bit levels, over each view's cloud region), master → now: noon chase
  2.16 → 0.99 (TAA 1.62 → 0.83); toward a dusk sun 1.29 → 0.45; binoculars ×12 1.92 → 0.35 and ×24 1.93 → 0.31, ×24
  pitching 0.4° as a ship does 0.96 → 0.38 (its 4 × 4 lattice 0.30 → 0.09); 2.6 km over an overcast deck 2.13 → 0.59;
  among cumulus at 2 km 2.16 → 0.96; the chase on a Fletcher at 32 kn 2.60 → 0.92 (TAA 1.72 → 0.71); low tier 2.09 →
  0.84, high 2.11 → 1.11, ultra 1.93 → 1.07 (crisper clouds, not grain). The clouds marched at the same instant at 32
  golden-ratio jitters and averaged (the truth the measure aims at) give 0.29–0.93 on those views, and the results now
  lie on it (dusk 0.45 against 0.46, overcast 0.59 against 0.57, binoculars 0.31 against 0.29, cumulus 0.96 against
  0.93), 1.0–2.2 levels from it (RMSE) against master's 1.3–2.7.
- **A capture taken the frame its camera arrives** (shot.ts with `--world-eye`) shows the full march, one sample a
  pixel, through the spatial filter: neighbour difference 0.44–0.95 (master 1.95–2.81), 0.7–1.5 levels from the
  32-jitter clouds (master 1.7–2.9). A capture from a camera held through the stepped seconds (the chase camera,
  `--seconds 5`) shows the reconstruction converged in motion.
- **Moving cameras and hulls**: no lattice and no blocks, binoculars pitching included; a hull steaming across the
  clouds leaves no pale trail behind its masts and funnels (master's stood until the pixels' cells came round).
- **A view that turns** (2026-10-04; "Measuring a moving view" below). The march reaches a pixel once in 16 updates,
  and a view turning at a mouse's pace carries the sky across the screen in a second or two: a second into a turn of
  45° a second (13 px a frame at 1600 × 900) 13% of the sky's pixels held no sample of their own, 18% one, 25% two or
  three and 27% eight; at 90° a second half held one or none and none held eight. Those pixels were the grain while
  panning: the ones without a sample a blotch of the march buffer that changed every frame, the ones with one to
  three their samples' noise, which the composite's filter only partly hides. With the prior, on the three frames
  after a second of turning (Medium clouds, 1600 × 900, levels of 255 over the clouds), master → now: the change from
  one frame to the next once the turn is taken out (each frame warped onto the one before) 0.81 → 0.64 at 45° a second
  and 1.19 → 0.76 at 90° under temporal AA, 1.01 → 0.69 and 1.38 → 0.79 under FXAA (the converged clouds warped the
  same way: 0.82–0.94, most of it the warp's own); the frames' error against the converged clouds 2.50 → 2.45 and
  2.53 → 2.41 (TAA), 2.50 → 2.43 and 2.53 → 2.38 (FXAA). A camera held still, the chase at 32 kn, binoculars pitching
  and a flight at 120 m/s measure as before (within 0.05). What a turn still costs is detail, not grain: the frames
  hold 0.35–0.39 of the converged clouds' fine detail (against the mean of four truths; see "Open problems").
- **A camera travelling near the clouds** (2026-10-05). Bill: the clouds turn "super super grainy" while the camera
  moves "super close" to them. A turning view measures clean (its sky keeps eight samples a pixel and its clouds are
  far); a camera travelling a few hundred metres from a cumulus does not: flying 100 m/s past one 300 m off, its pixels
  held one to three samples of their own and showed a snow of single samples over the cloud and a torn, sparkling
  fringe against the sky, where the clouds converged at the same place (and the march of that very update) show a soft
  fringe. A near cloud is a volume: as the camera travels its near part passes its far part, and its fringe passes the
  sky behind it, so no one motion carries a pixel's history. The march's own noise is not it (one sample a pixel
  against the converged clouds: 0.8–1.0 levels a few pixels across, near the cloud as from the sea), nor the scene's
  temporal AA (FXAA and no AA show the same), nor the history's rejection (keeping every clipped history changes
  nothing), nor noisy motion (a 5 × 5 Gaussian of the march's motion changes nothing). Three changes, together:
  - the march walks a cloud within 800 m of the camera in steps of 20 m (takram's 50 m and more), growing back by
    1.5 km (`march.wgsl`, `NEAR_STEP`; slabs keep their optical-depth steps). 8 and 12.5 m measured the same;
  - a pixel whose march neighbourhood moves 0.25–1.5 px an update against the view's turn alone (the parallax a
    travelling camera gives near cloud; a turn, a ship's camera and the wind's drift stay under it) takes a quarter of
    the march about it each update through a Gaussian of 1.25 march texels, and its mean keeps two samples of its own
    (`resolve.wgsl`, the sliding view): the near cloud is drawn from the march as it stands, softer and steady;
  - the composite's filter for young pixels is 2.5 px wide at one sample (was 1 px), weighs each neighbour by its
    samples and takes the prior-only pixels too, within 5 × 5 (`composite.wgsl`).

  Measured (Medium, the High preset's 1120 × 630 under TAA at 1600 × 900, partly cloudy noon, no wind; frames against
  the clouds converged at the same pose; grain = the frames' energy between 1–3 and 5–11 px over the converged
  clouds'), master → now: flying past at 100 m/s, error 2.97 → 1.87 levels, grain 1.62 → 1.13; orbiting (60 m/s and
  20°/s) 1.81 → 1.05, 1.13 → 0.71; flying 150 m/s with the cloud 1.5 km off 1.74 → 1.54, 1.00 → 0.69; a 60°/s turn
  from the bridge 0.66 → 0.70, 0.77 → 0.70 (a touch softer). A still view from the bridge is unchanged (0.01 levels); a
  still view 300 m from a cumulus in a 9 m/s wind takes the sliding view a little (the drift is parallax there) and
  its fringe comes nearer the converged clouds (error 1.99 → 1.74). Cost (clouds' passes, M5 Pro, medians, the same
  views): among cumulus 1.02 → 1.26 ms plus 0.05–0.1 ms in the composite; at sea 0.86–0.94 → 0.89–0.95 ms, plus up to
  0.15 ms in the composite while a view's pixels are young. 7 × 7 for the filter measured a little smoother for
  0.2 ms more. What a travelling view still shows: a soft, slightly dappled fringe where the converged clouds show an
  even haze.
- **Measuring near the clouds** (the 2026-10-05 method; scripts in that session's `.build/native/cloud-grain`, not
  kept): each frame drawn by `capture` alone after `set_camera {cut: false}` (one draw a frame, the clock standing so
  the clouds stand: a `step` then a capture draws every frame twice); the truth the same pose cut and held 300 frames.
  Take battles with `wind: 0`: with wind the clouds' drift in the seconds before the truth moves a near cloud, and the
  in-play picture and its truth differ for that alone. Judge by eye on crops as well: the snow sits in a few percent of
  a frame and whole-frame measures dilute it. The march buffer as it stands (`resolve.wgsl` storing the march texel),
  the sample counts drawn as colours and the parallax weight as a colour were the views that found the cause.
- **Measuring a moving view** (the 2026-10-04 method; the scripts stayed in that session's `.build/native/cloud-pan`).
  A custom battle on the manual clock; a camera at a fixed eye turned by `set_camera` with `cut: false` each frame
  (temporal AA keeps its history; no translation, so a frame warps exactly onto the next), or flown, or the chase
  camera turned by `helm_input`'s `look`. The truth of a frame is the same view held from a cut with time all but
  standing (`step` with `dt` 1e-6, 300 frames: the clouds and temporal AA converge as in play, 16 samples a pixel);
  turn the camera away for a frame first, or a quarter of the turn's history stays in it. One truth still carries
  about 0.4–0.65 levels of fine noise: for the detail a frame keeps, take the mean of four truths 512 frames apart
  (against one truth it reads about a tenth low). And take the truth at the frame's own instant: capture one frame and
  hold from there. A truth taken even a frame later has near clouds a fraction of a pixel on (the wind's drift, the
  flight's own travel), and the detail kept reads far too low (a flight's 0.42 read 0.29, its 0.34 read 0.08, from
  truths one frame late; seven frames late it reads under 0.1).
  A shader edited on disk replaces the embedded one a moment after the client's first frames and drops the clouds'
  history then: step a few frames and wait two seconds before anything is measured.
- **Edges** (contrast at the 32-jitter clouds' strongest edges, each image first blurred 1 px so noise does not count):
  0.79–0.92 of theirs where the camera was held four seconds (master 0.92–1.09, its grain included), the time the
  history spans as the clouds drift and evolve (below, "Open problems").
- **Overcast and storm from below** were a flat grey sheet in takram's layers (coverage 0.82 and 0.94): its coverage
  filter gives the low layer a density of at least 0.58 at its base at 0.82, its shape and detail erosion only carve
  where the density is under 1, and its sky light lights every point under the deck as if the whole clear sky stood
  over it. Since 2026-10-03 those weathers draw decks of their own with the slab light ("Weather"): rolls, thick and thin
  patches and gaps from below.
- **Night**: lit by the moon since 2026-10-03 ("Night: the moon and lightning"): from the bridge, toward the moon, its
  cumulus show their silver edges and moonlit tops over the dark sea, away from it grey shapes against the dark blue;
  from 2.6 km the tops are grey-white under the stars. Before, the sky's night floor alone lit them, dark shapes.

## GPU cost

`cargo run -p naval-clouds --example timings --release`: five scenes (noon from the bridge; sunset toward a 6° sun;
overcast overhead at coverage 0.82; 2.6 km over the deck; a dive) per tier at 2400 × 1350 (1600 × 900 at the game's
1.5 pixel ratio), live clouds (not held). Compute passes by GPU timestamps through the renderer core's timer, medians
over 120 frames; the clouds' total is the five passes of a frame together; "after a cut" is the full march of the
first update after a cut on a frame that leaves time standing (a capture's); "sky, whole" is the whole sky's own work
(atmosphere, clouds, environment, dome, composite, weather) by wall clock, frames with it alternating with frames
without, serialised to GPU idle.

Apple M5 Pro (Metal), 2026-10-02, load average 8–10 over the runs, with the trims (above) and the shadow maps marching
one cascade an update (below): milliseconds per frame, medians, the lower of two runs. In brackets, the clouds' total
with takram's settings instead of the trims, from runs alternating with them.

| Tier | Scene | Shadow maps | Their temporal pass | Ground map | March | Reconstruction | Clouds, total (before) | After a cut (march) | Sky, whole (wall) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Low | noon | 0.16 | 0.03 | 0.02 | 0.51 | 0.45 | 1.16 (1.18) | 4.9 | 1.5 |
| Low | sunset | 0.22 | 0.03 | 0.02 | 0.60 | 0.45 | 1.31 (1.31) | 5.6 | 1.8 |
| Low | overcast | 0.12 | 0.03 | 0.02 | 0.21 | 0.45 | 0.82 (0.84) | 2.4 | 1.2 |
| Low | air | 0.16 | 0.03 | 0.02 | 0.67 | 0.46 | 1.34 (1.36) | 7.3 | 1.8 |
| Low | dive | 0.17 | 0.03 | 0.02 | 0.41 | 0.46 | 1.08 (1.09) | 4.8 | 1.5 |
| Medium | noon | 0.14 | 0.03 | 0.02 | 0.98 | 0.45 | 1.63 (1.64) | 9.0 | 2.0 |
| Medium | sunset | 0.29 | 0.02 | 0.02 | 1.15 | 0.45 | 1.97 (1.95) | 10.7 | 2.6 |
| Medium | overcast | 0.11 | 0.03 | 0.03 | 0.39 | 0.45 | 1.02 (1.02) | 5.4 | 1.4 |
| Medium | air | 0.14 | 0.03 | 0.02 | 1.31 | 0.46 | 1.97 (1.97) | 13.8 | 2.4 |
| Medium | dive | 0.14 | 0.03 | 0.02 | 0.74 | 0.46 | 1.39 (1.40) | 9.0 | 1.8 |
| High | noon | 0.50 | 0.07 | 0.03 | 1.52 | 0.45 | 2.61 (3.56) | 14.4 | 3.2 |
| High | sunset | 1.15 | 0.08 | 0.03 | 1.71 | 0.46 | 3.50 (4.46) | 15.7 | 4.2 |
| High | overcast | 0.41 | 0.08 | 0.05 | 0.53 | 0.46 | 1.59 (2.02) | 7.2 | 2.2 |
| High | air | 0.50 | 0.08 | 0.03 | 1.87 | 0.47 | 3.14 (3.69) | 18.7 | 3.6 |
| High | dive | 0.50 | 0.07 | 0.03 | 0.98 | 0.46 | 2.24 (2.81) | 12.4 | 2.7 |
| Ultra | noon | 0.85 | 0.16 | 0.04 | 1.90 | 0.46 | 3.59 (5.02) | 18.2 | 4.2 |
| Ultra | sunset | 2.33 | 0.17 | 0.04 | 2.16 | 0.46 | 5.26 (7.75) | 20.7 | 6.0 |
| Ultra | overcast | 0.69 | 0.17 | 0.06 | 0.74 | 0.46 | 2.23 (3.13) | 10.9 | 3.1 |
| Ultra | air | 0.87 | 0.17 | 0.04 | 3.05 | 0.47 | 4.90 (5.98) | 30.6 | 5.3 |
| Ultra | dive | 0.84 | 0.16 | 0.04 | 1.68 | 0.46 | 3.40 (4.74) | 20.2 | 4.0 |

**One cascade an update.** The shadow maps march one cascade an update, round robin, and their temporal pass carries the
other cascades' history over unchanged. Each cascade is sampled with the matrix it was marched with
(`drawn_shadow_matrices`), so a cascade one or two updates old stays aligned with its contents. Every cascade is marched
after a cut or a fresh start (the round robin restarts with every new history, so held captures repeat), and so is any
cascade the view has left: `Cascade::escapes` takes the cascade's reach (its slice of the view widened by the fade
bands, the last one out to the marches' longest ray, since they sample it past the camera's far plane), its outline
along the light clipped to the freshly fitted square, and asks whether the square it was marched with holds all of it.
takram's temporal pass already blends a march in at 1%, so the maps look the same (the review captures match to 0.03 of
a level) for about a third of the march. In alternating runs of master and the round robin (2026-10-02, load average
19–28, the one-ship battle at 2400 × 1350) the clouds took 4.1–4.2 → 3.2–3.6 ms a frame at High and 8.0–8.3 → 4.4–4.5 ms
at Ultra.

The march is most of the cost; from High up the shadow maps come next and grow as the sun lowers (longer rays through
the layers), to about 45% of an Ultra sunset. The trims take the clouds from 3.3 to 2.6 ms a frame on average at High
and from 5.3 to 3.9 ms at Ultra (the sunset 7.8 → 5.3 ms), and the first full march after a cut by a quarter to nearly
half. A capture after a cut costs one full march once; a cut in play costs nothing extra.

**The grain rework** (2026-10-03, the reconstruction and spatial filter above). The same example (which now also
gives the tenth percentile of the march, the reconstruction and the scene pass holding the sky phase, and that pass on
the cut frames), master's crate and the branch's built side by side and run alternately, two rounds each, the lower
round's tenth percentile, Apple M5 Pro, load average 8–29 (milliseconds per frame at 2400 × 1350):

| Tier | Scene | March | Reconstruction | Clouds' compute | Scene pass with the sky (dome, composite) | That pass on a cut frame |
| --- | --- | --- | --- | --- | --- | --- |
| Low | noon | 0.50 → 0.49 | 1.33 → 0.77 | 1.99 → 1.43 | 2.20 → 1.75 | 5.58 → 6.31 |
| Low | sunset | 0.60 → 0.60 | 1.37 → 0.80 | 2.20 → 1.63 | 2.57 → 2.15 | 6.48 → 7.66 |
| Medium | noon | 0.96 → 0.96 | 1.33 → 0.78 | 2.49 → 1.95 | 2.60 → 2.18 | 10.00 → 11.04 |
| Medium | sunset | 1.15 → 1.11 | 1.36 → 0.78 | 2.87 → 2.23 | 3.18 → 2.61 | 12.29 → 12.94 |
| Medium | overcast | 0.41 → 0.39 | 1.38 → 0.79 | 1.94 → 1.33 | 2.10 → 1.87 | 6.29 → 7.11 |
| Medium | air | 1.37 → 1.30 | 1.40 → 0.79 | 3.04 → 2.27 | 3.19 → 2.52 | 15.78 → 15.68 |
| Medium | dive | 0.76 → 0.76 | 1.36 → 0.80 | 2.31 → 1.76 | 2.47 → 2.08 | 9.86 → 11.18 |
| High | noon | 1.45 → 1.45 | 1.34 → 0.78 | 3.29 → 2.72 | 3.59 → 3.14 | 17.01 → 17.62 |
| High | sunset | 1.70 → 1.68 | 1.39 → 0.80 | 4.25 → 3.63 | 4.61 → 4.32 | 20.98 → 21.77 |
| Ultra | noon | 1.86 → 1.86 | 1.34 → 0.80 | 4.06 → 3.52 | 4.34 → 4.11 | 25.43 → 27.84 |
| Ultra | sunset | 2.20 → 2.13 | 1.47 → 0.81 | 6.26 → 4.89 | 6.74 → 5.72 | 33.24 → 30.26 |

The march is unchanged. The reconstruction, which master's silhouette work (2026-10-03) had taken from 0.46 ms to
about 1.35 ms, now costs about 0.8 ms on every tier: one 3 × 3 read of the march gives the sample, the motion and the
clipping box, where master read it again for the nearest fragment and sampled the march another eight times to clip.
In play the composite adds one read a pixel, and the scene pass holding it measures no dearer (its span takes in
waiting on the reconstruction). On a cut frame, where the filter runs 5 × 5 over every pixel, that pass costs about
1 ms more (a frame that also marches every pixel, 9–30 ms); in play after a cut the filter runs, 5 × 5 and then 3 × 3,
on the pixels still gathering their first eight samples, two seconds or so.

## Open problems and follow-ups

- **Cost**: Ultra spends 2.2–5.3 ms of GPU on the clouds at 2400 × 1350, High 1.6–3.5 ms, Medium 1.0–2.0 ms, Low
  0.8–1.3 ms (table above), the worst at sunset. Every further lever measured shows (each against takram's settings,
  in interleaved runs): Ultra's maps at 512², High's size, lose most dusk shafts over a low bank (0.6–1.7 ms more at
  Ultra); turbulence off changes the clouds' shapes (about 1–1.5 ms); Ultra's march from High's 50 m steps gives hard
  edges from the air (0.8 ms); a 32-step shadow march changes the dusk light (0.5 ms at Ultra). Five multiple-scattering
  octaves and takram's interpolated sun and sky light save nothing measurable. The reconstruction is a fixed 0.8 ms a
  frame on every tier.
- **In play a cut fills in over 16 frames** (the march about each pixel at first, the prior): a camera switch shows a
  soft picture for a moment before it sharpens, and then reaches its full sample count over two to four seconds (the
  prior and the spatial filter hold the grain off meanwhile; a fifth of the prior is still in the mean at eight
  samples, two seconds in).
- **The march's own noise is what every view pays for** (measured 2026-10-04, Medium, 1600 × 900, the fine detail of
  the picture, levels of 255). One sample of a pixel is 2–2.5 levels from the converged clouds from the sea and about
  3.5 from 2.6 km looking down; the converged clouds (16 samples) keep 0.41 and 0.63 of it, beside 2.0 and 1.35 of
  fine detail. It is undersampling along the ray (steps of 50 m and more through a medium whose detail is 20–40 m
  across), not the jitter's use: a jitter of one step rather than two, the sun's short march jittered per sample
  rather than per ray, steps cut to a fixed optical depth in dense cloud and a ray stepping back to find the cloud's
  edge (three halvings) each changed the noise by under a tenth (the last makes edges crisper, the converged detail
  1.35 → 1.58 from above, for 0.6 ms of march). Steps half as long cut the converged noise by a third from above
  (0.63 → 0.43) and an eighth from the sea, for the march's cost again (1.2 → 2.0 ms at noon from the sea, 1.6 → 3.2 ms
  from the air, 2400 × 1350).
- **A camera held still in play is softer than the same view converged**, from above most: the wind's drift moves
  every pixel a few hundredths of a pixel an update, so its history is resampled every update and holds 8 samples,
  and the picture lags the clouds (from 2.6 km it keeps 0.65 of the converged clouds' fine detail, 0.90 with no
  wind; from the sea 0.97; under temporal AA 0.68 and 0.94). Tried and left: a history carried in whole pixels with each pixel's sub-pixel remainder
  kept (never resampled) keeps more detail (0.74) but shows the samples' own noise (1.07 → 1.88 levels), which the
  resampling had been averaging over neighbours, and a longer history to pay for it (16 or 32 samples) lags the
  clouds' drift and evolution (the error a few pixels across 1.6 → 2.5 → 3.7).
- **A flight loses the clouds' detail** (120 m/s at 1.2–3 km, the same measure, temporal AA, against the mean of
  four truths): the frames keep 0.42 of the converged clouds' fine detail looking down from 2.6 km at noon, 0.34 at
  dusk toward the sun, 0.39 over a storm's deck and 0.61 of the little a cumulus 300 m ahead has, though nearly every
  pixel holds its eight samples: near clouds move against far ones along a ray, the history is one picture, and it is
  resampled every update. The prior neither helps nor hurts there. More samples an update (a march of more than one pixel in 16 while the camera
  travels) or a march that needs fewer are what would. Since 2026-10-05 a near cloud the camera travels past is drawn
  from the march as it stands where its parallax is large ("A camera travelling near the clouds"): steadier and
  softer, not sharper.
- **A long history follows the clouds' drift but not their evolving shapes** (takram shifts the shape texture
  vertically as the weather moves, 2.4 m/s in a 12 m/s wind) or the Beer shadow maps' own lag (one cascade an update,
  blended in at 1%): a view the camera held still for seconds is a little softer at the cloud edges than the clouds
  marched at that instant (measured at 0.8–0.9 of their edge contrast), where master's grain read sharper.
  The theatre's cruise scene at 8× (`GameSky::time_scale`) is softer still: after 6 s in play its edge contrast is 0.66
  of a converged frame of the same moment, against 0.82 at 1× (High, scattered cumulus at noon, 2026-10-03). A 2-sample
  history while the clouds drift (in place of `MOST_MOVED_SAMPLES` 8) only took it to 0.71, so it was left as it is.
- **Weather that changes the clouds while the history runs** (coverage or layers drifting over seconds rather than
  cut): a pixel's mean follows within its 8 or 16 samples, faster where the change clips it.
- ~~**The air under a deck is lit as if the sun reached it**~~: since the haze lane (`haze.md`) the air under the
  clouds that cast the ground shadow takes their shade in the aerial-perspective volume, the sky's light under them in
  its place, so the band under a closed deck is gone from the dome, the clouds' front and every material. (Was: from
  sea level under a storm a bright band stood at the horizon, between the deck's far underside and the sea.)
- ~~**Fog banks and the uniform fog**~~: since the haze lane (`haze.md`) the march takes the air in front of the clouds'
  front from the aerial-perspective volume (`aerial_along(uv, front_depth)`), the same air the sea and ships take, so a
  bank sits inside the uniform haze. (Was: takram's aerial perspective in front of the banks, the weather's ramp on the
  sea and ships, so a bank 3 km off was crisper than the sea behind it.)
- **Rain shafts** read at a distance as dark grey curtains under the deck, which near the horizon look like low land.
- **Theatre weather changes** are new scenes (`TheatreDriver::cruise_changed`), not drifts: the new scene's clouds start
  at its own weather. Carrying the old scene's look into the new one's drift would smooth them.
- **One key light**: the march follows one body, so through the switch (2.5° after sunset) the low clouds are lit by
  neither beam (the sun's left them at about 1.4° down) and only by the twilight sky; a second body in the march would
  need a second set of shadow maps. The switch itself is continuous (the beams are equal there).
- **Lightning in the quarter-resolution march**: a flash's light on the clouds is the march buffer's, upsampled by the
  reconstruction's clipping box, so its edges are a quarter-resolution picture for the moment it lasts; at a still
  capture the full march shows it whole.
- **Light shafts over the sea and ships**: the shafts' length could darken the game's fog along the shadowed part of a
  ray, as takram's aerial perspective does.
- **The weather texture tiles** every 100 km: a cover pattern repeats at that period from the chart's height.

## Re-sync

The clouds no longer follow the web's `src/game/sky/clouds` (they are takram's); the weather (`src/game/sky/weather`)
still does, and a re-sync diffs only that.

## Needs from naval-sim

Nothing: the clouds and weather are presentation only.
