use bevy_ecs::prelude::*;

/// Placeholder for terrain height sampling, shared so every layer (roads,
/// buildings, transit, vehicles, agents — spec §3.3) can position things at
/// `heightAt(x, z) + offset` without depending on `mf-render` directly.
///
/// `mf-render`'s `terrain.rs` is the *real* owner: once fields are loaded it
/// replaces this resource's closure with a bilinear sample over
/// `LatestFields`/`CurrentCity`. Until then (Boot/MainMenu/Loading, or if
/// terrain hasn't baked yet) it defaults to flat ground at `y = 0`, which is
/// enough for `mf-game`'s camera rig and HUD to function headlessly.
#[derive(Resource)]
pub struct HeightAt(pub Box<dyn Fn(f32, f32) -> f32 + Send + Sync>);

impl Default for HeightAt {
    fn default() -> Self {
        HeightAt(Box::new(|_x: f32, _z: f32| 0.0))
    }
}

impl HeightAt {
    pub fn sample(&self, x: f32, z: f32) -> f32 {
        (self.0)(x, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_flat_ground() {
        let h = HeightAt::default();
        assert_eq!(h.sample(123.0, -456.0), 0.0);
    }

    #[test]
    fn can_be_replaced() {
        let h = HeightAt(Box::new(|x, z| x + z));
        assert_eq!(h.sample(2.0, 3.0), 5.0);
    }
}
