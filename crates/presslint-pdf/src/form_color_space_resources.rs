use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::page_color_space_classify::classify_color_space_entry;
use crate::page_resource_inheritance::{ResourceContext, unique_entry};
use crate::{
    ClassifiedColorSpaceResource, DictionaryEntrySpan, DictionaryValueKind, ObjectLookup, PdfName,
    SkippedColorSpaceResource, SkippedColorSpaceResourceReason, SkippedPageXObjectResourceReason,
    inspect_dictionary_entries, inspect_indirect_object_dictionary,
};

/// Classified own-scope `/Resources /ColorSpace` resources for one Form
/// `XObject`.
///
/// This is the single-object counterpart to the page-tree
/// [`PageColorSpaceResourcesInspection`](crate::PageColorSpaceResourcesInspection):
/// it classifies exactly the `/ColorSpace` sub-dictionary declared on one form
/// stream object's own `/Resources`, and never inherits page-scope colour spaces
/// into the form. The report stores only the classified colour-space family
/// model and small skip records; it retains no PDF bytes, object bodies,
/// resource dictionaries, stream bodies, or decoded data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormColorSpaceResourcesInspection {
    /// Resolved form stream object byte offset the resources were read from.
    pub object_byte_offset: usize,
    /// Classified colour-space resources, sorted/deduplicated by name.
    pub color_spaces: Vec<ClassifiedColorSpaceResource>,
    /// Form-local structural colour-space diagnostics.
    pub skipped: Vec<SkippedColorSpaceResource>,
}

/// Classify one Form `XObject`'s own `/Resources /ColorSpace` dictionary.
///
/// The form object at `object_byte_offset` is scanned for a direct or indirect
/// `/Resources` dictionary, then that dictionary's `/ColorSpace` sub-dictionary
/// is classified into the structural colour-space family model. Each entry is
/// classified through the SHARED
/// [`classify_color_space_entry`](crate::page_color_space_classify) used by the
/// page-scope inspector — this never forks the classifier.
///
/// Page-scope colour spaces are intentionally NOT inherited
/// (`ResourceContext::from_dictionary(..., None)`): per ISO 32000-1 §7.8.3 and
/// §8.10.2 (Table 95) a form paints against its OWN `/Resources` only, so a form
/// that omits `/ColorSpace` (or omits `/Resources`) gets an EMPTY colour-space
/// environment and its `cs CS0` stays an unresolved `Resource(CS0)` downstream —
/// the honest prepress-audit behaviour, with no obsolete PDF 1.1 page-resource
/// fallback.
///
/// This mirrors [`inspect_form_xobject_resources`](crate::inspect_form_xobject_resources)
/// (single object, no inheritance) and never returns a hard error: an unreadable
/// form dictionary, a missing `/ColorSpace`, or a malformed/unresolved entry all
/// become structured entries in [`FormColorSpaceResourcesInspection::skipped`].
#[must_use]
pub fn inspect_form_color_space_resources(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormColorSpaceResourcesInspection {
    let context = match inspect_indirect_object_dictionary(input, object_byte_offset) {
        Ok(dictionary) => ResourceContext::from_dictionary(input, lookup, &dictionary, None),
        Err(error) => {
            return report(
                object_byte_offset,
                Vec::new(),
                vec![skipped_entry(
                    object_byte_offset,
                    None,
                    resources_skip(SkippedPageXObjectResourceReason::PageDictionaryFailed {
                        error,
                    }),
                )],
            );
        }
    };
    inspect_effective_color_spaces(input, lookup, object_byte_offset, &context)
}

/// Classify the effective `/ColorSpace` sub-dictionary of one resolved,
/// `None`-inherited form resource context.
///
/// This mirrors the page inspector's `inspect_effective_color_spaces`, reusing
/// the same `unique_entry`/`inspect_dictionary_entries` lookup machinery and the
/// same per-entry `classify_color_space_entry`. The entries are sorted and
/// deduplicated by name for deterministic downstream inventory.
fn inspect_effective_color_spaces(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> FormColorSpaceResourcesInspection {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|reason| skipped_entry(object_byte_offset, None, resources_skip(reason)))
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::MissingColorSpaceResources,
        ));
        return report(object_byte_offset, Vec::new(), skipped);
    };

    let Some(cs_entry) = (match unique_entry(input, &resources.entries, b"/ColorSpace") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedColorSpaceResourceReason::DuplicateColorSpace {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return report(object_byte_offset, Vec::new(), skipped);
        }
    }) else {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::MissingColorSpace,
        ));
        return report(object_byte_offset, Vec::new(), skipped);
    };

    if cs_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::NonDictionaryColorSpace {
                value_kind: cs_entry.value_kind,
            },
        ));
        return report(object_byte_offset, Vec::new(), skipped);
    }

    let entries = match inspect_dictionary_entries(input, cs_entry.value_range.start) {
        Ok(entries) => entries,
        Err(error) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedColorSpaceResourceReason::ColorSpaceDictionaryFailed { error },
            ));
            return report(object_byte_offset, Vec::new(), skipped);
        }
    };

    let color_spaces = classify_color_space_entries(
        input,
        lookup,
        object_byte_offset,
        entries.entries,
        &mut skipped,
    );
    report(object_byte_offset, color_spaces, skipped)
}

fn classify_color_space_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedColorSpaceResource>,
) -> Vec<ClassifiedColorSpaceResource> {
    let mut classified = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_entry(
                object_byte_offset,
                Some(name),
                SkippedColorSpaceResourceReason::DuplicateColorSpaceName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_color_space_entry(input, lookup, &name, entry) {
            Ok(resource) => classified.push(resource),
            Err(reason) => skipped.push(skipped_entry(object_byte_offset, Some(name), reason)),
        }
    }
    classified
}

fn report(
    object_byte_offset: usize,
    mut color_spaces: Vec<ClassifiedColorSpaceResource>,
    skipped: Vec<SkippedColorSpaceResource>,
) -> FormColorSpaceResourcesInspection {
    color_spaces.sort_by(|left, right| left.name.cmp(&right.name));
    FormColorSpaceResourcesInspection {
        object_byte_offset,
        color_spaces,
        skipped,
    }
}

const fn skipped_entry(
    object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedColorSpaceResourceReason,
) -> SkippedColorSpaceResource {
    SkippedColorSpaceResource {
        page_object_byte_offset: object_byte_offset,
        resource_name,
        reason,
    }
}

const fn resources_skip(
    reason: SkippedPageXObjectResourceReason,
) -> SkippedColorSpaceResourceReason {
    SkippedColorSpaceResourceReason::Resources {
        resources_reason: reason,
    }
}
