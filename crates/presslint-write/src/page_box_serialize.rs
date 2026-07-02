//! Leaf page-box entry serialization and dictionary body-edit application.
//!
//! Split out of `page_boxes.rs` to keep the semantic page-box writer focused on
//! planning and leave the low-level byte machinery — minimal `/MediaBox` /
//! `/CropBox` literal serialization, the per-box replace/insert decision, its
//! matching [`DictionaryValueLocator`], and the descending-offset body splice —
//! here. The write op and the returned locator are derived from the same source
//! decision, so the planned boundary and the byte edit always agree.

use presslint_actions::DictionaryValueLocator;
use presslint_pdf::{IndirectRef, PageBoxKind, PageBoxSource, PageRectangle};
use presslint_types::ByteRange;

use crate::{AppliedBox, DictionaryEntryWrite};

/// One byte-range edit against the leaf dictionary body, in source coordinates.
pub struct BodyEdit {
    /// Inclusive start offset (source coordinates).
    start: usize,
    /// Exclusive end offset (equal to `start` for an insert).
    end: usize,
    /// Replacement bytes.
    bytes: Vec<u8>,
    /// Tie-break rank for equal-`start` inserts (lower is spliced first, so it
    /// ends up rightmost); `MediaBox` sorts leftmost in the output.
    tie: u8,
}

/// Push one box edit (replace a direct leaf entry, or insert after `<<`) and
/// return the applied-box report plus the source value locator used for the
/// matching mutation boundary.
pub fn push_box_edit(
    edits: &mut Vec<BodyEdit>,
    kind: PageBoxKind,
    rectangle: PageRectangle,
    source: &PageBoxSource,
    leaf: IndirectRef,
    dictionary_range: ByteRange,
) -> (AppliedBox, DictionaryValueLocator) {
    // Insertion point is immediately after the dictionary's opening `<<`.
    let insert_at = dictionary_range.start + 2;
    let tie = match kind {
        PageBoxKind::MediaBox => 1,
        PageBoxKind::CropBox => 0,
    };
    let (op, locator) = match source {
        PageBoxSource::Direct {
            target,
            key_range,
            value_range,
        } if *target == leaf => {
            edits.push(BodyEdit {
                start: key_range.start,
                end: value_range.end,
                bytes: serialize_entry(kind, rectangle, false),
                tie,
            });
            (
                DictionaryEntryWrite::Replace,
                DictionaryValueLocator::ExistingValue {
                    key_range: ByteRange {
                        start: key_range.start,
                        end: key_range.end,
                    },
                    value_range: ByteRange {
                        start: value_range.start,
                        end: value_range.end,
                    },
                },
            )
        }
        // Inherited, defaulted, or an unexpected foreign-target direct value all
        // become an explicit insert on this leaf; ancestors stay untouched.
        _ => {
            edits.push(BodyEdit {
                start: insert_at,
                end: insert_at,
                bytes: serialize_entry(kind, rectangle, true),
                tie,
            });
            (
                DictionaryEntryWrite::Insert,
                DictionaryValueLocator::InsertionPoint { dictionary_range },
            )
        }
    };
    (
        AppliedBox {
            kind,
            rectangle,
            op,
        },
        locator,
    )
}

/// Apply body edits against a copy of the original leaf dictionary body.
///
/// Edits are applied in descending start-offset order so earlier offsets stay
/// valid; equal-start inserts are ordered by `tie` so `/MediaBox` precedes
/// `/CropBox`. `dict_open` is the source offset the `body` copy starts at.
pub fn apply_body_edits(body: &mut Vec<u8>, mut edits: Vec<BodyEdit>, dict_open: usize) {
    edits.sort_by(|a, b| b.start.cmp(&a.start).then(a.tie.cmp(&b.tie)));
    for change in &edits {
        let start = change.start - dict_open;
        let end = change.end - dict_open;
        body.splice(start..end, change.bytes.iter().copied());
    }
}

/// Serialize `/MediaBox [llx lly urx ury]` minimally. `leading_space` prefixes a
/// single space for inserts after `<<`.
fn serialize_entry(kind: PageBoxKind, rectangle: PageRectangle, leading_space: bool) -> Vec<u8> {
    let key: &str = match kind {
        PageBoxKind::MediaBox => "/MediaBox",
        PageBoxKind::CropBox => "/CropBox",
    };
    let prefix = if leading_space { " " } else { "" };
    format!(
        "{prefix}{key} [{} {} {} {}]",
        format_number(rectangle.llx),
        format_number(rectangle.lly),
        format_number(rectangle.urx),
        format_number(rectangle.ury),
    )
    .into_bytes()
}

/// Format a finite `f64` as a minimal PDF decimal literal: no exponent, no
/// trailing zeros, and `0` for negative zero.
fn format_number(value: f64) -> String {
    if value == 0.0 {
        // Covers both `0.0` and `-0.0`.
        return "0".to_owned();
    }
    // Rust's `f64` `Display` is the shortest round-trip decimal with no exponent
    // and no trailing zeros, so it already matches the required literal shape.
    format!("{value}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use presslint_pdf::{PageBoxKind, PageRectangle};

    use super::{format_number, serialize_entry};

    #[test]
    fn minimal_number_formatting() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(612.0), "612");
        assert_eq!(format_number(1.5), "1.5");
        assert_eq!(format_number(-5.0), "-5");
        // No exponent even for magnitudes that `{:e}` would abbreviate.
        assert_eq!(format_number(1_000_000.0), "1000000");
    }

    #[test]
    fn serialize_entry_shapes() {
        let rect = PageRectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 612.0,
            ury: 792.0,
        };
        assert_eq!(
            serialize_entry(PageBoxKind::MediaBox, rect, false),
            b"/MediaBox [0 0 612 792]"
        );
        assert_eq!(
            serialize_entry(PageBoxKind::CropBox, rect, true),
            b" /CropBox [0 0 612 792]"
        );
    }
}
