//! Feature registry. Unlocking a feature later is one `Availability::Available`
//! change here plus its view.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Locked,
    Available,
}

/// Vector icon drawn by the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Media,
    Osd,
    Vitals,
    Timer,
    Snap,
    Clipboard,
    Notifications,
    Lyrics,
    Cli,
    Tile,
    /// Not a tile: Sweep lives inside the Vitals panel. The panel's button draws it.
    Sweep,
    Shelf,
    Ambient,
}

#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: Icon,
    pub availability: Availability,
}

const fn locked(id: &'static str, name: &'static str, icon: Icon) -> Feature {
    Feature {
        id,
        name,
        icon,
        availability: Availability::Locked,
    }
}

const fn available(id: &'static str, name: &'static str, icon: Icon) -> Feature {
    Feature {
        id,
        name,
        icon,
        availability: Availability::Available,
    }
}

pub const FEATURES: [Feature; 12] = [
    available("music", "Music", Icon::Media),
    available("osd", "OSD", Icon::Osd),
    available("vitals", "Vitals", Icon::Vitals),
    locked("timer", "Timer", Icon::Timer),
    locked("snap", "Snap", Icon::Snap),
    locked("clipboard", "Clipboard", Icon::Clipboard),
    locked("notifications", "Notifications", Icon::Notifications),
    locked("lyrics", "Lyrics", Icon::Lyrics),
    locked("cli", "dj CLI", Icon::Cli),
    locked("tile", "Tile & Scenes", Icon::Tile),
    locked("shelf", "Shelf", Icon::Shelf),
    locked("ambient", "Ambient", Icon::Ambient),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_unique_and_only_music_osd_and_vitals_are_available() {
        let ids: HashSet<_> = FEATURES.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), FEATURES.len());
        assert_eq!(FEATURES.len(), 12);
        let open: Vec<_> = FEATURES
            .iter()
            .filter(|f| f.availability == Availability::Available)
            .map(|f| f.id)
            .collect();
        assert_eq!(open, ["music", "osd", "vitals"]);
        assert_eq!(FEATURES[0].name, "Music");
    }

    #[test]
    fn sweep_is_not_a_tile() {
        // Sweep moved into the Vitals panel (Stage 5).
        assert!(FEATURES
            .iter()
            .all(|f| f.id != "sweep" && f.icon != Icon::Sweep));
    }
}
