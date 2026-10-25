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

pub const FEATURES: [Feature; 13] = [
    locked("media", "Media", Icon::Media),
    locked("osd", "OSD", Icon::Osd),
    locked("vitals", "Vitals", Icon::Vitals),
    locked("timer", "Timer", Icon::Timer),
    locked("snap", "Snap", Icon::Snap),
    locked("clipboard", "Clipboard", Icon::Clipboard),
    locked("notifications", "Notifications", Icon::Notifications),
    locked("lyrics", "Lyrics", Icon::Lyrics),
    locked("cli", "dj CLI", Icon::Cli),
    locked("tile", "Tile & Scenes", Icon::Tile),
    locked("sweep", "Sweep", Icon::Sweep),
    locked("shelf", "Shelf", Icon::Shelf),
    locked("ambient", "Ambient", Icon::Ambient),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_unique_and_all_locked_in_stage_1() {
        let ids: HashSet<_> = FEATURES.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), FEATURES.len());
        assert!(FEATURES
            .iter()
            .all(|f| f.availability == Availability::Locked));
    }
}
