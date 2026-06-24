//! Structural PDF access interfaces.
//!
//! This crate will own document opening, object lookup, stream access, and
//! deterministic write seams. The initial scaffold keeps only public data
//! contracts so higher-level crates can depend on a stable boundary.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// PDF indirect reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IndirectRef {
    /// Object number.
    pub object_number: u32,
    /// Generation number.
    pub generation: u16,
}

/// Document identity returned by an opener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentInfo {
    /// Number of pages.
    pub page_count: usize,
    /// PDF header version when known.
    pub pdf_version: Option<(u8, u8)>,
}
