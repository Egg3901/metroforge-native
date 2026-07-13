use mf_sim::city::{generate_city, preset_by_key};
use mf_sim::types::{Difficulty, RoadClass};

fn metrics(seed: u32, key: &str, diff: Difficulty) {
    let c = generate_city(seed, diff, None, preset_by_key(Some(key)));
    let f = &c.fields;
    let n = (f.w * f.h) as usize;
    let (mut water, mut parks, mut pop, mut jobs) = (0u32, 0u32, 0f64, 0f64);
    for i in 0..n {
        if f.water[i] == 1 {
            water += 1;
        }
        if f.parks[i] == 1 {
            parks += 1;
        }
        pop += f.population[i] as f64;
        jobs += f.jobs[i] as f64;
    }
    let art = c
        .roads
        .iter()
        .filter(|r| r.cls == RoadClass::Arterial)
        .count();
    let loc = c.roads.iter().filter(|r| r.cls == RoadClass::Local).count();
    let dpop: f64 = c.districts.iter().map(|d| d.population).sum();
    println!(
        "{seed} {key} {diff:?}: water={:.4} park={:.4} pop={} jobs={} roads={} art={} loc={} dist={} dpop={}",
        water as f64 / n as f64, parks as f64 / n as f64, pop.round(), jobs.round(),
        c.roads.len(), art, loc, c.districts.len(), dpop.round()
    );
}

fn main() {
    metrics(12345, "generic", Difficulty::Normal);
    metrics(777, "generic", Difficulty::Normal);
    metrics(12345, "nyc", Difficulty::Normal);
    metrics(42, "boston", Difficulty::Easy);
    metrics(999, "atlanta", Difficulty::Hard);
}
