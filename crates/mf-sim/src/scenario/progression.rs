//! Scenario progression manifest. Port of
//! `sim/src/core/scenario/progression.ts` + `content/scenarios/progression.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// Completing `from` unlocks each id in `to`.
pub type UnlockMap = BTreeMap<String, Vec<String>>;

/// Progression graph payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioProgressionManifest {
    /// Scenario ids playable with no prior clears.
    pub starters: Vec<String>,
    /// Unlock edges.
    pub unlocks: UnlockMap,
}

/// Full progression manifest.
pub fn scenario_progression() -> &'static ScenarioProgressionManifest {
    static MANIFEST: OnceLock<ScenarioProgressionManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| ScenarioProgressionManifest {
        starters: vec![
            "cleveland-first-riders".to_string(),
            "nyc-first-thousand".to_string(),
        ],
        unlocks: BTreeMap::from([
            (
                "cleveland-first-riders".to_string(),
                vec![
                    "cleveland-five-hundred".to_string(),
                    "cleveland-farebox-30".to_string(),
                ],
            ),
            (
                "cleveland-five-hundred".to_string(),
                vec![
                    "cleveland-farebox-80".to_string(),
                    "cleveland-roadworks".to_string(),
                ],
            ),
            (
                "cleveland-farebox-30".to_string(),
                vec!["cleveland-reach".to_string()],
            ),
            (
                "cleveland-farebox-80".to_string(),
                vec!["cleveland-austerity".to_string()],
            ),
            (
                "cleveland-reach".to_string(),
                vec!["cleveland-tram-line".to_string()],
            ),
            (
                "cleveland-roadworks".to_string(),
                vec!["cleveland-tram-line".to_string()],
            ),
            (
                "cleveland-tram-line".to_string(),
                vec![
                    "cleveland-austerity".to_string(),
                    "nyc-bus-spine".to_string(),
                ],
            ),
            (
                "cleveland-austerity".to_string(),
                vec!["nyc-last-stand".to_string()],
            ),
            (
                "nyc-first-thousand".to_string(),
                vec!["nyc-farebox-80".to_string(), "nyc-bus-spine".to_string()],
            ),
            (
                "nyc-farebox-80".to_string(),
                vec!["nyc-dig-season".to_string()],
            ),
            (
                "nyc-bus-spine".to_string(),
                vec!["nyc-pressure".to_string(), "nyc-express".to_string()],
            ),
            (
                "nyc-dig-season".to_string(),
                vec!["nyc-express".to_string()],
            ),
            (
                "nyc-pressure".to_string(),
                vec!["nyc-last-stand".to_string()],
            ),
            (
                "nyc-express".to_string(),
                vec!["nyc-last-stand".to_string()],
            ),
        ]),
    })
}

/// Scenario ids unlocked by completing `id`.
pub fn unlocks_from(id: &str) -> Vec<String> {
    scenario_progression()
        .unlocks
        .get(id)
        .cloned()
        .unwrap_or_default()
}

/// Scenario ids that must be cleared before `id` is available (OR prerequisites).
pub fn requires_for(id: &str) -> Vec<String> {
    let mut out: Vec<String> = scenario_progression()
        .unlocks
        .iter()
        .filter_map(|(completed, unlocked)| {
            if unlocked.iter().any(|u| u == id) {
                Some(completed.clone())
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

/// True when the id is present in progression (starter, unlock source, or target).
pub fn is_progression_known(id: &str) -> bool {
    let m = scenario_progression();
    if m.starters.iter().any(|s| s == id) || m.unlocks.contains_key(id) {
        return true;
    }
    m.unlocks.values().any(|v| v.iter().any(|u| u == id))
}

/// Given completed scenario ids, return catalog ids currently playable.
pub fn available_scenarios(completed: &BTreeSet<String>, catalog_ids: &[String]) -> Vec<String> {
    let m = scenario_progression();
    let mut open: BTreeSet<String> = m.starters.iter().cloned().collect();
    for (cleared, unlocked) in &m.unlocks {
        if !completed.contains(cleared) {
            continue;
        }
        for id in unlocked {
            open.insert(id.clone());
        }
    }
    catalog_ids
        .iter()
        .filter(|id| open.contains(*id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_and_unlock_edges_work() {
        let m = scenario_progression();
        assert!(m.starters.iter().any(|s| s == "cleveland-first-riders"));
        let opens = unlocks_from("cleveland-first-riders");
        assert!(opens.iter().any(|s| s == "cleveland-five-hundred"));
        assert!(requires_for("nyc-last-stand").len() >= 2);
    }

    #[test]
    fn available_filters_to_progression_open_set() {
        let catalog = vec![
            "cleveland-first-riders".to_string(),
            "cleveland-five-hundred".to_string(),
            "nyc-first-thousand".to_string(),
        ];
        let open0 = available_scenarios(&BTreeSet::new(), &catalog);
        assert_eq!(
            open0,
            vec![
                "cleveland-first-riders".to_string(),
                "nyc-first-thousand".to_string()
            ]
        );

        let mut done = BTreeSet::new();
        done.insert("cleveland-first-riders".to_string());
        let open1 = available_scenarios(&done, &catalog);
        assert!(open1.iter().any(|id| id == "cleveland-five-hundred"));
    }
}
