//! Health score: 100 minus penalties, mapped to a band, with reason tags.
//! Pure function of a `Reading`. The weights are ours (see `task/progress.md`, Stage 4).

use crate::vitals::Reading;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// 85+
    Daijobu,
    /// 65+
    Ok,
    /// 45+
    Hmm,
    /// below 45
    Dame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    HighCpu,
    HighRam,
    LowDisk,
    RestartSoon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    /// 0..=100.
    pub score: u8,
    pub band: Band,
    /// Reasons with a penalty above zero, largest penalty first.
    pub reasons: Vec<Reason>,
}

/// Scores a reading. Penalties are linear between the breakpoints in the contract.
pub fn health(_r: &Reading) -> Health {
    todo!("Stage 4 health model: see the health subagent brief")
}
