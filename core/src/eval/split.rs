//! Deterministic dev/test split: 70 % dev, 30 % test by FNV-1a hash of the file id.
//! Default runs use dev; test is held out for milestone reports.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Dev,
    Test,
    All,
}

/// Percentage of files that land in dev.
pub const DEV_PERCENT: u64 = 70;

pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `Dev` or `Test` for a file id.
pub fn split_of(id: &str) -> Split {
    if fnv1a(id) % 100 < DEV_PERCENT { Split::Dev } else { Split::Test }
}

impl Split {
    pub fn contains(self, id: &str) -> bool {
        self == Split::All || split_of(id) == self
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Test => "test",
            Split::All => "all",
        }
    }
}

impl fmt::Display for Split {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Split {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "dev" => Ok(Split::Dev),
            "test" => Ok(Split::Test),
            "all" => Ok(Split::All),
            _ => Err(format!("unknown split `{s}` (dev|test|all)")),
        }
    }
}
