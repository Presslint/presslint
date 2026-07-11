use serde::{Deserialize, Serialize};

use crate::font_classify::{
    ClassifiedFontResource, SkippedFontResource, SkippedFontResourceReason,
    inspect_effective_font_resource_entries, skipped_entry,
};
use crate::page_resource_inheritance::ResourceContext;
use crate::{ObjectLookup, SkippedPageXObjectResourceReason, inspect_indirect_object_dictionary};

/// Classified own-scope `/Resources /Font` resources for one Form `XObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormFontResourcesInspection {
    /// Resolved form stream object byte offset the resources were read from.
    pub object_byte_offset: usize,
    /// Classified `Font` resources, sorted/deduplicated by raw name.
    pub fonts: Vec<ClassifiedFontResource>,
    /// Form-local structural `Font` diagnostics.
    pub skipped: Vec<SkippedFontResource>,
}

/// Classify one Form `XObject`'s own `/Resources /Font` dictionary.
///
/// Page resources are intentionally not inherited:
/// `ResourceContext::from_dictionary(..., None)` gives the form only its own
/// resource dictionary, matching the existing form `ExtGState` inspector.
#[must_use]
pub fn inspect_form_font_resources(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormFontResourcesInspection {
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
    inspect_effective_fonts(input, lookup, object_byte_offset, &context)
}

fn inspect_effective_fonts(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> FormFontResourcesInspection {
    let effective =
        inspect_effective_font_resource_entries(input, lookup, object_byte_offset, context);
    report(object_byte_offset, effective.fonts, effective.skipped)
}

const fn report(
    object_byte_offset: usize,
    fonts: Vec<ClassifiedFontResource>,
    skipped: Vec<SkippedFontResource>,
) -> FormFontResourcesInspection {
    FormFontResourcesInspection {
        object_byte_offset,
        fonts,
        skipped,
    }
}

const fn resources_skip(reason: SkippedPageXObjectResourceReason) -> SkippedFontResourceReason {
    SkippedFontResourceReason::Resources {
        resources_reason: reason,
    }
}
