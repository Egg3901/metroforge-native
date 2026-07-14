//! P3-ENV behavioral parity vs TS reference (captured via bun).
use mf_sim::geology::column_at;
use mf_sim::geology_cost::underground_segment_cost;
use mf_sim::geometry::vec;
use mf_sim::weather::{climate_table, weather_at, Season, WeatherState};

#[test]
fn weather_sequence_matches_ts_nyc_12345() {
    let tbl = climate_table(Some("nyc"));
    // (tick, state, intensity, season, month)
    let expect: &[(u64, WeatherState, f64, Season, u32)] = &[
        (0, WeatherState::Overcast, 0.311132, Season::Winter, 0),
        (600, WeatherState::Clear, 0.127170, Season::Winter, 0),
        (1200, WeatherState::Snow, 0.543902, Season::Winter, 0),
        (100000, WeatherState::Clear, 0.113193, Season::Spring, 2),
        (5000000, WeatherState::Rain, 0.571041, Season::Spring, 4),
    ];
    for &(t, st, inten, se, mo) in expect {
        let w = weather_at(12345, t, &tbl);
        assert_eq!(w.state, st, "state @{t}");
        assert_eq!(w.season, se, "season @{t}");
        assert_eq!(w.month, mo, "month @{t}");
        assert!(
            (w.intensity - inten).abs() < 1e-4,
            "intensity @{t}: {} vs {}",
            w.intensity,
            inten
        );
    }
}

#[test]
#[allow(clippy::type_complexity)]
fn geology_cost_matches_ts() {
    // (city, seed, x, y, depth, wt, perM, stratum)
    let cases: &[(&str, u32, f64, f64, f64, f64, f64, &str)] = &[
        ("nyc", 12345, 0.0, 0.0, 12.0, 4.6622, 3.968000, "clay"),
        (
            "boston", 12345, 500.0, 500.0, 20.0, 4.0327, 9.139200, "clay",
        ),
        (
            "chicago", 12345, 1000.0, -1000.0, 12.0, 1.5000, 3.968000, "clay",
        ),
        ("generic", 1, 0.0, 0.0, 8.0, 8.7367, 2.900000, "clay"),
    ];
    for &(city, seed, x, y, depth, wt, per_m, strat) in cases {
        let col = column_at(Some(city), seed, 12000.0, None, None, vec(x, y));
        assert!(
            (col.water_table_depth - wt).abs() < 1e-3,
            "{city} wt {} vs {}",
            col.water_table_depth,
            wt
        );
        let r = underground_segment_cost(1.0, 100.0, &col, depth);
        assert!(
            (r.cost_per_m - per_m).abs() < 1e-3,
            "{city} perM {} vs {}",
            r.cost_per_m,
            per_m
        );
        assert_eq!(r.stratum.as_str(), strat, "{city} stratum");
    }
}
