use serde::{Deserialize, Serialize};

use crate::extgstate_classify::{
    ClassifiedExtGStateResource, SkippedExtGStateResource, SkippedExtGStateResourceReason,
    inspect_effective_extgstate_resource_entries, skipped_entry,
};
use crate::page_resource_inheritance::ResourceContext;
use crate::{ObjectLookup, SkippedPageXObjectResourceReason, inspect_indirect_object_dictionary};

/// Classified own-scope `/Resources /ExtGState` resources for one Form
/// `XObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormExtGStateResourcesInspection {
    /// Resolved form stream object byte offset the resources were read from.
    pub object_byte_offset: usize,
    /// Classified `ExtGState` resources, sorted/deduplicated by name.
    pub extgstates: Vec<ClassifiedExtGStateResource>,
    /// Form-local structural `ExtGState` diagnostics.
    pub skipped: Vec<SkippedExtGStateResource>,
}

/// Classify one Form `XObject`'s own `/Resources /ExtGState` dictionary.
///
/// Page resources are intentionally not inherited:
/// `ResourceContext::from_dictionary(..., None)` gives the form only its own
/// resource dictionary, matching the existing form colour-space inspector.
#[must_use]
pub fn inspect_form_extgstate_resources(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormExtGStateResourcesInspection {
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
    inspect_effective_extgstates(input, lookup, object_byte_offset, &context)
}

fn inspect_effective_extgstates(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> FormExtGStateResourcesInspection {
    let effective =
        inspect_effective_extgstate_resource_entries(input, lookup, object_byte_offset, context);
    report(object_byte_offset, effective.extgstates, effective.skipped)
}

const fn report(
    object_byte_offset: usize,
    extgstates: Vec<ClassifiedExtGStateResource>,
    skipped: Vec<SkippedExtGStateResource>,
) -> FormExtGStateResourcesInspection {
    FormExtGStateResourcesInspection {
        object_byte_offset,
        extgstates,
        skipped,
    }
}

const fn resources_skip(
    reason: SkippedPageXObjectResourceReason,
) -> SkippedExtGStateResourceReason {
    SkippedExtGStateResourceReason::Resources {
        resources_reason: reason,
    }
}
