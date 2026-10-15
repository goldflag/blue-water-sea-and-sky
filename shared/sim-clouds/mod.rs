//! Renderer-free cloud weather and visual obstruction. The native sky uses these same layer profiles and weather
//! texels. Sight rays integrate the coarse weather density, with the march's mean shape fill (0.5), rather than GPU
//! detail noise. Thus gaps are spatial and drift with wind; graphics quality and camera position cannot change sight.
//! The layer port and texture are MIT licensed; see THIRD_PARTY.md.
pub mod layers;
mod types;
pub mod weathers;
pub use types::{CloudGenus, HighCloudGenus, HighClouds};
pub use weathers::CloudWeather;

use crate::{
    catalog::Catalog,
    earth::{ChartVec2, CloudPhase, WorldChart, WorldPoint, calc},
    theatre::content::ScenarioSky,
};
use layers::Layers;
use std::{io::Cursor, sync::OnceLock};

pub const LOCAL_WEATHER_PNG: &[u8] = include_bytes!("local_weather.png");
pub const WEATHER_TILE_M: f64 = 100_000.0;
/// Two percent of the ship's contrast remains. This is a gameplay sight threshold, not a pilot calibration.
const SIGHT_DEPTH: f64 = 3.912;

/// One fixed weather field, its texture laid on the world chart's grid. Time is simulated seconds only.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloudField {
    pub layers: Layers,
    /// The weather texture's phase: it absorbs a bubble's translation (nonzero in a theatre's battle) and the wind's
    /// changes, so the field never jumps. Not a place.
    pub origin: CloudPhase,
    /// The cloud wind on the chart's grid axes, metres a second.
    pub velocity: ChartVec2,
    pub time_offset: f64,
    pub seconds: f64,
}
impl Default for CloudField {
    fn default() -> Self {
        Self::new(CloudWeather::default())
    }
}
impl CloudField {
    pub fn new(weather: CloudWeather) -> Self {
        Self {
            layers: weathers::layers_for(&weather),
            origin: CloudPhase::new(0.0, 0.0),
            velocity: ChartVec2::grid(0.0, 0.0),
            time_offset: 0.0,
            seconds: 0.0,
        }
    }
    /// The forecast's clouds, with the same explicit genus/base/high-sheet overrides as the native scene.
    pub fn resolve(
        catalog: &Catalog,
        map_id: &str,
        forecast: &str,
        sky: Option<&ScenarioSky>,
    ) -> Self {
        let preset = catalog.conditions["weather"]
            .as_array()
            .and_then(|rows| rows.iter().find(|w| w["id"] == forecast));
        let map = catalog.maps["maps"]
            .as_array()
            .and_then(|rows| rows.iter().find(|m| m["id"] == map_id));
        let coverage = sky
            .map(|s| s.cloud_cover / 100.0)
            .or_else(|| preset.and_then(|p| p["sky"]["coverage"].as_f64()))
            .or_else(|| map.and_then(|m| m["sky"]["coverage"].as_f64()))
            .unwrap_or(0.38);
        let high = sky.and_then(|s| s.high_cloud.as_deref()).and_then(|name| {
            let genus = HighCloudGenus::from_name(if name == "none" { "cirrus" } else { name })?;
            let (amount, altitude) = match genus {
                HighCloudGenus::Cirrus => (0.8, 9000.0),
                HighCloudGenus::Cirrostratus => (0.85, 8500.0),
                HighCloudGenus::Cirrocumulus => (0.75, 7500.0),
            };
            Some(HighClouds {
                genus,
                amount: if name == "none" { 0.0 } else { amount },
                altitude,
            })
        });
        let mut field = Self::new(CloudWeather {
            coverage: coverage.clamp(0.0, 1.0) as f32,
            base: sky.and_then(|s| s.cloud_base_m).map(|h| h as f32),
            genus: sky
                .and_then(|s| s.cloud_type.as_deref())
                .and_then(CloudGenus::from_name),
            high,
            fog: if forecast == "fog" { 1.0 } else { 0.0 },
            // Fog and rain already limit every observer through weather visibility. Cloud obstruction reads only
            // the cloud layers, so it does not add quality-dependent rain shafts or alter surface lookouts.
            ..CloudWeather::default()
        });
        // The native preset's cloud wind, along its default world direction (chosen battle wind replaces this).
        field.layers.0[3] = Default::default();
        // Along the grid's east until a battle's wind turns it (`Battle::new_on`, `set_wind`).
        field.velocity = ChartVec2::grid(
            preset.and_then(|p| p["cloudWind"].as_f64()).unwrap_or(12.0),
            0.0,
        );
        field
    }
    /// The cloud wind's speed as [`CloudField::resolve`] sets it, metres a second: its preset's, along the grid's
    /// east.
    pub fn preset_wind_mps(&self) -> f64 {
        self.velocity.grid_xz()[0]
    }
    /// The wind offset the native shader reads, including the translation from local battle to theatre coordinates.
    pub fn drift(&self) -> [f64; 2] {
        self.origin
            .drift(self.velocity, self.seconds + self.time_offset)
    }
    /// Change wind without teleporting the existing weather field.
    pub fn set_velocity(&mut self, velocity: ChartVec2) {
        let drift = self.drift();
        self.velocity = velocity;
        self.origin = CloudPhase::keeping(velocity, self.seconds + self.time_offset, drift);
    }
    /// Whether the clouds leave the sight line from `from` to `to` open. The weather texture lies on `chart`'s grid.
    pub fn clear_line(&self, chart: &WorldChart, from: WorldPoint, to: WorldPoint) -> bool {
        self.optical_depth(chart, from, to) < SIGHT_DEPTH
    }
    /// Integrate each clipped layer separately, so even a thin deck on a long diagonal ray cannot fall between
    /// samples. At most 64 probes per layer, with early termination once the line is opaque. The ray is marched in a
    /// calculation about `from`; each probe samples the weather texture where it lies on `chart`'s grid.
    pub fn optical_depth(&self, chart: &WorldChart, from: WorldPoint, to: WorldPoint) -> f64 {
        calc(from.pos(), |c| {
            let delta = c.delta3(from, to);
            // Its length as the sum of squares (not `hypot`).
            let length = [delta.x(), delta.y(), delta.z()]
                .iter()
                .map(|v| v * v)
                .sum::<f64>()
                .sqrt();
            if length < 1e-6 {
                return 0.0;
            }
            // Heights along the line: its ends', and the chord's sag below the straight run between them, taken at its
            // middle (18 m on a 30 km line), so a layer is clipped where the line truly passes through it.
            let (from_y, delta_y) = (from.height(), to.height() - from.height());
            let sag = (from_y + to.height()) * 0.5 - c.displace3(from, delta * 0.5).height();
            let drift = self.drift();
            let mut depth = 0.0;
            for layer in &self.layers.0 {
                if layer.height <= 0.0 || layer.coverage <= 0.0 {
                    continue;
                }
                let bottom = f64::from(layer.altitude);
                let top = bottom + f64::from(layer.height);
                let spans = if sag > SAG_M {
                    sagging_spans(from_y, delta_y, sag, bottom, top)
                } else if delta_y.abs() < 1e-9 {
                    if from_y <= bottom || from_y >= top {
                        continue;
                    }
                    [(0.0, 1.0), (0.0, 0.0)]
                } else {
                    let a = (bottom - from_y) / delta_y;
                    let b = (top - from_y) / delta_y;
                    [(a.min(b).max(0.0), a.max(b).min(1.0)), (0.0, 0.0)]
                };
                for (start, end) in spans {
                    if end <= start {
                        continue;
                    }
                    let span = (end - start) * length;
                    let steps = (span / 150.0).ceil().clamp(4.0, 64.0) as usize;
                    let step = span / steps as f64;
                    for n in 0..steps {
                        let t = start + (end - start) * (n as f64 + 0.5) / steps as f64;
                        let p = c.displace3(from, delta * t);
                        let [x, z] = p.pos().grid_xz(chart);
                        let texel = weather_texel([x - drift[0], z - drift[1]]);
                        let local = texel
                            .iter()
                            .zip(layer.channels)
                            .map(|(v, w)| v * f64::from(w))
                            .sum::<f64>()
                            .powf(f64::from(layer.weather_exponent));
                        let h = ((p.height() - bottom) / f64::from(layer.height)).clamp(0.0, 1.0);
                        let biased = h.powf(f64::from(layer.shape_altering_bias));
                        let shape = 1.0 - (biased * 2.0 - 1.0).powi(2);
                        let width = f64::from(layer.coverage_filter_width).max(1e-6);
                        let factor = 1.0 - f64::from(layer.coverage) * shape;
                        let weather =
                            ((local * (1.0 - width) + width - factor) / width).clamp(0.0, 1.0);
                        let profile = layer.density_profile;
                        let density = f64::from(profile.exp_term)
                            * (f64::from(profile.exponent) * h).exp()
                            + f64::from(profile.linear_term) * h
                            + f64::from(profile.constant_term);
                        depth += step
                            * 0.5
                            * (weather * f64::from(layer.density_scale) * density).clamp(0.0, 1.0);
                        if depth >= SIGHT_DEPTH {
                            return depth;
                        }
                    }
                }
            }
            depth
        })
    }
}

/// A sag below which a line is taken as straight in height (a millimetre).
const SAG_M: f64 = 1e-3;

/// The spans of a line's parameter `t` in `[0, 1]` where its height `h(t) = from_y + delta_y·t − 4·sag·t(1 − t)`
/// lies between `bottom` and `top`: the line's height is convex in `t`, so it is below `top` over one interval and
/// below `bottom` over one inside it, leaving at most two spans (an empty one is `(0, 0)`).
fn sagging_spans(from_y: f64, delta_y: f64, sag: f64, bottom: f64, top: f64) -> [(f64, f64); 2] {
    let (a, b) = (4.0 * sag, delta_y - 4.0 * sag);
    // Where `h(t) <= level`: the roots of `a·t² + b·t + from_y − level`, or none.
    let below = |level: f64| {
        let disc = b * b - 4.0 * a * (from_y - level);
        (disc >= 0.0).then(|| {
            let root = disc.sqrt();
            ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a))
        })
    };
    let Some((t1, t2)) = below(top) else {
        return [(0.0, 0.0); 2];
    };
    let (t1, t2) = (t1.max(0.0), t2.min(1.0));
    match below(bottom) {
        Some((b1, b2)) => [(t1, t2.min(b1)), (t1.max(b2), t2)],
        None => [(t1, t2), (0.0, 0.0)],
    }
}

/// Decode once, with the same row flip and bilinear repeat sampling as the native texture upload and shader.
pub fn weather_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let mut reader = png::Decoder::new(Cursor::new(LOCAL_WEATHER_PNG))
            .read_info()
            .expect("embedded weather PNG");
        let mut bytes = vec![0; reader.output_buffer_size().expect("weather size")];
        let info = reader.next_frame(&mut bytes).expect("weather texels");
        assert_eq!(
            (info.width, info.height, info.color_type),
            (512, 512, png::ColorType::Rgba)
        );
        let row = 512 * 4;
        (0..512)
            .rev()
            .flat_map(|y| bytes[y * row..(y + 1) * row].iter().copied())
            .collect()
    })
}
fn weather_texel(p: [f64; 2]) -> [f64; 4] {
    let uv = p.map(|v| v.rem_euclid(WEATHER_TILE_M) * 512.0 / WEATHER_TILE_M - 0.5);
    let cell = uv.map(|v| v.floor() as i32);
    let f = uv.map(|v| v - v.floor());
    let bytes = weather_bytes();
    let at = |dx: i32, dy: i32, ch: usize| {
        let x = (cell[0] + dx).rem_euclid(512) as usize;
        let y = (cell[1] + dy).rem_euclid(512) as usize;
        f64::from(bytes[(y * 512 + x) * 4 + ch]) / 255.0
    };
    std::array::from_fn(|ch| {
        let a = at(0, 0, ch) * (1.0 - f[0]) + at(1, 0, ch) * f[0];
        let b = at(0, 1, ch) * (1.0 - f[0]) + at(1, 1, ch) * f[0];
        a * (1.0 - f[1]) + b * f[1]
    })
}
