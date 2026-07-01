use serde::{Deserialize, Serialize};

use crate::{IndirectRef, PdfName};

/// Classified page-scope `XObject` resource target metadata.
///
/// This report record is intentionally small: it owns only the resource name,
/// the resolved indirect reference, and the resolved target object byte offset.
/// It does not retain PDF bytes, object bodies, dictionaries, stream bodies, or
/// decoded stream data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageXObjectResourceTarget {
    /// Resource name without the leading slash.
    pub name: PdfName,
    /// Indirect reference stored in the page-scope `/XObject` resource entry.
    pub reference: IndirectRef,
    /// Resolved target object byte offset.
    pub object_byte_offset: usize,
}

pub enum PageXObjectResourceSubtype {
    Image,
    Form,
}

pub struct ClassifiedPageXObjectResource {
    pub subtype: PageXObjectResourceSubtype,
    pub reference: IndirectRef,
    pub object_byte_offset: usize,
}
