//! Output of sheet analysis — what layout produces and what eval scores.

use crate::model::Rect;

/// Role of a cell, harmonized with the DECO / Strudel / TableSense label sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Title,
    Header,
    Data,
    /// Single-label row that starts a section inside a table ("Region: North").
    GroupHeader,
    /// Total or subtotal (label or value).
    Total,
    Note,
    Other,
}

impl Role {
    pub const ALL: [Role; 7] =
        [Role::Title, Role::Header, Role::Data, Role::GroupHeader, Role::Total, Role::Note, Role::Other];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Title => "title",
            Role::Header => "header",
            Role::Data => "data",
            Role::GroupHeader => "group_header",
            Role::Total => "total",
            Role::Note => "note",
            Role::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedTable {
    /// Full table range including header and total rows.
    pub rect: Rect,
    /// Number of header rows at the top of `rect` (0 = no header found).
    pub header_rows: u32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SheetLayout {
    pub tables: Vec<DetectedTable>,
    /// One role per entry of `sheet.cells`, same order.
    pub roles: Vec<Role>,
}
