use serde::{Deserialize, Serialize};

use crate::source_utils::{is_pdf_delimiter, is_pdf_whitespace, skip_whitespace_and_comments};
use crate::{
    DictionaryEntryByteRange, DictionaryEntrySpan, DictionaryValueKind, DocumentAccessBackend,
    DocumentAccessError, IndirectObjectDictionaryInspection, IndirectRef, ObjectLookup,
    ObjectResolutionError, PageTreeKidTargetInspection, PageTreeKidTargetsInspection,
    PageTreeKidTargetsInspectionError, PageTreeLeavesTruncation, PageTreeNodeType,
    ResolvedObjectData, ResolvedObjectPosition, inspect_document_access, inspect_object_dictionary,
    inspect_page_tree_kid_targets_resolved, resolve_object,
};

const MEDIA_BOX_KEY: &[u8] = b"/MediaBox";
const CROP_BOX_KEY: &[u8] = b"/CropBox";

/// Page-box inspection for all document leaf pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentPageBoxesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Successfully resolved leaf page boxes, in document order.
    pub pages: Vec<PageBoxesInspection>,
    /// Structured per-page or per-box skips.
    pub skipped: Vec<SkippedPageBox>,
}

/// Effective page boxes for one leaf `/Page`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageBoxesInspection {
    /// Zero-based document page index.
    pub page_index: usize,
    /// Indirect reference of the leaf `/Page`.
    pub leaf_reference: IndirectRef,
    /// Resolved leaf position.
    pub leaf_position: ResolvedObjectPosition,
    /// Effective `/MediaBox`.
    pub media_box: ResolvedPageBox,
    /// Effective `/CropBox`.
    pub crop_box: ResolvedPageBox,
}

/// Page boundary kind resolved by this inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageBoxKind {
    /// The required `/MediaBox` page boundary.
    MediaBox,
    /// The optional `/CropBox` page boundary.
    CropBox,
}

/// A rectangle expressed as four PDF numeric literals.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageRectangle {
    /// Lower-left x coordinate.
    pub llx: f64,
    /// Lower-left y coordinate.
    pub lly: f64,
    /// Upper-right x coordinate.
    pub urx: f64,
    /// Upper-right y coordinate.
    pub ury: f64,
}

/// One effective page box and its provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPageBox {
    /// Resolved box kind.
    pub kind: PageBoxKind,
    /// Effective rectangle value.
    pub effective: PageRectangle,
    /// Provenance for the effective value.
    pub source: PageBoxSource,
}

/// Provenance of an effective page box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PageBoxSource {
    /// The leaf page dictionary directly supplied the box.
    Direct {
        /// Leaf page object that supplied the direct value.
        target: IndirectRef,
        /// Byte range of the matching key.
        key_range: DictionaryEntryByteRange,
        /// Byte range of the matching value.
        value_range: DictionaryEntryByteRange,
    },
    /// An ancestor `/Pages` dictionary supplied the inherited value.
    Inherited {
        /// Ancestor page-tree node that supplied the value.
        ancestor: IndirectRef,
        /// Byte range of the matching key.
        key_range: DictionaryEntryByteRange,
        /// Byte range of the matching value.
        value_range: DictionaryEntryByteRange,
    },
    /// `/CropBox` was absent and defaulted to the effective `/MediaBox`.
    DefaultedToMediaBox,
}

/// One page-box skip diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageBox {
    /// Zero-based document page index when a leaf page was known.
    pub page_index: Option<usize>,
    /// Leaf page reference when a leaf page was known.
    pub leaf_reference: Option<IndirectRef>,
    /// Leaf page position when a leaf page was known.
    pub leaf_position: Option<ResolvedObjectPosition>,
    /// Box kind when the skip concerns one box.
    pub kind: Option<PageBoxKind>,
    /// Structured skip reason.
    pub reason: SkippedPageBoxReason,
}

/// Structured reason a page or page box was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedPageBoxReason {
    /// The leaf page dictionary lives in an object stream and has no source
    /// byte offsets suitable for future editing.
    CompressedLeafDictionary {
        /// Object stream containing the compressed page object.
        object_stream_number: usize,
        /// Member index within the object stream.
        index_within_object_stream: usize,
    },
    /// A page-tree child could not be resolved or expanded.
    NodeExpansionFailed {
        /// Page-tree kid reference.
        kid: IndirectRef,
        /// Delegated expansion failure.
        error: Box<PageTreeKidTargetsInspectionError>,
    },
    /// Resolving a page-tree child object failed.
    ObjectResolution {
        /// Reference that failed to resolve.
        reference: IndirectRef,
        /// Delegated object resolution failure.
        error: Box<ObjectResolutionError>,
    },
    /// Page-tree traversal stopped at a bounded recursion/cycle guard.
    TraversalTruncated {
        /// Reference whose descent was refused.
        kid: IndirectRef,
        /// Bound that stopped descent.
        truncation: PageTreeLeavesTruncation,
    },
    /// Duplicate matching keys were present in the relevant dictionary.
    DuplicateKey {
        /// First matching key range.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching key range.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The box key was absent where a required effective value was needed.
    MissingEffectiveMediaBox,
    /// The value was not a direct array.
    UnsupportedValueKind {
        /// Shallow value kind reported by dictionary inspection.
        value_kind: DictionaryValueKind,
    },
    /// The rectangle array was malformed or unsupported.
    MalformedRectangle,
}

/// Error returned when document-level page-box inspection cannot start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBoxInspectionError {
    /// Total source length.
    pub byte_len: usize,
    /// Delegated document-access failure.
    pub error: Box<DocumentAccessError>,
}

/// Inspect effective `/MediaBox` and `/CropBox` values for document leaf pages.
///
/// The inspector walks the page tree root-down, carrying inherited `/MediaBox`
/// and `/CropBox` values as compact metadata. A leaf direct value overrides the
/// inherited value; absent `/CropBox` defaults to the effective `/MediaBox`.
/// Only direct arrays of four finite numeric literals are accepted. Unsupported
/// shapes and compressed leaf dictionaries are reported as structured skips.
///
/// # Errors
///
/// Returns [`PageBoxInspectionError`] when the shared document-access spine
/// cannot identify the catalog, page-tree root, and cross-reference backend.
pub fn inspect_document_page_boxes(
    input: &[u8],
) -> Result<DocumentPageBoxesInspection, PageBoxInspectionError> {
    let access = inspect_document_access(input).map_err(|error| PageBoxInspectionError {
        byte_len: input.len(),
        error: Box::new(error),
    })?;

    let lookup = lookup_from_backend(&access.backend);
    let root = resolve_object(
        input,
        lookup,
        access.page_tree_root.reference,
        crate::MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    )
    .map_err(|error| PageBoxInspectionError {
        byte_len: input.len(),
        error: Box::new(DocumentAccessError {
            byte_len: input.len(),
            reason: crate::DocumentAccessRejection::PagesObject { error },
        }),
    })?;

    let mut walk = PageBoxWalk::new(input, lookup);
    let inherited = InheritedBoxes::default();
    walk.process_pages_node(&root, &inherited, 0);

    Ok(DocumentPageBoxesInspection {
        byte_len: input.len(),
        pages: walk.pages,
        skipped: walk.skipped,
    })
}

const fn lookup_from_backend(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}

#[derive(Clone, Default)]
struct InheritedBoxes {
    media_box: Option<ResolvedPageBox>,
    crop_box: Option<ResolvedPageBox>,
}

struct PageBoxWalk<'a> {
    input: &'a [u8],
    lookup: ObjectLookup<'a>,
    pages: Vec<PageBoxesInspection>,
    skipped: Vec<SkippedPageBox>,
    visited: std::collections::BTreeSet<u32>,
    page_index: usize,
}

impl<'a> PageBoxWalk<'a> {
    const fn new(input: &'a [u8], lookup: ObjectLookup<'a>) -> Self {
        Self {
            input,
            lookup,
            pages: Vec::new(),
            skipped: Vec::new(),
            visited: std::collections::BTreeSet::new(),
            page_index: 0,
        }
    }

    fn process_pages_node(
        &mut self,
        resolved: &ResolvedObjectData,
        inherited: &InheritedBoxes,
        depth: usize,
    ) {
        let reference = resolved_reference(resolved);
        if !self.visited.insert(reference.object_number) {
            self.push_skip(
                None,
                None,
                None,
                None,
                SkippedPageBoxReason::TraversalTruncated {
                    kid: reference,
                    truncation: PageTreeLeavesTruncation::Cycle {
                        object_number: reference.object_number,
                    },
                },
            );
            return;
        }
        if depth > crate::MAX_PAGE_TREE_DEPTH {
            self.push_skip(
                None,
                None,
                None,
                None,
                SkippedPageBoxReason::TraversalTruncated {
                    kid: reference,
                    truncation: PageTreeLeavesTruncation::MaxDepth {
                        max_depth: crate::MAX_PAGE_TREE_DEPTH,
                    },
                },
            );
            return;
        }

        let mut node_inherited = inherited.clone();
        if let Some(dictionary) = uncompressed_dictionary(self.input, resolved) {
            self.apply_inherited_from_dictionary(&dictionary, &mut node_inherited);
        }

        let targets = match inspect_page_tree_kid_targets_resolved(
            self.input,
            self.lookup,
            resolved,
            crate::MAX_XREF_STREAM_SECTION_DECODED_BYTES,
        ) {
            Ok(targets) => targets,
            Err(error) => {
                self.push_skip(
                    None,
                    None,
                    None,
                    None,
                    SkippedPageBoxReason::NodeExpansionFailed {
                        kid: reference,
                        error: Box::new(error),
                    },
                );
                return;
            }
        };
        self.process_targets(&targets, &node_inherited, depth);
    }

    fn process_targets(
        &mut self,
        targets: &PageTreeKidTargetsInspection,
        inherited: &InheritedBoxes,
        depth: usize,
    ) {
        for entry in &targets.entries {
            let PageTreeKidTargetInspection::Resolved { kid, target } = entry else {
                continue;
            };
            match target.node_type.node_type {
                PageTreeNodeType::Page => {
                    match resolve_object(
                        self.input,
                        self.lookup,
                        kid.reference,
                        crate::MAX_XREF_STREAM_SECTION_DECODED_BYTES,
                    ) {
                        Ok(resolved) => self.process_leaf(&resolved, inherited),
                        Err(error) => self.push_skip(
                            Some(self.page_index),
                            Some(kid.reference),
                            Some(target.position),
                            None,
                            SkippedPageBoxReason::ObjectResolution {
                                reference: kid.reference,
                                error: Box::new(error),
                            },
                        ),
                    }
                }
                PageTreeNodeType::Pages => {
                    match resolve_object(
                        self.input,
                        self.lookup,
                        kid.reference,
                        crate::MAX_XREF_STREAM_SECTION_DECODED_BYTES,
                    ) {
                        Ok(resolved) => self.process_pages_node(&resolved, inherited, depth + 1),
                        Err(error) => self.push_skip(
                            None,
                            Some(kid.reference),
                            Some(target.position),
                            None,
                            SkippedPageBoxReason::ObjectResolution {
                                reference: kid.reference,
                                error: Box::new(error),
                            },
                        ),
                    }
                }
                PageTreeNodeType::Other => {}
            }
        }
    }

    fn process_leaf(&mut self, resolved: &ResolvedObjectData, inherited: &InheritedBoxes) {
        let page_index = self.page_index;
        self.page_index += 1;
        let reference = resolved_reference(resolved);
        let position = resolved_position(resolved);

        if let ResolvedObjectPosition::Compressed {
            object_stream_number,
            index_within_object_stream,
        } = position
        {
            self.push_skip(
                Some(page_index),
                Some(reference),
                Some(position),
                None,
                SkippedPageBoxReason::CompressedLeafDictionary {
                    object_stream_number,
                    index_within_object_stream,
                },
            );
            return;
        }

        let Some(dictionary) = uncompressed_dictionary(self.input, resolved) else {
            return;
        };

        let media = match resolve_box_from_dictionary(
            self.input,
            &dictionary,
            reference,
            PageBoxKind::MediaBox,
            inherited.media_box.clone(),
        ) {
            BoxResolution::Resolved(media) => media,
            BoxResolution::Absent => {
                self.push_skip(
                    Some(page_index),
                    Some(reference),
                    Some(position),
                    Some(PageBoxKind::MediaBox),
                    SkippedPageBoxReason::MissingEffectiveMediaBox,
                );
                return;
            }
            BoxResolution::Skipped(reason) => {
                self.push_skip(
                    Some(page_index),
                    Some(reference),
                    Some(position),
                    Some(PageBoxKind::MediaBox),
                    reason,
                );
                return;
            }
        };

        let crop = match resolve_box_from_dictionary(
            self.input,
            &dictionary,
            reference,
            PageBoxKind::CropBox,
            inherited.crop_box.clone(),
        ) {
            BoxResolution::Resolved(crop) => crop,
            BoxResolution::Absent => ResolvedPageBox {
                kind: PageBoxKind::CropBox,
                effective: media.effective,
                source: PageBoxSource::DefaultedToMediaBox,
            },
            BoxResolution::Skipped(reason) => {
                self.push_skip(
                    Some(page_index),
                    Some(reference),
                    Some(position),
                    Some(PageBoxKind::CropBox),
                    reason,
                );
                return;
            }
        };

        self.pages.push(PageBoxesInspection {
            page_index,
            leaf_reference: reference,
            leaf_position: position,
            media_box: media,
            crop_box: crop,
        });
    }

    fn apply_inherited_from_dictionary(
        &self,
        dictionary: &IndirectObjectDictionaryInspection,
        inherited: &mut InheritedBoxes,
    ) {
        if let BoxResolution::Resolved(media) = resolve_box_from_dictionary(
            self.input,
            dictionary,
            dictionary.reference,
            PageBoxKind::MediaBox,
            None,
        ) {
            inherited.media_box = Some(ResolvedPageBox {
                source: inherited_source(media.source, dictionary.reference),
                ..media
            });
        }
        if let BoxResolution::Resolved(crop) = resolve_box_from_dictionary(
            self.input,
            dictionary,
            dictionary.reference,
            PageBoxKind::CropBox,
            None,
        ) {
            inherited.crop_box = Some(ResolvedPageBox {
                source: inherited_source(crop.source, dictionary.reference),
                ..crop
            });
        }
    }

    fn push_skip(
        &mut self,
        page_index: Option<usize>,
        leaf_reference: Option<IndirectRef>,
        leaf_position: Option<ResolvedObjectPosition>,
        kind: Option<PageBoxKind>,
        reason: SkippedPageBoxReason,
    ) {
        self.skipped.push(SkippedPageBox {
            page_index,
            leaf_reference,
            leaf_position,
            kind,
            reason,
        });
    }
}

const fn inherited_source(source: PageBoxSource, ancestor: IndirectRef) -> PageBoxSource {
    match source {
        PageBoxSource::Direct {
            key_range,
            value_range,
            ..
        } => PageBoxSource::Inherited {
            ancestor,
            key_range,
            value_range,
        },
        other => other,
    }
}

fn uncompressed_dictionary(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Option<IndirectObjectDictionaryInspection> {
    let ResolvedObjectData::Uncompressed { .. } = resolved else {
        return None;
    };
    match inspect_object_dictionary(input, resolved).ok()? {
        crate::ResolvedObjectDictionaryInspection::Uncompressed(dictionary) => Some(dictionary),
        crate::ResolvedObjectDictionaryInspection::Compressed(_) => None,
    }
}

enum BoxResolution {
    Resolved(ResolvedPageBox),
    Absent,
    Skipped(SkippedPageBoxReason),
}

fn resolve_box_from_dictionary(
    input: &[u8],
    dictionary: &IndirectObjectDictionaryInspection,
    target: IndirectRef,
    kind: PageBoxKind,
    inherited: Option<ResolvedPageBox>,
) -> BoxResolution {
    let key = match kind {
        PageBoxKind::MediaBox => MEDIA_BOX_KEY,
        PageBoxKind::CropBox => CROP_BOX_KEY,
    };
    let entry = match find_unique_entry(input, &dictionary.entries, key) {
        Ok(Some(entry)) => entry,
        Ok(None) => return inherited.map_or(BoxResolution::Absent, BoxResolution::Resolved),
        Err((first_key_range, duplicate_key_range)) => {
            return BoxResolution::Skipped(SkippedPageBoxReason::DuplicateKey {
                first_key_range,
                duplicate_key_range,
            });
        }
    };
    if entry.value_kind != DictionaryValueKind::Array {
        return BoxResolution::Skipped(SkippedPageBoxReason::UnsupportedValueKind {
            value_kind: entry.value_kind,
        });
    }
    let Some(rectangle) = parse_rectangle_array(input, entry.value_range) else {
        return BoxResolution::Skipped(SkippedPageBoxReason::MalformedRectangle);
    };
    BoxResolution::Resolved(ResolvedPageBox {
        kind,
        effective: rectangle,
        source: PageBoxSource::Direct {
            target,
            key_range: entry.key_range,
            value_range: entry.value_range,
        },
    })
}

fn find_unique_entry(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
    key: &[u8],
) -> Result<Option<DictionaryEntrySpan>, (DictionaryEntryByteRange, DictionaryEntryByteRange)> {
    let mut found: Option<DictionaryEntrySpan> = None;
    for &entry in entries {
        if input.get(entry.key_range.start..entry.key_range.end) == Some(key) {
            if let Some(first) = found {
                return Err((first.key_range, entry.key_range));
            }
            found = Some(entry);
        }
    }
    Ok(found)
}

fn parse_rectangle_array(input: &[u8], range: DictionaryEntryByteRange) -> Option<PageRectangle> {
    if input.get(range.start) != Some(&b'[') || input.get(range.end.checked_sub(1)?) != Some(&b']')
    {
        return None;
    }
    let mut cursor = range.start + 1;
    let limit = range.end - 1;
    let mut values = [0.0; 4];
    for value in &mut values {
        cursor = skip_whitespace_and_comments(input, cursor, limit);
        let (number, after) = parse_number(input, cursor, limit)?;
        *value = number;
        cursor = after;
    }
    cursor = skip_whitespace_and_comments(input, cursor, limit);
    if cursor != limit {
        return None;
    }
    Some(PageRectangle {
        llx: values[0],
        lly: values[1],
        urx: values[2],
        ury: values[3],
    })
}

fn parse_number(input: &[u8], start: usize, limit: usize) -> Option<(f64, usize)> {
    if start >= limit {
        return None;
    }
    let mut end = start;
    while end < limit && !is_pdf_whitespace(input[end]) && !is_pdf_delimiter(input[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    let text = std::str::from_utf8(input.get(start..end)?).ok()?;
    let value = text.parse::<f64>().ok()?;
    value.is_finite().then_some((value, end))
}

const fn resolved_reference(resolved: &ResolvedObjectData) -> IndirectRef {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => resolved.reference,
        ResolvedObjectData::Compressed { reference, .. } => *reference,
    }
}

const fn resolved_position(resolved: &ResolvedObjectData) -> ResolvedObjectPosition {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => ResolvedObjectPosition::Uncompressed {
            object_byte_offset: resolved.object_byte_offset,
            xref_generation: resolved.xref_generation,
        },
        ResolvedObjectData::Compressed {
            object_stream_number,
            index_within_object_stream,
            ..
        } => ResolvedObjectPosition::Compressed {
            object_stream_number: *object_stream_number,
            index_within_object_stream: *index_within_object_stream,
        },
    }
}
