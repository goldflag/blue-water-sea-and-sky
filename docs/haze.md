# Haze lane: one aerial perspective for everything in the world

Bill asked for proper distance haze (2026-10-03). Before this lane every system hazed differently: hulls, aircraft, the
sea and the land faded along the web's fog ramp (`pow((d − start) / (end − start), 1.4)`) toward the sky's far fog
map; effects toward the environment's sharpest level; spray and splashes not at all; the clouds through takram's
aerial perspective plus takram's thin haze, which the clouds' composite also laid over the sea and the hulls in front
of the cloud base (two hazes stacked); and the atmosphere's own `aerial()` had no callers. Past the fog's end a hull
took a fog colour that was not the sky behind it, so ships stayed visible beyond the visibility (a Bismarck 20 km off
in 17 km of visibility kept a fifth of her contrast).

Now one model draws the air between the eye and everything in the world: ships and aircraft, the sea, land, trees and
smoke, fire and gas, spray, splashes and bow spray, shells, tracers and debris, the dome and the clouds.

## The model

Along each view ray the air is two media.

- **The atmosphere** the dome and the clouds already take (takram's tables, `naval-sky`): molecules, the map's
  clear-air aerosol (its turbidity) and ozone. Its in-scatter up to a point is `sky_radiance_to_point` (graded as the
  clouds' aerial perspective is: the dome's shoulder and night floor). A view sees its `aerial_scale` share of it, half
  on a fair day ("Clear air aloft" below); the dome and the light take all of it.
- **The weather's haze** (`naval_sky::haze::Haze`): a marine boundary layer of aerosol, densest at the sea and thinning
  with height (scale height 250 m, `Haze::SURFACE`; 1 km until 2026-10-09) up to the layer's top, which is the
  weather's cloud base held to 400–1500 m (a marine boundary layer's clouds form at its top), then quickly above it
  (250 m). Its extinction at sea level is what the game's visibility V asks less the atmosphere's own there as a view
  sees it: `SIGHTING_DEPTH / V` for the two together (one constant, "Calibration" below), none where the atmosphere
  alone stops that much. Single-scattering albedo 0.95,
  Cornette–Shanks phase with `g = 0.75` (sea salt and sulphate droplets scatter nearly all they stop, strongly
  forward).

The haze is lit by:

- the sun and the moon through the atmosphere (the transmittance table, the twilight lift and the grade's gain:
  `direct_scale`, `moon_scale`, as the clouds take them), through the haze above the point (delta-Eddington scaled:
  what the haze scatters forward stays in the beam), and through the clouds (their ground shadow map down the light's
  rays; beyond the map's 40 km the map's mean rather than full light, so the far haze under a deck stays shaded);
- the sky's light: the environment (the sky above, the sea below, the clouds included) weighted by the haze's phase,
  a quadrature over bands of angle from the ray (the cone within 25° from a 12° level, rings at 50° and 105° and the
  back from the roughest level), so forward scattering takes the bright band of sky right behind the haze.

The haze dims the atmosphere's in-scatter by the haze in front of where the atmosphere scatters (its transmittance
weighted by the atmosphere's scattering along the ray), and the atmosphere dims the haze's. The tables light all the
air with the sun; where the air lies under the clouds that cast the ground shadow (under their base: the map's shade,
over their top: none, between: in proportion), the shadow takes the sun's share of the atmosphere's in-scatter too, and
the sky's light under the clouds (the environment, which holds them) lights that air instead. Under a closed deck the
far air is the deck's grey, not the sunlit band at the horizon the clouds lane saw (`clouds.md`). The sun's and the moon's
light at the sea pass through the same haze (`Sky::set_weather_haze` takes `SeaLevel` again through
`Haze::beam_transmittance`), so a low sun dims on the hulls as it does in the haze around them.

| Weather | V | Atmosphere at sea (550 nm) | Its share a view sees | Haze at sea (A) | Layer top | Haze optical depth up | Sun on the sea at 70°, 20°, 3° |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Clear | 35 km | 0.033 / km | 0.50 | 0.053 / km | 1500 m | 0.01 | 1.00, 0.99, 0.93 |
| Partly cloudy | 25 km | 0.056 / km | 0.50 | 0.069 / km | 1500 m | 0.02 | 0.99, 0.99, 0.91 |
| North Atlantic's own | 25 km | 0.056 / km | 0.50 | 0.069 / km | 1500 m | 0.02 | 0.99, 0.99, 0.91 |
| Overcast | 13 km | 0.128 / km | 0.86 | 0.076 / km | 950 m | 0.02 | 0.99, 0.98, 0.90 |
| Storm clouds | 8 km | 0.215 / km | 1.00 | 0.088 / km | 650 m | 0.02 | 0.99, 0.98, 0.89 |
| Fog | 4.5 km | 0.318 / km | 1.00 | 0.220 / km | 650 m | 0.05 | 0.98, 0.95, 0.74 |

Until 2026-10-09 a view saw all of the atmosphere and the haze thinned over 1 km: the haze at sea was 0.036, 0.041,
0.058, 0.089 and 0.220 per km (clear to fog), its optical depth up 0.03-0.13, and the sun on the sea at 3° 0.85, 0.83,
0.80, 0.75 and 0.48.

The fair weathers' visibilities rose later on 2026-10-03 to realistic ones (an ordinary open-ocean day is about
25 km, clear polar air 35–50 km): North Atlantic 17 → 25 km, Clear 28 → 35, Partly cloudy 20 → 25. Their haze at sea
was 0.086, 0.053 and 0.065 per km before, the sun on the sea at 3° 0.67, 0.78 and 0.74.

## Clear air aloft (2026-10-09)

Bill asked to fix the haze over the land seen from the air: from 1,500-3,000 m the land 10-30 km off was pale and
frosted, the biggest gap left to Microsoft Flight Simulator after the land-realism lanes (`terrain.md`). Measured
first (the theatre's California, Sagami Bay and Guadalcanal scenarios at their own 30-40 km visibilities, Ultra, the
land's display luminance in a box of each frame, against the same frame with the air off):

- The model was physically consistent. The transmittance fitted from the frames matched the extinction along the
  slant (0.56-0.69 at 550 nm over 15.6 km from 1,500 m, against 0.56 computed); the tables' in-scatter was exactly
  twice a marched single scattering of the sun in every view (the higher orders, the skylight, as much again as the
  sun near the ground); its radiance per unit of opacity sat just under the horizon sky's. Nothing was wrong; the air
  was simply this thick.
- Most of the veil was the tables' air, not the weather's haze: with the haze off Santa Monica's land kept 28.5 of
  its 41.6 (standard deviation, 8-bit), against 27.0 with it. The haze's profile alone (a 700 m mixed layer, a 300 m
  scale height) and a quarter of the map's aerosol moved the views by a level or two, because the visibility pins the
  extinction at the sea: what one medium gave up, the other took.

So the change is a choice, the web sky's in another form (its `AERIAL` scaled the distance beyond 15 km by a quarter
on fair days):

- **A view sees half of the tables' air on a fair day** (`model::aerial`, `AirCoefficients::aerial_scale`: half up to
  turbidity 2.5, all of it from 4.5, so overcast takes 0.86 and storm and fog all). The volume's opacity is that
  share's; its in-scatter is the tables' times their opacity at that share over their own (`thinned` in
  `aerial.wgsl`), so the air's colour at a distance is unchanged and only its depth thinner. The dome, the sun's and
  the sky's light, the environment and the clouds' own lighting keep all of it.
- **The haze takes the rest at the sea** (`Haze::extinction_at_sea` over the share a view sees), so the extinction at
  sea level is the same and a ship still goes at V (the sea-level views below moved a level or two).
- **The haze is a surface layer** (scale height 250 m, `Haze::SURFACE`): what the visibility at the sea is made of
  (sea salt, droplets swollen in the damp air) lies in the lowest few hundred metres; above it the air is clearer. A
  low sun crosses less of it, so the sun on the sea at 3° rose from 0.85 to 0.93 in clear weather.

| View (land box, 8-bit) | Air off | Before | After |
| --- | --- | --- | --- |
| Hakone's ridges from 700 m, standard deviation | 53.5 | 31.5 | 42.6 |
| Santa Monica from 1,500 m, standard deviation | 40.1 | 26.4 | 32.8 |
| The Los Angeles basin from 3,000 m, standard deviation | 26.2 | 5.6 | 9.6 |
| Honiara from 300 m, standard deviation | 41.7 | 24.1 | 30.0 |
| Guadalcanal's coast from 1,500 m, mean luminance | 75.7 | 117.9 | 102.6 |
| Palos Verdes through binoculars from a bridge, standard deviation | | 26.0 | 29.0 |
| Honiara's shore from a bridge, standard deviation | | 4.8 | 5.8 |

What it costs: a `pow` a slice. The study's captures are in `.build/haze/` (scratch harness `zz_haze_study.rs`, never
committed).

## Calibration (`naval_sky::haze::SIGHTING_DEPTH`)

One constant turns the game's visibility V into the air: the optical depth at sea level (550 nm) of the atmosphere
and the haze together over V. Everything else in the model is physics; this is a choice about what the game's
visibility means, and Bill picked **A** (2026-10-03) after seeing both rendered with the same shots.

- **A, 2.42** (the game's): ln(0.45 / 0.04): a ship goes at about V, where the simulation's lookouts lose her (`naval-sim`
  sensors read the same fog end). Her darker parts (0.45 contrast against the horizon close up) fall to 4 % there,
  what a target a fraction of a degree high needs (twice Koschmieder's 2 %, the eye's on a large one: Blackwell 1946);
  a black object's 2 %, the meteorological visibility, lies at 1.62 V. The look changes a lot: in the North Atlantic's
  17 km (its visibility until the fair weathers rose to 25 km) a broadside Bismarck at 10 km is a ghost through
  binoculars.
- **B, 1.2** (not taken; kept for the record): the old visual range. Clearer air for the same V: the weather adds a little haze
  in fair weather (0.004–0.015 per km) and none where the map's own air is thicker (overcast, storm, fog, whose
  turbidity closes the view at about V by itself). In 17 km a broadside Bismarck keeps about half of
  master's contrast and most of her pixels over 5 % out to 20 km, and fades out past it; ships stay visible past the
  lookouts' range in fair weather, as on master.

The visibility is the scene's fog end, as before: the weather's (`assets/maps/battle-conditions.v1.json`), the map's
(`environments.v1.json`), a custom battle's visibility setting, the port's 16.2 km (`PORT_FOG`, which followed a
22 km meteorological visibility: 20.4 km here). `naval-client`'s `set_fog` hands it to
`GameSky::set_visibility`; the rest of the authored fog (colour, start, power, sky blend) is no longer drawn.

## The volume

A froxel volume over the frame's camera (Hillaire 2020, "A Scalable and Production Ready Sky and Atmosphere Rendering
Technique"), `naval_render::aerial`:

- 32 × 32 columns over the screen, each column's ray in 32 slices of radial distance from the eye, exponential from 20 m
  to 450 km (`AERIAL_FAR`; each slice 38 % farther than the last). It reached 160 km (each slice a third farther) until
  2026-10-09, when a theatre view began drawing the land beyond its window out to 450 km (native/docs/terrain.md, "The
  horizon ring"): a far peak takes the air along its whole distance from the same model as the window's land, so it is a
  faint silhouette on a clear day and gone in haze or fog with no fade of its own. At a given distance a slice is 15 %
  deeper than before (no difference seen on land 15 km off). One `Rgba16Float` 3D texture of 32 × 32 × 64 holds the
  in-scatter (first 32 layers) and the opacity, 1 − transmittance (next 32): zeros are clear air, so a new volume, or a
  host's stand-in, draws as without air. Beside it a 32 × 32 dome slice: the haze along each column's ray to the end of
  the air (rgb in-scatter, alpha opacity).
- The renderer owns it and binds it at group 0 beside the camera (bindings 6 and 7), so every scene pipeline samples
  it by its fragment's world position: `naval::aerial::aerial_apply(color, world)` (straight alpha over what is behind is
  exact: what is behind took the air beyond the fragment already) and `aerial_apply_additive` (light added to what is
  behind: glows, flashes, tracers; the air only dims it). Nearer than the first slice a fragment takes that slice's
  share of it.
- naval-clouds' `AerialPerspective` (`src/aerial.rs`, `shaders/aerial.wgsl`) fills it each frame in `GameSky`'s
  prepare, after the sky's tables, the environment and the clouds' shadows: workgroups of 2 × 2 columns × 32 slices,
  each thread one slice interval in four steps, a scan down the column; the last slice's thread goes on to the end of
  the air for the dome slice.
- **A column whose ray meets the sea** (2026-10-04) is held at the horizon (`naval_sky::atmosphere::held`, as the
  dome's own sky is) where nothing in the world is drawn along it: its dome slice, and its slices wholly past the
  sea, take the air along the horizon's ray to their distance. The pixels just above the horizon (the dome, and the
  clouds 20–100 km off, which take their air at their own distance) read the columns below it bilinearly. Before,
  those columns held the far sea (the dome slice, opaque) and the air followed under the sea (the slices, the haze
  at its densest all the way), so a band over the horizon a column deep took them in the share the horizon stood
  within the columns' rows, and brightened and darkened as the view pitched (the port's menu camera showed it).
- The dome and the clouds' march read a buffer copy of the volume in the sky's own group (`naval_sky::aerial`, group 1
  binding 12, read by hand: bilinear over a layer, linear between slices): the march already samples as many textures
  as a stage may (16), and so do the environment bake and the dome. The dome draws `color × (1 − opacity) + in-scatter`
  from the dome slice; the march takes the air in front of the clouds' front (`aerial_along(uv, front_depth)`) in place
  of `apply_aerial_perspective`, so a cloud and a hull at the same distance take the same air.
- Under the sea the volume holds clear air (the water's own view takes over).

What went: the far fog map (the environment's binding 3, its bake `fog_main`, `fog_fill.wgsl`, `env_fog`): the
volume's texture in group 0 took its slot under the stage's sampled-texture limit, which the sea's pipeline sat at.
The fog ramp functions in `naval::lights` (`SceneFog` stays as the per-layer switch). takram's thin haze (the cloud
tiers' `haze` is off: the volume replaces it, in the march and in the composite's rule over the opaque scene).

## Measured

Apple M5 Pro (Metal), 2026-10-03.

**GPU cost**: the aerial-perspective pass 0.122 ms p50, 0.149 p90, 0.236 max in `naval-client bench --scene duel
--size 2400x1350 --seconds 20` (load average 14.5–14.9); earlier runs 0.104–0.123 ms p50 (load 8–25). It does not grow
with the resolution. The materials' two trilinear reads a fragment were within the
bench's noise under this load (the `scene water, sky` pass moved 6–12 ms between identical runs).

**Contrast** of a Bismarck broadside against what is behind her, the North Atlantic's own weather (17 km visibility
then, 25 km since),
noon, from 25 m, a lens keeping her 40 % of the frame wide (`.build/native/look-haze/shots.ts contrast`): the mean
display-luminance Weber contrast over her pixels, and the shares of her pixels over 2 % and over 5 %.

Mean contrast, then the share of her pixels over 5 %:

| Range | Master | A (2.42) | B (1.2) |
| --- | --- | --- | --- |
| 2 km | 0.353, 76 % | 0.186, 61 % | 0.230, 63 % |
| 5 km | 0.240, 56 % | 0.102, 50 % | 0.157, 53 % |
| 10 km | 0.267, 63 % | 0.059, 52 % | 0.135, 57 % |
| 15 km | 0.252, 90 % | 0.041, 42 % | 0.123, 84 % |
| 20 km | 0.198, 89 % | 0.022, 2 % | 0.089, 82 % |

On master she stayed plain past 20 km in 17 km of visibility (the ramp's fog colour was not the sky behind her). With
A she fades from the eye on and goes at about the visibility (almost none of her pixels over 5 % at 20 km). With B she
keeps most of her pixels over 5 % to 20 km at about half of master's mean contrast and fades out past it (the
atmosphere's 0.07 per km: 5 % of mean contrast near 28 km). Nearer than 10 km both are fainter than master, whose ramp
left the first 2.8 km clear. The other weathers with A at their own V (`--weather <id> --ranges`, before the clouds
lane's weather layers merged): storm clouds (V 8 km) 9 % over 5 % at 8 km, none at 10 km; fog (V 4.5 km) 12 % at
4.5 km, none at 6 km; overcast (V 13 km) 60 % at 13 km, 9 % at 16 km. With B those three are the map's air alone,
which closes at about the same distances.

## Stills

`.build/native/look-haze/shots.sh <dir>` (`shots.ts` beside it holds the cameras), before on master 42b7d3d3c, after
on this branch merged with master 594fbaccf..e2736e5b7 (the clouds lane's weather layers, fog banks and rain): A in
`after/`, B in `after-b/`, before / A / B stacked in `triples/`; `extra.sh` for the views that must keep working
(under water, the port, the theatre's cruise, the chase camera): `before-extra/`, `after-extra/` (A, before the merge).
The storm and fog stills differ from before in their clouds too (weather layers, fog banks, rain).

## Long range (2026-10-06)

Checked for theatre gun actions at 20–30 km (the fleet action realism programme). The haze closes at each battle's own
visibility, not at a fixed 15–18 km (that was the web game's fog ramp before this lane): a custom battle's visibility
setting, its weather's, a theatre bubble's `BattleSetup.visibility_m` (`theatre/mod.rs`, "The haze closes at the
bubble's visibility") and the cruise scene's visibility over the force (`cruise::conditions`). Through the binoculars
(`bun native/tools/long-range.ts`, a Hipper broadside on from 59 m, noon, `.build/native/long-range/after-*`):

- Clear (35 km): she is plain at 20 km and 25 km at 16× and recognisable by her profile at 29 km, her hull sitting on
  the horizon (from 59 m it is 27 km off); at 40 km only her masts and bridge top show over it.
- North Atlantic's own (25 km): plain at 20 km, a ghost at 25 km, nearly gone at 29 km.
- 18 km (Denmark Strait's scenario): faint at 20 km, gone past 25 km.

Her contrast falls as calibration A means; nothing here changed it. A scenario that wants a 30 km action in sight
needs a visibility of 35 km or more.

## Open problems

- **The environment holds no haze**: the sea's reflections and the hulls' ambient see the clear sky's horizon, a
  little bluer than the hazed dome just above it. Baking the haze into the environment would close the loop (and give
  the haze its own light back: its multiple scattering is in only as delta-Eddington's forward beam).
- **From aloft** the haze layer's top shows as a grey-brown band just under the horizon: the haze lit from the side
  scatters little toward the eye at `g = 0.75`; real marine haze layers seen from above look much the same, but the
  band is the one place the model's missing multiple scattering shows.
- **Ships are fainter at every range than on master**: contrast falls from the eye on (the ramp held it to its
  start), and with A a ship goes at the visibility the simulation spots at; a black silhouette against a low sun stays
  in sight farther than a grey hull in flat light, as she would at sea. The weathers' visibilities are the knob (the
  fair ones rose to North Atlantic 25 km, Clear 35 km and Partly cloudy 25 km on 2026-10-03).
- **The clouds' environment bake** keeps the atmosphere's own aerial perspective (`apply_aerial_perspective` in
  `hooks.wgsl`), without the haze, and lights the air under a deck with the sun: the environment's horizon under an
  overcast is brighter than the dome's, and the sky light the volume takes from it under a deck carries a little of
  that.
- **Into a low sun** the haze's forward glow (g = 0.75 scatters some 30 times an even share 3° from the sun) puts the
  horizon about 2.5 stops over middle grey, in AgX's shoulder at the fixed exposure the eye keeps into a sunset: a hull
  6 km off stands −0.4 against it in radiance but about 15 % on screen (`dusk-toward`). The display's and the eye's
  to weigh, not the haze's: since 2026-10-05 the eye closes for glare by day and the bloom is a third as strong
  (`lighting.md`, "Into the sun", which also measures the haze against a theatre's 46 km).
- **The authored fog's other terms** (`SceneFog`, `FogParameters`: colour, start, power, sky blend) still travel to the
  layers' uniforms and are not drawn; only the switch and the end are read.
- The weather-sky lane's fog banks are clouds: they take the air in front of them from the volume like any cloud, and
  sit inside the haze.
