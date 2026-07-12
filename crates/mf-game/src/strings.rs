//! Central player-facing copy table for `mf-game`.
//!
//! No heavyweight i18n crate: English lives in [`EN`], and [`current`] returns
//! the active table. A future locale loader can call [`set_current`] with
//! another `&'static Strings` (e.g. from a compiled-in `fr.rs` or a parsed
//! locale file) without touching call sites.
//!
//! Copy rules: ASCII hyphen-minus only (no Unicode en/em dashes). See the
//! unit test below and `scripts/check-strings-dashes.sh`.

use std::sync::atomic::{AtomicPtr, Ordering};

/// Typed English (and future locale) string table. Every player-facing
/// literal in menus / HUD / tutorial / goals / settings / toasts / errors
/// should live here as a field (or a format helper on this struct).
#[derive(Debug)]
#[allow(dead_code)] // exhaustive copy table; some consumers not yet keyed
pub struct Strings {
    // --- Brand / chrome -------------------------------------------------
    pub brand: &'static str,
    pub tagline: &'static str,

    // --- Main menu ------------------------------------------------------
    pub play: &'static str,
    pub load_game: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
    pub back: &'static str,
    pub back_arrow: &'static str,

    // --- Connecting / loading -------------------------------------------
    pub could_not_start_sim_prefix: &'static str,
    pub starting_simulation: &'static str,
    pub starting_simulation_attempt_prefix: &'static str,
    pub starting_simulation_attempt_of: &'static str,
    pub loading_city: &'static str,
    pub ready: &'static str,
    pub waiting: &'static str,
    pub loading_static_city: &'static str,
    pub loading_masks: &'static str,
    pub loading_fields: &'static str,
    pub loading_interface: &'static str,

    // --- City select / saves --------------------------------------------
    pub just_now: &'static str,
    pub minutes_ago_suffix: &'static str,
    pub hours_ago_suffix: &'static str,
    pub days_ago_suffix: &'static str,
    pub unknown_city: &'static str,
    pub empty: &'static str,
    pub city: &'static str,
    pub continue_label: &'static str,
    pub difficulty: &'static str,
    /// Explicit shareable seed (#140): new-game seed row label.
    pub seed: &'static str,
    /// Prefix for the pause modal / load browser "Seed: 123" line — kept
    /// separate from `seed` (the input row's label) since one needs a
    /// trailing colon/space and the other doesn't.
    pub seed_prefix: &'static str,
    pub randomize: &'static str,
    pub copy_seed: &'static str,
    pub copied: &'static str,
    pub start_prefix: &'static str,
    pub earn_more_star: &'static str,
    pub earn_more_stars: &'static str,
    pub pick_slot_to_continue: &'static str,
    pub autosaves: &'static str,
    pub manual_slots: &'static str,
    pub save_slot_prefix: &'static str,
    pub save_slot_empty_suffix: &'static str,
    pub autosave_slot_prefix: &'static str,

    // --- Settings -------------------------------------------------------
    pub quality: &'static str,
    pub theme: &'static str,
    pub weather: &'static str,
    pub day_night: &'static str,
    pub fog_and_clouds: &'static str,
    pub fog_and_clouds_gated: &'static str,
    pub autosave: &'static str,
    pub off: &'static str,
    pub every_n_sim_days_prefix: &'static str,
    pub every_n_sim_days_suffix: &'static str,
    pub autosave_ring_hint: &'static str,
    pub replay_tutorial: &'static str,
    pub ui_scale: &'static str,
    pub camera_sensitivity: &'static str,
    pub colorblind: &'static str,
    pub colorblind_off: &'static str,
    pub colorblind_deuteranopia: &'static str,
    pub colorblind_protanopia: &'static str,
    pub colorblind_tritanopia: &'static str,
    pub reduce_motion: &'static str,
    pub pause_on_start: &'static str,
    pub reduce_motion_hint: &'static str,

    // --- Quality / theme / difficulty labels (mirrored for locale swap) -
    pub quality_potato: &'static str,
    pub quality_low: &'static str,
    pub quality_medium: &'static str,
    pub quality_high: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub theme_purple: &'static str,
    pub difficulty_easy: &'static str,
    pub difficulty_normal: &'static str,
    pub difficulty_hard: &'static str,

    // --- In-game HUD ----------------------------------------------------
    pub day_prefix: &'static str,
    pub approval: &'static str,
    pub approval_up_prefix: &'static str,
    pub approval_down_prefix: &'static str,
    pub pop_prefix: &'static str,
    pub crowded_route: &'static str,
    pub crowded_routes: &'static str,
    pub open_busiest_crowded_route: &'static str,
    pub connecting_to_city: &'static str,
    pub speed_1x: &'static str,
    pub speed_10x: &'static str,
    pub speed_30x: &'static str,
    pub speed_120x: &'static str,
    pub surface_view: &'static str,
    pub subway_view: &'static str,
    pub goals: &'static str,

    // --- Weather HUD (v0.7) ---------------------------------------------
    pub weather_clear: &'static str,
    pub weather_overcast: &'static str,
    pub weather_rain: &'static str,
    pub weather_fog: &'static str,
    pub weather_snow: &'static str,
    pub weather_storm: &'static str,
    pub season_winter: &'static str,
    pub season_spring: &'static str,
    pub season_summer: &'static str,
    pub season_autumn: &'static str,
    pub event_blizzard: &'static str,
    pub event_heatwave: &'static str,
    pub weather_tooltip_season_prefix: &'static str,
    pub weather_tooltip_event_prefix: &'static str,

    // --- Pause ----------------------------------------------------------
    pub paused: &'static str,
    pub resume: &'static str,
    pub save_game: &'static str,
    pub quit_to_desktop: &'static str,

    // --- Fatal / connection ---------------------------------------------
    pub lost_connection_prefix: &'static str,

    // --- Tutorial -------------------------------------------------------
    pub tutorial_move_camera_title: &'static str,
    pub tutorial_select_station_title: &'static str,
    pub tutorial_place_stations_title: &'static str,
    pub tutorial_open_route_title: &'static str,
    pub tutorial_watch_vehicles_title: &'static str,
    pub tutorial_move_camera_body: &'static str,
    pub tutorial_select_station_body: &'static str,
    pub tutorial_place_stations_body: &'static str,
    pub tutorial_open_route_body: &'static str,
    pub tutorial_watch_vehicles_body: &'static str,
    pub tutorial_step_of_prefix: &'static str,
    pub tutorial_step_of_mid: &'static str,
    pub skip: &'static str,

    // --- Goals ----------------------------------------------------------
    pub goal_place_3_stations_title: &'static str,
    pub goal_place_3_stations_desc: &'static str,
    pub goal_lay_first_track_title: &'static str,
    pub goal_lay_first_track_desc: &'static str,
    pub goal_launch_route_title: &'static str,
    pub goal_launch_route_desc: &'static str,
    pub goal_coverage_25_title: &'static str,
    pub goal_coverage_25_desc: &'static str,
    pub goal_500_riders_title: &'static str,
    pub goal_500_riders_desc: &'static str,
    pub goal_approval_60_title: &'static str,
    pub goal_approval_60_desc: &'static str,
    pub goal_complete_prefix: &'static str,
    pub tier_prefix: &'static str,
    pub tier_locked_prefix: &'static str,
    pub tier_locked_mid: &'static str,
    pub tier_locked_suffix: &'static str,

    // --- Build UI / tools -----------------------------------------------
    pub mode_bus: &'static str,
    pub mode_tram: &'static str,
    pub mode_metro: &'static str,
    pub mode_rail: &'static str,
    pub tool_select: &'static str,
    pub tool_bus_station: &'static str,
    pub tool_tram_station: &'static str,
    pub tool_tram_station_locked: &'static str,
    pub tool_route: &'static str,
    pub tool_bulldoze: &'static str,
    pub tool_undo: &'static str,
    pub routes: &'static str,
    pub place_station_context_prefix: &'static str,
    pub place_station_cash_mid: &'static str,
    pub not_quoted_yet: &'static str,
    pub route_context_prefix: &'static str,
    pub route_context_mid: &'static str,
    pub route_context_cost: &'static str,
    pub bulldoze_context: &'static str,
    pub no_routes_yet: &'static str,
    pub line_prefix: &'static str,
    pub live_crowding: &'static str,
    pub live_crowding_pct_prefix: &'static str,
    pub route_list_stations: &'static str,
    pub route_list_vehicles: &'static str,
    pub farebox_per_day: &'static str,
    pub operating_cost_per_day: &'static str,
    pub net_per_day: &'static str,
    pub vehicles: &'static str,
    pub fare: &'static str,
    pub now_prefix: &'static str,
    pub name: &'static str,
    pub delete: &'static str,
    pub confirm_delete: &'static str,
    pub unknown_error: &'static str,
    pub cannot_build_prefix: &'static str,

    // --- Panels / finance / station -------------------------------------
    pub bankrupt_banner: &'static str,
    pub approval_collapsed_banner: &'static str,
    pub time_up_banner: &'static str,
    pub station_prefix: &'static str,
    pub station_max_level: &'static str,
    pub upgrade_to_level_prefix: &'static str,
    pub level_prefix: &'static str,
    pub boarding_per_day: &'static str,
    pub arriving_per_day: &'static str,
    pub catchment_district: &'static str,
    pub people: &'static str,
    pub jobs: &'static str,
    pub routes_serving_station: &'static str,
    pub no_routes_reach_station: &'static str,
    pub finance: &'static str,
    pub cash: &'static str,
    pub loan_balance: &'static str,
    pub yesterday: &'static str,
    pub fares: &'static str,
    pub subsidy: &'static str,
    pub operations: &'static str,
    pub maintenance: &'static str,
    pub interest: &'static str,
    pub net: &'static str,
    pub net_last_7_days: &'static str,
    pub transit_share: &'static str,
    pub coverage: &'static str,
    pub daily_transit_trips: &'static str,
    pub farebox_recovery: &'static str,
    pub lifetime_earnings: &'static str,
    pub insights: &'static str,

    // --- Report ---------------------------------------------------------
    pub verdict_bankrupt: &'static str,
    pub verdict_lost_faith: &'static str,
    pub verdict_time_up: &'static str,
    pub verdict_complete: &'static str,
    pub day: &'static str,
    pub population_served: &'static str,
    pub net_last_day: &'static str,
    pub keep_playing: &'static str,
    pub back_to_menu: &'static str,

    // --- Saves / toasts -------------------------------------------------
    pub save_failed_prefix: &'static str,
    pub load_failed_prefix: &'static str,
    pub autosaved: &'static str,
    pub saved_to_slot_prefix: &'static str,
    pub demand_overlay_toast: &'static str,
    pub unserved_overlay_toast: &'static str,
    pub traffic_overlay_toast: &'static str,
    pub star_earned_prefix: &'static str,

    // --- Campaign star goals --------------------------------------------
    pub star_cover_city_prefix: &'static str,
    pub star_cover_city_suffix: &'static str,
    pub star_keep_approval_prefix: &'static str,
    pub star_keep_approval_suffix: &'static str,
    pub star_carry_trips_prefix: &'static str,
    pub star_carry_trips_suffix: &'static str,
    pub star_transit_share_prefix: &'static str,
    pub star_transit_share_suffix: &'static str,
    pub star_net_positive_prefix: &'static str,
    pub star_net_positive_suffix: &'static str,

    // --- Minimap --------------------------------------------------------
    pub minimap: &'static str,
    pub no_city_loaded: &'static str,

    // --- City select ----------------------------------------------------
    pub city_select_hint: &'static str,

    // --- Routes panel ---------------------------------------------------
    pub sort: &'static str,
    pub sort_crowding: &'static str,
    pub sort_riders: &'static str,
    pub sort_net_income: &'static str,
    pub close: &'static str,
    pub no_routes_panel_hint: &'static str,
    pub multi_select_hint: &'static str,
    pub stops: &'static str,
    pub stops_edit_hint: &'static str,
    pub apply_stop_order: &'static str,
    pub revert: &'static str,
    pub edit_stops_in_world: &'static str,
    pub pause: &'static str,
    pub next_color: &'static str,
    pub delete_route: &'static str,
    pub route_row_stops_mid: &'static str,
    pub route_row_riders_mid: &'static str,
    pub route_paused_suffix: &'static str,
    pub crowding_prefix: &'static str,
    pub route_update_failed_prefix: &'static str,

    // --- Shortcuts help overlay (? / Slash) -----------------------------
    pub shortcuts_title: &'static str,
    pub shortcuts_hint: &'static str,
    pub sc_section_camera: &'static str,
    pub sc_section_build: &'static str,
    pub sc_section_view: &'static str,
    pub sc_pan: &'static str,
    pub sc_orbit: &'static str,
    pub sc_zoom: &'static str,
    pub sc_bus_tool: &'static str,
    pub sc_route_tool: &'static str,
    pub sc_bulldoze_tool: &'static str,
    pub sc_rotate: &'static str,
    pub sc_multiselect: &'static str,
    pub sc_confirm: &'static str,
    pub sc_subway: &'static str,
    pub sc_demand: &'static str,
    pub sc_finance: &'static str,
    pub sc_map: &'static str,
    pub sc_minimap: &'static str,
    pub sc_photo: &'static str,
    pub sc_fullscreen: &'static str,
    pub sc_pause: &'static str,
    pub sc_help: &'static str,
}

/// Default English table. Future locales swap via [`set_current`].
pub static EN: Strings = Strings {
    brand: "MetroForge",
    tagline: "Build the network. Move the city.",

    play: "Play",
    load_game: "Load Game",
    settings: "Settings",
    quit: "Quit",
    back: "Back",
    back_arrow: "< Back",

    could_not_start_sim_prefix: "Could not start the simulation: ",
    starting_simulation: "Starting the simulation...",
    starting_simulation_attempt_prefix: "Starting the simulation (attempt ",
    starting_simulation_attempt_of: " of ",
    loading_city: "Loading city",
    ready: "ready",
    waiting: "waiting",
    loading_static_city: "Static city",
    loading_masks: "Masks",
    loading_fields: "Fields",
    loading_interface: "Interface",

    just_now: "just now",
    minutes_ago_suffix: "m ago",
    hours_ago_suffix: "h ago",
    days_ago_suffix: "d ago",
    unknown_city: "Unknown city",
    empty: "Empty",
    city: "City",
    continue_label: "Continue",
    difficulty: "Difficulty",
    seed: "Seed",
    seed_prefix: "Seed: ",
    randomize: "Randomize",
    copy_seed: "Copy seed",
    copied: "Copied",
    start_prefix: "Start - ",
    earn_more_star: "Earn {} more star",
    earn_more_stars: "Earn {} more stars",
    pick_slot_to_continue: "Pick a slot to continue",
    autosaves: "Autosaves",
    manual_slots: "Manual slots",
    save_slot_prefix: "Slot ",
    save_slot_empty_suffix: " (empty)",
    autosave_slot_prefix: "Autosave ",

    quality: "Quality",
    theme: "Theme",
    weather: "Weather",
    day_night: "Day / night cycle",
    fog_and_clouds: "Fog & clouds",
    fog_and_clouds_gated: "Fog & clouds (Medium+)",
    autosave: "Autosave",
    off: "Off",
    every_n_sim_days_prefix: "Every ",
    every_n_sim_days_suffix: " sim-days",
    autosave_ring_hint: "Keeps a ring of 3 autosaves",
    replay_tutorial: "Replay tutorial",
    ui_scale: "UI scale",
    camera_sensitivity: "Camera sensitivity",
    colorblind: "Colorblind",
    colorblind_off: "Off",
    colorblind_deuteranopia: "Deuteranopia",
    colorblind_protanopia: "Protanopia",
    colorblind_tritanopia: "Tritanopia",
    reduce_motion: "Reduce motion",
    pause_on_start: "Pause on start",
    reduce_motion_hint: "Disables UI fades and menu camera drift",

    quality_potato: "Potato",
    quality_low: "Low",
    quality_medium: "Medium",
    quality_high: "High",
    theme_light: "Light",
    theme_dark: "Dark",
    theme_purple: "Purple",
    difficulty_easy: "Easy",
    difficulty_normal: "Normal",
    difficulty_hard: "Hard",

    day_prefix: "Day ",
    approval: "Approval",
    approval_up_prefix: "▲ Approval ",
    approval_down_prefix: "▼ Approval ",
    pop_prefix: "Pop ",
    crowded_route: " crowded route",
    crowded_routes: " crowded routes",
    open_busiest_crowded_route: "Open the busiest crowded route",
    connecting_to_city: "Connecting to city...",
    speed_1x: "1x",
    speed_10x: "10x",
    speed_30x: "30x",
    speed_120x: "120x",
    surface_view: "Surface view",
    subway_view: "Subway view",
    goals: "Goals",

    weather_clear: "Clear",
    weather_overcast: "Overcast",
    weather_rain: "Rain",
    weather_fog: "Fog",
    weather_snow: "Snow",
    weather_storm: "Storm",
    season_winter: "Winter",
    season_spring: "Spring",
    season_summer: "Summer",
    season_autumn: "Autumn",
    event_blizzard: "Blizzard",
    event_heatwave: "Heat wave",
    weather_tooltip_season_prefix: "Season: ",
    weather_tooltip_event_prefix: "Event: ",

    paused: "Paused",
    resume: "Resume",
    save_game: "Save game",
    quit_to_desktop: "Quit to desktop",

    lost_connection_prefix: "Lost connection to the sim: ",

    tutorial_move_camera_title: "Move the camera",
    tutorial_select_station_title: "Pick the Station tool",
    tutorial_place_stations_title: "Place two stations",
    tutorial_open_route_title: "Open a route",
    tutorial_watch_vehicles_title: "Watch it run",
    tutorial_move_camera_body: "Drag to pan. Scroll to zoom.",
    tutorial_select_station_body: "Click Station in the toolbar below.",
    tutorial_place_stations_body: "Click a road to drop a station. Place two.",
    tutorial_open_route_body: "Pick the Route tool. Click both stations. Double click to open the line.",
    tutorial_watch_vehicles_body: "Vehicles now serve your line. You are ready.",
    tutorial_step_of_prefix: "Step ",
    tutorial_step_of_mid: " of ",
    skip: "Skip",

    goal_place_3_stations_title: "Place 3 stations",
    goal_place_3_stations_desc: "Drop down three stations to start your network.",
    goal_lay_first_track_title: "Lay your first track",
    goal_lay_first_track_desc: "Connect two stations with a track.",
    goal_launch_route_title: "Launch a route",
    goal_launch_route_desc: "Turn a connected line into a running route.",
    goal_coverage_25_title: "Cover a quarter of the city",
    goal_coverage_25_desc: "Get transit coverage to 25%.",
    goal_500_riders_title: "Reach 500 daily riders",
    goal_500_riders_desc: "Grow daily transit trips to 500.",
    goal_approval_60_title: "Win over the city",
    goal_approval_60_desc: "Get approval above 60%.",
    goal_complete_prefix: "Goal complete: ",
    tier_prefix: "Tier ",
    tier_locked_prefix: "Tier ",
    tier_locked_mid: " locked. Finish tier ",
    tier_locked_suffix: " to unlock it.",

    mode_bus: "bus",
    mode_tram: "tram",
    mode_metro: "metro",
    mode_rail: "rail",
    tool_select: "Select",
    tool_bus_station: "Bus station (1)",
    tool_tram_station: "Tram station",
    tool_tram_station_locked: "Tram station. Locked until Tram unlocks.",
    tool_route: "Route (2)",
    tool_bulldoze: "Bulldoze (3)",
    tool_undo: "Undo",
    routes: "Routes",
    place_station_context_prefix: "Click to place a ",
    place_station_cash_mid: " station. Cash on hand: ",
    not_quoted_yet: "not quoted yet",
    route_context_prefix: "Click stations to add. Enter confirms, Esc cancels. ",
    route_context_mid: " station(s) selected. Estimated cost: ",
    route_context_cost: ".",
    bulldoze_context: "Click a station or track to demolish.",
    no_routes_yet: "No routes yet. Use the Route tool to string stations together.",
    line_prefix: "Line ",
    live_crowding: "Live crowding",
    live_crowding_pct_prefix: "Live crowding ",
    route_list_stations: " station(s), ",
    route_list_vehicles: " vehicle(s), mode ",
    farebox_per_day: "Farebox / day",
    operating_cost_per_day: "Operating cost / day",
    net_per_day: "Net / day",
    vehicles: "Vehicles",
    fare: "Fare",
    now_prefix: "now ",
    name: "Name",
    delete: "Delete",
    confirm_delete: "Confirm delete",
    unknown_error: "unknown error",
    cannot_build_prefix: "Cannot build there: ",

    bankrupt_banner: "Bankrupt. The city has taken over your network.",
    approval_collapsed_banner: "Approval collapsed. Your network has been shut down.",
    time_up_banner: "Time is up. This scenario has ended.",
    station_prefix: "Station ",
    station_max_level: "This station is at its maximum level.",
    upgrade_to_level_prefix: "Upgrade to level ",
    level_prefix: "Level ",
    boarding_per_day: "Boarding / day",
    arriving_per_day: "Arriving / day",
    catchment_district: "Catchment district",
    people: "People",
    jobs: "Jobs",
    routes_serving_station: "Routes serving this station",
    no_routes_reach_station: "No routes reach this station yet. Use the Route tool to connect it.",
    finance: "Finance",
    cash: "Cash",
    loan_balance: "Loan balance",
    yesterday: "Yesterday",
    fares: "Fares",
    subsidy: "Subsidy",
    operations: "Operations",
    maintenance: "Maintenance",
    interest: "Interest",
    net: "Net",
    net_last_7_days: "Net, last 7 days",
    transit_share: "Transit share",
    coverage: "Coverage",
    daily_transit_trips: "Daily transit trips",
    farebox_recovery: "Farebox recovery",
    lifetime_earnings: "Lifetime earnings",
    insights: "Insights",

    verdict_bankrupt: "Bankrupt",
    verdict_lost_faith: "The city lost faith",
    verdict_time_up: "Time is up",
    verdict_complete: "Scenario complete",
    day: "Day",
    population_served: "Population served",
    net_last_day: "Net (last day)",
    keep_playing: "Keep playing",
    back_to_menu: "Back to menu",

    save_failed_prefix: "Save failed: ",
    load_failed_prefix: "Load failed: ",
    autosaved: "Autosaved",
    saved_to_slot_prefix: "Saved to slot ",
    demand_overlay_toast: "Demand overlay on. Arcs show where the city wants to travel. Press G again for unserved trips.",
    unserved_overlay_toast: "Unserved overlay on. These arcs are trips you are losing to cars right now.",
    traffic_overlay_toast: "Traffic overlay on. Roads glow green to red by congestion. Press G again to clear.",
    star_earned_prefix: "Star earned: ",

    star_cover_city_prefix: "Cover ",
    star_cover_city_suffix: "% of the city",
    star_keep_approval_prefix: "Keep approval at ",
    star_keep_approval_suffix: "% or higher",
    star_carry_trips_prefix: "Carry ",
    star_carry_trips_suffix: " daily transit trips",
    star_transit_share_prefix: "Reach ",
    star_transit_share_suffix: "% transit mode share",
    star_net_positive_prefix: "Run ",
    star_net_positive_suffix: " days in a row without losing money",

    minimap: "Minimap",
    no_city_loaded: "No city loaded",

    city_select_hint: "Arrows move. Enter plays. Hover for the accent edge.",

    sort: "Sort",
    sort_crowding: "Crowding",
    sort_riders: "Riders",
    sort_net_income: "Net income",
    close: "Close",
    no_routes_panel_hint: "No routes yet. Place two stations and press R.",
    multi_select_hint: "Shift click stations to multi select, then Enter to connect.",
    stops: "Stops",
    stops_edit_hint: "Click a station in the world to add or remove. Drag rows to reorder.",
    apply_stop_order: "Apply stop order",
    revert: "Revert",
    edit_stops_in_world: "Edit stops in world",
    pause: "Pause",
    next_color: "Next color",
    delete_route: "Delete route",
    route_row_stops_mid: " stops · ",
    route_row_riders_mid: " riders/day · ",
    route_paused_suffix: " · paused",
    crowding_prefix: "Crowding ",
    route_update_failed_prefix: "Route update failed: ",

    shortcuts_title: "Shortcuts",
    shortcuts_hint: "Press ? or Esc to close",
    sc_section_camera: "Camera",
    sc_section_build: "Build",
    sc_section_view: "View",
    sc_pan: "Pan",
    sc_orbit: "Orbit",
    sc_zoom: "Zoom",
    sc_bus_tool: "Bus station",
    sc_route_tool: "Route tool",
    sc_bulldoze_tool: "Bulldoze",
    sc_rotate: "Rotate placement",
    sc_multiselect: "Multi select stations",
    sc_confirm: "Confirm route",
    sc_subway: "Subway view",
    sc_demand: "Demand overlay",
    sc_finance: "Finance",
    sc_map: "Map mode",
    sc_minimap: "Minimap",
    sc_photo: "Photo mode",
    sc_fullscreen: "Fullscreen",
    sc_pause: "Pause / back",
    sc_help: "This help",
};

static CURRENT: AtomicPtr<Strings> = AtomicPtr::new(std::ptr::null_mut());

fn resolve_current() -> &'static Strings {
    let ptr = CURRENT.load(Ordering::Acquire);
    if ptr.is_null() {
        &EN
    } else {
        // SAFETY: only ever set to a `'static` Strings via [`set_current`].
        unsafe { &*ptr }
    }
}

/// Active string table (defaults to [`EN`]).
pub fn current() -> &'static Strings {
    resolve_current()
}

/// Swap the active table. Intended for a future locale loader; pass a
/// `&'static Strings` (compiled-in locale or leaked parsed table).
#[allow(dead_code)] // Public API for future locale swap; unused until locales land.
pub fn set_current(table: &'static Strings) {
    CURRENT.store(table as *const Strings as *mut Strings, Ordering::Release);
}

/// Reset to English (tests / locale unload).
#[allow(dead_code)] // Paired with [`set_current`]; used by unit tests.
pub fn reset_to_en() {
    CURRENT.store(std::ptr::null_mut(), Ordering::Release);
}

#[allow(dead_code)] // format helpers for entries whose consumers are not yet keyed
impl Strings {
    pub fn starting_simulation_attempt(&self, attempt: u32, max: u32) -> String {
        format!(
            "{}{attempt}{}{max})...",
            self.starting_simulation_attempt_prefix, self.starting_simulation_attempt_of
        )
    }

    pub fn could_not_start_sim(&self, msg: &str) -> String {
        format!("{}{msg}", self.could_not_start_sim_prefix)
    }

    pub fn lost_connection(&self, msg: &str) -> String {
        format!("{}{msg}", self.lost_connection_prefix)
    }

    pub fn loading_status(&self, label: &str, ready: bool) -> String {
        let status = if ready { self.ready } else { self.waiting };
        format!("{label}: {status}")
    }

    pub fn relative_minutes_ago(&self, mins: u64) -> String {
        format!("{mins}{}", self.minutes_ago_suffix)
    }

    pub fn relative_hours_ago(&self, hours: u64) -> String {
        format!("{hours}{}", self.hours_ago_suffix)
    }

    pub fn relative_days_ago(&self, days: u64) -> String {
        format!("{days}{}", self.days_ago_suffix)
    }

    pub fn playtime_hm(&self, hours: u64, mins: u64) -> String {
        format!("{hours}h {mins}m")
    }

    pub fn playtime_m(&self, mins: u64) -> String {
        format!("{mins}m")
    }

    pub fn earn_more_stars(&self, n: u32) -> String {
        let template = if n == 1 {
            self.earn_more_star
        } else {
            self.earn_more_stars
        };
        template.replacen("{}", &n.to_string(), 1)
    }

    pub fn save_subtitle(&self, city: &str, day: u32, stops: usize, playtime: &str) -> String {
        format!("{city} · Day {day} · {stops} stops · {playtime}")
    }

    pub fn start_city(&self, city: &str, difficulty: &str) -> String {
        format!("{}{city} ({difficulty})", self.start_prefix)
    }

    pub fn every_n_sim_days(&self, n: u32) -> String {
        format!(
            "{}{n}{}",
            self.every_n_sim_days_prefix, self.every_n_sim_days_suffix
        )
    }

    pub fn day_clock(&self, day: u32, hour: u32, minute: u32) -> String {
        format!("{}{day}  {hour:02}:{minute:02}", self.day_prefix)
    }

    /// Player-facing label for a weather sky state (v0.7 HUD chip).
    pub fn weather_label(&self, state: mf_protocol::WeatherState) -> &'static str {
        use mf_protocol::WeatherState as W;
        match state {
            W::Clear => self.weather_clear,
            W::Overcast => self.weather_overcast,
            W::Rain => self.weather_rain,
            W::Fog => self.weather_fog,
            W::Snow => self.weather_snow,
            W::Storm => self.weather_storm,
        }
    }

    /// Player-facing season label (weather chip tooltip).
    pub fn season_label(&self, season: mf_protocol::Season) -> &'static str {
        use mf_protocol::Season as S;
        match season {
            S::Winter => self.season_winter,
            S::Spring => self.season_spring,
            S::Summer => self.season_summer,
            S::Autumn => self.season_autumn,
        }
    }

    /// Player-facing headline-event label (weather chip tooltip).
    pub fn event_label(&self, event: mf_protocol::WeatherEvent) -> &'static str {
        use mf_protocol::WeatherEvent as E;
        match event {
            E::Blizzard => self.event_blizzard,
            E::Heatwave => self.event_heatwave,
        }
    }

    /// Tooltip body for the weather chip: season line, plus an event line when
    /// a headline event is active.
    pub fn weather_tooltip(
        &self,
        season: Option<mf_protocol::Season>,
        event: Option<mf_protocol::WeatherEvent>,
    ) -> String {
        let mut lines = Vec::new();
        if let Some(s) = season {
            lines.push(format!(
                "{}{}",
                self.weather_tooltip_season_prefix,
                self.season_label(s)
            ));
        }
        if let Some(e) = event {
            lines.push(format!(
                "{}{}",
                self.weather_tooltip_event_prefix,
                self.event_label(e)
            ));
        }
        lines.join("\n")
    }

    pub fn approval_pct(&self, pct: f64, trend: i8) -> String {
        if trend > 0 {
            format!("{}{pct:.0}%", self.approval_up_prefix)
        } else if trend < 0 {
            format!("{}{pct:.0}%", self.approval_down_prefix)
        } else {
            format!("{} {pct:.0}%", self.approval)
        }
    }

    pub fn pop(&self, n: &str) -> String {
        format!("{}{n}", self.pop_prefix)
    }

    pub fn crowded_routes_chip(&self, count: usize) -> String {
        let suffix = if count == 1 {
            self.crowded_route
        } else {
            self.crowded_routes
        };
        format!("{count}{suffix}")
    }

    pub fn slot_label(&self, n: u8) -> String {
        format!("{}{n}", self.save_slot_prefix)
    }

    pub fn slot_empty_label(&self, n: u8) -> String {
        format!(
            "{}{n}{}",
            self.save_slot_prefix, self.save_slot_empty_suffix
        )
    }

    pub fn autosave_label(&self, n: u8) -> String {
        format!("{}{n}", self.autosave_slot_prefix)
    }

    pub fn tutorial_step_of(&self, step: usize, total: usize) -> String {
        format!(
            "{}{step}{}{total}",
            self.tutorial_step_of_prefix, self.tutorial_step_of_mid
        )
    }

    pub fn goal_complete(&self, title: &str) -> String {
        format!("{}{title}", self.goal_complete_prefix)
    }

    pub fn tier(&self, n: u8) -> String {
        format!("{}{n}", self.tier_prefix)
    }

    pub fn tier_locked(&self, locked: u8, finish: u8) -> String {
        format!(
            "{}{locked}{}{finish}{}",
            self.tier_locked_prefix, self.tier_locked_mid, self.tier_locked_suffix
        )
    }

    pub fn line(&self, n: usize) -> String {
        format!("{}{n}", self.line_prefix)
    }

    pub fn station(&self, id: impl std::fmt::Display) -> String {
        format!("{}{id}", self.station_prefix)
    }

    pub fn upgrade_to_level(&self, level: u32) -> String {
        format!("{}{level}", self.upgrade_to_level_prefix)
    }

    pub fn level_mode(&self, level: u32, mode: &str) -> String {
        format!("{}{level} {mode}", self.level_prefix)
    }

    pub fn place_station_context(&self, mode: &str, cash: &str) -> String {
        format!(
            "{}{mode}{}{cash}",
            self.place_station_context_prefix, self.place_station_cash_mid
        )
    }

    pub fn route_context(&self, count: usize, quote: &str) -> String {
        format!(
            "{}{count}{}{quote}{}",
            self.route_context_prefix, self.route_context_mid, self.route_context_cost
        )
    }

    pub fn route_list_subtitle(&self, stations: usize, vehicles: usize, mode: &str) -> String {
        format!(
            "{stations}{}{vehicles}{}{mode}",
            self.route_list_stations, self.route_list_vehicles
        )
    }

    pub fn live_crowding_pct(&self, pct: f64) -> String {
        format!("{}{pct:.0}%", self.live_crowding_pct_prefix)
    }

    pub fn now_fare(&self, fare: &str) -> String {
        format!("{}{fare}", self.now_prefix)
    }

    pub fn cannot_build(&self, detail: &str) -> String {
        format!("{}{detail}", self.cannot_build_prefix)
    }

    pub fn save_failed(&self, err: &str) -> String {
        format!("{}{err}", self.save_failed_prefix)
    }

    pub fn load_failed(&self, err: &str) -> String {
        format!("{}{err}", self.load_failed_prefix)
    }

    pub fn saved_to_slot(&self, n: u8) -> String {
        format!("{}{n}", self.saved_to_slot_prefix)
    }

    pub fn star_earned(&self, description: &str) -> String {
        format!("{}{description}", self.star_earned_prefix)
    }

    pub fn star_cover_city(&self, pct: f64) -> String {
        format!(
            "{}{pct:.0}{}",
            self.star_cover_city_prefix, self.star_cover_city_suffix
        )
    }

    pub fn star_keep_approval(&self, pct: f64) -> String {
        format!(
            "{}{pct:.0}{}",
            self.star_keep_approval_prefix, self.star_keep_approval_suffix
        )
    }

    pub fn star_carry_trips(&self, trips: &str) -> String {
        format!(
            "{}{trips}{}",
            self.star_carry_trips_prefix, self.star_carry_trips_suffix
        )
    }

    pub fn star_transit_share(&self, pct: f64) -> String {
        format!(
            "{}{pct:.0}{}",
            self.star_transit_share_prefix, self.star_transit_share_suffix
        )
    }

    pub fn star_net_positive_days(&self, days: u32) -> String {
        format!(
            "{}{days}{}",
            self.star_net_positive_prefix, self.star_net_positive_suffix
        )
    }

    pub fn route_row_subtitle(
        &self,
        stops: usize,
        riders: &str,
        mode: &str,
        paused: bool,
    ) -> String {
        let paused_suffix = if paused { self.route_paused_suffix } else { "" };
        format!(
            "{stops}{}{riders}{}{mode}{paused_suffix}",
            self.route_row_stops_mid, self.route_row_riders_mid
        )
    }

    pub fn crowding_pct(&self, pct: f64) -> String {
        format!("{}{pct:.0}%", self.crowding_prefix)
    }

    pub fn route_update_failed(&self, detail: &str) -> String {
        format!("{}{detail}", self.route_update_failed_prefix)
    }
}

/// Walk every `&'static str` field on the active table (used by the dash
/// gate and any future locale integrity checks).
#[cfg(test)]
pub fn all_static_strings(s: &Strings) -> Vec<&'static str> {
    vec![
        s.brand,
        s.tagline,
        s.play,
        s.load_game,
        s.settings,
        s.quit,
        s.back,
        s.back_arrow,
        s.could_not_start_sim_prefix,
        s.starting_simulation,
        s.starting_simulation_attempt_prefix,
        s.starting_simulation_attempt_of,
        s.loading_city,
        s.ready,
        s.waiting,
        s.loading_static_city,
        s.loading_masks,
        s.loading_fields,
        s.loading_interface,
        s.just_now,
        s.minutes_ago_suffix,
        s.hours_ago_suffix,
        s.days_ago_suffix,
        s.unknown_city,
        s.empty,
        s.city,
        s.continue_label,
        s.difficulty,
        s.seed,
        s.seed_prefix,
        s.randomize,
        s.copy_seed,
        s.copied,
        s.start_prefix,
        s.earn_more_star,
        s.earn_more_stars,
        s.pick_slot_to_continue,
        s.autosaves,
        s.manual_slots,
        s.save_slot_prefix,
        s.save_slot_empty_suffix,
        s.autosave_slot_prefix,
        s.quality,
        s.theme,
        s.weather,
        s.day_night,
        s.fog_and_clouds,
        s.fog_and_clouds_gated,
        s.autosave,
        s.off,
        s.every_n_sim_days_prefix,
        s.every_n_sim_days_suffix,
        s.autosave_ring_hint,
        s.replay_tutorial,
        s.ui_scale,
        s.camera_sensitivity,
        s.colorblind,
        s.colorblind_off,
        s.colorblind_deuteranopia,
        s.colorblind_protanopia,
        s.colorblind_tritanopia,
        s.reduce_motion,
        s.pause_on_start,
        s.reduce_motion_hint,
        s.quality_potato,
        s.quality_low,
        s.quality_medium,
        s.quality_high,
        s.theme_light,
        s.theme_dark,
        s.theme_purple,
        s.difficulty_easy,
        s.difficulty_normal,
        s.difficulty_hard,
        s.day_prefix,
        s.approval,
        s.approval_up_prefix,
        s.approval_down_prefix,
        s.pop_prefix,
        s.crowded_route,
        s.crowded_routes,
        s.open_busiest_crowded_route,
        s.connecting_to_city,
        s.speed_1x,
        s.speed_10x,
        s.speed_30x,
        s.speed_120x,
        s.surface_view,
        s.subway_view,
        s.goals,
        s.paused,
        s.resume,
        s.save_game,
        s.quit_to_desktop,
        s.lost_connection_prefix,
        s.tutorial_move_camera_title,
        s.tutorial_select_station_title,
        s.tutorial_place_stations_title,
        s.tutorial_open_route_title,
        s.tutorial_watch_vehicles_title,
        s.tutorial_move_camera_body,
        s.tutorial_select_station_body,
        s.tutorial_place_stations_body,
        s.tutorial_open_route_body,
        s.tutorial_watch_vehicles_body,
        s.tutorial_step_of_prefix,
        s.tutorial_step_of_mid,
        s.skip,
        s.goal_place_3_stations_title,
        s.goal_place_3_stations_desc,
        s.goal_lay_first_track_title,
        s.goal_lay_first_track_desc,
        s.goal_launch_route_title,
        s.goal_launch_route_desc,
        s.goal_coverage_25_title,
        s.goal_coverage_25_desc,
        s.goal_500_riders_title,
        s.goal_500_riders_desc,
        s.goal_approval_60_title,
        s.goal_approval_60_desc,
        s.goal_complete_prefix,
        s.tier_prefix,
        s.tier_locked_prefix,
        s.tier_locked_mid,
        s.tier_locked_suffix,
        s.mode_bus,
        s.mode_tram,
        s.mode_metro,
        s.mode_rail,
        s.tool_select,
        s.tool_bus_station,
        s.tool_tram_station,
        s.tool_tram_station_locked,
        s.tool_route,
        s.tool_bulldoze,
        s.tool_undo,
        s.routes,
        s.place_station_context_prefix,
        s.place_station_cash_mid,
        s.not_quoted_yet,
        s.route_context_prefix,
        s.route_context_mid,
        s.route_context_cost,
        s.bulldoze_context,
        s.no_routes_yet,
        s.line_prefix,
        s.live_crowding,
        s.live_crowding_pct_prefix,
        s.route_list_stations,
        s.route_list_vehicles,
        s.farebox_per_day,
        s.operating_cost_per_day,
        s.net_per_day,
        s.vehicles,
        s.fare,
        s.now_prefix,
        s.name,
        s.delete,
        s.confirm_delete,
        s.unknown_error,
        s.cannot_build_prefix,
        s.bankrupt_banner,
        s.approval_collapsed_banner,
        s.time_up_banner,
        s.station_prefix,
        s.station_max_level,
        s.upgrade_to_level_prefix,
        s.level_prefix,
        s.boarding_per_day,
        s.arriving_per_day,
        s.catchment_district,
        s.people,
        s.jobs,
        s.routes_serving_station,
        s.no_routes_reach_station,
        s.finance,
        s.cash,
        s.loan_balance,
        s.yesterday,
        s.fares,
        s.subsidy,
        s.operations,
        s.maintenance,
        s.interest,
        s.net,
        s.net_last_7_days,
        s.transit_share,
        s.coverage,
        s.daily_transit_trips,
        s.farebox_recovery,
        s.lifetime_earnings,
        s.insights,
        s.verdict_bankrupt,
        s.verdict_lost_faith,
        s.verdict_time_up,
        s.verdict_complete,
        s.day,
        s.population_served,
        s.net_last_day,
        s.keep_playing,
        s.back_to_menu,
        s.save_failed_prefix,
        s.load_failed_prefix,
        s.autosaved,
        s.saved_to_slot_prefix,
        s.demand_overlay_toast,
        s.traffic_overlay_toast,
        s.unserved_overlay_toast,
        s.star_earned_prefix,
        s.star_cover_city_prefix,
        s.star_cover_city_suffix,
        s.star_keep_approval_prefix,
        s.star_keep_approval_suffix,
        s.star_carry_trips_prefix,
        s.star_carry_trips_suffix,
        s.star_transit_share_prefix,
        s.star_transit_share_suffix,
        s.star_net_positive_prefix,
        s.star_net_positive_suffix,
        s.minimap,
        s.no_city_loaded,
        s.city_select_hint,
        s.sort,
        s.sort_crowding,
        s.sort_riders,
        s.sort_net_income,
        s.close,
        s.no_routes_panel_hint,
        s.multi_select_hint,
        s.stops,
        s.stops_edit_hint,
        s.apply_stop_order,
        s.revert,
        s.edit_stops_in_world,
        s.pause,
        s.next_color,
        s.delete_route,
        s.route_row_stops_mid,
        s.route_row_riders_mid,
        s.route_paused_suffix,
        s.crowding_prefix,
        s.route_update_failed_prefix,
        s.shortcuts_title,
        s.shortcuts_hint,
        s.sc_section_camera,
        s.sc_section_build,
        s.sc_section_view,
        s.sc_pan,
        s.sc_orbit,
        s.sc_zoom,
        s.sc_bus_tool,
        s.sc_route_tool,
        s.sc_bulldoze_tool,
        s.sc_rotate,
        s.sc_multiselect,
        s.sc_confirm,
        s.sc_subway,
        s.sc_demand,
        s.sc_finance,
        s.sc_map,
        s.sc_minimap,
        s.sc_photo,
        s.sc_fullscreen,
        s.sc_pause,
        s.sc_help,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_table_has_no_em_or_en_dashes() {
        for s in all_static_strings(&EN) {
            assert!(
                !s.contains('\u{2013}') && !s.contains('\u{2014}'),
                "en/em dash in strings table: {s:?}"
            );
        }
    }

    #[test]
    fn current_defaults_to_en() {
        reset_to_en();
        assert!(std::ptr::eq(current(), &EN));
    }

    #[test]
    fn set_current_swaps_table() {
        reset_to_en();
        // Point at EN again via set_current to exercise the atomic path.
        set_current(&EN);
        assert!(std::ptr::eq(current(), &EN));
        reset_to_en();
    }
}
