#[derive(Clone, Debug, Default)]
pub struct Row {
    pub name: String,
    pub usage: String,
    pub description: String,
    pub pid: Option<u32>,
    pub memory: Option<u64>,
    pub private_commit: Option<u64>,
    pub peak: Option<u64>,
    pub section: bool,
}

impl Row {
    pub fn resource(
        name: impl Into<String>,
        usage: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            usage: usage.into(),
            description: description.into(),
            ..Self::default()
        }
    }
    pub fn section(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            section: true,
            ..Self::resource(name, "", description)
        }
    }
}

pub fn bytes(value: Option<u64>) -> String {
    let Some(value) = value else {
        return "Unavailable".into();
    };
    let (divisor, unit) = if value >= 1_000_000_000 {
        (1_000_000_000.0, "GB")
    } else if value >= 1_000_000 {
        (1_000_000.0, "MB")
    } else {
        (1_000.0, "KB")
    };
    format!("{:.2} {unit}", value as f64 / divisor)
}

pub const ROW_HEIGHT: f32 = 64.0;
pub const BODY_TOP: f32 = 146.0;
pub fn visible_rows(height: f32) -> usize {
    ((height - BODY_TOP - 40.0) / ROW_HEIGHT).max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_units_and_unknown_are_distinct() {
        assert_eq!(bytes(None), "Unavailable");
        assert_eq!(bytes(Some(0)), "0.00 KB");
        assert_eq!(bytes(Some(999_999)), "1000.00 KB");
        assert_eq!(bytes(Some(1_000_000)), "1.00 MB");
        assert_eq!(bytes(Some(1_500_000_000)), "1.50 GB");
        assert_eq!(visible_rows(0.0), 1);
    }
}
