//! Procedural place-name banks. Port of `sim/src/core/city/names.ts`.
//!
//! All picks flow through an [`Rng`] so a seed reproduces the same city.
//! Naming model (no em/en dashes, matching the TS generator's style):
//!   street   = <root> <suffix>
//!   park     = <root> Park | <feature>
//!   district = <root> | <root> <area>
//!   city     = <prefix?><root><ending?>

use crate::rng::Rng;
use std::collections::HashSet;

/// Street roots (trees, presidents, nature, ordinals, virtues).
pub const STREET_ROOTS: &[&str] = &[
    "Elm",
    "Oak",
    "Maple",
    "Cedar",
    "Pine",
    "Birch",
    "Willow",
    "Walnut",
    "Chestnut",
    "Spruce",
    "Aspen",
    "Poplar",
    "Sycamore",
    "Cypress",
    "Hickory",
    "Magnolia",
    "Dogwood",
    "Washington",
    "Jefferson",
    "Lincoln",
    "Madison",
    "Monroe",
    "Jackson",
    "Adams",
    "Franklin",
    "Hamilton",
    "Roosevelt",
    "Wilson",
    "Grant",
    "Cleveland",
    "Harrison",
    "Kennedy",
    "Garfield",
    "Tyler",
    "Polk",
    "Pierce",
    "Hayes",
    "Taft",
    "River",
    "Lake",
    "Hill",
    "Valley",
    "Forest",
    "Meadow",
    "Brook",
    "Spring",
    "Summit",
    "Ridge",
    "Grove",
    "Glen",
    "Prairie",
    "Highland",
    "Woodland",
    "Fern",
    "Sunset",
    "Sunrise",
    "Bay",
    "Harbor",
    "Bridge",
    "Mill",
    "Canal",
    "Dock",
    "First",
    "Second",
    "Third",
    "Fourth",
    "Fifth",
    "Sixth",
    "Seventh",
    "Eighth",
    "Ninth",
    "Tenth",
    "Eleventh",
    "Twelfth",
    "Church",
    "School",
    "Market",
    "Union",
    "Commerce",
    "Industry",
    "Depot",
    "Station",
    "Center",
    "Main",
    "Broad",
    "High",
    "Front",
    "Water",
    "Park",
    "College",
    "Liberty",
    "Freedom",
    "Independence",
    "Victory",
    "Progress",
    "Prospect",
    "Hope",
    "Franklin",
    "Clark",
    "Baker",
    "Cooper",
    "Carter",
    "Bishop",
    "Foster",
    "Warren",
    "Sherman",
    "Grand",
    "Central",
    "State",
    "Federal",
    "Capitol",
    "Court",
    "Vine",
    "Cherry",
    "Peach",
    "Laurel",
    "Holly",
    "Rose",
    "Clover",
    "Sage",
    "Juniper",
    "Beacon",
    "Lantern",
    "Harvest",
    "Orchard",
    "Garden",
    "Pasture",
    "Windmill",
];

/// Street suffixes (weighted toward common ones).
pub const STREET_SUFFIXES: &[&str] = &[
    "Street",
    "Avenue",
    "Boulevard",
    "Road",
    "Lane",
    "Drive",
    "Court",
    "Place",
    "Way",
    "Terrace",
    "Parkway",
    "Circle",
    "Trail",
    "Row",
    "Alley",
    "Crossing",
];

/// Index-aligned suffix weights (Street/Avenue/Road most common).
pub const SUFFIX_WEIGHTS: &[f64] = &[
    9.0, 8.0, 4.0, 7.0, 5.0, 5.0, 3.0, 3.0, 2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0,
];

/// Park roots.
pub const PARK_ROOTS: &[&str] = &[
    "Riverside",
    "Lakeside",
    "Hillcrest",
    "Fairmount",
    "Highland",
    "Meadowbrook",
    "Cedar Grove",
    "Oakwood",
    "Willow Creek",
    "Elmwood",
    "Forest Glen",
    "Sunset",
    "Liberty",
    "Memorial",
    "Veterans",
    "Founders",
    "Heritage",
    "Pioneer",
    "Washington",
    "Lincoln",
    "Franklin",
    "Jefferson",
    "Roosevelt",
    "Kennedy",
    "Garfield",
    "Prospect",
    "Overlook",
    "Greenfield",
    "Brookdale",
    "Fernwood",
];

/// Standalone park names.
pub const PARK_FEATURES: &[&str] = &[
    "The Commons",
    "The Green",
    "The Esplanade",
    "City Gardens",
    "Botanical Gardens",
    "Central Green",
    "The Arboretum",
    "Waterfront Park",
    "Harbor Green",
    "The Promenade",
    "Founders Square",
    "Veterans Field",
    "Riverwalk",
];

/// Neighborhood / district roots.
pub const DISTRICT_ROOTS: &[&str] = &[
    "Fairview",
    "Riverton",
    "Oakdale",
    "Ashford",
    "Bexley",
    "Clarendon",
    "Danforth",
    "Eastgate",
    "Westbrook",
    "Northfield",
    "Southport",
    "Brookhaven",
    "Cedarhurst",
    "Glenwood",
    "Kingsley",
    "Lakemont",
    "Millbrook",
    "Norwood",
    "Parkside",
    "Ravenswood",
    "Sherwood",
    "Thornton",
    "Vernon",
    "Whitfield",
    "Ashbury",
    "Bishop",
    "Carver",
    "Dover",
    "Elmhurst",
    "Foxridge",
    "Granby",
    "Hartwell",
    "Ironside",
    "Kensington",
    "Larkspur",
    "Montrose",
    "Oldtown",
    "Pinecrest",
    "Quarry",
    "Rosedale",
    "Stonegate",
    "Tanner",
    "Underhill",
    "Vale",
    "Weston",
];

/// District area words.
pub const DISTRICT_AREAS: &[&str] = &[
    "Heights", "Hills", "Park", "Village", "Gardens", "Landing", "Square", "Point", "Crossing",
    "Junction", "Flats", "Quarter", "District", "Commons", "Row",
];

/// City name prefixes.
pub const CITY_PREFIXES: &[&str] = &[
    "Fort", "Lake", "New", "Port", "Mount", "Saint", "North", "South", "East", "West",
];

/// City name roots.
pub const CITY_ROOTS: &[&str] = &[
    "Spring", "Clarion", "Aurora", "Bethel", "Camden", "Dayton", "Elgin", "Fenwick", "Gables",
    "Haven", "Ithaca", "Jasper", "Kingston", "Laurel", "Marion", "Newton", "Orion", "Preston",
    "Quincy", "Raleigh", "Salem", "Trenton", "Auburn", "Verona", "Warren", "Yardley", "Ashland",
    "Bristol", "Concord", "Denton", "Easton",
];

/// City name endings.
pub const CITY_ENDINGS: &[&str] = &[
    "field", "ton", "ville", "burg", "ford", "dale", "wood", "port", "boro", "haven",
];

/// One street name, e.g. "Jefferson Boulevard". Mirrors `streetName`.
pub fn street_name(rng: &mut Rng) -> String {
    let root = *rng.pick(STREET_ROOTS).unwrap();
    let suffix = STREET_SUFFIXES
        .get(rng.weighted(SUFFIX_WEIGHTS))
        .copied()
        .unwrap_or("Street");
    format!("{root} {suffix}")
}

/// One park name, e.g. "Riverside Park" or "The Commons". Mirrors `parkName`.
pub fn park_name(rng: &mut Rng) -> String {
    if rng.chance(0.28) {
        return (*rng.pick(PARK_FEATURES).unwrap()).to_string();
    }
    format!("{} Park", rng.pick(PARK_ROOTS).unwrap())
}

/// One neighborhood name, e.g. "Fairview" or "Bishop Heights". Mirrors
/// `districtName`.
pub fn district_name(rng: &mut Rng) -> String {
    let root = *rng.pick(DISTRICT_ROOTS).unwrap();
    if rng.chance(0.45) {
        return format!("{root} {}", rng.pick(DISTRICT_AREAS).unwrap());
    }
    root.to_string()
}

/// One city name, e.g. "Springfield", "Fort Clarion", "Lake Aurora". Mirrors
/// `cityName`.
pub fn city_name(rng: &mut Rng) -> String {
    let root = *rng.pick(CITY_ROOTS).unwrap();
    if rng.chance(0.3) {
        return format!("{} {root}", rng.pick(CITY_PREFIXES).unwrap());
    }
    format!("{root}{}", rng.pick(CITY_ENDINGS).unwrap())
}

/// Draw `n` unique names from a generator fn, deterministically. Falls back to
/// numbered suffixes if the bank is exhausted. Mirrors `uniqueNames`.
pub fn unique_names(rng: &mut Rng, n: usize, gen: fn(&mut Rng) -> String) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut guard = 0usize;
    while out.len() < n && guard < n * 20 {
        guard += 1;
        let name = gen(rng);
        if seen.contains(&name) {
            continue;
        }
        seen.insert(name.clone());
        out.push(name);
    }
    let mut dup = 2u32;
    while out.len() < n {
        out.push(format!("{} {dup}", gen(rng)));
        dup += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_are_unique_and_sized() {
        let mut rng = Rng::from_seed(3);
        let names = unique_names(&mut rng, 12, district_name);
        assert_eq!(names.len(), 12);
        let set: HashSet<_> = names.iter().collect();
        assert_eq!(set.len(), 12);
    }

    #[test]
    fn names_are_deterministic() {
        let a = {
            let mut r = Rng::from_seed(99);
            unique_names(&mut r, 8, district_name)
        };
        let b = {
            let mut r = Rng::from_seed(99);
            unique_names(&mut r, 8, district_name)
        };
        assert_eq!(a, b);
    }
}
