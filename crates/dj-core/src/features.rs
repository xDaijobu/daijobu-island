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
    available("timer", "Timer", Icon::Timer),
    available("snap", "Snap", Icon::Snap),
    available("clipboard", "Clipboard", Icon::Clipboard),
    locked("notifications", "Notifications", Icon::Notifications),
    locked("lyrics", "Lyrics", Icon::Lyrics),
    // Stage T: a shell in the island. This was the locked "dj CLI" tile; it keeps the icon.
    available("terminal", "Terminal", Icon::Cli),
    // Stage 12a: window zones. Scenes (12b) and the Boot Scene (12c) come later under the
    // same tile.
    available("tile", "Tile & Scenes", Icon::Tile),
    locked("shelf", "Shelf", Icon::Shelf),
    locked("ambient", "Ambient", Icon::Ambient),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_unique_and_stage_eight_features_are_available() {
        let ids: HashSet<_> = FEATURES.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), FEATURES.len());
        assert_eq!(FEATURES.len(), 12);
        let open: Vec<_> = FEATURES
            .iter()
            .filter(|f| f.availability == Availability::Available)
            .map(|f| f.id)
            .collect();
        assert_eq!(
            open,
            [
                "music",
                "osd",
                "vitals",
                "timer",
                "snap",
                "clipboard",
                "terminal",
                "tile"
            ]
        );
        // Eight of twelve are ready; the Home footer counts them.
        assert_eq!(open.len(), 8);
        assert_eq!(FEATURES[0].name, "Music");
        let tile = FEATURES.iter().find(|f| f.id == "tile").unwrap();
        assert_eq!((tile.name, tile.icon), ("Tile & Scenes", Icon::Tile));
    }

    #[test]
    fn the_terminal_tile_replaces_the_locked_cli_tile_in_place() {
        // The old "dj CLI" tile is gone (Stage 11 was dropped), and Terminal sits in its slot.
        assert!(FEATURES.iter().all(|f| f.id != "cli" && f.name != "dj CLI"));
        let i = FEATURES.iter().position(|f| f.id == "terminal").unwrap();
        assert_eq!(i, 8);
        let f = FEATURES[i];
        assert_eq!((f.name, f.icon), ("Terminal", Icon::Cli));
        assert_eq!(f.availability, Availability::Available);
    }

    #[test]
    fn sweep_is_not_a_tile() {
        // Sweep moved into the Vitals panel (Stage 5).
        assert!(FEATURES
            .iter()
            .all(|f| f.id != "sweep" && f.icon != Icon::Sweep));
    }
}
