//! On-screen volume (OSD) model: a temporary overlay on top of Idle/Home. Time enters
//! only through `step(dt)`, so everything here is deterministic.

use crate::spring::{Spring, SpringSpec};

/// How long the pill stays after the last change (seconds).
pub const HOLD_SECS: f32 = 1.5;
/// Level spring: the bar glides to new levels (response 0.25 s, zeta 1.0).
const LEVEL_SPEC: SpringSpec = SpringSpec::new(0.25, 1.0);
/// Cross-fade of the OSD content in/out (same as island content fade: 0.25 s, zeta 1.0).
const FADE_SPEC: SpringSpec = SpringSpec::new(0.25, 1.0);

/// Volume level 0.0..=1.0 and mute state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub level: f32,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeIcon {
    Muted,
    Low,
    Mid,
    High,
}

/// NaN -> 0.0, then clamped to 0.0..=1.0.
pub fn clamp_level(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

/// `clamp_level(level) * 100` rounded to the nearest integer.
pub fn percent(level: f32) -> u32 {
    (clamp_level(level) * 100.0).round() as u32
}

/// Muted -> Muted; otherwise by percent: 0..=33 Low, 34..=66 Mid, 67..=100 High.
pub fn icon_for(level: f32, muted: bool) -> VolumeIcon {
    if muted {
        return VolumeIcon::Muted;
    }
    match percent(level) {
        0..=33 => VolumeIcon::Low,
        34..=66 => VolumeIcon::Mid,
        _ => VolumeIcon::High,
    }
}

/// Everything the renderer needs for the OSD. `alpha` is the content cross-fade 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OsdView {
    pub alpha: f32,
    pub level: f32,
    pub percent: u32,
    pub muted: bool,
    pub icon: VolumeIcon,
}

#[derive(Clone, Debug)]
pub struct Osd {
    target: Volume,
    /// Seconds left before the OSD hides (ignored while pinned).
    hold: f32,
    pinned: bool,
    level: Spring,
    fade: Spring,
}

impl Default for Osd {
    fn default() -> Self {
        Self::new()
    }
}

impl Osd {
    /// Hidden, volume level 0.0 unmuted.
    pub fn new() -> Self {
        Self {
            target: Volume {
                level: 0.0,
                muted: false,
            },
            hold: 0.0,
            pinned: false,
            level: Spring::new(0.0, LEVEL_SPEC, 0.001),
            fade: Spring::new(0.0, FADE_SPEC, 0.002),
        }
    }

    /// Sets the target volume (clamped), shows the OSD and resets the hold timer.
    pub fn show(&mut self, v: Volume) {
        self.target = Volume {
            level: clamp_level(v.level),
            muted: v.muted,
        };
        self.level.retarget(self.target.level, LEVEL_SPEC);
        self.hold = HOLD_SECS;
        self.sync_fade();
    }

    /// While pinned (the user is dragging on the bar) the hold timer does not count down
    /// and the OSD stays visible. Unpinning restarts the timer at `HOLD_SECS`.
    pub fn pin(&mut self, on: bool) {
        if on == self.pinned {
            return;
        }
        self.pinned = on;
        if !on {
            self.hold = HOLD_SECS;
        }
        self.sync_fade();
    }

    /// Advances the hold timer (unless pinned), the level spring and the fade spring.
    /// Returns true on the call where the OSD goes from visible to hidden.
    pub fn step(&mut self, dt: f32) -> bool {
        let was_visible = self.is_visible();
        if !self.pinned {
            self.hold = (self.hold - dt.max(0.0)).max(0.0);
        }
        self.sync_fade();
        self.level.step(dt);
        self.fade.step(dt);
        was_visible && !self.is_visible()
    }

    /// True while the hold timer is running or pinned. Fade-out after hiding is not "visible".
    pub fn is_visible(&self) -> bool {
        self.pinned || self.hold > 0.0
    }

    /// The target volume (what the user last set/observed).
    pub fn volume(&self) -> Volume {
        self.target
    }

    pub fn view(&self) -> OsdView {
        OsdView {
            alpha: self.fade.value.clamp(0.0, 1.0),
            level: clamp_level(self.level.value),
            percent: percent(self.target.level),
            muted: self.target.muted,
            icon: icon_for(self.target.level, self.target.muted),
        }
    }

    /// True while visible, or while any spring is not settled. The app must keep
    /// stepping while this is true.
    pub fn is_animating(&self) -> bool {
        self.is_visible() || !self.level.settled() || !self.fade.settled()
    }

    fn sync_fade(&mut self) {
        let target = if self.is_visible() { 1.0 } else { 0.0 };
        self.fade.retarget(target, FADE_SPEC);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(level: f32) -> Volume {
        Volume {
            level,
            muted: false,
        }
    }

    fn run(osd: &mut Osd, secs: f32) {
        let dt = 1.0 / 120.0;
        let mut t = 0.0;
        while t < secs {
            osd.step(dt);
            t += dt;
        }
    }

    #[test]
    fn hold_timer_hides_after_hold_secs() {
        let mut osd = Osd::new();
        osd.show(vol(0.5));
        assert!(!osd.step(1.4));
        assert!(osd.is_visible());
        assert!(osd.step(0.2));
        assert!(!osd.is_visible());
    }

    #[test]
    fn show_resets_hold() {
        let mut osd = Osd::new();
        osd.show(vol(0.5));
        osd.step(1.0);
        osd.show(vol(0.6));
        osd.step(1.0);
        assert!(osd.is_visible());
        osd.step(0.6);
        assert!(!osd.is_visible());
    }

    #[test]
    fn pin_keeps_visible_until_released() {
        let mut osd = Osd::new();
        osd.show(vol(0.5));
        osd.pin(true);
        assert!(!osd.step(10.0));
        assert!(osd.is_visible());
        osd.pin(false);
        assert!(!osd.step(1.4));
        assert!(osd.is_visible());
        assert!(osd.step(0.2));
        assert!(!osd.is_visible());
    }

    #[test]
    fn clamps_levels() {
        assert_eq!(clamp_level(-0.2), 0.0);
        assert_eq!(clamp_level(1.7), 1.0);
        assert_eq!(clamp_level(f32::NAN), 0.0);
        let mut osd = Osd::new();
        osd.show(Volume {
            level: 2.0,
            muted: false,
        });
        assert_eq!(osd.volume().level, 1.0);
    }

    #[test]
    fn percent_rounds() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.5), 50);
        assert_eq!(percent(1.0), 100);
    }

    #[test]
    fn icon_thresholds() {
        assert_eq!(icon_for(0.9, true), VolumeIcon::Muted);
        assert_eq!(icon_for(0.0, false), VolumeIcon::Low);
        assert_eq!(icon_for(0.33, false), VolumeIcon::Low);
        assert_eq!(icon_for(0.34, false), VolumeIcon::Mid);
        assert_eq!(icon_for(0.66, false), VolumeIcon::Mid);
        assert_eq!(icon_for(0.67, false), VolumeIcon::High);
        assert_eq!(icon_for(1.0, false), VolumeIcon::High);
    }

    #[test]
    fn level_and_fade_settle() {
        let mut osd = Osd::new();
        osd.show(vol(0.5));
        run(&mut osd, 1.0);
        assert!((osd.view().alpha - 1.0).abs() < 1e-3);
        run(&mut osd, 1.0);
        let v = osd.view();
        assert!((v.level - 0.5).abs() < 0.001);
        assert_eq!(v.percent, 50);
        run(&mut osd, 1.0);
        let v = osd.view();
        assert_eq!(v.alpha, 0.0);
        assert!(!osd.is_animating());
    }
}
