# Third-party code

The repository's own code is under the MIT licence in `LICENSE`. Parts of it are ports of other people's work,
and their notices stay with the files they cover:

| Files | Derived from | Licence | Notice |
| --- | --- | --- | --- |
| `sky/shaders/bruneton.wgsl`, `sky/shaders/precompute.wgsl`, `sky/src/tables.rs` | takram three-geospatial's atmosphere, itself based on Eric Bruneton's Precomputed Atmospheric Scattering | MIT; BSD 3-Clause | [sky/THIRD_PARTY.md](sky/THIRD_PARTY.md) |
| `sky/data/bright_stars.tsv` | Yale Bright Star Catalogue, 5th revised ed. (CDS catalogue V/50) | Public catalogue data | [sky/THIRD_PARTY.md](sky/THIRD_PARTY.md) |
| `clouds/shaders/*`, `clouds/src/*` (march, shadow maps, temporal passes, haze, cascades), `clouds/assets/shape.png` | takram `@takram/three-clouds`, with TileableVolumeNoise, three-csm, volsample and Playdead's temporal | MIT | [clouds/THIRD_PARTY.md](clouds/THIRD_PARTY.md) |
| `shared/sim-clouds/layers.rs`, `shared/sim-clouds/local_weather.png` | takram `@takram/three-clouds` | MIT | [shared/sim-clouds/THIRD_PARTY.md](shared/sim-clouds/THIRD_PARTY.md) |

The ocean (`ocean/`) is original work. `clouds/assets/blue_noise.png` and the clouds' detail noise
(`clouds/shaders/detail_noise.wgsl`) are original too; takram's NVIDIA spatiotemporal blue noise texture, which is
licensed for non-commercial use only, is neither used nor included.
