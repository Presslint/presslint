use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::page_resource_inheritance::{
    ResolveReferenceError, ResourceContext, resolve_reference, unique_entry,
};
use crate::page_xobject_resource_targets::{
    ClassifiedPageXObjectResource, PageXObjectResourceSubtype,
};
use crate::{
    DictionaryEntrySpan, DictionaryValueKind, ObjectLookup, PageXObjectResourceTarget, PdfName,
    SkippedPageXObjectResource, SkippedPageXObjectResourceReason, inspect_dictionary_entries,
    inspect_indirect_object_dictionary, parse_indirect_reference,
};

/// Classified own-scope `XObject` resources for one Form `XObject`.
///
/// This is the single-object counterpart to the page-tree
/// [`PageXObjectResourcesInspection`](crate::PageXObjectResourcesInspection): it
/// classifies exactly the `/Resources /XObject` dictionary declared on one form
/// stream object and never inherits page-scope resources into the form. The
/// report stores only structural metadata, `PdfName` lists, and small skip
/// records; it retains no PDF bytes, object bodies, resource dictionaries,
/// stream bodies, or decoded data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormXObjectResourcesInspection {
    /// Resolved form stream object byte offset the resources were read from.
    pub object_byte_offset: usize,
    /// Sorted/deduplicated targets whose resolved `XObject` subtype is `/Image`.
    pub image_xobjects: Vec<PageXObjectResourceTarget>,
    /// Sorted/deduplicated targets whose resolved `XObject` subtype is `/Form`.
    pub form_xobjects: Vec<PageXObjectResourceTarget>,
    /// Sorted/deduplicated names whose resolved `XObject` subtype is `/Image`.
    pub image_xobject_names: Vec<PdfName>,
    /// Sorted/deduplicated names whose resolved `XObject` subtype is `/Form`.
    pub form_xobject_names: Vec<PdfName>,
    /// Form-local structural resource diagnostics.
    pub skipped: Vec<SkippedPageXObjectResource>,
}

/// Classify one Form `XObject`'s own `/Resources /XObject` dictionary.
///
/// The form object at `object_byte_offset` is scanned for a direct or indirect
/// `/Resources` dictionary, then that dictionary's direct `/XObject` entries are
/// resolved and classified into `/Image` and `/Form` subtypes exactly like the
/// page-scope inspector. Page-scope resources are intentionally NOT inherited: a
/// form paints against its own `/Resources` only, so a missing form resource
/// leaves the corresponding nested `Do` name unclassified rather than borrowing
/// the invoking page's resources.
///
/// This never returns a hard error: an unreadable form dictionary, a malformed
/// or unresolved resource reference, or a non-`/Image`/`/Form` subtype all
/// become structured entries in [`FormXObjectResourcesInspection::skipped`].
#[must_use]
pub fn inspect_form_xobject_resources(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormXObjectResourcesInspection {
    let context = match inspect_indirect_object_dictionary(input, object_byte_offset) {
        Ok(dictionary) => ResourceContext::from_dictionary(input, lookup, &dictionary, None),
        Err(error) => {
            return empty_report(
                object_byte_offset,
                vec![skipped_resource(
                    object_byte_offset,
                    None,
                    SkippedPageXObjectResourceReason::PageDictionaryFailed { error },
                )],
            );
        }
    };
    inspect_effective_xobjects(input, lookup, object_byte_offset, &context)
}

fn inspect_effective_xobjects(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> FormXObjectResourcesInspection {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|reason| skipped_resource(object_byte_offset, None, reason))
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skipped_resource(
            object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::MissingResources,
        ));
        return empty_report(object_byte_offset, skipped);
    };

    let Some(xobject_entry) = (match unique_entry(input, &resources.entries, b"/XObject") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_resource(
                object_byte_offset,
                None,
                SkippedPageXObjectResourceReason::DuplicateXObject {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return empty_report(object_byte_offset, skipped);
        }
    }) else {
        skipped.push(skipped_resource(
            object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::MissingXObject,
        ));
        return empty_report(object_byte_offset, skipped);
    };

    if xobject_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_resource(
            object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::NonDictionaryXObject {
                value_kind: xobject_entry.value_kind,
            },
        ));
        return empty_report(object_byte_offset, skipped);
    }

    let xobjects = match inspect_dictionary_entries(input, xobject_entry.value_range.start) {
        Ok(xobjects) => xobjects,
        Err(error) => {
            skipped.push(skipped_resource(
                object_byte_offset,
                None,
                SkippedPageXObjectResourceReason::XObjectDictionaryFailed { error },
            ));
            return empty_report(object_byte_offset, skipped);
        }
    };

    let (images, forms) = classify_xobject_entries(
        input,
        lookup,
        object_byte_offset,
        xobjects.entries,
        &mut skipped,
    );
    report(object_byte_offset, skipped, images, forms)
}

fn classify_xobject_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedPageXObjectResource>,
) -> (
    Vec<PageXObjectResourceTarget>,
    Vec<PageXObjectResourceTarget>,
) {
    let mut images = Vec::new();
    let mut forms = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_resource(
                object_byte_offset,
                Some(name),
                SkippedPageXObjectResourceReason::DuplicateXObjectName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_xobject_entry(input, lookup, entry) {
            Ok(classification) => {
                let target = PageXObjectResourceTarget {
                    name,
                    reference: classification.reference,
                    object_byte_offset: classification.object_byte_offset,
                    image_metadata: classification.image_metadata,
                };
                match classification.subtype {
                    PageXObjectResourceSubtype::Image => images.push(target),
                    PageXObjectResourceSubtype::Form => forms.push(target),
                }
            }
            Err(reason) => skipped.push(skipped_resource(object_byte_offset, Some(name), reason)),
        }
    }
    (images, forms)
}

fn classify_xobject_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedPageXObjectResource, SkippedPageXObjectResourceReason> {
    if entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
        return Err(SkippedPageXObjectResourceReason::NonReferenceXObject {
            value_kind: entry.value_kind,
        });
    }
    let reference = parse_indirect_reference(input, entry.value_range.start).map_err(|error| {
        SkippedPageXObjectResourceReason::MalformedXObjectReference {
            reference_reason: error.reason,
        }
    })?;
    let (_, object_byte_offset) =
        resolve_reference(lookup, reference.reference).map_err(|reason| match reason {
            ResolveReferenceError::Unresolved {
                reference,
                location,
            } => SkippedPageXObjectResourceReason::UnresolvedXObjectReference {
                reference,
                location,
            },
            ResolveReferenceError::GenerationMismatch {
                reference,
                xref_generation,
            } => SkippedPageXObjectResourceReason::XObjectGenerationMismatch {
                reference,
                xref_generation,
            },
        })?;

    let target =
        inspect_indirect_object_dictionary(input, object_byte_offset).map_err(|error| {
            SkippedPageXObjectResourceReason::XObjectTargetDictionaryFailed {
                reference: reference.reference,
                object_byte_offset,
                error,
            }
        })?;
    let subtype = classify_subtype(input, object_byte_offset, &target.entries)?;
    let image_metadata = subtype.image_metadata(input, &target.entries);
    Ok(ClassifiedPageXObjectResource {
        subtype,
        reference: reference.reference,
        object_byte_offset,
        image_metadata,
    })
}

fn classify_subtype(
    input: &[u8],
    object_byte_offset: usize,
    entries: &[DictionaryEntrySpan],
) -> Result<PageXObjectResourceSubtype, SkippedPageXObjectResourceReason> {
    let Some(subtype) = unique_entry(input, entries, b"/Subtype").map_err(
        |(first_key_range, duplicate_key_range)| {
            SkippedPageXObjectResourceReason::DuplicateSubtype {
                object_byte_offset,
                first_key_range,
                duplicate_key_range,
            }
        },
    )?
    else {
        return Err(SkippedPageXObjectResourceReason::MissingSubtype { object_byte_offset });
    };

    if subtype.value_kind != DictionaryValueKind::Name {
        return Err(SkippedPageXObjectResourceReason::NonNameSubtype {
            object_byte_offset,
            value_kind: subtype.value_kind,
        });
    }

    match &input[subtype.value_range.start..subtype.value_range.end] {
        b"/Image" => Ok(PageXObjectResourceSubtype::Image),
        b"/Form" => Ok(PageXObjectResourceSubtype::Form),
        other => Err(SkippedPageXObjectResourceReason::UnknownSubtype {
            object_byte_offset,
            subtype: other.to_vec(),
        }),
    }
}

fn report(
    object_byte_offset: usize,
    skipped: Vec<SkippedPageXObjectResource>,
    mut images: Vec<PageXObjectResourceTarget>,
    mut forms: Vec<PageXObjectResourceTarget>,
) -> FormXObjectResourcesInspection {
    images.sort_by(|left, right| left.name.cmp(&right.name));
    forms.sort_by(|left, right| left.name.cmp(&right.name));
    let image_xobject_names = images.iter().map(|target| target.name.clone()).collect();
    let form_xobject_names = forms.iter().map(|target| target.name.clone()).collect();
    FormXObjectResourcesInspection {
        object_byte_offset,
        image_xobjects: images,
        form_xobjects: forms,
        image_xobject_names,
        form_xobject_names,
        skipped,
    }
}

fn empty_report(
    object_byte_offset: usize,
    skipped: Vec<SkippedPageXObjectResource>,
) -> FormXObjectResourcesInspection {
    report(object_byte_offset, skipped, Vec::new(), Vec::new())
}

const fn skipped_resource(
    object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedPageXObjectResourceReason,
) -> SkippedPageXObjectResource {
    SkippedPageXObjectResource {
        page_object_byte_offset: object_byte_offset,
        resource_name,
        reason,
    }
}
