//! INTEGRATION STUB (ship-plan #25 v0.4): the parallel `v04/campaign` agent
//! owns the real `CampaignProgress` persistence (star ratings earned per
//! scenario, unlock tracking, presumably its own save file under
//! `config.rs`'s `ProjectDirs` pattern). Neither that branch nor a
//! `mf-campaign` crate exists in this tree yet, so this module exists solely
//! so `hud.rs`'s main-menu city-select grid (this wave's mission) can
//! compile and behave sensibly standalone.
//!
//! Contract (as handed off for this wave — verify against the real
//! `v04/campaign` branch at integration and reconcile any drift):
//! `CITY_ORDER` = `["nyc","atlanta","boston","chicago","cleveland","dc","la",
//! "philly","seattle","sf"]` (nyc first, remainder alphabetical); `nyc` is
//! always unlocked; city at index `i` unlocks once the player's total stars
//! across every city is `>= 2*i`.
//!
//! Delete this file and its `mod campaign;` line in `main.rs` once the real
//! module lands — or, if the landed shape differs, reconcile call sites in
//! `hud.rs` against it instead.

use bevy::prelude::*;
use std::collections::HashMap;

/// Fixed city unlock order — index into this array is the `i` in the
/// `2*i` stars-required unlock formula below.
pub const CITY_ORDER: [&str; 10] = [
    "nyc",
    "atlanta",
    "boston",
    "chicago",
    "cleveland",
    "dc",
    "la",
    "philly",
    "seattle",
    "sf",
];

/// Per-city star ratings (0-3). Everything defaults to zero stars until the
/// real campaign system lands, which means every city past `nyc` reads as
/// locked — the safe default for a progression system that doesn't exist
/// yet (a stub that unlocked everything would let this wave's menu ship
/// looking "done" when it isn't).
#[derive(Resource, Debug, Clone, Default)]
pub struct CampaignProgress {
    stars: HashMap<String, u8>,
}

impl CampaignProgress {
    /// Stars earned for `key` (0-3), or 0 if never played.
    pub fn stars(&self, key: &str) -> u8 {
        self.stars.get(key).copied().unwrap_or(0)
    }

    /// `nyc` is always unlocked; every other city unlocks once the player's
    /// total stars across all cities reaches `2 * CITY_ORDER`'s index of
    /// `key`. An unrecognized key (shouldn't happen — callers iterate
    /// `CITY_ORDER` itself) is treated as locked rather than panicking.
    pub fn city_unlocked(&self, key: &str) -> bool {
        let Some(i) = CITY_ORDER.iter().position(|&k| k == key) else {
            return false;
        };
        if i == 0 {
            return true;
        }
        let total_stars: u32 = self.stars.values().map(|&s| s as u32).sum();
        total_stars >= 2 * i as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nyc_always_unlocked_with_zero_stars() {
        let progress = CampaignProgress::default();
        assert!(progress.city_unlocked("nyc"));
    }

    #[test]
    fn non_nyc_locked_with_zero_stars() {
        let progress = CampaignProgress::default();
        for key in &CITY_ORDER[1..] {
            assert!(
                !progress.city_unlocked(key),
                "{key} should be locked at 0 stars"
            );
        }
    }

    #[test]
    fn unlock_threshold_matches_two_times_index() {
        let mut progress = CampaignProgress::default();
        // atlanta is index 1: needs total >= 2 stars.
        progress.stars.insert("nyc".to_string(), 1);
        assert!(!progress.city_unlocked("atlanta"));
        progress.stars.insert("nyc".to_string(), 2);
        assert!(progress.city_unlocked("atlanta"));
    }

    #[test]
    fn unknown_key_is_locked_not_panicking() {
        let progress = CampaignProgress::default();
        assert!(!progress.city_unlocked("not-a-real-city"));
        assert_eq!(progress.stars("not-a-real-city"), 0);
    }
}
