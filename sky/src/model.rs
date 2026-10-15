//! The air as numbers, renderer-free: the planet, takram's atmosphere (Bruneton's model) and the coefficients an
//! authored scene maps onto it, optical depth, the units the tables are read in, the twilight exposure, the dome's
//! grade and the light that reaches the sea. The scattering itself is precomputed on the GPU
//! (`shaders/precompute.wgsl`); this is the CPU side, in f64.
//!
//! Lengths are kilometres and coefficients per kilometre; the RGB channels stand for 680, 550 and 440 nm.

use glam::DVec3;

pub type Rgb = [f64; 3];

/// Sea-level radius of the planet (km, takram's `AtmosphereParameters.DEFAULT`) and the top of its air: 100 km up
/// as the game's sky had it, rather than takram's 60 km, so the air over a camera in the blue hour (in the planet's
/// shadow up to about 60 km with the sun 8° down) is still lit.
pub const PLANET_RADIUS: f64 = 6360.0;
pub const ATMOSPHERE_TOP: f64 = 6460.0;
/// Scale heights (km) of the molecules and of the aerosol (takram's exp_scale −1/8 and −1/1.2).
pub const RAYLEIGH_HEIGHT: f64 = 8.0;
pub const MIE_HEIGHT: f64 = 1.2;
/// Ozone's tent: absorption peaks at 25 km and falls to nothing 15 km above and below.
pub const OZONE_PEAK: f64 = 25.0;
pub const OZONE_HALF_WIDTH: f64 = 15.0;
/// takram's air at sea level: molecules, the clear-sky aerosol (and its single-scattering albedo).
const EARTH_RAYLEIGH: Rgb = [5.802e-3, 13.558e-3, 33.1e-3];
const CLEAR_MIE: f64 = 3.996e-3;
const MIE_ALBEDO: f64 = 3.996e-3 / 4.44e-3;
/// Ozone's absorption: the game's grade over takram's (0.65e-3, 1.881e-3, 0.085e-3). The red channel stands for
/// broadband red, which the Chappuis band absorbs far more than at 680 nm; at takram's value the blue hour turns
/// magenta (more so under its red-weighted sky factors), at this one it stays blue.
const OZONE_ABSORPTION: Rgb = [2.4e-3, 3e-3, 0.136e-3];
/// The sun above the air (takram's solar irradiance at 680, 550 and 440 nm), its angular radius (rad), the
/// ground's albedo and the lowest sun the tables hold (cos 120°).
pub const SOLAR_IRRADIANCE: Rgb = [1.474, 1.8504, 1.91198];
pub const SUN_ANGULAR_RADIUS: f64 = 0.004675;
pub const GROUND_ALBEDO: f64 = 0.1;
pub const MU_S_MIN: f64 = -0.5;
/// The maritime haze an authored turbidity adds over the clear aerosol (per km at sea level, `turbidity` 1 adds
/// none): the game's own scale, kept so each map's air reads as it did.
const AEROSOL_PER_TURBIDITY: f64 = 0.027;
const AEROSOL_HAZE: f64 = 5.0;
/// Albedo of the sea under diffuse light.
pub const SEA_ALBEDO: f64 = 0.06;

/// takram's radiance-to-luminance factors (`sunRadianceToLuminance`, `skyRadianceToLuminance`), which it divides by
/// the luminance of the sun's unit radiance so they fit half floats (`…ToRelativeLuminance`).
const SUN_RADIANCE_TO_LUMINANCE: Rgb = [98242.786222, 69954.398112, 66475.012354];
const SKY_RADIANCE_TO_LUMINANCE: Rgb = [114974.916437, 71305.954816, 65310.548555];

/// Rec. 709 luminance.
pub fn luminance(c: Rgb) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// `sunRadianceToRelativeLuminance`.
pub fn sun_relative_luminance() -> Rgb {
    let unit = luminance(SUN_RADIANCE_TO_LUMINANCE);
    SUN_RADIANCE_TO_LUMINANCE.map(|value| value / unit)
}

/// `skyRadianceToRelativeLuminance`.
pub fn sky_relative_luminance() -> Rgb {
    let unit = luminance(SUN_RADIANCE_TO_LUMINANCE);
    SKY_RADIANCE_TO_LUMINANCE.map(|value| value / unit)
}

/// The sun above the air in the tables' relative luminance.
pub fn solar_luminance() -> Rgb {
    let factors = sun_relative_luminance();
    std::array::from_fn(|c| SOLAR_IRRADIANCE[c] * factors[c])
}

/// Game units per unit of the tables' relative luminance: the sun's luminance above the air is the scene's
/// `sun_intensity` (the sea's units, about 6 by day).
pub fn game_scale(sun_intensity: f64) -> f64 {
    sun_intensity / luminance(solar_luminance())
}

/// The game's white balance over takram's colours: the sun above the air is white in the game (the scene light,
/// the sea's glints and the ships' light all take it so), while takram's luminance makes it warm (its solar
/// spectrum in relative luminance is about 1.91, 1.71, 1.68). Per channel, the sun's luminance over its colour.
pub fn white_balance() -> Rgb {
    let solar = solar_luminance();
    let level = luminance(solar);
    solar.map(|channel| level / channel)
}

/// Gains from the authored scene onto the model (`AUTHORED` in model.ts).
pub mod authored {
    pub const RAYLEIGH_EARTH: f64 = 0.4;
    pub const RAYLEIGH_DENSITY: f64 = 0.6;
    pub const RAYLEIGH_CHROMA: f64 = 0.5;
    pub const MIE_UNIT: f64 = 0.25;
    pub const MIE_EXPONENT: f64 = 0.7;
    pub const MIE_G_MAX: f64 = 0.85;
}

/// The share of the tables' air a view ray sees (`AirCoefficients::aerial_scale`): `FAIR` of it in a fair weather's
/// (turbidity up to `FROM`), all of it in a hazy one's (`TO` and over), smoothly between. The dome and the light keep
/// all of it. An art choice toward Microsoft Flight Simulator's clear distance (2026-10-09, `native/docs/haze.md`,
/// "Clear air aloft"): seen from the air, the land 10-30 km off kept a third of its contrast under the tables' whole
/// air, molecules and aerosol at sea-level density along a low slant. The web's sky scaled the distance beyond 15 km
/// by a quarter on fair days (`AERIAL` in model.ts).
pub mod aerial {
    pub const FAIR: f64 = 0.5;
    pub const FROM: f64 = 2.5;
    pub const TO: f64 = 4.5;
}

/// A scene's authored atmosphere (`SkyScene.atmosphere`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuthoredAtmosphere {
    /// Molecular scattering; Sky Pro's scale, where 0.4 is physical Earth.
    pub rayleigh: f64,
    /// Aerosol load: 1 very clear, 10 heavy haze.
    pub turbidity: f64,
    /// Strength of the forward aureole around the sun.
    pub mie: f64,
    /// Asymmetry of the aerosol, 0–0.99.
    pub mie_g: f64,
    /// Multiple-scattering fill of the dome.
    pub multiple: f64,
}

/// What the air is made of for one scene: the coefficients the tables are computed for (`rayleigh`,
/// `mie_scattering`, `mie_extinction`, `ozone`, `mie_g`) and the authored terms the dome applies over them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirCoefficients {
    pub rayleigh: Rgb,
    pub mie_scattering: Rgb,
    pub mie_extinction: Rgb,
    pub ozone: Rgb,
    pub mie_g: f64,
    pub mie_gain: f64,
    pub multiple: f64,
    /// The share of this air a view ray sees ([`aerial`]); the weather's haze takes the rest of what the visibility
    /// asks at the sea (`crate::haze::Haze::extinction_at_sea`).
    pub aerial_scale: f64,
    pub chroma: f64,
}

pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    // three's MathUtils.smoothstep: 0 at or below edge0, 1 at or above edge1 (edges in either order).
    if x <= edge0 {
        return 0.0;
    }
    if x >= edge1 {
        return 1.0;
    }
    let t = (x - edge0) / (edge1 - edge0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Map a scene's authored atmosphere onto takram's: the molecules by the authored blue, the aerosol as takram's
/// clear sky plus the maritime haze of the authored turbidity, ozone as takram's.
pub fn air_coefficients(atmosphere: &AuthoredAtmosphere) -> AirCoefficients {
    let blue = atmosphere.rayleigh.max(0.0) / authored::RAYLEIGH_EARTH;
    let molecules = blue.powf(authored::RAYLEIGH_DENSITY);
    let excess = (atmosphere.turbidity - 1.0).max(0.0);
    let aerosol =
        CLEAR_MIE + AEROSOL_PER_TURBIDITY * excess * (1.0 + (excess / AEROSOL_HAZE).powi(2));
    let mie_scattering = [aerosol; 3];
    AirCoefficients {
        rayleigh: EARTH_RAYLEIGH.map(|value| value * molecules),
        mie_scattering,
        mie_extinction: mie_scattering.map(|value| value / MIE_ALBEDO),
        ozone: OZONE_ABSORPTION,
        mie_g: atmosphere.mie_g.clamp(0.0, authored::MIE_G_MAX),
        mie_gain: (atmosphere.mie.max(0.0) / authored::MIE_UNIT).powf(authored::MIE_EXPONENT),
        multiple: atmosphere.multiple.max(0.0),
        aerial_scale: lerp(
            aerial::FAIR,
            1.0,
            smoothstep(aerial::FROM, aerial::TO, atmosphere.turbidity),
        ),
        chroma: blue.powf(authored::RAYLEIGH_CHROMA),
    }
}

/// One layer of a density profile (`DensityProfileLayer` in `shaders/bruneton.wgsl`, padded to 32 bytes):
/// `exp_term * exp(exp_scale * h) + linear_term * h + constant_term`, clamped to [0, 1].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DensityProfileLayer {
    pub width: f32,
    pub exp_term: f32,
    pub exp_scale: f32,
    pub linear_term: f32,
    pub constant_term: f32,
    pub padding: [f32; 3],
}

impl DensityProfileLayer {
    const fn new(
        width: f32,
        exp_term: f32,
        exp_scale: f32,
        linear_term: f32,
        constant_term: f32,
    ) -> Self {
        Self {
            width,
            exp_term,
            exp_scale,
            linear_term,
            constant_term,
            padding: [0.0; 3],
        }
    }

    fn density(&self, altitude: f64) -> f64 {
        (f64::from(self.exp_term) * (f64::from(self.exp_scale) * altitude).exp()
            + f64::from(self.linear_term) * altitude
            + f64::from(self.constant_term))
        .clamp(0.0, 1.0)
    }
}

/// Two layers, the lower below its `width` (`DensityProfile`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DensityProfile {
    pub lower: DensityProfileLayer,
    pub upper: DensityProfileLayer,
}

impl DensityProfile {
    pub fn density(&self, altitude: f64) -> f64 {
        if altitude < f64::from(self.lower.width) {
            self.lower.density(altitude)
        } else {
            self.upper.density(altitude)
        }
    }
}

/// takram's profiles: molecules and aerosol fall off exponentially, ozone is a tent peaking at 25 km.
pub const RAYLEIGH_DENSITY: DensityProfile = DensityProfile {
    lower: DensityProfileLayer::new(0.0, 0.0, 0.0, 0.0, 0.0),
    upper: DensityProfileLayer::new(0.0, 1.0, -0.125, 0.0, 0.0),
};
pub const MIE_DENSITY: DensityProfile = DensityProfile {
    lower: DensityProfileLayer::new(0.0, 0.0, 0.0, 0.0, 0.0),
    upper: DensityProfileLayer::new(0.0, 1.0, -0.833_333, 0.0, 0.0),
};
pub const ABSORPTION_DENSITY: DensityProfile = DensityProfile {
    lower: DensityProfileLayer::new(25.0, 0.0, 0.0, 1.0 / 15.0, -2.0 / 3.0),
    upper: DensityProfileLayer::new(0.0, 0.0, 0.0, -1.0 / 15.0, 8.0 / 3.0),
};

/// `AtmosphereParameters` as the shaders read it (`shaders/bruneton.wgsl`, kilometres): 304 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AtmosphereParameters {
    pub solar_irradiance: [f32; 3],
    pub sun_angular_radius: f32,
    pub bottom_radius: f32,
    pub top_radius: f32,
    pub mie_phase_function_g: f32,
    pub mu_s_min: f32,
    pub rayleigh_scattering: [f32; 3],
    pub padding_a: f32,
    pub mie_scattering: [f32; 3],
    pub padding_b: f32,
    pub mie_extinction: [f32; 3],
    pub padding_c: f32,
    pub absorption_extinction: [f32; 3],
    pub padding_d: f32,
    pub ground_albedo: [f32; 3],
    pub padding_e: f32,
    pub rayleigh_density: DensityProfile,
    pub mie_density: DensityProfile,
    pub absorption_density: DensityProfile,
}

impl AtmosphereParameters {
    /// The tables' air for a scene's coefficients.
    pub fn new(air: &AirCoefficients) -> Self {
        let f = |c: Rgb| c.map(|v| v as f32);
        Self {
            solar_irradiance: f(SOLAR_IRRADIANCE),
            sun_angular_radius: SUN_ANGULAR_RADIUS as f32,
            bottom_radius: PLANET_RADIUS as f32,
            top_radius: ATMOSPHERE_TOP as f32,
            mie_phase_function_g: air.mie_g as f32,
            mu_s_min: MU_S_MIN as f32,
            rayleigh_scattering: f(air.rayleigh),
            mie_scattering: f(air.mie_scattering),
            mie_extinction: f(air.mie_extinction),
            absorption_extinction: f(air.ozone),
            ground_albedo: [GROUND_ALBEDO as f32; 3],
            rayleigh_density: RAYLEIGH_DENSITY,
            mie_density: MIE_DENSITY,
            absorption_density: ABSORPTION_DENSITY,
            ..Default::default()
        }
    }
}

/// Relative densities at an altitude (km): molecules, aerosol, ozone (takram's profiles).
pub fn densities(altitude: f64) -> Rgb {
    [
        RAYLEIGH_DENSITY.density(altitude),
        MIE_DENSITY.density(altitude),
        ABSORPTION_DENSITY.density(altitude),
    ]
}

/// Extinction (per km) at an altitude.
pub fn extinction(air: &AirCoefficients, altitude: f64) -> Rgb {
    let [rayleigh, mie, ozone] = densities(altitude);
    std::array::from_fn(|c| {
        air.rayleigh[c] * rayleigh + air.mie_extinction[c] * mie + air.ozone[c] * ozone
    })
}

/// Distance (km) from radius `r` along a ray with zenith cosine `mu` to the top of the air.
pub fn distance_to_top(r: f64, mu: f64) -> f64 {
    (-r * mu
        + (r * r * (mu * mu - 1.0) + ATMOSPHERE_TOP * ATMOSPHERE_TOP)
            .max(0.0)
            .sqrt())
    .max(0.0)
}

/// True when a ray from radius `r` with zenith cosine `mu` meets the sea.
pub fn hits_ground(r: f64, mu: f64) -> bool {
    mu < 0.0 && r * r * (mu * mu - 1.0) + PLANET_RADIUS * PLANET_RADIUS >= 0.0
}

/// Optical depth from radius `r` along zenith cosine `mu` to the top of the air, by the midpoint rule over `samples`
/// intervals growing quadratically from `r` (accurate with few samples where the air thins along the ray); infinite
/// where the ray meets the sea.
pub fn optical_depth(air: &AirCoefficients, r: f64, mu: f64, samples: u32) -> Rgb {
    if hits_ground(r, mu) {
        return [f64::INFINITY; 3];
    }
    let mut out = [0.0; 3];
    let length = distance_to_top(r, mu);
    let mut previous = 0.0;
    for i in 1..=samples {
        let t = length * (f64::from(i) / f64::from(samples)).powi(2);
        let middle = (t + previous) / 2.0;
        let dt = t - previous;
        previous = t;
        let radius = (r * r + middle * middle + 2.0 * r * middle * mu).sqrt();
        let step = extinction(air, radius - PLANET_RADIUS);
        for c in 0..3 {
            out[c] += step[c] * dt;
        }
    }
    out
}

/// Transmittance from sea level toward a body at `elevation_deg`: zero below the horizon.
pub fn sea_transmittance(air: &AirCoefficients, elevation_deg: f64) -> Rgb {
    optical_depth(air, PLANET_RADIUS, elevation_deg.to_radians().sin(), 500).map(|depth| {
        if depth.is_finite() {
            (-depth).exp()
        } else {
            0.0
        }
    })
}

/// The sky's own exposure as the sun sets: (sun elevation°, gain), interpolated in log. The game's table
/// (`TWILIGHT` in model.ts: 3.5, 7.5, 18, 70, 260, 600) with the twilight below the horizon lifted an eighth more,
/// where takram's scattering leaves the sky away from the sun that much darker than the sky it replaced.
pub const TWILIGHT: [(f64, f64); 7] = [
    (3.0, 1.0),
    (0.0, 3.5),
    (-2.0, 8.4),
    (-4.0, 20.2),
    (-6.0, 78.4),
    (-8.0, 291.0),
    (-10.0, 672.0),
];

pub fn twilight_lift(sun_elevation_deg: f64) -> f64 {
    let first = TWILIGHT[0];
    let last = TWILIGHT[TWILIGHT.len() - 1];
    if sun_elevation_deg >= first.0 {
        return first.1;
    }
    if sun_elevation_deg <= last.0 {
        return last.1;
    }
    let upper = TWILIGHT
        .iter()
        .position(|&(elevation, _)| elevation < sun_elevation_deg)
        .expect("between the first and last pairs");
    let (e1, g1) = TWILIGHT[upper - 1];
    let (e0, g0) = TWILIGHT[upper];
    lerp(g0.ln(), g1.ln(), (sun_elevation_deg - e0) / (e1 - e0)).exp()
}

/// The dome's grade over the physical sky (`SKY_GRADE`).
pub mod sky_grade {
    pub const GAIN: f64 = 1.4;
    pub const SATURATION: f64 = 1.35;
    pub const MOON: f64 = 0.25;
    pub const FLOOR: [f64; 3] = [0.006, 0.0105, 0.02];
    pub const FLOOR_HORIZON: f64 = 0.5;
    pub const SHOULDER_KNEE: f64 = 0.45;
    pub const SHOULDER_CEILING: f64 = 0.95;
}

/// Radiance of the night floor along a direction's vertical component.
pub fn night_floor(y: f64) -> Rgb {
    let horizon = 1.0 + sky_grade::FLOOR_HORIZON * (1.0 - y.abs()).powi(4);
    sky_grade::FLOOR.map(|value| value * horizon)
}

/// The aerosol phase's narrow forward core (`AUREOLE`).
pub const AUREOLE_CORE: f64 = 0.35;
pub const AUREOLE_CORE_G: f64 = 0.92;

/// The terms of the aerosol phase that depend on the scene alone (`phaseTerms`): lobe and core.
pub fn phase_terms(g: f64, gain: f64) -> (Rgb, Rgb) {
    use std::f64::consts::PI;
    let lobe = [
        3.0 / (8.0 * PI) * (1.0 - g * g) / (2.0 + g * g) * (1.0 - AUREOLE_CORE) * gain,
        1.0 + g * g,
        2.0 * g,
    ];
    let core = [
        AUREOLE_CORE * (1.0 - AUREOLE_CORE_G * AUREOLE_CORE_G) / (4.0 * PI) * gain,
        1.0 + AUREOLE_CORE_G * AUREOLE_CORE_G,
        2.0 * AUREOLE_CORE_G,
    ];
    (lobe, core)
}

pub fn rayleigh_phase(nu: f64) -> f64 {
    3.0 / (16.0 * std::f64::consts::PI) * (1.0 + nu * nu)
}

pub fn mie_phase(nu: f64, g: f64) -> f64 {
    3.0 / (8.0 * std::f64::consts::PI) * (1.0 - g * g) * (1.0 + nu * nu)
        / ((2.0 + g * g) * (1.0 + g * g - 2.0 * g * nu).max(1e-4).powf(1.5))
}

fn henyey_greenstein(nu: f64, g: f64) -> f64 {
    (1.0 - g * g) / (4.0 * std::f64::consts::PI * (1.0 + g * g - 2.0 * g * nu).max(1e-4).powf(1.5))
}

pub fn aureole_phase(nu: f64, g: f64) -> f64 {
    (1.0 - AUREOLE_CORE) * mie_phase(nu, g) + AUREOLE_CORE * henyey_greenstein(nu, AUREOLE_CORE_G)
}

/// Sky irradiance on the sea (a horizontal surface), colour × intensity, estimated on the CPU by single
/// scattering with a multiple-scattering fill (the GPU's tables are not read back): the sun of `irradiance` above
/// the air, the twilight `lift`, the dome's gain and night floor.
pub fn sky_irradiance(air: &AirCoefficients, sun: DVec3, irradiance: f64, lift: f64) -> Rgb {
    use std::f64::consts::PI;
    let (rings, sectors, steps) = (4, 8, 16);
    let mut sum = [0.0; 3];
    for ring in 0..rings {
        for sector in 0..sectors {
            let a = (ring as f64 + 0.5) / rings as f64;
            let phi = 2.0 * PI * (sector as f64 + 0.5) / sectors as f64;
            let sine = a.sqrt();
            let cosine = (1.0 - a).sqrt();
            let (dx, dy, dz) = (sine * phi.cos(), cosine, sine * phi.sin());
            let nu = dx * sun.x + dy * sun.y + dz * sun.z;
            let phase_r = rayleigh_phase(nu);
            let phase_m = aureole_phase(nu, air.mie_g) * air.mie_gain;
            let length = distance_to_top(PLANET_RADIUS, dy);
            let mut previous = 0.0;
            let mut view = [0.0; 3];
            for i in 1..=steps {
                let t = length * (i as f64 / steps as f64).powi(2);
                let middle = (t + previous) / 2.0;
                let dt = t - previous;
                previous = t;
                let (px, py, pz) = (dx * middle, PLANET_RADIUS + dy * middle, dz * middle);
                let r = (px * px + py * py + pz * pz).sqrt();
                let sun_depth =
                    optical_depth(air, r, (px * sun.x + py * sun.y + pz * sun.z) / r, 16);
                let [rayleigh, mie, ozone] = densities(r - PLANET_RADIUS);
                for c in 0..3 {
                    let lit = if sun_depth[c].is_finite() {
                        (-sun_depth[c]).exp()
                    } else {
                        0.0
                    };
                    let source = (air.rayleigh[c] * rayleigh * phase_r
                        + air.mie_scattering[c] * mie * phase_m)
                        * lit
                        * (1.0 + 0.5 * air.multiple);
                    let extinction_step = (air.rayleigh[c] * rayleigh
                        + air.mie_extinction[c] * mie
                        + air.ozone[c] * ozone)
                        * dt;
                    sum[c] += source * (-view[c] - extinction_step / 2.0).exp() * dt
                        / (rings * sectors) as f64;
                    view[c] += extinction_step;
                }
            }
        }
    }
    let floor_glow = night_floor(0.5);
    let scale = PI * irradiance * lift * sky_grade::GAIN;
    std::array::from_fn(|c| sum[c] * scale + PI * floor_glow[c])
}

/// The scene light's grade (`SUN_GRADE`): the sun's hue at the sea (its colour through the air relative to the zenith's,
/// desaturated and tinted), and the readability floor its level never falls under as it sets (a share of the zenith's
/// sun), faded out at the horizon.
pub mod sun_grade {
    pub const TINT: [f64; 3] = [1.0, 0.95, 0.85];
    pub const SATURATION: f64 = 0.6;
    pub const FLOOR: f64 = 0.12;
    pub const SETTING: [f64; 2] = [-1.0, 2.0];
}
/// Share of the moonlight above the air that reaches the sea once the moon is up (`MOON_GRADE`).
pub mod moon_grade {
    pub const TRANSMITTANCE: f64 = 0.8;
    pub const RISEN: f64 = 6.0;
}

/// Sunlight and moonlight at the sea, colour × intensity in the sea's units (`seaLevelLight`). The sun is the tables'
/// sun at the sea: the scene's `sun_intensity` above the air (`game_scale`) through the air, at the luminance the dome's
/// sky and the clouds' sunlight are lit by, never under the readability floor (a share of the zenith's sun) as it sets,
/// faded out at the horizon. Its hue is the game's grade: the colour through the air relative to the zenith's,
/// desaturated and tinted.
///
/// Until 2026-10-09 the level was the zenith's relative transmittance held up by a readability ramp that reached the
/// whole sun above the air at 18°: by day the world's sun skipped the air (12–27 % over the tables' sun between 75°
/// and 30°, more in a hazy map's air, which it ignored), while the sky it stood in was lit by the sun through it.
pub fn sea_level_light(
    sun_elevation: f64,
    sun_intensity: f64,
    air: &AirCoefficients,
    moon_irradiance: Rgb,
    moon_elevation: f64,
) -> (Rgb, Rgb) {
    let sun_at_sea = sea_transmittance(air, sun_elevation.max(0.5));
    let zenith = sea_transmittance(air, 90.0);
    let relative: Rgb =
        std::array::from_fn(|c| (sun_at_sea[c] / zenith[c]).powf(sun_grade::SATURATION));
    let peak = relative.iter().cloned().fold(1e-6, f64::max);
    let tint = sun_grade::TINT;
    let mut sun: Rgb = std::array::from_fn(|c| tint[c] * relative[c] / peak);
    let colour_peak = sun.iter().cloned().fold(f64::MIN, f64::max);
    for value in &mut sun {
        *value /= colour_peak;
    }
    // The brightest channel at which the hue has the luminance of the sun through the air (above it the sun is white,
    // `sun_intensity` in every channel), and the floor's: the zenith's sun at its hue, `FLOOR` of it.
    let physical = luminance(sun_at_sea) / luminance(sun);
    let floor = sun_grade::FLOOR * luminance(zenith) / luminance(tint);
    let level = sun_intensity
        * physical.max(floor)
        * smoothstep(sun_grade::SETTING[0], sun_grade::SETTING[1], sun_elevation);
    for value in &mut sun {
        *value *= level;
    }
    let moon_level = moon_grade::TRANSMITTANCE * smoothstep(0.0, moon_grade::RISEN, moon_elevation);
    (sun, moon_irradiance.map(|value| value * moon_level))
}

/// Linear value of an 8-bit sRGB channel, as three's `Color` converts a hex colour.
pub fn srgb_to_linear(channel: f64) -> f64 {
    if channel < 0.04045 {
        channel * 0.0773993808
    } else {
        (channel * 0.9478672986 + 0.0521327014).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn north_atlantic() -> AirCoefficients {
        air_coefficients(&AuthoredAtmosphere {
            rayleigh: 0.38,
            turbidity: 2.2,
            mie: 0.5 * 0.45,
            mie_g: 0.72,
            multiple: 1.0,
        })
    }

    #[test]
    fn twilight_lift_matches_its_table() {
        assert_eq!(twilight_lift(10.0), 1.0);
        assert!((twilight_lift(0.0) - 3.5).abs() < 1e-12);
        assert!((twilight_lift(-6.0) - 78.4).abs() < 1e-9);
        assert_eq!(twilight_lift(-40.0), 672.0);
        let half = twilight_lift(-1.0);
        assert!((half - (3.5f64 * 8.4).sqrt()).abs() < 1e-9);
    }

    /// The parameters' layout is the WGSL struct's (304 bytes, the profiles at 16-byte offsets).
    #[test]
    fn atmosphere_parameters_fit_a_uniform_buffer() {
        assert_eq!(std::mem::size_of::<AtmosphereParameters>(), 304);
        assert_eq!(
            std::mem::offset_of!(AtmosphereParameters, rayleigh_density),
            112
        );
        assert_eq!(std::mem::offset_of!(AtmosphereParameters, mie_density), 176);
        assert_eq!(
            std::mem::offset_of!(AtmosphereParameters, absorption_density),
            240
        );
    }

    /// A clear sky (turbidity 1, rayleigh 0.4) is takram's default air.
    #[test]
    fn a_clear_scene_is_takrams_default_air() {
        let air = air_coefficients(&AuthoredAtmosphere {
            rayleigh: 0.4,
            turbidity: 1.0,
            mie: 0.25,
            mie_g: 0.8,
            multiple: 1.0,
        });
        let parameters = AtmosphereParameters::new(&air);
        for (value, takram) in parameters
            .rayleigh_scattering
            .iter()
            .zip([0.005802, 0.013558, 0.0331])
        {
            assert!((f64::from(*value) - takram).abs() < 1e-7);
        }
        assert!((f64::from(parameters.mie_scattering[0]) - 0.003996).abs() < 1e-7);
        assert!((f64::from(parameters.mie_extinction[0]) - 0.00444).abs() < 1e-7);
        assert_eq!(parameters.top_radius, 6460.0);
    }

    /// The game's scale makes the sun's luminance above the air the scene's intensity.
    #[test]
    fn the_sun_above_the_air_has_the_scenes_intensity() {
        let solar = solar_luminance();
        assert!((luminance(solar) * game_scale(6.6) - 6.6).abs() < 1e-12);
        // takram's relative factors make the sun's unit radiance one unit of luminance.
        assert!((luminance(sun_relative_luminance()) - 1.0).abs() < 1e-12);
    }

    /// Sunlight at the sea: white-tinted at noon, warm and dimmer low, gone below the horizon, continuous through
    /// sunset.
    #[test]
    fn sea_level_light_follows_the_sun() {
        let air = north_atlantic();
        let moon = [0.64 * srgb_to_linear(180.0 / 255.0), 0.0, 0.0];
        let (noon, _) = sea_level_light(60.0, 6.6, &air, moon, -20.0);
        assert!(noon[0] < 6.6 && noon[0] > 5.0, "{noon:?}");
        assert!(noon[2] > 0.8 * noon[0] && noon[2] < noon[0]);
        let (low, _) = sea_level_light(5.0, 6.6, &air, moon, -20.0);
        assert!(
            low[0] < noon[0] && low[2] / low[0] < noon[2] / noon[0],
            "{low:?}"
        );
        let (set, _) = sea_level_light(-2.0, 6.6, &air, moon, -20.0);
        assert_eq!(set, [0.0; 3]);
        let mut previous = sea_level_light(8.0, 6.6, &air, moon, -20.0).0[0];
        for tenth in (-20..80).rev() {
            let value = sea_level_light(f64::from(tenth) / 10.0, 6.6, &air, moon, -20.0).0[0];
            assert!(
                value <= previous + 1e-9 && previous - value < 0.4,
                "{tenth}: {value} after {previous}"
            );
            previous = value;
        }
    }

    /// By day the sun at the sea is the tables' sun (the scene's intensity above the air, through it), as the dome's sky
    /// and the clouds' sunlight have it: its luminance follows the elevation and the air's aerosol, never the whole sun
    /// above the air. The readability floor holds only as it sets.
    #[test]
    fn the_sun_at_the_sea_is_the_tables_sun() {
        let moon = [0.0; 3];
        let clear = north_atlantic();
        let hazy = air_coefficients(&AuthoredAtmosphere {
            rayleigh: 0.38,
            turbidity: 4.8,
            mie: 0.5 * 0.45,
            mie_g: 0.72,
            multiple: 1.0,
        });
        for air in [clear, hazy] {
            for elevation in [10.0, 30.0, 45.0, 61.0, 75.0, 90.0] {
                let (sun, _) = sea_level_light(elevation, 6.6, &air, moon, -20.0);
                let through = 6.6 * luminance(sea_transmittance(&air, elevation));
                assert!(
                    (luminance(sun) / through - 1.0).abs() < 1e-9,
                    "{elevation}: {} against {through}",
                    luminance(sun)
                );
            }
        }
        let noon =
            |air: &AirCoefficients| luminance(sea_level_light(61.0, 6.6, air, moon, -20.0).0);
        assert!(noon(&hazy) < 0.95 * noon(&clear));
        // As it sets the floor holds it: 12 % of the zenith's sun at its hue, faded out at the horizon.
        let (low, _) = sea_level_light(1.0, 6.6, &clear, moon, -20.0);
        let zenith = luminance(sea_transmittance(&clear, 90.0)) / luminance(sun_grade::TINT);
        let fade = smoothstep(sun_grade::SETTING[0], sun_grade::SETTING[1], 1.0);
        assert!(
            (low[0] / (6.6 * sun_grade::FLOOR * zenith * fade) - 1.0).abs() < 1e-9,
            "{low:?}"
        );
    }

    /// The CPU's optical depth over 500 intervals agrees with a fine Simpson rule.
    #[test]
    fn optical_depth_converges() {
        let air = north_atlantic();
        let coarse = optical_depth(&air, PLANET_RADIUS + 0.03, 0.1, 500);
        let fine = optical_depth(&air, PLANET_RADIUS + 0.03, 0.1, 20000);
        for c in 0..3 {
            assert!(
                (coarse[c] / fine[c] - 1.0).abs() < 1e-3,
                "{coarse:?} {fine:?}"
            );
        }
    }
}
