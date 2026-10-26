//! The Settings panel's logic: the hotkey chip cycles, the controls and their hit
//! testing, and what the panel shows. Pure; the app applies the actions (autostart,
//! hotkey registration, saving the config).

use crate::config::{IslandStyle, SnapConfig, SnapHotkey};
use crate::layout;

/// The order the Snap hotkey chip cycles through.
const HOTKEYS: [SnapHotkey; 4] = [
    SnapHotkey::PrtSc,
    SnapHotkey::CtrlSpace,
    SnapHotkey::CtrlAltS,
    SnapHotkey::CtrlShiftS,
];

/// The order the alt chip cycles through. `None` is "off" (`"none"` in the config).
const ALT_HOTKEYS: [Option<SnapHotkey>; 4] = [
    Some(SnapHotkey::CtrlSpace),
    Some(SnapHotkey::CtrlAltS),
    Some(SnapHotkey::CtrlShiftS),
    None,
];

/// The `[snap] alt_hotkey` value for "off".
pub const ALT_OFF: &str = "none";

/// The chip after `current`: `PrtSc → Ctrl+Space → Ctrl+Alt+S → Ctrl+Shift+S → PrtSc`.
pub fn next_hotkey(current: SnapHotkey) -> SnapHotkey {
    let i = HOTKEYS.iter().position(|k| *k == current).unwrap_or(0);
    HOTKEYS[(i + 1) % HOTKEYS.len()]
}

/// The alt chip after `current`: `Ctrl+Space → Ctrl+Alt+S → Ctrl+Shift+S → off → Ctrl+Space`,
/// skipping the main key (the two can't be the same). An unknown current value starts
/// the cycle over.
pub fn next_alt_hotkey(current: Option<SnapHotkey>, main: SnapHotkey) -> Option<SnapHotkey> {
    let n = ALT_HOTKEYS.len();
    let i = ALT_HOTKEYS
        .iter()
        .position(|k| *k == current)
        .unwrap_or(n - 1);
    (1..=n)
        .map(|step| ALT_HOTKEYS[(i + step) % n])
        .find(|k| *k != Some(main))
        .unwrap_or(None)
}

/// The alt chip's text and its config value.
pub fn alt_label(alt: Option<SnapHotkey>) -> &'static str {
    alt.map_or("off", SnapHotkey::label)
}

pub fn alt_config(alt: Option<SnapHotkey>) -> &'static str {
    alt.map_or(ALT_OFF, SnapHotkey::label)
}

/// A click on the Snap hotkey chip: the next main key. When it lands on the alt key,
/// the alt moves on (it can't be the same key).
pub fn cycle_hotkey(snap: &mut SnapConfig) {
    let main = next_hotkey(snap.hotkey());
    snap.hotkey = main.label().to_string();
    let alt = SnapHotkey::parse(&snap.alt_hotkey);
    if alt == Some(main) {
        snap.alt_hotkey = alt_config(next_alt_hotkey(alt, main)).to_string();
    }
}

/// A click on the alt chip: the next alt key, never the main key.
pub fn cycle_alt_hotkey(snap: &mut SnapConfig) {
    let alt = next_alt_hotkey(snap.alt_hotkey(), snap.hotkey());
    snap.alt_hotkey = alt_config(alt).to_string();
}

/// A control in the Settings panel. Back is handled by the island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    ToggleAutostart,
    CycleHotkey,
    CycleAltHotkey,
    /// Notch ↔ pill.
    ToggleStyle,
}

/// A control the pointer can land on, in pill-local px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsHit {
    Back,
    Action(SettingsAction),
}

/// The control at view-local `(x, y)`.
pub fn hit(x: f32, y: f32) -> Option<SettingsHit> {
    if layout::ST_BACK.contains(x, y) {
        Some(SettingsHit::Back)
    } else if layout::ST_TOGGLE.contains(x, y) {
        Some(SettingsHit::Action(SettingsAction::ToggleAutostart))
    } else if layout::ST_HOTKEY.contains(x, y) {
        Some(SettingsHit::Action(SettingsAction::CycleHotkey))
    } else if layout::ST_ALT.contains(x, y) {
        Some(SettingsHit::Action(SettingsAction::CycleAltHotkey))
    } else if layout::ST_STYLE.contains(x, y) {
        Some(SettingsHit::Action(SettingsAction::ToggleStyle))
    } else {
        None
    }
}

/// What the panel draws. Built by the app from the config and the registration results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsView {
    pub autostart: bool,
    pub hotkey: SnapHotkey,
    pub alt: Option<SnapHotkey>,
    /// Another app holds the key: the chip draws in the warn colour.
    pub hotkey_taken: bool,
    pub alt_taken: bool,
    pub style: IslandStyle,
}

impl Default for SettingsView {
    fn default() -> Self {
        Self {
            autostart: false,
            hotkey: SnapHotkey::PrtSc,
            alt: Some(SnapHotkey::CtrlSpace),
            hotkey_taken: false,
            alt_taken: false,
            style: IslandStyle::default(),
        }
    }
}

impl SettingsView {
    /// The short status line under the rows, if a key could not be registered.
    pub fn status(&self) -> Option<&'static str> {
        (self.hotkey_taken || self.alt_taken).then_some("taken by another app")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SnapHotkey::*;

    #[test]
    fn main_hotkey_cycles_through_the_four_keys() {
        let mut k = PrtSc;
        let mut seen = vec![k];
        for _ in 0..4 {
            k = next_hotkey(k);
            seen.push(k);
        }
        assert_eq!(seen, [PrtSc, CtrlSpace, CtrlAltS, CtrlShiftS, PrtSc]);
    }

    #[test]
    fn alt_hotkey_cycles_and_includes_off() {
        let mut k = Some(CtrlSpace);
        let mut seen = vec![k];
        for _ in 0..4 {
            k = next_alt_hotkey(k, PrtSc);
            seen.push(k);
        }
        assert_eq!(
            seen,
            [
                Some(CtrlSpace),
                Some(CtrlAltS),
                Some(CtrlShiftS),
                None,
                Some(CtrlSpace)
            ]
        );
    }

    #[test]
    fn alt_hotkey_skips_the_main_key() {
        assert_eq!(next_alt_hotkey(Some(CtrlSpace), CtrlAltS), Some(CtrlShiftS));
        assert_eq!(next_alt_hotkey(None, CtrlSpace), Some(CtrlAltS));
        assert_eq!(next_alt_hotkey(Some(CtrlShiftS), CtrlSpace), None);
        // Every main key, every start: the result is never the main key.
        for main in HOTKEYS {
            for start in ALT_HOTKEYS {
                let next = next_alt_hotkey(start, main);
                assert_ne!(next, Some(main), "{main:?} {start:?}");
            }
        }
    }

    #[test]
    fn alt_names() {
        assert_eq!(alt_label(None), "off");
        assert_eq!(alt_config(None), "none");
        assert_eq!(alt_config(Some(CtrlAltS)), "Ctrl+Alt+S");
        assert_eq!(SnapHotkey::parse(alt_config(None)), None);
    }

    fn mid(r: layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn controls_are_hit_where_they_are_drawn() {
        let cases = [
            (layout::ST_BACK, Some(SettingsHit::Back)),
            (
                layout::ST_TOGGLE,
                Some(SettingsHit::Action(SettingsAction::ToggleAutostart)),
            ),
            (
                layout::ST_HOTKEY,
                Some(SettingsHit::Action(SettingsAction::CycleHotkey)),
            ),
            (
                layout::ST_ALT,
                Some(SettingsHit::Action(SettingsAction::CycleAltHotkey)),
            ),
        ];
        for (r, want) in cases {
            let (x, y) = mid(r);
            assert_eq!(hit(x, y), want, "{r:?}");
        }
        assert_eq!(hit(2.0, 100.0), None);
        assert_eq!(
            hit(layout::SETTINGS_W - 1.0, layout::SETTINGS_H - 1.0),
            None
        );
    }

    #[test]
    fn status_shows_only_when_a_key_is_taken() {
        let mut v = SettingsView::default();
        assert_eq!(v.status(), None);
        v.alt_taken = true;
        assert_eq!(v.status(), Some("taken by another app"));
    }

    fn snap(main: &str, alt: &str) -> SnapConfig {
        SnapConfig {
            hotkey: main.into(),
            alt_hotkey: alt.into(),
            ..SnapConfig::default()
        }
    }

    #[test]
    fn a_main_chip_click_moves_the_alt_off_the_same_key() {
        let mut c = snap("PrtSc", "Ctrl+Space");
        cycle_hotkey(&mut c);
        assert_eq!(c.hotkey(), CtrlSpace);
        assert_eq!(c.alt_hotkey(), Some(CtrlAltS));
        assert_eq!(c.alt_hotkey, "Ctrl+Alt+S");
        // An alt that is off stays off.
        let mut c = snap("PrtSc", "none");
        cycle_hotkey(&mut c);
        assert_eq!((c.hotkey(), c.alt_hotkey()), (CtrlSpace, None));
        assert_eq!(c.alt_hotkey, "none");
    }

    #[test]
    fn an_alt_chip_click_never_lands_on_the_main_key() {
        let mut c = snap("Ctrl+Alt+S", "Ctrl+Space");
        cycle_alt_hotkey(&mut c);
        assert_eq!(c.alt_hotkey(), Some(CtrlShiftS));
        cycle_alt_hotkey(&mut c);
        assert_eq!(c.alt_hotkey(), None);
        cycle_alt_hotkey(&mut c);
        assert_eq!(c.alt_hotkey(), Some(CtrlSpace));
    }
}
