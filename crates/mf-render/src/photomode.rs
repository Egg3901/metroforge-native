//! Photo / cinematic mode render support (additive): a local time-of-day
//! override for the day/night rig, plus optional cinematic letterbox bars.
//!
//! Owned by `mf-game`'s `photomode.rs`, which writes [`PhotoModeRender`] when
//! the player toggles photo mode. When inactive (`active == false`), every
//! system here early-returns — zero cost beyond a bool check. No film grain,
//! no watermarks: letterbox is the only optional framing overlay.

use bevy::prelude::*;

/// Shared render-side knobs for photo mode. `mf-game` writes; `daynight` and
/// the letterbox system read. Default is fully inert.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PhotoModeRender {
    /// Photo mode is engaged (HUD hidden on the game side).
    pub active: bool,
    /// When `Some`, day/night uses this hour (0..24) instead of the sim tick.
    /// Cleared on exit so the sim-driven clock resumes exactly.
    pub override_hour: Option<f32>,
    /// Draw cinematic letterbox bars (clean output — no grain/watermark).
    pub letterbox: bool,
    /// Target frame aspect for letterbox (width/height). 2.39 ≈ scope.
    pub letterbox_aspect: f32,
}

impl Default for PhotoModeRender {
    fn default() -> Self {
        PhotoModeRender {
            active: false,
            override_hour: None,
            letterbox: false,
            letterbox_aspect: 2.39,
        }
    }
}

#[derive(Resource, Default)]
struct LetterboxState {
    top: Option<Entity>,
    bottom: Option<Entity>,
}

#[derive(Component)]
struct LetterboxBar;

pub struct MfPhotoModeRenderPlugin;

impl Plugin for MfPhotoModeRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhotoModeRender>()
            .init_resource::<LetterboxState>()
            .add_systems(
                Update,
                maintain_letterbox_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

fn maintain_letterbox_system(
    render: Res<PhotoModeRender>,
    mut state: ResMut<LetterboxState>,
    mut commands: Commands,
    windows: Query<&Window>,
    mut nodes: Query<&mut Node, With<LetterboxBar>>,
) {
    let want = render.active && render.letterbox;
    if !want {
        if let Some(e) = state.top.take() {
            commands.entity(e).despawn();
        }
        if let Some(e) = state.bottom.take() {
            commands.entity(e).despawn();
        }
        return;
    }

    let aspect = render.letterbox_aspect.max(1.01);
    let (bar_frac, win_w, win_h) = match windows.single() {
        Ok(w) => {
            let w_px = w.width().max(1.0);
            let h_px = w.height().max(1.0);
            let framed_h = w_px / aspect;
            let bar = ((h_px - framed_h) * 0.5).max(0.0) / h_px;
            (bar, w_px, h_px)
        }
        Err(_) => (0.12, 1920.0, 1080.0),
    };
    // Degenerate (window already wider than target): hide bars.
    if bar_frac < 0.001 || win_w <= 0.0 || win_h <= 0.0 {
        if let Some(e) = state.top.take() {
            commands.entity(e).despawn();
        }
        if let Some(e) = state.bottom.take() {
            commands.entity(e).despawn();
        }
        return;
    }

    let height = Val::Percent(bar_frac * 100.0);
    let spawn_bar = |commands: &mut Commands, top: Val| {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top,
                    width: Val::Percent(100.0),
                    height,
                    ..default()
                },
                BackgroundColor(Color::BLACK),
                GlobalZIndex(i32::MAX),
                LetterboxBar,
                Name::new("photo-mode-letterbox"),
            ))
            .id()
    };

    match state.top {
        Some(e) => {
            if let Ok(mut node) = nodes.get_mut(e) {
                node.height = height;
                node.top = Val::Px(0.0);
            }
        }
        None => {
            state.top = Some(spawn_bar(&mut commands, Val::Px(0.0)));
        }
    }
    match state.bottom {
        Some(e) => {
            if let Ok(mut node) = nodes.get_mut(e) {
                node.height = height;
                node.top = Val::Percent((1.0 - bar_frac) * 100.0);
            }
        }
        None => {
            state.bottom = Some(spawn_bar(
                &mut commands,
                Val::Percent((1.0 - bar_frac) * 100.0),
            ));
        }
    }
}
