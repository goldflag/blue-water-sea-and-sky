# Lighting lane (batch 4)

Closes the lighting gaps the full-feature gate-B rerun found between the web frame and the native one
(`docs/archive/native-client-slice-log.md`, "Like for like"): fog on ships, aircraft and effects; the effects' point
lights on hulls, airframes and land; reflections of hulls and land in the sea; scorch on hulls. Crates touched:
`naval-render` (a shared module, no pass), `naval-ships`, `naval-aircraft`, `naval-terrain`, `naval-ocean`,
`naval-effects` (a light list and its fog). The slice (`naval-client`, the slice-4 lane's) is not changed here; its
wiring is under "Wiring for the slice". Outputs go to `.build/native/lighting/` and the effects lane's
`.build/native/combat-effects/`.

## Status (2026-09-30)

| Gap | State | Web source |
| --- | --- | --- |
| Fog on ships, aircraft, tracers, silhouettes, effects | Replaced on 2026-10-03 by one aerial perspective for everything (`haze.md`) | `src/game/ocean/screen/fog.ts` (`scene.fogNode`, which three applies to every material with `fog` on) |
| Effect point lights on hulls, airframes, land | Done | `CombatEffects.ts` (four flashes), `LocalizedFireEffects.ts` (two fire lights), `VisualEnvironment.ts` (`boltLight`); three r185 `PointLightNode`, `getDistanceAttenuation` |
| Point lights on the sea | Departs from the web since 2026-10-03: the sea takes them (see below) | `OceanSurfaceMaterial.ts` sets `fragmentNode`, which bypasses three's lighting |
| Screen-space reflections (hulls and land in the sea) | Done | `src/game/ocean/screen/reflections.ts`, `OceanSurfaceMaterial.ts`, `physicalReflection.ts` (`traced`, `TRACE_BELOW`) |
| Cloud shadows on the glitter | Already there (confirmed) | the glitter's `beam` in `surface.wgsl` takes `cloud_shadow` since batch 3 |
| Scorch on hulls | Done | `src/game/ShipScorch.ts`, `ShipMaterialPalette.ts` (`weather`) |
| Night mesh light shares | Departs from the web since 2026-10-02 (see "Night") | `VisualEnvironment.meshLightShares` (`MESH_MOONLIGHT` 1.5, `MESH_NIGHT_FILL` 1.6) |
| Day fill, cloud gain, shadow floor, eye adaptation | Departs from the web since 2026-10-03 (see "Realism") | `meshLightShares`, `EyeAdaptation.ts`, `CLOUD_SHADOW_FLOOR` |

## Night (2026-10-02)

After takram's sky and clouds came in (PR #829), ships glowed at night: a close Bismarck's side was 200–440 × the sea
beside it and as bright as at dusk, under a clear sky, broken cloud or a closed deck alike. Three things lit hulls (and
the land) that nothing else in the scene took, all tuned for the web's (and the old native sky's) brighter night, with
takram's night sky and sea about 4 × darker:

- **The moon at 1.5 ×** (`MESH_MOONLIGHT`): lit meshes now take the moon as it is, as the sea does
  (`ShipLighting::daylight`, `naval_terrain::catalog::mesh_light_shares`).
- **Cloud shadows stopped only the sun**: the clouds' ground shadow map was marched toward the sun and faded out as it
  set, so an overcast deck let the full moon onto every hull while the sea and sky stayed dark. It now marches toward
  the active body (`CloudUniforms::ground_light`: the moon once it outshines the sun), its strength from that body's
  elevation, so clouds shade moonlight on hulls, land and the sea's glitter.
- **A constant hemisphere fill after dark** (1.6 × the map's ambient, `MESH_NIGHT_FILL`), 5–25 × the night sky by
  itself: after dark there is none now (`ShipLighting::fill`, `mesh_light_shares`), a shaded side has only the
  environment's night sky, and a lightning flash still lifts the fill as the web's did (1.6 × the ambient × the flash).
  The web's fill ramp holds from an intensity of 1 up (dawn and dusk unchanged); below it the fill goes out with the
  sun's light, so a moonless night has none either and the switch to the moon does not jump. (Superseded on 2026-10-03:
  no fill by day either, see "Realism".)

Measured on a close Bismarck at the night preset (sun 28° down, full moon), 1600 × 900, the ship's linear luminance
against the sky beside it (`.build/native/night-ships/`, `shots.sh` and `sheet.py`; eye adaptation on and off read the
same, so it plays no part), before → the moon and its cloud shadow → no fill after dark: clear 0.076 → 0.056 → 0.025
(sky 0.006), broken cloud with the ship under a cloud 0.075 → 0.022 → 0.0015 (sky 0.001), overcast 0.070 → 0.018 →
0.0011 (sky 0.0012). Dusk and noon frames are identical before and after. Under a deck at night a hull is now a dark
shape against a dark sky, as at sea. Since 2026-10-06 the display grade below opens the world's shadows for gameplay.

## Night readability (2026-10-06)

Bill found the night world too dark to play while the sky could be darker. The display pass
(`naval-render/shaders/display_pass.wgsl`, `scene_radiance`) now lifts the world's shadows by at most 1.25 stops and
dims the sky's shadows by a quarter stop. Reversed scene depth distinguishes the sea, hulls, aircraft and land
(positive) from the sky and clouds (zero), so the same grade reaches every scene, including port and theatre. The
world takes the full lift within 1.5 km, fading smoothly to the sky's grade by 12 km: distant haze and the sea's horizon stay joined
to the sky, and the lift does not make distant shapes stand out through haze.

The grade fades in as the clear sky's mean luminance falls from −3 to −5.5 in log2, the same range in which the eye
stops opening. It uses the sky mean's clear-sky luminance in w, falling back to rgb when a host supplies no clear sky:
cloud cover and the camera's direction do not change the dusk fade. Without a sky mean it is disabled, even if a previous
scene left one in the buffer. The shadow adjustment fades out between scene luminances of 0.02 and 0.2, retaining
firelight, muzzle flashes and celestial highlights. It multiplies existing radiance: black stays black, and the
world retains its shading and haze. The physical light, cloud shadows, bloom and metering keep their existing values.

**Under water (2026-10-12).** A pixel seen through water takes the near world's grade whatever its depth. The open
deep has no depth, so it was graded as sky while the water above eye level, which ends on the sea's underside, was
graded as world: at night a camera under the sea showed a hard line at eye level, the deep about 1.5 stops darker
than the water over it. The display now reads the ocean's underwater probe (binding 7, `Renderer::set_underwater`,
which `naval-client` sets each frame while the ocean draws): where a pixel's ray starts under the waves the grade is
the near world's. It is per pixel, so with the waterline across the lens the sky above it keeps the sky's grade.
Above water nothing changes (pixel-identical). The sky through Snell's window is still graded as world, the sea's
surface being what the depth holds there.

**Smoke (2026-10-12).** Smoke, spray and fire write no depth, so the grade took them for whatever lay behind: a
burning ship's column was lifted where it crossed the sea or a hull and dimmed where it crossed the sky, a hard line
through it at the horizon and her upperworks a pale flat shape inside it. The scene now carries the transparents' cover: while
the grade is on, the renderer zeroes the scene's alpha as the `Transparent` phase starts (`transparent_cover.wgsl`, a
full-screen draw inside the scene pass; nothing drawn before reads that alpha), the transparents' own blending leaves
`a + A (1 − a)` there, and the display pass grades what they cover of a pixel as near world, whatever the depth behind
it. No copy and no extra pass. Checked on `effects-trial.ts --only underway-far --hours 21.5 --stills`.

**Overcast nights (2026-10-13).** An overcast night's sea toward the moon was 1.8 times a clear night's on screen
(22 against 12 in 8-bit), its environment 1.6 times what a converged bake gives. The cause was the clouds'
environment bake, which lit a deck from the middle of its long steps (`clouds.md`, "The environment": a deck is lit
at its base). The grade here did not change.

`naval-render/tests/post.rs` checks the actual GPU display against paired sky/world surfaces: opposite adjustments
at night, a smooth dusk transition, an overcast day unchanged, the same grade under night clouds, preserved black
and highlights, no adjustment with a missing sky mean, and distant surfaces joining the sky's grade.

Reviewed against HEAD's packed reference in the native client at 1280 × 720: Bismarck, seed 1941, wind 8 m/s, one
second of battle, broadside and chase views, clear, partly cloudy and overcast nights, plus clear dusk and noon
controls. Captures and the comparison are in `.build/native/night-readability/`. The noon views differ by 0.06–0.10
of an 8-bit level on average between the packed release reference and the checkout's development client.

## Realism (2026-10-03)

Bill asked for the most realistic light the scene can give. Five changes, each reviewed on the 20-view set
(`.build/native/takram/views.sh`: sea level at noon, morning, dusk and night in several weathers, aircraft at 0.9–9 km,
a dive, the chart), before and after:

- **One brightness scale.** The dome is graded `sky_grade::GAIN` (1.4) over the tables, and so was the clouds' skylight,
  but the sun's and moon's direct light was not: the sky stood 1.4 × over the sunlit world under it. The clouds made up
  for it with a gain of their own (`CLOUD_GAIN` 1.9, on their whole radiance, the bake's too, which washed the sea and
  lit ships' shaded sides with over-bright cloud). Now the sun and moon on the world (`SeaLevel`, so every lit mesh and
  the sea) and on the clouds (`direct_scale`) take the dome's gain, and the clouds take no gain of their own.
- **No hemisphere fill.** A mesh's ambient is the sky's environment (and the sea in it) alone, by day as by night: the
  web's fill (the map's ambient below an intensity of 4, which lifted dawn, dusk and overcast hulls) is gone. Only a
  lightning flash fills (`LIGHTNING_FILL` 1.6 × the ambient × the flash: `ShipLighting::fill`, `mesh_light_shares`).
  Ships, aircraft, land, and trees alike.
- **No cloud-shadow floor.** `CLOUD_SHADOW_FLOOR` is 0 (was 0.15): under a thick cloud the direct beam is gone, and the
  light there is the sky's, which the environment gives. Under overcast a ship is evenly lit, with no false
  directional shading from 15 % of the sun.
- **An eye that adapts both ways** (`eye_adaptation.wgsl`, `EyeAdaptationSettings`). The bake writes, in the atlas's
  alpha, the luminance each texel would have without clouds (prefiltered with the radiance), and the sky mean carries
  it in w. The eye opens against that clear sky: a view darker than a clear noon from the bridge (`reference` −0.37)
  opens by all it lacks (`strength` 1, at most 2 stops), so an overcast noon comes out near a clear one. Two limits keep
  it an eye's (2026-10-03, after a hull glowed over dark sea and ships glowed again at night): it never lifts the view's
  brightest tenth past `highlight` (1.5 stops over the clear sky's mean, a clear noon's highlights), so a sunlit hull
  filling a view of dark sea opens it 0.1–0.5 stop instead of a full stop or more; and it fades out as the clear sky
  darkens from day to night (`light_from` −3 to `dark_from` −5.5, log2 luminance), so dusk and night keep the fixed
  exposure (a dark night sea had opened it the full 3 stops). It closes against the view's own sky: only a view
  brighter than its sky (sunlit cloud tops from above) closes, by its excess (`close` 1, at most 1 stop), so looking at
  the sky keeps the fixed exposure. Since 2026-10-05 it also closes for glare by day (`glare` 1, see "Into the sun"). Hosts without the clear sky (the harness's sky mean, a stand-in)
  write w 0 and meter against the sky's own mean.
- **The sea mirrors every cloud.** With the clouds at their true brightness the reflection needs no correction (a
  bluer sea under cloud was tried and dropped: `.build/native/sea-wash/`).

## Into the sun (2026-10-05)

Bill found views into the sun washed out, with hulls and aircraft in the sun's path glowing as if emissive, and the
haze heavier than the visibility. Measured on a carrier group 420 m off, the sun 20–25° up, the North Atlantic's air at
46 km (Eastern Solomons' theatre visibility; `.build/native/sunglare/`):

- **The glow was the bloom.** The high pass takes a highlight's whole graded value and the five mips spread 0.9 of it
  up to a tenth of the view, so the glitter round a hull veiled her: bloom off, the same hull is crisp and detailed. The
  strength is 0.1 now (the web's 0.3, `render-core.md`); fires and their light on the plating look the same at 0.3, 0.1
  and 0.05.
- **The wash-out was the fixed exposure.** Each metered cell counts at most 0.5 stop over the sky's mean, and the eye
  closed only for a view brighter than its own sky, so into the sun it closed 0.2 stop and the aureole, the horizon and
  the glitter sat in AgX's shoulder. The eye now closes for glare by day: the brightest tenth's excess over `highlight`
  (1.5 stops over the clear sky's mean), at most `max_down` (1). The brightest tenth reads about 1 at a clear noon, 0.75
  under overcast, 1.5 over sunlit clouds from 2.6 km, 1.75 with a sunlit flight deck in a view away from the sun (0.25
  stop), and 2.25–3.25 into the sun (0.75–1 stop). A sunset with the sun in view closes too (about a stop): the sea
  darkens and the ships go to silhouettes, as a camera's would.
- **The haze matches the visibility.** The extinction is `SIGHTING_DEPTH / V` or the map's own air where that is
  thicker (the North Atlantic's 0.056 per km is a 43 km visibility at A's calibration, so a theatre's 46 km draws as
  43). A broadside Bismarck keeps 85–90 % of her pixels over 5 % contrast out to 20 km into and away from the sun. Her
  mean contrast on screen into the sun was half of hers away from it (0.13 against 0.31 at 20 km, 0.25 against 1.2 at
  6 km): the sky behind her in the display's shoulder, not the air. Closing for glare raises her mean contrast into the
  sun by 14–19 % at every range (away from the sun nothing changes).

Since 2026-10-12 the sun itself glares (`render-core.md`, "The sun's glare"): a veil about its disc by what the frame
shows of it, which the bloom adds. It is the sun's alone: the sea's glitter blooms as it did, at the strength of 0.1.

## Sun and sky at the sea (2026-10-09)

Carrier aircraft paint looked flat next to a photo of a Wildcat in flight. The sunlit paint stood about 1.5 × the sky
behind it, against 1.06 × in the photo, and the hinomaru went pink.

The readings below were taken at sea level in the North Atlantic's clear weather: turbidity 1.5, an aerosol optical
depth of 0.024 at 550 nm. Values are luminance in the sea's units with `sky_grade::GAIN`. They come from
`naval-sky/tests/light_balance.rs` (ignored; run with `--ignored --nocapture`; `TURBIDITY` and `MIE` override the air).

| Sun | Sun on a surface facing it: world before / now / tables | Sky on a level surface: tables / environment integrated / as meshes read it | Direct on a level surface ÷ sky: before / now | Physical: IES clear sky / Bird, AOD 0.1 |
| --- | --- | --- | --- | --- |
| 10° | 4.66 / 3.72 / 3.81 | 0.377 / 0.341 / 0.302 | 2.7 / 2.1 | 0.9 / 1.7 |
| 30° | 8.44 / 6.60 / 6.66 | 0.576 / 0.502 / 0.448 | 9.4 / 7.4 | 3.6 / 4.6 |
| 45° | 8.63 / 7.28 / 7.32 | 0.635 / 0.552 / 0.510 | 12.0 / 10.1 | 4.8 / 6.4 |
| 61° | 8.73 / 7.61 / 7.65 | 0.671 / 0.583 / 0.568 | 13.4 / 11.7 | 5.7 / 7.8 |
| 75° | 8.76 / 7.75 / 7.78 | 0.687 / 0.597 / 0.597 | 14.2 / 12.6 | 6.2 / 8.5 |

The physical column has two sources:

- **IES clear sky**, in illuminance (Rea 2000, the IESNA Lighting Handbook): the beam is 127.5 klx × exp(−0.21 / sin α)
  and the sky 0.8 + 15.5 √(sin α) klx.
- **Bird**, broadband (Bird and Hulstrom 1981). In illuminance it is about 0.8 × this, because the beam gives about
  100 lm/W and the sky about 125 (Perez 1990).

The open ocean's aerosol optical depth is about 0.11 (the Maritime Aerosol Network, Smirnov et al. 2009).

- **The world's sun skipped the air (fixed).** The dome, the clouds' sunlight and the environment take the tables' sun
  through the air. `model::sea_level_light`, the sun on every lit mesh and on the sea, did not:
  - From 18° up a readability ramp held it at the whole sun above the air. That is 12–27 % over the tables' sun
    between 75° and 30°.
  - It ignored the weather's aerosol: 8.75 against the tables' 7.10 at turbidity 3.

  Its luminance is now the tables' sun. The tint and desaturation are unchanged, and the readability floor holds only
  as it sets. Sunlit hulls, airframes and the glitter lose 0.17–0.33 stop of radiance by day (0.1–0.25 on screen) and
  about a quarter stop at dusk. Night is unchanged.
- **The environment keeps its energy.**
  - Level 0, integrated over the hemisphere, matches the dome.
  - The roughest level × π (`env_irradiance`) uses a Gaussian lobe, not a cosine one. It reads 3 % under the dome on a
    level surface, and 5–20 % over it on upright surfaces and the underside.
- **The dome's aureole is short.** In the clear weather `mie_gain` (`(mie × SUN_HAZE / 0.25)^0.7`) is 0.45. The tables
  take the aerosol's share out of the beam, but the dome puts back less than half of it round the sun. Its sky is
  therefore 13 % under the tables' own (`sun_sky_irradiance`); with a gain of 1 the two agree within 3 %.
- **The clear weather is clearer than the sea's air.** Even with the fix, the direct light stands about twice the
  physical ratio over the sky. At turbidity 3 (optical depth 0.09) with an aureole gain of 1, the ratio at 61° would be
  6.5 (sun 7.08, sky as meshes read it 0.95), within the physical range.
- **The exposure is about 0.8 stop brighter than a camera's.**
  - One unit is about 13.8 kcd/m², taking the tables' sun as 127.5 klx.
  - Middle grey sits at about 2.3 kcd/m², against sunny-16's 4.1 (EV 15, K 12.5).
  - A sun-facing grey at 61° reads 2.7 × its albedo (3.0 before); a camera gives 1.45.
  - The grade gain is consistent: one scale for the dome, the clouds and the sun. It accounts for half a stop of the
    difference.
- **AgX turns saturated reds pink in its shoulder, and desaturates them even at a camera's exposure.** For the
  hinomaru's albedo (190, 30, 40), sRGB out of the display:

  | Light on the paint | AgX | Punchy (AgX's power-1.35 look) | Khronos PBR Neutral |
  | --- | --- | --- | --- |
  | 1 × its albedo | (176, 58, 54) | | |
  | A camera's 1.45 × | (199, 80, 74) | | |
  | 2.7 × | (232, 120, 111) | (218, 87, 79) | (246, 60, 69) |

Proposals, not built. The stills are in the lane's review set; the scripts are in `.build/light-balance/`
(`retone.py` re-tones a `capture` HDR offline).

1. **A hazier clear weather**: turbidity 3, with an authored mie of 0.556 for an aureole gain of 1.
   - Better: in the aircraft view the sky moves to the photo's (sRGB 109, 144, 180 against the photo's 113, 133, 168;
     today's is 89, 129, 168), and the paint stands closer to it.
   - Worse: the sea from a ship goes milky and flatter (+0.3 to +0.5 stop), and dusk loses its warm sky (−0.3 to
     −0.5 stop).
2. **Exposure down 0.8 stop** (sunny-16). This leaves the paint-to-sky ratio unchanged, and the sky goes darker and
   bluer than the photo's.
3. **Another look.**
   - Punchy: the hinomaru comes out (159, 69, 71) on screen against AgX's (171, 87, 88). It also deepens the
     shadows and darkens and saturates the sky.
   - PBR Neutral: (181, 52, 65), with a deep blue sky.
   - Either changes the look of every scene.

## The meshes' light follows the sky (2026-10-04)

Hulls and aircraft took the sun's direction, colour and strength once, when their scene was built
(`ShipLighting::daylight`), and only the lightning fill each frame after it. The sky, the sea, the land and the effects
read `GameSky::light` every frame, so a scene whose sun moved after it was built (a theatre's clock through an evening,
a battle's, the console's hour) kept its hulls under the build's sun: lit as at the hour the scene opened while the sky
and sea went dark, and shaded from the old bearing. In theatre it showed on a ship followed into dusk, and went away
when another ship was followed, since that builds a new scene. `BattleScene::update_effects` now hands both layers the
sky's light each frame (`ShipLighting::follow`).

Checked through the harness's `weather` RPC on a clear-sky Bismarck (`.build/native/relight/`): built at noon and taken
to 23:00, master's hull stays sunlit and mirrors in a black sea; with the fix the frame is the one a scene built at
23:00 draws, byte for byte. A scene whose sun stands still moves a little too (the noon frame by a mean of 0.11
levels): the build took the light before the scene's visibility was set (`GameSky::set_visibility`), so hulls had the
sun without the haze's share of the beam, which the sea had.

## What is ported, file by file

| Native | Ports | Notes |
| --- | --- | --- |
| `naval-render/src/lights.rs`, `shaders/naval/lights.wgsl` (`naval::lights`) | `PointLightNode`'s `directPointLight`, `getDistanceAttenuation`; `oceanFog` | `PointLight`, `PointLights` (at most 8; the web's scene holds 7), `SceneFog`, their uniform blocks, `PointLight::lightning` (`syncLighting`'s candela and ceiling). No bindings: each layer keeps the blocks in its own uniforms |
| `naval-ships/shaders/ship.wgsl`, `src/renderer.rs` | three's physical model per point light (`direct_light` for each); `oceanFog` after lighting | `ShipLighting::fog`, `ShipLighting::point_lights`; the fog's sky behind is the environment's sharpest level along the ray with the sea's `max(ray.y, 0)`, as the sea's fog reads it; impact marks fogged before their blend |
| `naval-aircraft/shaders/aircraft.wgsl`, `sprites.wgsl`, `src/layer.rs` | the same for airframes and canopies; fog on the tracer and silhouette batches (`MeshBasicNodeMaterial` fogs too) | the sprite pipelines bind the sky's environment at group 2 |
| `naval-terrain/shaders/lighting.wgsl`, `bindings.wgsl` | `TerrainLightingModel.direct` for point lights ("other lights pass as they are": not shaded by the land's own shadow) | `TerrainLayer::set_point_lights`; trees too |
| `naval-effects/src/combat.rs`, `fires.rs`, `layer.rs`, `shaders/common.wgsl` | the list three would hand its materials | `CombatEffects::point_lights()` (flashes still burning, then the two fire lights) after `update`; `From<PointLight> for naval_render::PointLight`; `EffectsLayer::set_environment` makes the effects' fog read the sky behind per ray (the flat `fog.sky` stand-in stays for hosts without one); `From<SceneFog> for EffectFog` |
| `naval-ocean/shaders/screen_reflection.wgsl`, `surface.wgsl`, `src/surface.rs` | `screenSpaceReflection` line for line: near-plane and viewport clipping, 1/w interpolation, bisection within a depth-proportional thickness, edge searches, the smear over the lobe's in-plane spread (four taps), counting only the image above the water point | traced along the lobe's `traced` ray, smeared toward `lobe.up` by `lobe.in_plane` about `TRACE_BELOW`, mixed into the sky reflection before the foam; `QualityTier::reflection_steps` (High 16, Ultra 32, none below) as a shader def; `OceanSurface::reflections` (`screen_space`, `max_distance` 400) live; the colour copy is bound filterable |
| `naval-ocean/src/lib.rs` | `VisualEnvironment.applyFog` feeding `scene.fogNode` | `From<FogParameters> for SceneFog` (the sea's fog as every material takes it) |
| `naval-ships/src/scorch.rs`, `shaders/ship.wgsl`, `shaders/noise.wgsl`, `shaders/fleet.wgsl` | `ShipScorch` whole: spots, `addStrike`, `addFire`, `sharedVent`, `heat`, `burn`, burn-out, `hullBounds`, `measuredDeck`, `ventSeat`, `writeRow`, `hullDamage`, `color`, `roughness`, `glow`; MaterialX `mx_worley_noise_float` (jitter 1, metric 1: three's squared distance) | see "Scorch" |
| `naval-ships/src/definition.rs` | `mainDeckHeight` | `Hull::deck_heights`, `Hull::main_deck_height` |

### Point lights and the sea

The web's sea is not lit by its point lights: `OceanSurfaceMaterial` builds its whole output in `fragmentNode`, which
replaces three's lighting (lights are applied only when `fragmentNode` is null; fog is applied either way). What reads
as firelight on the water in the web's `fire-night` and `hit-close` frames is the lit hull and the fires mirrored by the
screen-space reflections, plus the fires' glow sprites on the water (already in `naval-effects`). With the reflections
ported, the native `fire-night` sea shows the same warm reflections under the burning hull
(`.build/native/combat-effects/side-by-side/fire-night/01-night.png`).

Since 2026-10-03 the native sea takes them (`native/docs/ocean.md`, "The scene's point lights on the sea"): every
flash, fire and lightning's stroke lights it through its own glitter, water-body and foam models, inverse square with no
cut-off, the glints under the knee the sun's or moon's take. A salvo lays a glitter path to the eye like the moon's, a
burning ship an orange one across the swell, a stroke a white one. The sea's list is its own
(`CombatEffects::sea_point_lights`): the four flashes, the stroke and the three most relevant fires in view at any
distance, where hulls keep the two within 1.5 km. Hulls keep three's windowed reach (a 16-inch muzzle flash lights
nothing past 126 m): the sea beyond it is lit where hulls are not.

### Scorch

`Fleet` holds a `ShipScorch` (`fleet.scorch`). Each frame the host hands it the strikes the events record
(`Fleet::scorch_strike(ship, &ScorchStrike)`: hull-frame point, HE or AP, outcome, the impact mark's width; an HE
burst's scorch is half its mark across, as the mark is sized from its charge since 2026-10-10; a torpedo leaves none) and every
burning, scorched or sunk hull's fires (`Fleet::scorch_update(dt, drift, &[(ShipId, ScorchHull)])`: per compartment its
vent, size and intensity; per mount its frame at its train, gunhouse and intensity; the hull's world velocity; sunk and
whether the loss burns out fully). `Fleet::prepare` publishes the nearest 16 scorched visible hulls into a 66 × 16
`Rgba32Float` texture (binding 20 of the ship frame group) with the web's row layout (the embers' clock in the hull
texel's last lane), and writes each ship's row into its `FleetShip` record (the former padding), so a fragment reads
only its own hull's row instead of searching the rows by position as the web does. The damage applies to paint and teak
under the weather, as `ShipMaterialPalette.weather` layers it; impact marks and plain glTF materials take none.

Differences from the web, all small: the bounds are the model's exact vertices at the current pose (the web unions each
mesh's bounding box, slightly larger); raycasts walk the posed triangles with each part's sphere as the only cull (the
web uses its `SurfaceChunks`; since the frame-hygiene lane the searches skip runs of triangles by their bounds, with the
same results); a hull is measured once, on its first strike or fire (about 10 ms for a large preset then, 0.2–0.5 ms
now: `frame-hygiene.md`). The
web's `emberStrength` review knob is not ported.

## Side by side

Each web frame is the game's own (the effects lane's `effects-web.ts` replays of `scripts/browser/effectsScenes.ts`, or
the harness's `shot.ts --web`); native left or right as labelled in each file.

| What | File (repository root) |
| --- | --- |
| Fire lights on the hull at night, and the burning hull mirrored in the sea | `.build/native/combat-effects/side-by-side/fire-night/01-night.png` (web left) |
| A penetration's flash lighting the plating, and its streak in the water | `.build/native/combat-effects/side-by-side/hit-close/00-0.02_s.png` |
| Hull reflections along the waterline (SSR) | `.build/native/lighting/ssr-close-side-by-side.png` (web left) |
| Fog on a Bismarck 9 km off through a 1.5° lens: web, native, native without ship fog | `.build/native/lighting/fog-9km-trio.png` |
| Scorch after fires: turrets and superstructure, burning and out | `.build/native/combat-effects/side-by-side/s-fire/*.png` |
| Scorch around strikes: 38 cm HE on the belt, 15 and 38 cm on the superstructure | `.build/native/combat-effects/side-by-side/s-hits/03-*.png`, `04-*.png` |
| Scorch at night, embers | `.build/native/combat-effects/side-by-side/s-fire-night/*.png` |

Measured:

- **Fog**: over the distant hull's pixels (`fog-9km`), the web's mean sRGB colour is (133.0, 153.8, 175.0) with a
  spread of 22.4 levels, native's (132.6, 153.3, 174.2) and 24.9; mean absolute difference 5.6 levels, against 17.0
  without the ship fog (whose hull is darker, 121 to 163, and twice as contrasty).
- **Point lights**: in the effects stills the flash and the fires light the plating where the web's do (the stills now
  also light the hull at the web's night shares: before, native's night hull was near black against the web's
  moonlit grey). The web's flashes bloom more (native stills draw without bloom).
- **Reflections**: the share of pixels the effects change rose where the web's flash and fires are mirrored
  (`hit-close` first frame: 4.3 % with the lights, 9.0 % with the reflections too, of the web's 30.9 %; `fire-night`
  at night: 16.7 % and 19.1 % of 31.4 %; most of the rest is the web's bloom and brighter flash); the six image checks with a hull over the sea moved only in the water beside the hull
  (`.build/native/harness/checks/*-compare.png`) and were re-blessed.
- **Scorch**: the soot and char land on the same turrets, superstructure and belt plating in the same shapes; the
  web's frames also carry impact marks, which the stills do not place.

## GPU cost

Apple M5 Pro, Metal, 2026-09-30 14:31–14:46, release `naval-client bench --scene one-ship` at 1920 × 1080, the chase
view, 30 serial frames per round, pass timestamps (serial p50), each configuration run twice alternating, **load average
7–10** with other agents building. The switches were local to the measurement (a patch to the slice, not committed).

| Feature | Pass | Without | With | Cost |
| --- | --- | --- | --- | --- |
| Screen-space reflections (16 steps) | `scene water, sky` | 2.01–2.03 ms | 2.80–2.81 ms | **+0.79 ms** (whole-frame GPU p50 4.1–4.3 → 4.9–5.1 ms) |
| Seven point lights on the helm's hull | `scene opaque` | 0.30–0.32 ms | 0.36–0.37 ms | +0.05 ms |
| Ship fog | `scene opaque` | 0.32–0.35 ms | 0.30–0.31 ms | within noise |
| Scorch, 20 strike spots on the helm's hull | `scene opaque` | 0.30–0.37 ms | 0.38–0.41 ms | about +0.06 ms |

The reflections cost what the web's cost: the same march (16 steps, three searches of eight bisections, four smeared
reads), on every sea pixel that faces up. With none of the effects' lights shining the light loop is skipped on a uniform
count. CPU: `CombatEffects::point_lights` is a few copies; the scorch's state step is microseconds per hull, plus about
10 ms once per hull on its first strike or fire (the raycasts).

## Wiring for the slice

For the slice-4 lane (`naval-client`). All of it was run locally to make the stills and measurements above.

At scene creation (`BattleScene::new`, after the map resolves and the layers exist):

```rust
// The scene fog every material takes (the sea's `scene.fogNode`): hulls, airframes and effects. Since 2026-10-03 it
// only switches the aerial perspective on (`haze.md`); its end is the visibility the sky's haze takes:
// `sky.set_visibility(map.ocean.fog.end)`.
let fog = naval_render::SceneFog::from(map.ocean.fog);
ships.layer.lighting.fog = fog;
air.layer.lighting.fog = fog;
effects.combat.fog = fog.into();
effects.combat.set_environment(sky.sky.environment()); // the effects' fog reads the sky behind per ray
// Screen-space reflections are on by default on High and Ultra (`ocean.surface.reflections`); they need
// `ocean.read_scene_copies = true`, which the slice already sets. Graphics → Reflections `scene` maps to
// `ocean.surface.reflections.screen_space`.
```

Each frame, after `effects.combat.effects.update(...)` and before `Renderer::render` (end of `update_effects`):

```rust
let mut lights: Vec<naval_render::PointLight> =
    self.effects.combat.effects.point_lights().into_iter().map(Into::into).collect();
let bolt = self.sky.lightning().bolt;
if bolt.intensity > 0.0 {
    lights.push(naval_render::PointLight::lightning(bolt.position, bolt.intensity, camera.eye));
}
let lights = naval_render::PointLights::new(lights);
self.ships.layer.lighting.point_lights = lights;
self.air.layer.lighting.point_lights = lights;
if let Some(land) = &mut self.terrain {
    land.set_point_lights(&lights);
}
```

Scorch, in `BattleEffects::update` beside the impact marks (it has the frame's `combat_events`, the `frames` it built for
the effects and the ships layer); the fleet ship of a hull id is `ships.ships.iter().find(|s| *s.id == *id).map(|s| s.ship)`:

```rust
use naval_effects::damage::{Filling, HullStrike, StrikeOutcome};
use naval_ships::{MountFire, RoomFire, ScorchHull, ScorchStrike};

let fleet = &mut ships.layer.fleet;
for event in &combat_events {
    let Some(strike) = HullStrike::from_event(event) else { continue };
    let Some(ship) = ship_of(&strike.ship_id) else { continue };
    // A mount's strike is in its own frame: the event's world point taken back to the hull.
    let point = if strike.mount_id.is_some() {
        let [x, y, z] = event.position;
        fleet.hull(ship).inverse().transform_point3(Vec3::new(x as f32, y as f32, z as f32))
    } else {
        strike.position.as_vec3()
    };
    fleet.scorch_strike(ship, &ScorchStrike {
        point,
        high_explosive: strike.filling == Filling::HighExplosive,
        outcome: match strike.outcome {
            StrikeOutcome::Penetration => naval_ships::StrikeOutcome::Penetration,
            StrikeOutcome::Stopped => naval_ships::StrikeOutcome::Stopped,
            StrikeOutcome::Ricochet => naval_ships::StrikeOutcome::Ricochet,
        },
        mark_width: strike.style.width as f32,
    });
}
// Every hull's fires: each compartment's vent and size, each mount's frame at its train and its gunhouse.
let fires: Vec<(Vec<RoomFire>, Vec<MountFire>)> = frames.iter().map(scorch_fires).collect();
let hulls: Vec<_> = frames.iter().zip(&fires).filter_map(|(frame, (rooms, mounts))| {
    Some((ship_of(&frame.id)?, ScorchHull {
        rooms, mounts,
        velocity: frame.velocity.as_vec3(),
        sunk: frame.sunk,
        full_burnout: false, // the loss's cause is not in the packet yet (see "Needs")
    }))
}).collect();
if dt > 0.0 {
    fleet.scorch_update(dt as f32, self.combat.effects.lighting.wind.as_vec3(), &hulls);
}
```

`scorch_fires(&ShipFrame)` is in `naval-effects/examples/stills.rs` (fifteen lines: `profile.vents` with
`room_fires`, `profile.mount_frame(i, &mount_trains)` and `gunhouse` with `mount_fires`); copy it. A new battle calls
`fleet.scorch.clear()`.

## Open problems

- ~~**The fog's sky behind**~~: superseded on 2026-10-03: every material takes the aerial perspective (`haze.md`),
  the atmosphere and the weather's haze along its own ray, and the fog map is gone.
- ~~**Lightning's flash**~~: done by the sea-sky lane: `naval-client` lifts the ships' and aircraft's fill by `1 + flash` each
  frame, and the effects' ambient (`BattleEffects::set_flash`).
- **Scorch without the loss's cause**: a sunk hull burns out to the flooded share (0.35 plus its fire) since the packet
  does not say whether a magazine sank her.
- **No bloom in the effects stills**: the web's flashes bloom over the plating they light; the slice has bloom.

## Re-sync

Web sources re-read against master `6cd598cf2` (2026-09-30) by the re-sync lane (`docs/archive/native-resync.md`); the next re-sync diffs
from there. `ShipScorch.ts` and `LocalizedFireEffects.ts` take the battle's pace for the hull's way through the smoke; the
native scorch and point lights read the packet's world velocity, already paced by naval-sim. No change.

## Needs from naval-sim

- ~~The defeat cause of a sunk ship in the render packet~~: done (sim seams lane). `ShipVisual::defeat_cause` carries
  it, and naval-client's scorch burns a hull out fully for `magazine` and `structural-fallback` (`hull-failure` went
  with hit points).
