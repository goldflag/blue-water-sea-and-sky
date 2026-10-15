//! The stars' place on the Earth against the theatre's sun: both are reckoned from the theatre's sidereal time, and
//! the client draws both in one sky, so the frame the stars stand by must put the sun's own place among them where
//! the theatre's sun stands.

use glam::DVec3;
use naval_sim::theatre::sun;
use naval_sky::state::{CelestialModel, Place, direction_from_angles};

/// Where right ascension `ra` and declination `dec` (degrees) stand in a scene over `place` (x east, z south).
fn stands(place: Place, ra: f64, dec: f64) -> DVec3 {
    let mut model = CelestialModel::default();
    model.set_place(Some(place));
    let (ra, dec) = (ra.to_radians(), dec.to_radians());
    model.star_rotation.transpose()
        * DVec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin())
}

/// The theatre's sun as the client draws it (`theatre::conditions::sun`): a true bearing B is the sky's azimuth
/// 180° − B.
fn drawn_sun(start: &str, utc_offset_hours: f64, lat: f64, lon: f64) -> DVec3 {
    let sun = sun::position(start, utc_offset_hours, 0.0, lat, lon);
    direction_from_angles(sun.elevation_deg, 180.0 - sun.azimuth_deg)
}

fn place(start: &str, utc_offset_hours: f64, lat: f64, lon: f64) -> Place {
    Place {
        latitude: lat,
        sidereal: sun::sidereal_deg(start, utc_offset_hours, 0.0, lon).unwrap(),
    }
}

#[test]
fn the_sun_stands_at_its_own_place_among_the_stars() {
    // The March equinox of 2000 fell at 07:35 UTC on the 20th. That afternoon the sun was 0.3° along the ecliptic
    // from the First Point of Aries: right ascension 0.28°, declination 0.12°. Morning and afternoon, north and south
    // of the equator, east and west of Greenwich.
    for (start, utc_offset_hours, lat, lon) in [
        ("2000-03-20T15:00", 0.0, 30.0, -20.0),
        ("2000-03-20T15:00", 0.0, -35.0, 60.0),
        ("2000-03-21T02:00", 11.0, -9.1, 159.8),
        ("2000-03-20T09:00", -6.0, 64.6, -90.0),
    ] {
        let sun = drawn_sun(start, utc_offset_hours, lat, lon);
        let among_stars = stands(place(start, utc_offset_hours, lat, lon), 0.28, 0.12);
        let apart = sun.angle_between(among_stars).to_degrees();
        assert!(
            apart < 0.3,
            "{start} at {lat}, {lon}: {apart} degrees apart"
        );
    }
}

#[test]
fn the_sun_stands_among_the_summer_stars_at_the_june_solstice() {
    // 21 June 2000, 01:48 UTC: right ascension 6 h, declination +23.44°. At noon that day it had moved 0.4° on.
    let (start, lat, lon) = ("2000-06-21T12:00", 50.0, -30.0);
    let sun = drawn_sun(start, 0.0, lat, lon);
    let among_stars = stands(place(start, 0.0, lat, lon), 90.44, 23.44);
    let apart = sun.angle_between(among_stars).to_degrees();
    assert!(apart < 0.3, "{apart} degrees apart");
}
