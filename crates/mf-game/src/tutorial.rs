//! First-launch onboarding (playable-alpha wave): a short guided flow the
//! first time a player reaches `InGame`. Five dismissible steps walk the
//! core loop — move the camera, pick the Station tool, place stations, open
//! a route, watch it run — each advancing when the player actually performs
//! the action (or via Skip). Completion (or Skip) persists into
//! `config.toml`'s `tutorial_completed` flag (`config.rs`), so the flow only
//! ever shows once; a "Replay tutorial" entry in Settings re-arms it.
//!
//! The step *state machine* (which step is active, how it advances, how Skip
//! and completion resolve) is deliberately split into pure, ECS-free helpers
//! ([`TutorialStep`], [`step_satisfied`], [`StepInputs`]) so it can be unit
//! tested without a running Bevy app or a live sidecar.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_state::{LatestFrame, LatestUi};

use crate::audio::{PlaySfx, Sfx};
use crate::camera::CameraRig;
use crate::config::MfConfig;
use crate::state::AppState;
use crate::tools::{ActiveTool, ToolState};

// ---------------------------------------------------------------------
// Pure step state machine (no ECS types — unit-tested directly).
// ---------------------------------------------------------------------

/// One onboarding step. Ordered as the player performs them; `ALL` is the
/// canonical sequence and the only place the ordering lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TutorialStep {
    /// Drag to pan / scroll to zoom the camera.
    MoveCamera,
    /// Select the place-station tool from the toolbar.
    SelectStationTool,
    /// Place two stations on the map (two so a route is possible next).
    PlaceStations,
    /// Link the stations into a route and open the line.
    OpenRoute,
    /// Watch vehicles begin serving the new line.
    WatchVehicles,
}

impl TutorialStep {
    /// Canonical ordering. Everything else (first step, advancing, the
    /// "Step N of M" counter) derives from this single list.
    pub const ALL: [TutorialStep; 5] = [
        TutorialStep::MoveCamera,
        TutorialStep::SelectStationTool,
        TutorialStep::PlaceStations,
        TutorialStep::OpenRoute,
        TutorialStep::WatchVehicles,
    ];

    /// The step the flow opens on.
    pub fn first() -> TutorialStep {
        TutorialStep::ALL[0]
    }

    /// Zero-based position in [`TutorialStep::ALL`].
    fn index(self) -> usize {
        TutorialStep::ALL
            .iter()
            .position(|&s| s == self)
            .expect("every variant is in ALL")
    }

    /// One-based number for the "Step N of M" counter.
    pub fn number(self) -> usize {
        self.index() + 1
    }

    /// The next step, or `None` when this is the last one (the flow is
    /// finished). Pure: drives both the auto-advance and the Skip paths.
    pub fn next(self) -> Option<TutorialStep> {
        TutorialStep::ALL.get(self.index() + 1).copied()
    }

    /// Terse, imperative title. No em/en dashes, no filler.
    pub fn title(self) -> &'static str {
        match self {
            TutorialStep::MoveCamera => "Move the camera",
            TutorialStep::SelectStationTool => "Pick the Station tool",
            TutorialStep::PlaceStations => "Place two stations",
            TutorialStep::OpenRoute => "Open a route",
            TutorialStep::WatchVehicles => "Watch it run",
        }
    }

    /// One-line instruction. Same copy rules as [`TutorialStep::title`].
    pub fn body(self) -> &'static str {
        match self {
            TutorialStep::MoveCamera => "Drag to pan. Scroll to zoom.",
            TutorialStep::SelectStationTool => "Click Station in the toolbar below.",
            TutorialStep::PlaceStations => "Click a road to drop a station. Place two.",
            TutorialStep::OpenRoute => {
                "Pick the Route tool. Click both stations. Double click to open the line."
            }
            TutorialStep::WatchVehicles => "Vehicles now serve your line. You are ready.",
        }
    }
}

/// What the player has done since the flow began, distilled to just the
/// signals the steps care about. Built each frame from live resources by
/// the systems below; kept as a plain struct so [`step_satisfied`] stays a
/// pure function that unit tests can drive with hand data.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepInputs {
    /// The camera target or zoom moved past a small deadzone since start.
    pub camera_moved: bool,
    /// A place-station tool is currently the active tool.
    pub station_tool_selected: bool,
    /// Stations added since the flow began.
    pub stations_added: usize,
    /// Routes added since the flow began.
    pub routes_added: usize,
    /// At least one vehicle is on the map.
    pub vehicles_running: bool,
}

/// Whether `step`'s completion condition is met given the player's progress.
/// Pure and total — the whole advance decision for one step in one place.
pub fn step_satisfied(step: TutorialStep, inputs: &StepInputs) -> bool {
    match step {
        TutorialStep::MoveCamera => inputs.camera_moved,
        TutorialStep::SelectStationTool => inputs.station_tool_selected,
        // Two stations so the OpenRoute step that follows is actually
        // reachable (a route needs at least two stops).
        TutorialStep::PlaceStations => inputs.stations_added >= 2,
        TutorialStep::OpenRoute => inputs.routes_added >= 1,
        TutorialStep::WatchVehicles => inputs.vehicles_running,
    }
}

// ---------------------------------------------------------------------
// Runtime state (thin ECS wrapper over the pure machine above).
// ---------------------------------------------------------------------

/// Where the flow is right now. `Active` carries the current step; the
/// baseline snapshot for delta signals lives in [`TutorialState`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TutorialPhase {
    /// Not showing (either never started this session, or finished/skipped).
    #[default]
    Inactive,
    /// Showing `TutorialStep`.
    Active(TutorialStep),
}

/// Baseline captured on the first active frame, so "stations added" / camera
/// movement are measured relative to where the player started rather than
/// from an absolute zero that a loaded save would already be past.
#[derive(Clone, Copy, Debug)]
struct Baseline {
    cam_target: Vec2,
    cam_distance: f32,
    stations: usize,
    routes: usize,
}

/// Camera must move at least this far (world units of target pan) OR change
/// zoom by at least [`ZOOM_DEADZONE`] before the MoveCamera step counts —
/// a deadzone so an incidental single-pixel nudge doesn't skip the step.
const PAN_DEADZONE: f32 = 40.0;
const ZOOM_DEADZONE: f32 = 60.0;

#[derive(Resource, Default)]
pub struct TutorialState {
    pub phase: TutorialPhase,
    baseline: Option<Baseline>,
    /// Set by the Settings "Replay tutorial" button; the InGame systems pick
    /// it up and (re)start the flow even if it already completed once.
    replay_requested: bool,
}

impl TutorialState {
    /// Settings "Replay tutorial": re-arm the flow. Takes effect immediately
    /// if already `InGame`, otherwise on the next city load.
    pub fn request_replay(&mut self) {
        self.replay_requested = true;
    }

    /// Begin at the first step, discarding any stale baseline so it is
    /// recaptured against the current world on the next update.
    fn begin(&mut self) {
        self.phase = TutorialPhase::Active(TutorialStep::first());
        self.baseline = None;
    }

    /// The step currently shown, if any.
    pub fn current_step(&self) -> Option<TutorialStep> {
        match self.phase {
            TutorialPhase::Active(step) => Some(step),
            TutorialPhase::Inactive => None,
        }
    }

    /// Advance past the current step (auto-advance path). Returns `true` when
    /// this call finished the whole flow, so the caller can persist
    /// completion once.
    fn advance(&mut self) -> bool {
        if let TutorialPhase::Active(step) = self.phase {
            match step.next() {
                Some(next) => {
                    self.phase = TutorialPhase::Active(next);
                    false
                }
                None => {
                    self.phase = TutorialPhase::Inactive;
                    true
                }
            }
        } else {
            false
        }
    }

    /// Skip the whole flow. Returns `true` when it was actually active (so
    /// completion is persisted exactly once).
    fn skip(&mut self) -> bool {
        let was_active = matches!(self.phase, TutorialPhase::Active(_));
        self.phase = TutorialPhase::Inactive;
        was_active
    }
}

// ---------------------------------------------------------------------
// Plugin wiring.
// ---------------------------------------------------------------------

pub struct MfTutorialPlugin;

impl Plugin for MfTutorialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialState>()
            .add_systems(OnEnter(AppState::InGame), begin_tutorial_on_enter)
            .add_systems(
                Update,
                advance_tutorial_system.run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                tutorial_overlay_system
                    .run_if(in_state(AppState::InGame))
                    .run_if(|| !crate::design_system::hud_hidden())
                    .run_if(|| !crate::design_system::hud_scene_enabled()),
            );
    }
}

/// First reach of `InGame`: start the flow unless it has already been
/// completed/skipped once (persisted in `config.toml`). A pending replay
/// request always wins over the persisted flag.
fn begin_tutorial_on_enter(mut tutorial: ResMut<TutorialState>, config: Res<MfConfig>) {
    if tutorial.replay_requested || !config.tutorial_completed {
        tutorial.begin();
        tutorial.replay_requested = false;
    }
}

/// Auto-advance: build [`StepInputs`] from live resources and advance the
/// flow whenever the current step's condition is met. Also honors a
/// mid-session replay request (Settings button pressed while `InGame`).
#[allow(clippy::too_many_arguments)]
fn advance_tutorial_system(
    mut tutorial: ResMut<TutorialState>,
    mut config: ResMut<MfConfig>,
    rigs: Query<&CameraRig>,
    tool: Res<ToolState>,
    ui_state: Res<LatestUi>,
    frame: Res<LatestFrame>,
) {
    if tutorial.replay_requested {
        tutorial.begin();
        tutorial.replay_requested = false;
    }
    if tutorial.current_step().is_none() {
        return;
    }

    let rig = rigs.iter().next();
    let stations_now = ui_state.0.as_ref().map(|s| s.stations.len()).unwrap_or(0);
    let routes_now = ui_state.0.as_ref().map(|s| s.routes.len()).unwrap_or(0);

    // Capture the baseline on the first active frame, then wait a frame so
    // deltas are measured against it.
    if tutorial.baseline.is_none() {
        tutorial.baseline = Some(Baseline {
            cam_target: rig.map(|r| r.target).unwrap_or(Vec2::ZERO),
            cam_distance: rig.map(|r| r.distance).unwrap_or(0.0),
            stations: stations_now,
            routes: routes_now,
        });
        return;
    }
    let baseline = tutorial.baseline.expect("populated just above");

    let camera_moved = rig
        .map(|r| {
            r.target.distance(baseline.cam_target) > PAN_DEADZONE
                || (r.distance - baseline.cam_distance).abs() > ZOOM_DEADZONE
        })
        .unwrap_or(false);

    let inputs = StepInputs {
        camera_moved,
        station_tool_selected: matches!(tool.active, ActiveTool::PlaceStation(_)),
        stations_added: stations_now.saturating_sub(baseline.stations),
        routes_added: routes_now.saturating_sub(baseline.routes),
        vehicles_running: frame
            .0
            .as_ref()
            .map(|f| f.vehicle_count > 0)
            .unwrap_or(false),
    };

    if let Some(step) = tutorial.current_step() {
        if step_satisfied(step, &inputs) && tutorial.advance() {
            persist_completed(&mut config);
        }
    }
}

/// Draws the current step's hint card, anchored near the HUD control it
/// refers to, plus a Skip button that dismisses the whole flow.
fn tutorial_overlay_system(
    mut contexts: EguiContexts,
    mut tutorial: ResMut<TutorialState>,
    mut config: ResMut<MfConfig>,
    pause: Res<crate::state::PauseState>,
    mut sfx: EventWriter<PlaySfx>,
) -> Result {
    let Some(step) = tutorial.current_step() else {
        return Ok(());
    };
    if pause.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let fade =
        crate::design_system::panel_fade(ctx, egui::Id::new(("tutorial_hint_fade", step.number())));

    // Anchor toolbar-related steps just above the bottom toolbar, and the
    // camera/vehicle steps up near the top bar, so the hint sits beside the
    // control it is talking about.
    let (anchor, offset) = match step {
        TutorialStep::MoveCamera | TutorialStep::WatchVehicles => (
            egui::Align2::CENTER_TOP,
            egui::vec2(0.0, crate::design_system::HINT_TOP_OFFSET),
        ),
        _ => (
            egui::Align2::CENTER_BOTTOM,
            egui::vec2(0.0, crate::design_system::HINT_BOTTOM_OFFSET),
        ),
    };

    let mut skip_clicked = false;
    egui::Area::new(egui::Id::new("tutorial_hint"))
        .order(crate::design_system::ORDER_HINT)
        .anchor(anchor, offset)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::default()
                .fill(tutorial_panel_bg())
                .stroke(egui::Stroke::new(
                    crate::design_system::HINT_STROKE_WIDTH,
                    tutorial_accent(),
                ))
                .corner_radius(crate::design_system::CORNER_RADIUS)
                .inner_margin(egui::Margin::symmetric(
                    crate::design_system::HINT_MARGIN_H,
                    crate::design_system::HINT_MARGIN_V,
                ))
                .show(ui, |ui| {
                    ui.set_max_width(crate::design_system::HINT_MAX_WIDTH);
                    ui.label(
                        egui::RichText::new(format!(
                            "Step {} of {}",
                            step.number(),
                            TutorialStep::ALL.len()
                        ))
                        .size(crate::design_system::TEXT_XS)
                        .color(tutorial_accent())
                        .strong(),
                    );
                    ui.add_space(crate::design_system::SPACE_XXS / 2.0);
                    ui.label(
                        egui::RichText::new(step.title())
                            .size(crate::design_system::TEXT_MD + 2.0)
                            .strong()
                            .color(tutorial_text()),
                    );
                    ui.add_space(crate::design_system::SPACE_XXS);
                    ui.label(
                        egui::RichText::new(step.body())
                            .size(crate::design_system::TEXT_SM)
                            .color(tutorial_text()),
                    );
                    ui.add_space(crate::design_system::SPACE_XS + 2.0);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Skip").size(crate::design_system::TEXT_XS + 1.0),
                        ))
                        .clicked()
                    {
                        skip_clicked = true;
                    }
                });
        });

    if skip_clicked && tutorial.skip() {
        sfx.write(PlaySfx(Sfx::Cancel));
        persist_completed(&mut config);
    }
    Ok(())
}

/// Mark the flow completed and persist it, so it never shows unprompted
/// again. Idempotent — a second call (skip after auto-finish, say) is a
/// harmless re-write.
fn persist_completed(config: &mut MfConfig) {
    config.set_tutorial_completed(true);
}

// Theme-aware colors, delegated to the shared design system (same source
// `hud.rs` reads) so the card matches the active theme.
fn tutorial_panel_bg() -> egui::Color32 {
    crate::design_system::current_colors().panel_bg
}
fn tutorial_text() -> egui::Color32 {
    crate::design_system::current_colors().text
}
fn tutorial_accent() -> egui::Color32 {
    crate::design_system::current_colors().accent
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ordering / navigation -------------------------------------------

    #[test]
    fn first_is_move_camera() {
        assert_eq!(TutorialStep::first(), TutorialStep::MoveCamera);
    }

    #[test]
    fn steps_number_one_through_five() {
        let numbers: Vec<usize> = TutorialStep::ALL.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn next_walks_the_sequence_then_ends() {
        let mut step = TutorialStep::first();
        let mut visited = vec![step];
        while let Some(n) = step.next() {
            step = n;
            visited.push(step);
        }
        assert_eq!(visited, TutorialStep::ALL.to_vec());
        assert_eq!(TutorialStep::WatchVehicles.next(), None);
    }

    // --- step_satisfied --------------------------------------------------

    #[test]
    fn move_camera_needs_movement() {
        let mut i = StepInputs::default();
        assert!(!step_satisfied(TutorialStep::MoveCamera, &i));
        i.camera_moved = true;
        assert!(step_satisfied(TutorialStep::MoveCamera, &i));
    }

    #[test]
    fn select_tool_needs_the_station_tool() {
        let mut i = StepInputs::default();
        assert!(!step_satisfied(TutorialStep::SelectStationTool, &i));
        i.station_tool_selected = true;
        assert!(step_satisfied(TutorialStep::SelectStationTool, &i));
    }

    #[test]
    fn place_stations_needs_two() {
        let mut i = StepInputs {
            stations_added: 1,
            ..Default::default()
        };
        assert!(!step_satisfied(TutorialStep::PlaceStations, &i));
        i.stations_added = 2;
        assert!(step_satisfied(TutorialStep::PlaceStations, &i));
    }

    #[test]
    fn open_route_needs_a_route() {
        let mut i = StepInputs::default();
        assert!(!step_satisfied(TutorialStep::OpenRoute, &i));
        i.routes_added = 1;
        assert!(step_satisfied(TutorialStep::OpenRoute, &i));
    }

    #[test]
    fn watch_vehicles_needs_a_vehicle() {
        let mut i = StepInputs::default();
        assert!(!step_satisfied(TutorialStep::WatchVehicles, &i));
        i.vehicles_running = true;
        assert!(step_satisfied(TutorialStep::WatchVehicles, &i));
    }

    // --- runtime advance / skip / replay ---------------------------------

    #[test]
    fn advance_moves_to_next_step_without_finishing() {
        let mut t = TutorialState::default();
        t.begin();
        assert_eq!(t.current_step(), Some(TutorialStep::MoveCamera));
        assert!(!t.advance());
        assert_eq!(t.current_step(), Some(TutorialStep::SelectStationTool));
    }

    #[test]
    fn advancing_off_the_last_step_finishes_and_deactivates() {
        let mut t = TutorialState::default();
        t.begin();
        // Walk to the last step.
        for _ in 0..TutorialStep::ALL.len() - 1 {
            assert!(!t.advance());
        }
        assert_eq!(t.current_step(), Some(TutorialStep::WatchVehicles));
        // The final advance reports completion and clears the flow.
        assert!(t.advance());
        assert_eq!(t.current_step(), None);
        assert_eq!(t.phase, TutorialPhase::Inactive);
    }

    #[test]
    fn skip_reports_completion_only_when_active() {
        let mut t = TutorialState::default();
        // Inactive: skipping is a no-op that must not re-persist completion.
        assert!(!t.skip());
        t.begin();
        assert!(t.skip());
        assert_eq!(t.current_step(), None);
        // A second skip after already finishing is inert.
        assert!(!t.skip());
    }

    #[test]
    fn advance_on_inactive_flow_is_inert() {
        let mut t = TutorialState::default();
        assert!(!t.advance());
        assert_eq!(t.phase, TutorialPhase::Inactive);
    }

    #[test]
    fn request_replay_sets_the_flag() {
        let mut t = TutorialState::default();
        assert!(!t.replay_requested);
        t.request_replay();
        assert!(t.replay_requested);
    }

    #[test]
    fn begin_resets_baseline_and_starts_at_first_step() {
        let mut t = TutorialState {
            baseline: Some(Baseline {
                cam_target: Vec2::new(5.0, 5.0),
                cam_distance: 999.0,
                stations: 3,
                routes: 1,
            }),
            ..Default::default()
        };
        t.begin();
        assert!(t.baseline.is_none());
        assert_eq!(t.current_step(), Some(TutorialStep::first()));
    }
}
