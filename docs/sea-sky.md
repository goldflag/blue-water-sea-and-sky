# Sea and sky lane

Single battle was removed on 2026-10-07. Standalone battle UI checks named in older measurement sections are retired;
current interface checks use theatre scenarios ([interface verification](../../docs/ui-verification.md)).

Closes the sea, sky and terrain gaps the earlier lanes listed: the sea reacting to hulls, the underwater view, the
clouds' cost, the far fog's colour, lightning on meshes, the dark valleys and the port's cloud-shadow clearing. Crates
touched: `naval-sky` (the fog map), `naval-ocean`, `naval-clouds` (the shadow's clearing, the cost tools),
`naval-ships` and `naval-terrain` (their fog), `naval-client` (wiring). Outputs go to `.build/native/sea-sky/`.

## Status (2026-10-01)

| Item | State | PR |
| --- | --- | --- |
| The far fog from the sky's own fog radiance | Done, then replaced on 2026-10-03 by the aerial perspective (`haze.md`): the fog map is gone | this lane's first PR |
| Lightning brightening the ambient light | Done: ships' and aircraft's hemisphere fill (since 2026-10-03 the flash's alone), the effects' ambient; the land already took it | first PR |
| The port's berth clearing | Done: the clouds' shadow clears within 350 m of the berth, back by 900 m; the far sea keeps its patches | first PR |
| A battle's weather and visibility | Done on the way: the scenario's `weather` and `visibility` (`battle_load`'s, a film's, the slice duel's `clear`) reach the sky and fog; they were dropped and every battle drew the map's cumulus. Re-blessed `ship-fletcher-broadside` (the duel's clear sky, as the web's) and `parity-clouds` (partly cloudy's own deck) | first PR |
| The clouds' cost | Explained: like for like the native march costs what the web's does (1.02–1.13×); the 2–5× compared two methods | #750 (`clouds.md`, "Against the web") |
| The dark valleys | The web was wrong (a TSL loop-counter clash walks a spiral); native now matches what the game ships | #748 (`terrain.md`) |
| The sea reacting to hulls | Done: `hullSea.ts` ported; against the game with it on the replays land as close as the uncoupled pair did | the hull sea PR |
| The underwater view | Done: the surface from below, the water's fog and colour over the scene, the waterline's film; the eye holds under water | the underwater PR |
| The wet band from the drawn sea | Done (2026-10-01): hulls read the sea the surface draws beside them (the field with the sea around hulls, the wake field, the bow waves) from the ocean's patch about each coupled hull | the wet band PR |
| The console's wind and the sim's sea | Done: the developer console's wind eases the sim's sea, and the drawn waves, the sea around hulls and the hulls' motion follow it as one sea | the console wind PR |
| Pause holds the picture | Done: a battle's pause, the port's menu and a theatre's paused clock hold one presentation time; the sea, wakes, bow waves, spray, smoke, effects and clouds stand to the byte, the camera moves on | the pause PR |

## The far fog (`env_fog`)

Superseded on 2026-10-03 (`haze.md`): the air between the eye and every material is the aerial perspective, the
atmosphere and the weather's haze integrated along each ray, and the fog map below (the environment's binding 3, its
bake and its stand-in fill) is gone. Kept as history.

The web fogs every material toward `OceanSky.createFogSampler` (`src/game/ocean/screen/fog.ts`): the sky from the
camera without discs, stars or clouds, greyed under a closed deck (`Sky.ts`'s `fog` source, `naval_sky::atmosphere::
fog_radiance` natively). That needs the sky's own bindings, which the sea (four groups in use) and the ships cannot
take, so every material read the environment atlas's sharpest level instead, which holds the clouds.

Now the environment's bind group carries the fog itself: binding 3 is a 514 × 128 `Rgba16Float` map over the upper
hemisphere (512 columns round the compass plus one wrapped each side; rows climb by the square root of the elevation,
so most sit near the horizon, where fog is read), and `naval_sky::environment::env_fog(fog_map, sampler, direction)`
reads it. The sky bakes it (`fog_main` in `environment_bake.wgsl`) in the environment's update whenever what
`fog_radiance` reads changes (the lights, the camera's sky-view sections, the grade, the haze) or its tables were
redone; a stand-in environment fills it from its atlas (`EnvironmentMap::fill_fog_from_atlas`, naval-ships' stand-ins;
naval-ocean's mirror fills it from its equirectangular bake on the CPU). A shader that does not declare binding 3 binds
the group unchanged. Readers: the sea (`surface.wgsl`), hulls (`ship.wgsl`), the land and trees
(`lighting.wgsl`, `terrain.wgsl`, `trees.wgsl`; the 1.1° lift that kept the atlas's horizon row out is
gone). Aircraft and the effects still read the atlas (their lanes': one line each).

Measured (`.build/native/sea-sky/fog/`): the sea itself barely moves (overcast and a partly cloudy dusk, the old sea fog
against the new on the same build: 0.01–0.02 levels), since the horizon row of the atlas held little cloud. The land
moves where distant hills stood in the fog against a cloudy horizon (the terrain lane's stills against the web, before →
after, whole frame, mean levels): Ravnsund binoculars 3.80 → 0.41, Vestfjord binoculars 23.6 → 19.5, Strait of Dover
aerial 4.03 → 2.74, Ravnsund aerial 9.76 → 8.82, Vestfjord aerial 18.4 → 17.1, Galeb Channel aerial 17.7 → 16.8; no view
worse (`terrain-compare-{before,after}-fog.json`, `fog/terrain/*.png`). The bake costs 0.006 ms of GPU (p50, one-ship
scene, 1920 × 1080, load 8.8) and runs only when its inputs change.

## The sea around hulls (`hullSea.ts`)

Combat poses every hull on its own two long sines (naval-sim's `SeaState`); the wave field draws an unrelated spectrum
of the same height statistics, so a hull rode a sea the water beside it did not show: the water-effects lane's destroyer
at 25 m/s floated over a trough and threw no spray. The web blends the field's long waves (the first cascade low-passed
by its mip chain) into combat's sea near the nearest 16 hulls, keeping the shorter waves, foam, wake and bow waves on top.
Since 2026-10-04 the native client draws combat's sea everywhere (weight 1 at every point: the long swell the hulls ride
runs to the horizon, and every hull at any range sits in it; `ocean.md`, "The swell"); the per-hull weights below remain
for callers that do not set it (the tests and the stills).

- **CPU** (`naval-ocean/src/hull_sea.rs`): slots, weight and gradient, the variance-keeping blend, the packed components
  (phases reduced in f64 about the camera), `cascade_modes` and `split_level` (which mip of the first cascade splits its
  waves into those the coupling replaces and the rest), `surface_contact` and `hull_footprints` (`src/game/hullSea.ts`).
  The tests are hullSea.test.ts's, and the split of the realistic and art-directed storm seas is pinned to Bun's
  (1.75, 4, 1.75, none).
- **GPU** (`naval-ocean/shaders/waves.wgsl`, `naval_ocean::waves`): the wave field's group (binding 5 now holds the
  coupling, visible to compute as well), `displacement` with the blend where the vertex lands, `hull_surface` (the grid
  slope and strain added with the first cascade's, the world slope of combat's sea and of the blend, its unresolved
  variance) in the surface's `wave_surface` (only while a hull is coupled, a uniform branch), and `height_at` (the
  choppy inversion with the coupling, for the passes beside the surface).
- **Driving it**: `Ocean::set_hull_sea(waves, time, footprints, camera)`; `naval-client` couples every frame from the
  drawn poses, combat's sea and the presentation time (`BattleEffects::update`), each hull's contact from its deck edge
  and height (the packet carries no wave heave, so a sinking hull's fade sits within the sea's amplitude of the web's).
  Only the realistic sea state couples, as on the web. It runs whenever the sea is drawn (`BattleEffects::couple_hull_sea`),
  whatever the effects' switch; a submarine's contact reads its periscope head. In port the berth rides its own sea
  clock (`PortScene::sea_time`), which the coupling and the bow spray now both read (`BattleEffects::sea_time`), as the
  web's `coupleHullSea` and `presentedSea` do: the port's opening view against the web 6.38 → 1.57 levels
  (`.build/native/sea-sky/hull/port-opening-trio.png`). The bow spray rises from combat's sea at that time
  (`EffectsFrame::sea`), as the web's does; with the drawn water now meeting the hull, both agree.

Measured against the game as it ships (`water-stills.ts`, now `--hullsea on` by default; the native replay follows
each capture's flag), whole frame (mean levels, share over 24): `.build/native/sea-sky/hull-sea-table.txt`.

| Capture | Web off vs native off | Web on vs native on | Web on vs native before |
| --- | --- | --- | --- |
| `wake-stern` | 4.34 · 5.7 % | 4.34 · 5.7 % | 12.68 · 20.4 % |
| `wake-destroyer` | 4.23 · 5.5 % | 4.21 · 5.5 % | 6.15 · 8.4 % |
| `bow-wave` (proxy hull fills the frame) | 9.25 · 14.7 % | 9.11 · 14.7 % | 16.99 · 31.3 % |
| `bow-wave-ahead` | 3.94 · 3.7 % | 3.87 · 3.6 % | 14.79 · 27.4 % |
| `bow-spray-heavy` (proxy hull fills the frame) | 18.47 · 23.0 % | 18.51 · 21.2 % | 31.68 · 42.6 % |
| `bow-spray-heavy-ahead` (25 m/s) | 4.24 · 5.0 % | 3.90 · 4.5 % | 19.83 · 30.9 % |

What the coupling changes on the web and natively correlates per pixel at 0.81–0.97. Side by side:
`.build/native/sea-sky/hull-bow-spray-heavy-ahead-trio.png` (web | native before | native after: the Fletcher's bow now
meets the crest the web's meets) and `.build/native/water-effects/side-by-side/`. The client: re-blessed `chase-one-ship`, `ship-bismarck-broadside`, `ship-bismarck-bowquarter`,
`fleet-measure30`, `port-bismarck`, `sea-near`, `sea-wide` (the hidden one-ship Bismarck's sea, as the web couples hidden
hulls too), `ship-bismarck-close`, `ship-bismarck-top`, `ship-fletcher-broadside`, `sky-morning-sun`, `theatre-cruise`,
`theatre-bubble`: in each only the water near a hull moved.

Cost: the surface pass (`scene water, sky`, one-ship chase, 1920 × 1080, load 14–20) 2.66–2.67 ms before, 2.82–2.98 ms
after: about 0.15–0.3 ms with the hull's reach filling most of the sea in view; nothing where no hull is coupled.

The water-effects stills had stopped running (the hull prepass's camera group predates render core II's camera layout);
`water_stills` now reads the renderer's opaque copies as the slice does.

## The wet band from the drawn sea (`HullWetBand.ts`)

The web darkens and glosses every hull just above the moving sea, reading per hull fragment the sea `Game.ts` hands
`HullWetBand.setSea`: `ocean.waveField.heightAt(xz)` (the choppy inversion, with the sea around hulls) plus
`shipWake.surfaceHeight(x, z)` (the wake field and the bow waves at the vertex stage's feature size). Natively the band
read a flat sea at 0, so a hull riding a trough showed her topsides and deck wet, one on a crest her band drowned, and
the bow wave's crest never wetted the stem.

The ship pipeline uses all four default groups (camera, frame, colour map, environment) and the wave field's arrays
ping-pong every frame, so the hulls do not bind the field. The ocean evaluates the heights for them instead:

- **The patches** (`naval_ocean::hull_heights`, `shaders/hull_heights.wgsl`): with `Ocean::enable_hull_heights_in`, every
  hull `Ocean::set_hull_sea` couples (the nearest 16 to the camera, coupled or not: the art-directed sea gets patches
  too) gets a patch over her waterplane plus a margin for the stem's rake and the flare (4 % of her length, at least
  6 m, each end; 10 % of her beam, at least 3 m, each side). A compute pass in the ocean's prepare, after the field's
  update and the wakes' encode, writes `height_at(xz) + wake_height(xz)` (the very functions the surface and the
  underwater probe read) at up to 256 texels from stern to stem and 64 from port to starboard, one layer of an
  `R32Float` array per patch: about a metre along a battleship, half a metre along a destroyer, finer across. Only the
  patches whose bounds reach the camera's view are evaluated (a hull out of view draws nothing), each thinned to a
  texel every two pixels of its nearest point (16 × 8 at the least), so a fleet's distant hulls cost little; the
  patches' uniform carries each one's texels, which the read follows. The groups 0–2 it shares with the underwater
  probe (`sea_read.rs`: the camera copy the bow waves read, the wake's bindings, the field).
- **The read** (naval-ships' `ship.wgsl`, `sea_height`): frame group bindings 21 (the heights, read with `textureLoad`,
  four texels blended) and 22 (the patches, 528 bytes, `HullHeightsUniforms` byte for byte; `tests/sea_heights.rs` holds
  them equal). A fragment inside a patch reads it; outside every patch (a hull beyond the nearest 16) the flat sea at
  `SeaState::level`, as before. The band's response, `lowest`, `highest` and climb are the web's; `crest` now comes from
  the bow waves (`BowWaves::crest_height`, `HullWetBand.update`'s `crestHeight()`).
- **Wiring** (`naval-client` `scene.rs`): the scene enables the patches and hands them to the ships once
  (`ShipLayer::set_sea_heights`); each frame the coupling places them, the band's crest follows the bow waves, and the
  band reads them only while the ocean is drawn (`ShipRenderer::read_sea_heights`; the layer switch off keeps them
  stale, so the flat sea then).

Bound against the defaults, the ship fragment stage now holds 13 of 16 sampled textures and 8 of 12 uniform buffers
(CORE_SHADOWS), so the field's own bindings (three more textures, two samplers, three uniforms and a 40 kB wake block)
were not needed.

Checks: `naval-ocean/tests/hull_heights.rs` (a flat field and calm wake: every patch texel holds the CPU's
`coupled_height` at its world point within 5 mm, two hulls, one partly in contact, at full resolution and thinned
for a camera 6 km off; the unit tests cover the view test and the thinning) and `naval-ships/tests/sea_heights.rs`
(a patch at the mean sea draws the flat band, a sea 3 m higher wets the hull 3 m higher, unread heights give the flat
sea back).

Against the web (`compare.sh` with `LANE=wet-band`, `.build/native/wet-band/stills/`: web | master | branch, crops
`*-crop3.png` web, master, branch and |master − branch| × 4):

| Case | What changed | Numbers |
| --- | --- | --- |
| A Fletcher at 0.55 throttle in a 25 m/s sea, abeam at 70 m (`fletcher-25`) | She rode a trough: master wetted her topsides and stern deck; now dry above the water, the band hugging the drawn waterline | her topsides' luminance (the web's framing aligned on her superstructure, `aligned.py`): web 70.6, master 58.5, branch 70.4 |
| The port's Bismarck abeam at 150 m (`port`) | The band's upper edge now undulates with the water beside her as the web's does; master drew a straight line | waterline rows, aligned: web vs master 4.89 levels (6.3 % of pixels over 24), web vs branch 2.61 (2.2 %); whole frame 4.37 → 4.24 |
| Bismarck abeam at 150 m in a 15 m/s sea (`bismarck-broadside-15`) | The drawn water stood above the flat sea there: master's band was under it; now a band rides above the water | the waterline sits at another height than on the web (the battles' sea clocks are not held alike, as under water), so no per-pixel figure |

A storm (the `bow-spray-storm` film's 22 m/s, and a still of that battle) changes nothing visible: the hulls are lit
at 7 levels there, so the band's darkening moves pixels by one to three levels.

Cost (`native/tools/wet-band/bench.ts`, master's client and the branch's interleaved, three rounds of 120 frames, medians,
1920 × 1080, Apple M5 Pro, 2026-10-01, load average 26–28, `.build/native/wet-band/bench2.txt`): the hull heights' pass
0.026 ms for Bismarck abeam at 150 m, 0.009 ms behind her in the chase view and 0.049 ms for the 30-ship battle's
sixteen nearest hulls (0.28 ms before the view test and thinning, at load 10–14); the scene's opaque pass, where the
band reads them, unchanged within the noise (0.585 → 0.582, 0.234 → 0.237, 0.447 → 0.424 ms).

Image checks: `fleet-measure30` re-blessed. The branch moves 6 of its 57,600 pixels more than 24 levels (the band at
the hulls' waterlines, mean 0.03 levels against master's render), which tipped it past the 1 % line: master's own render
already sat 0.64 levels and nearly 1 % off its reference, since the HUD lane's aim circles were added. The new reference
holds both. Every other check is unchanged within its tolerance (`chase-one-ship`, the Bismarck and Fletcher checks
0.00–0.13, `port-bismarck` 0.08).

## The console's wind and the sim's sea (`goldflag/console-wind-sim-sea`)

The coupling above draws combat's sea around hulls, but that sea kept the battle's starting wind when the developer
console changed it: after "wind 25" in a calm battle the far waves and the spray rose while the hulls rode, and the
water beside them showed, the calm. Now a battle's console wind goes to the sim (`Battle::change_wind`,
`native/docs/ui-contract.md`, "The console's wind and the sim's sea"), which eases its two long sines to the new wind
over four seconds of battle time (the waves it has subside to calm, the new wind's build, at one rate), and every packet
carries the sea and its share of its settled height. From the console's first wind change, `weather::follow_sim_sea`
draws the scene's sea from the packets: the coupling and the bow spray take the packet's sea, and the wave field takes
the calibrated wind sea of the sim sea's wind and direction at its share, rebuilt in twentieths of the share. The share
is `WaveField::height_scale`: it scales the drawn sea's height after `drawn_sea` has resolved the realistic sea state's
wavelength (and so the tiles) from the full height, so the waves keep their wave numbers, directions and phases and only
their amplitudes move (scaling the height itself would have shortened the peak and regrown the tiles at every step,
moving every crest). The crest normalisation (`waves.sea.x`) and the whitecap statistics take the scaled height. The far
waves, the water beside the hulls and the hulls' motion so change together.

Measured through the RPC with the real UI (`bun native/crates/naval-client/tools/console-wind.ts`, 11 checks; a custom
battle sailed at 2 m/s from the battle rail, then "wind 25" typed into the console; 2026-10-01, Apple M5 Pro, load
average 12-15; `.build/native/console-wind/`):

| | 2 m/s | 25 m/s, settled |
| --- | --- | --- |
| Sim sea (amplitude, wavelength) | 0.046 m, 28 m | 4.09 m, 249 m |
| Drawn waves (significant height) | 0.1 m | 8.8 m |
| The helm hull's heave range / largest roll / largest pitch (8 s) | 0.003 m / 0.013° / 0.006° | 3.35 m / 4.21° / 1.07° |
| Her heave against the sea over her footprint, lagged as her hydrostatics lag it (correlation; residual sd) | 0.82; 0.001 m | 0.975; 0.22 m |
| Frame CPU, p50 / max | 1.40 / 2.9 ms | 1.46 / 4.6 ms |

The storm's waves were up 0.07 s after the console's command and settled 3.8 s after it; the share rose monotonically
and the drawn height stayed within a step of it; during the change the frames that rebuild the spectrum peaked at 7.1 ms
of CPU (p50 1.40 ms). Stills, calm | two seconds in | settled: `broadside-calm-during-storm.png`,
`bowQuarter-calm-during-storm.png`, `chase-calm-during-storm.png` (the hull sits in the drawn water throughout, the
spray at her waterline in the storm); the samples and summary in `run/`.

## The underwater view (`screen/underwater.ts`)

The owner hit it playing: the chase camera's look-around dives under the keel, waves wash over a low camera, the port's
orbit and the free and shell cameras go under. Natively the sea seen from below drew its upper side's shading and the
scene stayed clear and unfogged. Now, as on the web:

- **The probe** (`naval_ocean::underwater::UnderwaterProbe`, `shaders/underwater_probe.wgsl`): while the near plane
  may reach the water (`near_plane_may_be_submerged` against the surface's bound: the field's crest at most twice the
  significant height, plus the wake's 8.1 m), a compute pass in the ocean's prepare writes, on a 128 × 72 grid over the
  screen, each near-plane point's depth below the waves and the wake (`height_at` with the hull sea, `wake_height`), and
  the camera's own depth below the waves. Same frame, where the web's readback lags one. The frame the near plane
  leaves the water's reach it is written once more, all above the water, so the clouds (which read it to draw nothing
  where a ray starts under the waves, `clouds.md`) can trust it without knowing whether it ran.
- **The underside** (`surface.wgsl`, `underside`): back faces seen from a submerged camera (or above it) show Snell's
  window, the refracted sky (clouds and all, from the environment) raised by water's n² on the way in, and, outside it
  and in the window's own reflection, the deep mirrored by total internal reflection: the water's light along the
  mirrored ray, as the composite lights the water about the camera (`water_path_light`, `underwater_ray.wgsl`). The
  air's fog does not lie over it. (The web's `seaFromBelow` mirrored 2.5 times the sky's blurred overhead radiance,
  which made the surface from below a bright sheet; see "Realism, 2026-10-03".)
- **The composite** (`naval_ocean::underwater::UnderwaterLayer`, `shaders/underwater.wgsl`): its own layer after the
  effects (before the aim overlay), two blended full-screen draws in the transparent phase: the scene times
  `exp(−c × d)` to the first surface behind each pixel (the backdrop depth copy, the farthest of five neighbours; c the
  maps' absorption swatch, the water's beam attenuation), then the in-scattered light (closed form along the ray, the
  daylight fading with depth by the diffuse attenuation, the refracted sun's forward lobe) and the waterline's film in
  the transmission tint, 1.6 px wide. Above the water it draws nothing; it asks for the depth copy
  only while the camera is within 25 m of the surface's bound.
- **Exposure**: the eye adaptation eases back to the fixed exposure while the camera is under (`Session`, as
  `eyeAdaptation.update(dt, on && camera.y > 0)`). The web's easing of the absorption to a twentieth for a submerged
  camera (`submerged_absorption`) is gone (below).

Against the web (`native/tools/sea-sky/underwater-shots.sh`, `.build/native/sea-sky/underwater/*-side-by-side.png`,
whole frame, mean levels and share over 24): from 4 m under astern of Bismarck 7.6 · 11.3 % (her stern, the surface
from below, the fogged deep; the waves differ in phase, the battles' sea clocks not being held alike), the chase camera
10° under the keel 4.8 · 6.9 %, the port's orbit 6° under 1.2 · 0.1 %, the lens at the waterline (0.4 m under: the deep
below, the bright grazing underside, the film, the sea above) the same structure with the waves at another phase. Those
figures predate the realism pass below, which leaves the web behind on purpose.

**Realism, 2026-10-03.** The owner found the underwater view "really unrealistic now (not that it was great before)".
The "now" was the takram clouds painted over the sea's underside (`clouds.md`, "Under water"); the "before" was the
web's look: a bright underside everywhere and water clear for a kilometre. Now:

- **Snell's window and the mirror**: looking up, a bright window about 97° across with the sky in it, ringed by the
  dark, sparkling mirror of the deep; looking level, the surface from below is that mirror, all but indistinguishable
  from the deep beyond the window's sparkle, as in footage.
- **Visibility**: the water under the camera is the water the sea shows from above. The maps' absorption swatches, read
  as beam attenuation, give 40–65 m in blue, 20–37 m in green and about 10 m in red (Jerlov oceanic I–II), where the
  web's twentieth gave a kilometre: a hull 65 m off no longer shows, and the chase camera's look-around 320 m out
  under the keel sees only water.
- **Clearer under the camera** (2026-10-03, Bill: a hull's bottom should read from under the water): each metre of
  water under the camera counts as `UNDERWATER_CLARITY` (0.15) metres of the map's water, for the image's dimming and
  the light the water scatters alike (`underwater.wgsl`). The swatches as measured give about 30 m in blue and green
  and 10 m in red to 5 % contrast on the North Atlantic, so a hull vanished within 30 m; now it reads as a dark shape
  at 30–100 m and faintly at 150 m, with the keel crisp from 30 m down, the water's colour unchanged and red still the
  first to go, so a far hull turns blue-green. The sea's underside (the deep it mirrors) and every view from above are
  unchanged (pixel-identical). At dusk the clearer water shows that submerged hulls are shaded as in air: the sunset's
  sky reflection glows on the hull below the waterline (it showed faintly before).
- **Depth**: the daylight fades by the diffuse attenuation K_d, the beam less clear ocean water's forward scattering
  (0.07 /m, at least a fifth of the beam: K_d(blue) 0.012–0.03 /m across the maps), so the deep stays lit and turns
  from teal toward navy with depth (blue light at 30 m is about half the surface's) instead of keeping its colour.
  With one coefficient for both, as before, realistic visibility turned the deep black within 20 m.
- **Left as they were**: the exposure stays fixed under water (a diver's eye would open; the deep reads dark beyond
  30 m at noon), the environment has no sun disc, so the window shows the sun's glow but not its refracted disc and
  sparkle, and there are no light shafts (caustics came on 2026-10-04, below).

Image check `underwater-astern`: 4 m under, 8 m astern of Bismarck: the surface from below within 40 levels of the
deep it mirrors (the sky or clouds through it read 80 and more above), the deep blue-green, her stern in the middle.
Before/after sheets (pre-takram, master, branch at noon partly cloudy, clear and overcast, dusk and night; 3, 10 and
30 m down looking level, up and down; astern of her and under the keel):
`.build/native/underwater/sheet-<time>-<weather>.jpg` from `.build/native/underwater/shots.sh`.

Cost (one-ship, 1920 × 1080, load 12–14): the probe is below the timer's resolution, the composite about 0.13 ms
(transparent pass 0.27 against 0.14 ms above water), and the sea's own pass is cheaper under (fewer front faces); above
the water nothing runs.

Known differences: transparents drawn after the composite's layer (the aim overlay) are not fogged; the web fogs the
whole frame. The sky's shafts and rain veil run before the composite (the web runs them after), so under water they
are attenuated with the scene.

**A view to work from, for the U-boats (2026-10-04).** The owner asked for the underwater view to be taken further for
the Type VIIC, and for it to stay functional: nothing that makes a hull, a wake or the surface harder to read. Four
things changed, native only, and two were left out. `native/tools/sea-sky/uboat-shots.sh [out]` takes every view named
here with this checkout's client or, with `BINARY=`, another (the boat held at a depth by a helm input sent again every
third of a second: a held input expires, and her captain then takes her to periscope depth himself).

- **What stands under the sea is lit through its water** (naval-ships' `ship.wgsl`, `water_light` and
  `water_irradiance`; `ShipLighting::water`, which naval-client sets each frame from the ocean's colours and the
  composite's `UNDERWATER_CLARITY`, now a constant of `naval_ocean::underwater` the composite's uniforms carry). A
  fragment below the sea's height over it (the wet band's patches about the hulls, the mean sea beyond them) takes: the
  sun refracted toward the vertical, less its Fresnel reflection, dimmed along its slant by the K_d the composite
  fades the daylight by (the clarity included, so a hull stands in the light of the water drawn round her); in place of
  the sky's irradiance and its mirror image, the daylight through Snell's window (all of it on a surface facing up, a
  fifth on a vertical one, none on one turned past 138.6°) and the water's own glow (from below 1.5 % of the daylight
  going down, `WATER_UPWELLING`, and four times that from the side, `WATER_SIDELIGHT`); the point lights and a flash's
  fill dimmed alike; and no aerial haze (the sea's surface and the composite veil what is under water). It holds
  whether the camera is above or below the surface, over 0.25 m at the waterline. This ends the wart above: the sunset
  no longer glows on plating under the waterline. The two glow constants are set by the contrast below: at 0.06 and
  ×1 a hull's bilge took more light than the sky's environment had given it and her contrast fell; at ×6 her side
  came up to the water's own brightness and her outline went with it.
- **Caustics** (`naval_ocean::caustics`, `shaders/caustics.wgsl`, `Ocean::caustics`; the ships' binding 23,
  `ShipRenderer::set_sea_caustics`): the net of light the waves draw on what is under them, from the wave field itself.
  The light entering the sea at x lands at x + s·∇h(x) at depth d (s = d/4), so a point is lit 1 / |det(I + s·H)| as
  brightly as under a flat sea, H the surface's Hessian where its light entered (two fixed-point steps back up the
  ray; taken straight above the point instead, the gains averaged 1.2–1.6 and the threads lay beside where they fall).
  The slopes are the cascades' derivative maps differenced at each level's texel or at 0.06 of its depth, whichever is
  wider (the ripples the field does not hold and the water's scattering blur a deep net). A compute pass writes four
  256² layers, for 2, 5, 12.5 and 31 m, each a square about the camera laid on the world so it repeats (32, 64, 128
  and 256 m); the ships blend two layers for a fragment's depth, read down the refracted sun's slant, take coarser
  layers where a pixel is wider than a layer's texel, and fade the net as the water scatters the beam
  (exp(−0.07 × path)) and from the surface down to 2 m. `tests/caustics.rs` reads the layers back over 5, 9 and
  15 m/s seas: mean 0.97–0.99 at every level (the waves move the light, they do not make it), deviation 0.45–0.48 at
  2 m falling to 0.21–0.38 at 31 m, under half a percent at the brightest drawn (4). The pass runs while the camera
  is below 25 m over the mean sea (`CEILING`), and the ships fade the net out from 12 m: from higher the sea's own
  surface hides it.
- **Foam and wakes on the sea's underside** (`surface.wgsl`, the `underside` branch): from below the surface showed
  only Snell's window and the deep's mirror, so a ship overhead left no mark but her hull. Whitecaps, a wake's churned
  water, the bubbles under both and a torpedo's track (the wake's foam) now pass their light down as a pale glow,
  darker than the sky in the window and brighter than the mirror outside it; the foam's own lace is drawn at half
  strength through the bubbles' soft cloud, so the surface's hard edges do not print on the underside. A wake reads
  from 20 m down and 100 m astern (`wake-overhead`). A torpedo's track from below is not in the stills.
- **Motes** (`UnderwaterLayer`, `shaders/underwater_motes.wgsl`, `show_motes`): open water gives the eye nothing to
  tell its own motion by. 768 points of an 18 m cube that repeats over the world stand still within 9 m of the lens
  (about sixty in a view), each a dot 1.2–3 pixels across lit at 2.5 times the water along its ray, hidden by whatever
  stands nearer and drawn only where its pixel's ray starts under the waves. In a still they are a few faint specks.
  How they read in motion under temporal anti-aliasing has not been looked at in a window.
- **Left out**: light shafts (they add light between the eye and a hull, which is what the contrast below must not
  lose, and the forward lobe already brightens the water toward the sun), and fogging the aim overlay (unchanged).
  The land under the sea is still lit as in air, and so are aircraft that have gone in.

**A hull's contrast against the water** (`native/tools/sea-sky/uboat-contrast.py`: a Bismarck broadside from a camera
6 m down, the Weber contrast of her linear luminance against the same view's water without her, over the pixels she
covers below the surface; master → branch): at 30 m 0.559 → 0.628, at 60 m 0.375 → 0.391, at 100 m 0.200 → 0.203; her
faintest tenth 0.338 → 0.437, 0.221 → 0.197 and 0.105 → 0.100 (the last rows of her bottom, which the water's glow
lights a level or two more than the sky's environment did).

**Above the water** the checks hold (`check.ts`: every one but `underwater-astern` within its tolerance, blessed
again). A surface ship from her bow quarter differs by 0.06 levels on average (her plating under the waterline, seen
through the water); the open sea over a boat at 10 m by 0.002.

**Cost** (Apple M5 Pro, 1920 × 1080, the camera 5 m down beside the one-ship scene's Bismarck, three alternating
rounds of 120 frames against master's client, load average 32–38; medians): the caustics' pass 0.10 ms (0.32 ms before
the first cascade was left out of the shallow levels and the pass kept to a low camera); the opaque pass, where the
hulls take the water's light, 0.53–0.57 ms against 0.53–0.66; water and sky 1.40–1.98 against 1.44–1.59; the
transparent pass, with the motes, 0.15–0.48 against 0.17–0.43: nothing measurable but the caustics.

**Night (2026-10-12).** Bill found the underwater view at night split at eye level: teal water above, near-black
below. The display's night grade took the open deep for sky; it now takes every pixel seen through water as the
near world ([night readability](lighting.md#night-readability-2026-10-06)). Stills, master beside the branch:
`.build/native/underwater/night/fix-sheet.png` (8 m under beside Bismarck) and `wl-sheet.png` (the waterline
across the lens).

**Seen while here, not changed**: the chase camera of a submerged boat stays above the sea (`chase-10m`: the view is
the open sea, the boat unseen), so the underwater view is reached by the look-around, the free camera and photo mode;
a lens awash draws the near wave it looks through as a flat dark shape (`periscope-awash`); at dusk the boat at 10 m
is all but black against the water at the fixed exposure (`dusk-quarter-10m`).

## Lightning's flash

`VisualEnvironment.syncLighting` lifts the hemisphere fill every lit mesh takes by `1 + flash`; natively meshes have no
fill but the flash's, 1.6 × the ambient × the flash (`ShipLighting::fill`, `docs/lighting.md` "Realism"), and the
effects get `ambient × (1 + flash)`. `naval-client` now does both each frame (`BattleScene::update_effects`: the ships'
and aircraft's `fill_intensity`, `BattleEffects::set_flash` for the combat and water effects' lighting), holding the
flash on frames that do not move as the bolt is held; the land already took it (`TerrainLight::flash`). The harness's
new `sky_lightning` command forces a strike as the web's `forceStrike` does and reads the flash and bolt back.

The sky review's night strike (`lightning`: 23 h, storm, a ground bolt at (2600, 0, 380) 6 ms in, seed 7, a Bismarck at
the origin and another 2.5 km east, the bridge camera; `native/tools/sea-sky/lightning.ts`) against the web game's frame
of the same scene: native with the strike 15.8 levels from the web (24.6 % of pixels over 24), the same native frame
without it 64.8 (`lightning/night-trio.png`: web | native, strike over | native, struck). The flash reads 0.42 then, so
the hulls' fill is 1.42 times the night's.

## The port's berth clearing

`PORT_CLEARING` (`clearBerth` in `VisualEnvironment.ts`): no cloud shades the ship or the water within 350 m of the berth,
the shadow back in full by 900 m, while the clouds cross the sky. The clouds' shadow uniforms (now 64 bytes) carry a
clearing (`CloudLayer::set_clearing(Some(CloudClearing::port(berth)))`), which `cloud_shadow` and `cloud_shadow_fast`
apply after the deck's shade, so the hull, the sea and the land all agree. The port no longer switches the shadow off.
The port's far sea from 320 m up (`port/far-sea-trio.png`): 1.58 levels from the web before, 0.84 after.

## Pause holds the picture (`goldflag/native-pause-freeze`)

The owner paused and saw the sea and the bow waves run on. The web's `Game.frame` makes a paused frame dt 0
(`this.paused`, the Esc menu at sea and the Menu in port) and so stops everything that moves with the battle, while the
camera rig, the gun-aim circles and the input keep `realDt` and `performance.now()`.

**The clocks a native frame reads**, and what ran while paused before:

| Clock | What reads it | Before | Now |
| --- | --- | --- | --- |
| Wall time (`Instant`) | the window's frame interval (`view.rs`, `ui_play`), the sim host's pacing, the theatre thread's turn, the live packets' interpolation alpha, the audio director (`audio_clock`) | ran (the sim host resets its deadline on resume; the theatre's step accumulator grows only while running; the alpha clamps at 1 with no new packet) | unchanged |
| The presentation clock (`Session::clock`: `FrameTime { seconds, dt, frame }`) | every layer: the ocean's own wave time (`Ocean::time += dt`), the wake field, trails, bow waves, hull foam and torpedo tracks (`dt`), the spray (`EffectsFrame::dt`), combat effects and rounds (`dt`), scorch and impact marks, ensigns and radars (`ctx.time.dt`), the sky (`seek(seconds)`), clouds, weather and lightning (`seconds - last`), the eye (`dt`), a theatre's cruise scene (`CruiseSim`: its presented clock moves by the session's step) | held only for a custom battle's pause; ran under the port's menu and while a theatre's clock stood (`self.paused = paused && theatre.is_none()`); the RPC's `step` moved it whatever the pause | held whenever `Session::presentation_held()`: a battle's pause (its Esc menu, the page's, the RPC's `pause`), the port's menu (`set_port_paused`, also `set_paused` in port), a theatre whose clock is paused (the deck's Space or pause button, an alert's pause), at sea or in a battle. `advance_live` and `step` then draw frames at one time with dt 0; the frame index moves on |
| The battle's time (the packets' `time` between the latest two at the alpha) | the sea around hulls, the bow spray's sea, combat effects' `now`, aircraft tracers | stood with the battle (no packets) | unchanged |
| The berth's sea clock (`PortScene::sea_time`) | the berth's ride, the coupling and the spray in port | ran under the port's menu | moves by the presentation step: holds under the menu |
| The camera's time | the rig (zoom, orbit and follow easing, free flight), the pad's look, the gun-aim circles, the input's clock (`Helm::frame`) | the presentation step: a paused battle's camera could not ease or zoom | wall time (`Session::wall_dt`, 0 on a frame drawn again), as the web's `realDt` |
| The clouds' march (`CloudLayer::updates`: the jitter and block order) | every update | marched on: a paused sky shimmered by the jitter | a new frame at the same time is a paused one (`GameSky` passes `set_still`; a frame drawn again keeps its index and the state of the frame it repeats): under a still view the march converges and stops after `HOLD_UPDATES` (240 updates since 2026-10-03, 48 before), keeping its history, so the clouds stand to the bit; a moving camera marches on from its history |

So the custom battle's Esc pause already stood but for the clouds' shimmer (and a camera that could not move), and the
port's menu and a theatre's pause (Space, at sea and in a bubble) ran everything on. **Against the web**: the Esc menu and
the port's Menu now match it. A paused theatre does not: the web's theatre pause stops only the theatre's clock
(`Game.paused` stays false, a bubble's `runningSpeed` is 0), so its sea, wakes, smoke and spray run on at `realDt`; the
native client holds them, as the owner asked. Two things still differ: the eye holds its exposure while paused (it
steps by the presentation's dt; the web's adapts on `realDt`, so a camera turned under the pause re-meters there), and
under temporal anti-aliasing (the default since 2026-10-03) a held frame under a still camera repeats the last frame's
jitter and history, so it stands to the byte too (before, the sea, which takes the current frame alone, danced by the
jitter: the battle case 3.0 levels on 70 % of pixels; `aa-tracers.md`, "Temporal AA as the default"). In port, Escape, a lost capture or focus never pause (the web's handlers skip
the port); its pause is the page's Menu alone.

A theatre scene that comes on screen while the clock stands (the Ship view opened on a paused theatre) stands as it
comes aboard: its trails are laid (`lay_trails`), and its smoke and bow waves build once the clock runs.

**Resume**: the clock goes on from the held time by each frame's step (`advance_live` clamps a frame to 0.1 s, so no
held time is replayed), the sim host and the theatre from where they stood at the wall's pace.

Measured (`pause-freeze.ts`, 960 × 540, Apple M5 Pro, 2026-10-01 22:00, load average 20–30; release client): every
held pair byte-equal in the duel, Denmark Strait's first bubble, its force at sea and the port; a held frame costs what
a running one does, within the loaded machine's noise, never a spike (median CPU 1.2–2.4 ms held against 1.7–2.6
running, GPU 2.1–2.9 against 2.2–2.8 over three runs; the clouds stop marching); on resume each frame steps 1/60 s and
the battle runs at most 2 ticks ahead of the wall time since the resume.

**Checks**: `cargo test -p naval-client --test all -- pause_freeze::` (a live duel paused, two captures two seconds apart byte
equal, a zoom under the pause moves the camera, the clock goes on by a frame's step on resume and the battle at the
wall's pace; the port's menu; a manual Denmark Strait bubble paused by the deck, with a zoom under it);
`ui-contract-check.ts` (the port's Menu and Escape at sea at full ahead each hold the picture, through the page);
`bun native/tools/pause-freeze.ts` (live, through the RPC: the duel, Denmark Strait's first bubble, its force at sea and
the port; stills, a sheet, frame cost and the resume's steps and ticks in `.build/native/pause-freeze/report.json`).

## What is left

- **The wet band beyond the patches**: a hull past the nearest 16 to the camera reads the flat sea (her band is a pixel
  or two by then); the patches smooth away waves shorter than two texels (centimetres high). The band, like the web's,
  follows the water as it stands: no memory of where it has just been (the web's bow spray wetness, `ShipWeather.spray`,
  which does dry over seconds, is not ported).
- ~~**Fog on aircraft and effects**~~ (superseded: they take the aerial perspective, `haze.md`) still read the environment's sharpest level: switch them to `env_fog` (one line
  each, their lanes).
- **Under water**, the aim overlay drawn after the composite is not fogged, and the sky's shafts and veil run before it
  (the web: after).
- **The sea's cost**: the clouds fork measured the ocean at about 4.7 ms a frame at 2400 × 1350 on `naval-client bench`
  (2.1–2.9 ms for its scene pass at 1920 × 1080 here). Profiled on the M5 Pro (2026-10-03, `ocean.md`, "The sea's GPU
  cost on High"): 3.8–6.1 ms at 2592 × 1545, spread over waves, sky lobe, reflections and foam, about 1 ms of it the
  reflections' mere presence; no look-neutral win, so the presets draw it smaller and temporal AA upscales it.

## Tools

- `native/tools/sea-sky/compare.sh <dir> <name> <shot args>`: the shot with master's client, the branch's, and the
  web's (`trio.py`: web | master | branch with the numbers); `LANE=<lane>` writes under `.build/native/<lane>/` and takes
  the clients from its `bin/`. The clients must be built without hot reload
  (`--no-default-features`): a hot-reloading client picks up whatever WGSL is on disk at its first frame, so a master
  binary run in a branch's worktree draws with the branch's shaders.
- `bun native/tools/sea-sky/fogbench.ts [scene] [eye target]`: every pass's GPU cost over 120 frames (`telemetry`'s `gpu`).
- `native/tools/sea-sky/underwater-shots.sh`: the underwater cases beside the web.
- `native/tools/sea-sky/uboat-shots.sh [out] [only]`: the underwater view as a U-boat's captain uses it (the boat at 7,
  10 and 30 m, the lens awash, a keel and a wake from below, a hull 30, 60 and 100 m off with and without the ships, a
  dusk pair), into `.build/native/underwater/<out>/`; `BINARY=` for another client. `uboat-contrast.py <dir>…` reads a
  hull's contrast against the water from the hull views.
- `bun native/tools/sea-sky/lightning.ts`: the sky review's night strike on the native client.
- `bun native/tools/pause-freeze.ts [--only battle|theatre|cruise|port] [--graphics <json>]`: a pause holds the picture
  (above): two stills 2 s apart while paused, a moved camera, the resume; exits 1 when a held pair differs or a resume
  jumps.
- `native/tools/wet-band/`: `diffcrop.py` (the same crop of several stills stacked, with the last two's difference),
  `aligned.py` (the web's still aligned to the branch's on a region the band does not touch, then a region measured on
  all three) and `bench.ts` (each pass's GPU time on master's client and the branch's, interleaved round by round).

## Needs from naval-sim

Nothing so far.
