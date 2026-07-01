use serde::{Deserialize, Serialize};

use crate::{DictionaryEntrySpan, ImageXObjectMetadata, IndirectRef, PdfName};

/// Classified page-scope `XObject` resource target metadata.
///
/// This report record is intentionally small: it owns the resource name, the
/// resolved indirect reference, the resolved target object byte offset, and,
/// for image targets, the structural [`ImageXObjectMetadata`] read from the
/// resolved image dictionary. It does not retain PDF bytes, object bodies,
/// dictionaries, stream bodies, or decoded stream data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageXObjectResourceTarget {
    /// Resource name without the leading slash.
    pub name: PdfName,
    /// Indirect reference stored in the page-scope `/XObject` resource entry.
    pub reference: IndirectRef,
    /// Resolved target object byte offset.
    pub object_byte_offset: usize,
    /// Structural image dictionary metadata for `/Subtype /Image` targets;
    /// `None` for `/Subtype /Form` targets.
    pub image_metadata: Option<ImageXObjectMetadata>,
}

pub enum PageXObjectResourceSubtype {
    Image,
    Form,
}

impl PageXObjectResourceSubtype {
    pub fn image_metadata(
        &self,
        input: &[u8],
        entries: &[DictionaryEntrySpan],
    ) -> Option<ImageXObjectMetadata> {
        match self {
            Self::Image => Some(crate::inspect_image_xobject_metadata(input, entries)),
            Self::Form => None,
        }
    }
}

pub struct ClassifiedPageXObjectResource {
    pub subtype: PageXObjectResourceSubtype,
    pub reference: IndirectRef,
    pub object_byte_offset: usize,
    pub image_metadata: Option<ImageXObjectMetadata>,
}
