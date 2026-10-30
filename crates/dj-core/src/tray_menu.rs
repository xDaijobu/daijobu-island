#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    ToggleVisible,
    Theme(usize),
    ToggleAutostart,
    MusicShow,
    SnapPreviews,
    SnapPins,
    Settings,
    Clipboard,
    DeveloperMemory,
    Quit,
}

pub struct MenuSpec<'a> {
    pub themes: &'a [(String, bool)],
    pub autostart: bool,
    pub previews: bool,
    pub pins: bool,
}

pub const WIDTH: f32 = 300.0;
pub const ROW_HEIGHT: f32 = 32.0;
pub const MARGIN: f32 = 16.0;
pub const MAX_VISIBLE: usize = 12;

pub struct Row {
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
    pub command: Option<MenuCommand>,
    pub children: Vec<Row>,
}

impl Row {
    fn action(label: &str, command: MenuCommand, enabled: bool, checked: bool) -> Self {
        Self {
            label: label.into(),
            enabled,
            checked,
            command: Some(command),
            children: Vec::new(),
        }
    }

    fn group(label: &str, children: Vec<Row>) -> Self {
        Self {
            label: label.into(),
            enabled: !children.is_empty(),
            checked: false,
            command: None,
            children,
        }
    }
}

pub struct Menu {
    root: Vec<Row>,
    page: Option<usize>,
    pub selected: Option<usize>,
    pub offset: usize,
    pub visible_count: usize,
}

impl Menu {
    pub fn new(spec: &MenuSpec<'_>) -> Self {
        use MenuCommand::*;
        Self {
            root: vec![
                Row::action("Show / Hide", ToggleVisible, true, false),
                Row::action("Settings…", Settings, true, false),
                Row::action("Developer memory…", DeveloperMemory, true, false),
                Row::action("Clipboard", Clipboard, true, false),
                Row::group(
                    "Theme",
                    spec.themes
                        .iter()
                        .enumerate()
                        .map(|(i, (name, checked))| Row::action(name, Theme(i), true, *checked))
                        .collect(),
                ),
                Row::action("Music", MusicShow, true, false),
                Row::action("Focus Snap previews", SnapPreviews, spec.previews, false),
                Row::action("Show pinned screenshots", SnapPins, spec.pins, false),
                Row::action("Start with Windows", ToggleAutostart, true, spec.autostart),
                Row::action("Quit", Quit, true, false),
            ],
            page: None,
            selected: Some(0),
            offset: 0,
            visible_count: spec.themes.len().clamp(9, MAX_VISIBLE),
        }
    }

    pub fn rows(&self) -> &[Row] {
        self.page.map_or(&self.root, |i| &self.root[i].children)
    }

    pub fn title(&self) -> &str {
        self.page.map_or("Daijobu Island", |i| &self.root[i].label)
    }

    pub fn is_submenu(&self) -> bool {
        self.page.is_some()
    }

    pub fn height(&self) -> f32 {
        MARGIN * 2.0 + ROW_HEIGHT * (2 + self.visible_count) as f32
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !(MARGIN..WIDTH - MARGIN).contains(&x) || y < MARGIN + ROW_HEIGHT {
            return None;
        }
        let visible = ((y - MARGIN - ROW_HEIGHT) / ROW_HEIGHT) as usize;
        let i = self.offset + visible;
        (visible < self.visible_count && self.rows().get(i).is_some_and(|r| r.enabled)).then_some(i)
    }

    pub fn step(&mut self, forward: bool) {
        let n = self.rows().len();
        if n == 0 {
            return;
        }
        let mut i = self.selected.unwrap_or(if forward { n - 1 } else { 0 });
        for _ in 0..n {
            i = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            if self.rows()[i].enabled {
                self.select(i);
                return;
            }
        }
    }

    pub fn select(&mut self, i: usize) {
        if !self.rows().get(i).is_some_and(|r| r.enabled) {
            return;
        }
        self.selected = Some(i);
        self.offset = self
            .offset
            .min(i)
            .max((i + 1).saturating_sub(self.visible_count));
    }

    pub fn activate(&mut self) -> Option<MenuCommand> {
        let i = self.selected?;
        let row = self.rows().get(i)?;
        if !row.enabled {
            return None;
        }
        if let Some(command) = row.command {
            return Some(command);
        }
        if self.page.is_none() {
            self.page = Some(i);
            self.offset = 0;
            self.selected = None;
            self.step(true);
        }
        None
    }

    pub fn back(&mut self) -> bool {
        if let Some(i) = self.page.take() {
            self.offset = 0;
            self.select(i);
            true
        } else {
            false
        }
    }

    pub fn mnemonic(&mut self, ch: char) {
        let n = self.rows().len();
        let start = self.selected.map_or(0, |i| i + 1);
        for j in 0..n {
            let i = (start + j) % n;
            let r = &self.rows()[i];
            if r.enabled
                && r.label
                    .chars()
                    .next()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&ch))
            {
                self.select(i);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        let themes = (0..30)
            .map(|i| (format!("Theme {i}"), i == 4))
            .collect::<Vec<_>>();
        Menu::new(&MenuSpec {
            themes: &themes,
            autostart: true,
            previews: false,
            pins: false,
        })
    }

    #[test]
    fn navigation_skips_disabled_wraps_and_preserves_actions() {
        let mut m = menu();
        m.select(5);
        m.step(true);
        assert_eq!(m.selected, Some(8));
        assert!(m.rows()[8].checked);
        assert_eq!(m.activate(), Some(MenuCommand::ToggleAutostart));
        m.step(true);
        assert_eq!(m.activate(), Some(MenuCommand::Quit));
        m.step(true);
        assert_eq!(m.activate(), Some(MenuCommand::ToggleVisible));
        assert_eq!(m.hit(20.0, 48.0), Some(0));
        assert_eq!(m.hit(0.0, 48.0), None);
        assert_eq!(m.hit(20.0, 48.0 + 6.0 * ROW_HEIGHT), None);
    }

    #[test]
    fn submenus_scroll_and_back_retains_parent() {
        let mut m = menu();
        m.select(4);
        assert_eq!(m.activate(), None);
        assert_eq!(m.title(), "Theme");
        assert!(m.rows()[4].checked);
        m.select(29);
        assert_eq!(m.offset, 18);
        assert_eq!(m.activate(), Some(MenuCommand::Theme(29)));
        m.step(true);
        assert_eq!(m.offset, 0);
        assert!(m.back());
        assert_eq!(m.selected, Some(4));
        assert!(!m.back());
    }

    #[test]
    fn native_music_opens_panel_without_engine_actions() {
        let mut m = menu();
        m.select(5);
        assert_eq!(m.activate(), Some(MenuCommand::MusicShow));
        assert!(m.rows()[5].children.is_empty());
    }
}
