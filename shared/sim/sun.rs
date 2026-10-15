//! The sun's elevation over the theatre, for a bubble's `times` preset and the frame's sunset and end of night. A low-precision
//! solar position (the Astronomical Almanac's, good to about 0.01° over the twentieth century): enough to
//! choose between dawn, day, dusk and night. The sidereal time it is reckoned from also turns the native client's
//! stars ([`sidereal_deg`]), and the moon is reckoned beside it ([`super::moon`]).
//!
//! The simulation reads the sun only through `Scenario::sun_at` and its kin (`theatre::content`), on the scenario's one
//! UTC clock ([`StartUtc`], `Scenario::start_utc`): the forms here that take a `start` and an offset are for the native
//! client's sky and for tests.
use super::content::parse_start;

/// Days from 2000-01-01T12:00 UTC to the start of `year-month-day` 00:00 UTC.
fn days_from_j2000(year: i32, month: u32, day: u32) -> f64 {
    // Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse).
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_1970 = era * 146097 + doe - 719468;
    // 2000-01-01T12:00 is day 10957.5 after 1970-01-01T00:00.
    days_1970 as f64 - 10957.5
}

/// A scenario's clock in UTC: its tick 0 as days from 2000-01-01T12:00 UTC (J2000.0), resolved once from its local
/// `start` and `utcOffsetHours` (`Scenario::start_utc`). The sun is reckoned from it and the run's seconds at whatever
/// place it is read (step C's C14: one UTC clock, light read at its own place); `utcOffsetHours` is otherwise only the
/// deck clock's zone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartUtc(f64);
impl StartUtc {
    /// The clock of a run that starts at local `start` (`YYYY-MM-DDTHH:MM`) in a zone `utc_offset_hours` east of
    /// Greenwich; none for a start that does not parse.
    pub fn of(start: &str, utc_offset_hours: f64) -> Option<Self> {
        let (y, mo, d, h, mi) = parse_start(start)?;
        Some(Self(
            days_from_j2000(y, mo, d) + (h as f64 + mi as f64 / 60.0 - utc_offset_hours) / 24.0,
        ))
    }
    /// Days from 2000-01-01T12:00 UTC `seconds` after tick 0.
    #[inline]
    pub fn days(self, seconds: f64) -> f64 {
        self.0 + seconds / 86400.0
    }
}

/// Days from 2000-01-01T12:00 UTC to `seconds` after the scenario's local `start`; none for a start that does not parse.
pub(super) fn days(start: &str, utc_offset_hours: f64, seconds: f64) -> Option<f64> {
    StartUtc::of(start, utc_offset_hours).map(|clock| clock.days(seconds))
}

/// The local sidereal time in degrees at longitude `lon` (degrees east), `n` days from 2000-01-01T12:00 UTC.
pub(super) fn sidereal(n: f64, lon: f64) -> f64 {
    (280.460_618_37 + 360.985_647_366_29 * n + lon).rem_euclid(360.0)
}

/// The sun's ecliptic longitude in radians, `n` days from 2000-01-01T12:00 UTC (not reduced to a turn).
pub(super) fn ecliptic_longitude(n: f64) -> f64 {
    let rad = f64::to_radians;
    let mean_longitude = (280.460 + 0.985_647_4 * n).rem_euclid(360.0);
    let anomaly = rad((357.528 + 0.985_600_3 * n).rem_euclid(360.0));
    rad(mean_longitude + 1.915 * anomaly.sin() + 0.020 * (2.0 * anomaly).sin())
}

/// The local sidereal time in degrees at longitude `lon` (degrees east), `seconds` after the scenario's local `start`:
/// the right ascension on the meridian there, which with the latitude places every star in the sky. None for a start
/// that does not parse.
pub fn sidereal_deg(start: &str, utc_offset_hours: f64, seconds: f64, lon: f64) -> Option<f64> {
    days(start, utc_offset_hours, seconds).map(|n| sidereal(n, lon))
}

/// The sun's elevation in degrees at `lat, lon` (degrees), `seconds` after the scenario's local `start`, and
/// whether it is still rising (before local solar noon).
pub fn elevation(
    start: &str,
    utc_offset_hours: f64,
    seconds: f64,
    lat: f64,
    lon: f64,
) -> (f64, bool) {
    let sun = position(start, utc_offset_hours, seconds, lat, lon);
    (sun.elevation_deg, sun.rising)
}

/// Where the sun stands over a point of the theatre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPosition {
    /// Degrees above the horizon.
    pub elevation_deg: f64,
    /// True bearing, degrees clockwise from north.
    pub azimuth_deg: f64,
    /// Before local solar noon.
    pub rising: bool,
}

/// The sun at `lat, lon` (degrees), `seconds` after the scenario's local `start`; overhead for a start that does not
/// parse.
pub fn position(
    start: &str,
    utc_offset_hours: f64,
    seconds: f64,
    lat: f64,
    lon: f64,
) -> SunPosition {
    position_utc(StartUtc::of(start, utc_offset_hours), seconds, lat, lon)
}

/// The sun at `lat, lon` (degrees), `seconds` after tick 0 on `clock`; overhead with no clock (a start that does not
/// parse).
pub fn position_utc(clock: Option<StartUtc>, seconds: f64, lat: f64, lon: f64) -> SunPosition {
    let Some(n) = clock.map(|clock| clock.days(seconds)) else {
        return SunPosition {
            elevation_deg: 90.0,
            azimuth_deg: 180.0,
            rising: true,
        };
    };
    let rad = f64::to_radians;
    let ecliptic = ecliptic_longitude(n);
    let obliquity = rad(23.439 - 0.000_000_4 * n);
    let declination = (obliquity.sin() * ecliptic.sin()).asin();
    let right_ascension = (obliquity.cos() * ecliptic.sin()).atan2(ecliptic.cos());
    let sidereal = rad(sidereal(n, lon));
    let hour_angle = crate::geometry::wrap_angle(sidereal - right_ascension);
    let phi = rad(lat);
    let sin_el = phi.sin() * declination.sin() + phi.cos() * declination.cos() * hour_angle.cos();
    // Clockwise from north: east of the meridian before noon (a negative hour angle), west after it.
    let azimuth = (-hour_angle.sin())
        .atan2(declination.tan() * phi.cos() - phi.sin() * hour_angle.cos())
        .to_degrees()
        .rem_euclid(360.0);
    SunPosition {
        elevation_deg: sin_el.clamp(-1.0, 1.0).asin().to_degrees(),
        azimuth_deg: azimuth,
        rising: hour_angle < 0.0,
    }
}

/// Seconds after the start of the next sunset (the upper limb, −0.833°) after `seconds`, within a day.
pub fn next_sunset(
    start: &str,
    utc_offset_hours: f64,
    seconds: f64,
    lat: f64,
    lon: f64,
) -> Option<f64> {
    next_sunset_utc(StartUtc::of(start, utc_offset_hours), seconds, lat, lon)
}

/// [`next_sunset`] on `clock`.
pub fn next_sunset_utc(clock: Option<StartUtc>, seconds: f64, lat: f64, lon: f64) -> Option<f64> {
    next_crossing(clock, seconds, lat, lon, -0.833, false)
}

/// Seconds after the start of the next end of night after `seconds`, within a day: the sun's centre climbing to the
/// horizon, where the dark that slows a deck (`air::conditions::deck_pace`, `reports::darkness`) ends.
pub fn next_sunrise(
    start: &str,
    utc_offset_hours: f64,
    seconds: f64,
    lat: f64,
    lon: f64,
) -> Option<f64> {
    next_sunrise_utc(StartUtc::of(start, utc_offset_hours), seconds, lat, lon)
}

/// [`next_sunrise`] on `clock`.
pub fn next_sunrise_utc(clock: Option<StartUtc>, seconds: f64, lat: f64, lon: f64) -> Option<f64> {
    next_crossing(clock, seconds, lat, lon, 0.0, true)
}

/// The first time after `seconds`, within a day, the sun's elevation crosses `horizon` degrees, climbing (`rising`) or
/// setting: found in ten-minute steps, then to the second.
fn next_crossing(
    clock: Option<StartUtc>,
    seconds: f64,
    lat: f64,
    lon: f64,
    horizon: f64,
    rising: bool,
) -> Option<f64> {
    const STEP: f64 = 600.0;
    // On the side of `horizon` the crossing leaves: below it before a sunrise, above it before a sunset.
    let before = |s: f64| {
        let above = position_utc(clock, s, lat, lon).elevation_deg - horizon;
        if rising { above < 0.0 } else { above > 0.0 }
    };
    let mut a = seconds;
    let mut was = before(a);
    for _ in 0..(86400.0 / STEP) as usize {
        let b = a + STEP;
        let is = before(b);
        if was && !is {
            let (mut lo, mut hi) = (a, b);
            for _ in 0..20 {
                let mid = (lo + hi) / 2.0;
                if before(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(hi.round());
        }
        (a, was) = (b, is);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noon_at_the_equinox_on_the_equator_is_overhead_and_midnight_is_below() {
        let (noon, _) = elevation("2000-03-20T12:00", 0.0, 0.0, 0.0, 0.0);
        assert!(noon > 88.0, "{noon}");
        let (midnight, _) = elevation("2000-03-20T00:00", 0.0, 0.0, 0.0, 0.0);
        assert!(midnight < -80.0, "{midnight}");
    }
    #[test]
    fn sidereal_time_is_the_right_ascension_on_the_meridian() {
        // Greenwich at the epoch: 18 h 41 m 50 s.
        let epoch = sidereal_deg("2000-01-01T12:00", 0.0, 0.0, 0.0).unwrap();
        assert!((epoch - 280.4606).abs() < 1e-3, "{epoch}");
        // It gains a day on the sun in a year: 3 m 56.6 s a day, and four minutes for each degree east.
        let day = sidereal_deg("2000-01-01T12:00", 0.0, 86400.0, 0.0).unwrap();
        assert!((day - epoch - 0.985_647).abs() < 1e-5, "{day}");
        let east = sidereal_deg("2000-01-01T12:00", 0.0, 0.0, 15.0).unwrap();
        assert!((east - epoch - 15.0).abs() < 1e-9, "{east}");
        // Local time is the scenario's: 23:00 at UTC+11 is noon at Greenwich.
        let local = sidereal_deg("2000-01-01T23:00", 11.0, 0.0, 0.0).unwrap();
        assert!((local - epoch).abs() < 1e-9, "{local}");
        // The sun's own right ascension is on the meridian at local solar noon: 0 h at the March equinox.
        let noon = sidereal_deg("2000-03-20T12:00", 0.0, 0.0, 0.0).unwrap();
        assert!(noon < 3.0 || noon > 357.0, "{noon}");
        assert_eq!(sidereal_deg("soon", 0.0, 0.0, 0.0), None);
    }
    #[test]
    fn denmark_strait_on_23_may_1941_sets_late_and_rises_early() {
        // Sunset at 64.6°N 28.4°W on 23 May 1941 is near 23:30 GMT.
        let sunset = next_sunset("1941-05-23T20:00", 0.0, 0.0, 64.6, -28.4).unwrap();
        let hours = 20.0 + sunset / 3600.0;
        assert!((23.0..24.2).contains(&hours), "{hours}");
        let (evening, rising) = elevation("1941-05-23T20:00", 0.0, 0.0, 64.6, -28.4);
        assert!(evening > 5.0 && !rising, "{evening}");
    }
    #[test]
    fn the_sun_bears_south_at_noon_and_sets_north_of_west_in_a_northern_summer() {
        let noon = position("1941-05-23T13:54", 0.0, 0.0, 64.6, -28.4);
        assert!((noon.azimuth_deg - 180.0).abs() < 3.0, "{noon:?}");
        let set = next_sunset("1941-05-23T20:00", 0.0, 0.0, 64.6, -28.4).unwrap();
        let sunset = position("1941-05-23T20:00", 0.0, set, 64.6, -28.4);
        assert!((300.0..340.0).contains(&sunset.azimuth_deg), "{sunset:?}");
        // The morning sun is east of the meridian, the evening's west of it.
        let morning = position("1941-05-24T08:00", 0.0, 0.0, 64.6, -28.4);
        assert!(
            morning.rising && (60.0..140.0).contains(&morning.azimuth_deg),
            "{morning:?}"
        );
    }
    #[test]
    fn the_night_over_eastern_solomons_ends_soon_after_six_and_next_ends_a_day_later() {
        // 8°S 161.5°E on 24 August 1942, local time UTC+11: the sun's centre clears the horizon near 06:20.
        let at = |s: f64| next_sunrise("1942-08-24T06:00", 11.0, s, -8.0, 161.5).unwrap();
        let end = at(0.0);
        assert!((600.0..2400.0).contains(&end), "{end}");
        assert!(elevation("1942-08-24T06:00", 11.0, end - 1.0, -8.0, 161.5).0 < 0.0);
        assert!(elevation("1942-08-24T06:00", 11.0, end, -8.0, 161.5).0 >= 0.0);
        // By day the next end of night is the next morning's, after the evening's sunset.
        let next = at(6.0 * 3600.0);
        assert!((next - end - 86400.0).abs() < 120.0, "{next}");
        let sunset = next_sunset("1942-08-24T06:00", 11.0, 6.0 * 3600.0, -8.0, 161.5).unwrap();
        assert!(sunset < next, "{sunset}");
    }
}
