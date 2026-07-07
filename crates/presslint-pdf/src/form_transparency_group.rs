use serde::{Deserialize, Serialize};

use crate::transparency_group_classify::{
    ClassifiedTransparencyGroup, SkippedTransparencyGroup, SkippedTransparencyGroupReason,
    classify_transparency_group_entry, skipped_group,
};
use crate::{ObjectLookup, inspect_indirect_object_dictionary};

/// Classified own top-level `/Group` for one Form `XObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormTransparencyGroupInspection {
    /// Resolved form stream object byte offset the group was read from.
    pub object_byte_offset: usize,
    /// Classified transparency group when the form has a valid
    /// `/Group << /S /Transparency ... >>`.
    pub group: Option<ClassifiedTransparencyGroup>,
    /// Form-local structural `/Group` diagnostics.
    pub skipped: Vec<SkippedTransparencyGroup>,
}

/// Classify one Form `XObject`'s own top-level `/Group` dictionary.
///
/// Page groups and page resources are intentionally not inherited.
#[must_use]
pub fn inspect_form_transparency_group(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormTransparencyGroupInspection {
    match inspect_indirect_object_dictionary(input, object_byte_offset) {
        Ok(dictionary) => match classify_transparency_group_entry(
            input,
            lookup,
            object_byte_offset,
            &dictionary.entries,
        ) {
            Ok(group) => report(object_byte_offset, group, Vec::new()),
            Err(skip) => report(object_byte_offset, None, vec![skip]),
        },
        Err(error) => report(
            object_byte_offset,
            None,
            vec![skipped_group(
                object_byte_offset,
                SkippedTransparencyGroupReason::ObjectDictionaryFailed { error },
            )],
        ),
    }
}

const fn report(
    object_byte_offset: usize,
    group: Option<ClassifiedTransparencyGroup>,
    skipped: Vec<SkippedTransparencyGroup>,
) -> FormTransparencyGroupInspection {
    FormTransparencyGroupInspection {
        object_byte_offset,
        group,
        skipped,
    }
}
