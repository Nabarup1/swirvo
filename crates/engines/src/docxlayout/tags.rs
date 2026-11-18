//! The logical structure of laid-out pages, for tagged (accessible) PDF.
//!
//! Layout creates a node for every paragraph, heading, list, table, row, cell, figure and
//! note it places, and puts an [`Op::Mark`](super::layout::Op::Mark) in the page's drawing
//! operations wherever the content that follows belongs to another node. The renderer turns
//! nodes into the PDF's structure tree and the marked runs into marked content, so a
//! screen reader reads the document in order with its headings, lists and tables.
//!
//! Nodes created while layout tries something and backs out (keep-with-next, balanced
//! columns) are never referenced by a mark and leave nothing in the PDF.

/// Content that isn't part of the document's logical content: rules, shading, borders.
pub const ARTIFACT: u32 = u32::MAX;
/// Running headers and footers, page numbers in them included.
pub const HEADER: u32 = u32::MAX - 1;
pub const FOOTER: u32 = u32::MAX - 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numbering {
    None,
    Disc,
    Decimal,
    LowerRoman,
    UpperRoman,
    LowerAlpha,
    UpperAlpha,
}

impl Numbering {
    /// From a list label as shown: "1.", "iv)", "(b)", "•".
    pub fn of_label(label: &str) -> Numbering {
        let core: String = label
            .trim()
            .trim_matches(|c: char| c == '.' || c == ')' || c == '(' || c == ':')
            .to_string();
        if core.is_empty() {
            return Numbering::None;
        }
        if core.chars().all(|c| c.is_ascii_digit()) {
            return Numbering::Decimal;
        }
        let roman = |c: char| "ivxlcdm".contains(c.to_ascii_lowercase());
        if core.chars().all(roman) && core.len() > 1 || core == "i" || core == "I" {
            return if core.chars().all(|c| c.is_ascii_lowercase()) {
                Numbering::LowerRoman
            } else {
                Numbering::UpperRoman
            };
        }
        if core.chars().all(|c| c.is_ascii_lowercase()) {
            return Numbering::LowerAlpha;
        }
        if core.chars().all(|c| c.is_ascii_uppercase()) {
            return Numbering::UpperAlpha;
        }
        if core.chars().any(|c| c.is_alphanumeric()) {
            Numbering::None
        } else {
            Numbering::Disc
        }
    }
}

/// What a node is, in PDF's standard structure types.
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    P,
    /// Heading level 1–6 and its text.
    H(u8, String),
    L(Numbering),
    LI,
    LBody,
    Table,
    TR,
    TH,
    TD,
    /// A picture with its alternative text.
    Figure(String),
    /// A footnote or endnote.
    Note,
}

#[derive(Debug, Default, Clone)]
pub struct Tags {
    /// Role and parent of each node; the index is the node id.
    pub nodes: Vec<(Role, Option<u32>)>,
}

impl Tags {
    pub fn add(&mut self, role: Role, parent: Option<u32>) -> u32 {
        // Real ids never reach the reserved artifact values: that many nodes can't fit in
        // memory first.
        let id = self.nodes.len() as u32;
        self.nodes.push((role, parent.filter(|&p| p < FOOTER)));
        id
    }

    pub fn role(&self, id: u32) -> Option<&Role> {
        self.nodes.get(id as usize).map(|n| &n.0)
    }

    pub fn parent(&self, id: u32) -> Option<u32> {
        self.nodes.get(id as usize).and_then(|n| n.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_numbering_from_labels() {
        assert_eq!(Numbering::of_label("1."), Numbering::Decimal);
        assert_eq!(Numbering::of_label("(12)"), Numbering::Decimal);
        assert_eq!(Numbering::of_label("iv)"), Numbering::LowerRoman);
        assert_eq!(Numbering::of_label("I."), Numbering::UpperRoman);
        assert_eq!(Numbering::of_label("b)"), Numbering::LowerAlpha);
        assert_eq!(Numbering::of_label("C."), Numbering::UpperAlpha);
        assert_eq!(Numbering::of_label("•"), Numbering::Disc);
        assert_eq!(Numbering::of_label(""), Numbering::None);
    }
}
