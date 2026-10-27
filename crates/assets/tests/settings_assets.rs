use assets::{EnvironmentDescription, EnvironmentSettings};

fn decode(name: &str) -> EnvironmentDescription {
    let path = format!("{}/tests/fixtures/settings/{name}.dat", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).expect("read settings fixture");
    EnvironmentDescription::decode(&bytes).unwrap_or_else(|failure| panic!("{name}: {failure:?}"))
}

#[test]
fn library_sky_presets_decode() {
    for name in ["Midday", "Sunrise", "Sunset", "Midnight", "DefaultSky"] {
        std::assert_matches!(decode(name).settings, EnvironmentSettings::Sky(_), "{name}");
    }
}

#[test]
fn library_water_decodes() {
    std::assert_matches!(decode("DefaultWater").settings, EnvironmentSettings::Water(_));
}

#[test]
fn library_day_cycle_decodes() {
    std::assert_matches!(decode("DefaultDaycycle").settings, EnvironmentSettings::DayCycle(_));
}
