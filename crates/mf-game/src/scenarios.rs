//! Playable scenario catalog for the city-select picker (#25). Metadata is
//! baked from `content/scenarios/manifest.json` (generated from the sim's
//! `sim/src/content/scenarios/*.json` authoring files). Rules are mapped
//! onto [`mf_protocol::ScenarioRules`] for the `init` wire payload; the
//! sidecar resolves full win/lose trees via `scenarioId`.

use mf_protocol::{ScenarioRules, TransitMode};

/// One row in the baked scenario manifest.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub city_key: String,
    pub tier: u8,
    pub difficulty: String,
    pub starting_budget: f64,
    pub starting_modes: Vec<String>,
    #[serde(default)]
    pub lock_modes: Option<bool>,
    #[serde(default)]
    pub daily_subsidy: Option<f64>,
    pub deadline_days: u32,
    #[serde(default)]
    pub era_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct Manifest {
    scenarios: Vec<ScenarioEntry>,
}

const MANIFEST_JSON: &str = include_str!("../../../content/scenarios/manifest.json");

/// Cash balance used for sandbox mode (effectively unlimited for normal play).
pub const SANDBOX_STARTING_CASH: f64 = 999_999_999.0;

/// Special scenario id persisted in saves when sandbox mode is active.
pub const SANDBOX_SCENARIO_ID: &str = "sandbox";

fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(MANIFEST_JSON).expect("content/scenarios/manifest.json must parse")
    })
}

/// All playable scenarios in manifest order.
pub fn all() -> &'static [ScenarioEntry] {
    &manifest().scenarios
}

/// Lookup by stable scenario id.
pub fn by_id(id: &str) -> Option<&'static ScenarioEntry> {
    all().iter().find(|s| s.id == id)
}

/// Scenarios whose `cityKey` matches the selected city preset.
pub fn for_city(city_key: &str) -> Vec<&'static ScenarioEntry> {
    all().iter().filter(|s| s.city_key == city_key).collect()
}

/// Default scenario for a city (first manifest row for that city), if any.
pub fn default_for_city(city_key: &str) -> Option<&'static ScenarioEntry> {
    for_city(city_key).into_iter().next()
}

fn parse_mode(s: &str) -> Option<TransitMode> {
    match s {
        "bus" => Some(TransitMode::Bus),
        "tram" => Some(TransitMode::Tram),
        "metro" => Some(TransitMode::Metro),
        "rail" => Some(TransitMode::Rail),
        _ => None,
    }
}

/// Map a manifest row onto wire [`ScenarioRules`] (mirrors
/// `rulesFromScenario` in the TS sim).
pub fn rules_for(entry: &ScenarioEntry) -> ScenarioRules {
    ScenarioRules {
        scenario_id: Some(entry.id.clone()),
        starting_modes: entry
            .starting_modes
            .iter()
            .filter_map(|m| parse_mode(m))
            .collect(),
        lock_modes: entry.lock_modes,
        max_day: Some(entry.deadline_days),
        approval_floor: None,
        starting_cash: Some(entry.starting_budget),
        daily_subsidy: entry.daily_subsidy,
        era_label: entry.era_label.clone(),
    }
}

/// Sandbox preset: unlimited funds, every mode unlocked, no calendar limit.
pub fn sandbox_rules() -> ScenarioRules {
    ScenarioRules {
        scenario_id: Some(SANDBOX_SCENARIO_ID.to_string()),
        starting_modes: vec![
            TransitMode::Bus,
            TransitMode::Tram,
            TransitMode::Metro,
            TransitMode::Rail,
        ],
        lock_modes: Some(false),
        max_day: None,
        approval_floor: None,
        starting_cash: Some(SANDBOX_STARTING_CASH),
        daily_subsidy: None,
        era_label: Some("Sandbox".to_string()),
    }
}

/// Effective `init` rules from pending-init fields.
pub fn effective_rules(sandbox: bool, scenario_id: Option<&str>) -> Option<ScenarioRules> {
    if sandbox {
        return Some(sandbox_rules());
    }
    let id = scenario_id?;
    let entry = by_id(id)?;
    Some(rules_for(entry))
}

/// Effective `init` scenario id (wire `scenarioId`). Omitted for sandbox and
/// free play so the sidecar does not attach a data-driven scenario def.
pub fn effective_scenario_id(sandbox: bool, scenario_id: Option<&str>) -> Option<String> {
    if sandbox {
        return None;
    }
    scenario_id.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_loads_all_fifteen_scenarios() {
        assert_eq!(all().len(), 15);
    }

    #[test]
    fn cleveland_first_riders_rules_match_authoring() {
        let entry = by_id("cleveland-first-riders").expect("scenario");
        let rules = rules_for(entry);
        assert_eq!(rules.scenario_id.as_deref(), Some("cleveland-first-riders"));
        assert_eq!(rules.starting_cash, Some(12_000_000.0));
        assert_eq!(rules.max_day, Some(45));
        assert_eq!(rules.starting_modes, vec![TransitMode::Bus]);
        assert_eq!(rules.lock_modes, Some(true));
        assert_eq!(rules.daily_subsidy, Some(35_000.0));
    }

    #[test]
    fn sandbox_rules_unlock_every_mode_with_high_cash() {
        let rules = sandbox_rules();
        assert_eq!(rules.starting_cash, Some(SANDBOX_STARTING_CASH));
        assert_eq!(rules.lock_modes, Some(false));
        assert_eq!(rules.max_day, None);
        assert_eq!(rules.starting_modes.len(), 4);
    }

    #[test]
    fn for_city_filters_by_preset_key() {
        let cle = for_city("cleveland");
        let nyc = for_city("nyc");
        assert!(cle.len() >= 7);
        assert!(nyc.len() >= 7);
        assert!(for_city("boston").is_empty());
    }

    #[test]
    fn effective_rules_none_for_free_play() {
        assert!(effective_rules(false, None).is_none());
    }
}
