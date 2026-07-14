//! Precision page-level graphics-state guard for the device-colour converter.
//!
//! A page's content streams share graphics state, so one unsafe `gs` activation
//! poisons the whole page. This guard scans already-decoded streams and checks
//! only the resources that are actually named by `gs`. Declared-but-unused
//! resources, and resources with only harmless unclassified keys such as `/LW`,
//! do not block conversion.
//!
//! Resource matching is SEMANTIC (ISO 32000-1 §7.3.5): the `gs` operand and the
//! classified/skipped resource names are decoded before comparison, so `/GS1`
//! and `/GS#31` are one name. The safety resolution requires exactly one
//! classified semantic match and no matching skip; multiple matches, a matching
//! skip, and a malformed/undecodable operand all fail closed rather than
//! first-winning a semantic duplicate. A strictly undecodable report name is a
//! literal-spelling poison only when its raw bytes equal the decoded operand,
//! matching permissive-reader ambiguity without poisoning unrelated names. Raw
//! public report names are untouched.

use presslint_pdf::{
    ClassifiedExtGStateResource, PageExtGStateResourcesInspection, PageTransparencyGroupInspection,
};
use presslint_syntax::{Token, TokenKind};

use crate::{
    content_edit_pipeline::PipelineSkipReason, page_content_sequence::PageContentSequence,
    page_xobject_policy::decode_pdf_name,
};

const GS_OPERATOR: &[u8] = b"gs";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ExtGStateUnsafeFlags {
    bits: u8,
    gs_count: u32,
}

impl ExtGStateUnsafeFlags {
    const OVERPRINT: u8 = 1 << 0;
    const TRANSPARENCY: u8 = 1 << 1;
    const UNRESOLVED: u8 = 1 << 2;
    const UNCLASSIFIED: u8 = 1 << 3;

    const fn is_empty(self) -> bool {
        self.bits == 0
    }

    const fn has(self, bit: u8) -> bool {
        self.bits & bit != 0
    }

    const fn add(&mut self, bit: u8) {
        self.bits |= bit;
    }

    const fn into_skip_reason(self) -> PipelineSkipReason {
        PipelineSkipReason::ExtGStateUnsafe {
            overprint: self.has(Self::OVERPRINT),
            transparency: self.has(Self::TRANSPARENCY),
            unresolved: self.has(Self::UNRESOLVED),
            unclassified: self.has(Self::UNCLASSIFIED),
            gs_count: self.gs_count,
        }
    }
}

/// Return a page skip reason when the decoded streams activate unsafe or
/// unknowable page-scope `ExtGState` parameters.
#[must_use]
pub fn extgstate_page_skip_reason(
    page_resources: Option<&PageExtGStateResourcesInspection>,
    sequence: &PageContentSequence,
) -> Option<PipelineSkipReason> {
    let mut flags = ExtGStateUnsafeFlags::default();
    scan_sequence(sequence, page_resources, &mut flags);
    if flags.gs_count == 0 || flags.is_empty() {
        None
    } else {
        Some(flags.into_skip_reason())
    }
}

/// Return a page skip reason when the page dictionary establishes, or hides
/// whether it establishes, a transparency group.
#[must_use]
pub fn transparency_group_page_skip_reason(
    page_group: Option<&PageTransparencyGroupInspection>,
) -> Option<PipelineSkipReason> {
    let page_group = page_group?;
    if let Some(group) = &page_group.group {
        return Some(PipelineSkipReason::TransparencyGroupUnsafe {
            transparency: group.transparency,
            unresolved: false,
            unclassified: group.has_unclassified_safety_field(),
        });
    }
    (!page_group.skipped.is_empty()).then_some(PipelineSkipReason::TransparencyGroupUnsafe {
        transparency: false,
        unresolved: true,
        unclassified: true,
    })
}

fn scan_sequence(
    sequence: &PageContentSequence,
    page_resources: Option<&PageExtGStateResourcesInspection>,
    flags: &mut ExtGStateUnsafeFlags,
) {
    let decoded = sequence.bytes();
    let tokens = sequence.tokens();
    for record in sequence.records() {
        if tokens[record.operator.token_index].source_bytes(decoded) != Some(GS_OPERATOR) {
            continue;
        }
        flags.gs_count = flags.gs_count.saturating_add(1);
        let Some(raw_name) = gs_operand_name(&record.operands, decoded, tokens) else {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        };
        // A malformed/undecodable operand can never be proven to name one
        // classified resource; fail closed.
        let Some(operand) = decode_pdf_name(raw_name) else {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        };
        let Some(page_resources) = page_resources else {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        };
        // Incomplete coverage: a namespace-level (nameless) structural skip
        // means the classified set is not authoritative. When resources were
        // still classified, a `gs` match cannot be trusted to be the whole
        // story, so fail closed as unclassified rather than accept it. With an
        // empty classified set the `gs` already falls through to the unresolved
        // path below, so that outcome is left unchanged.
        if has_incomplete_coverage(page_resources) {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        }
        if has_matching_skip(page_resources, &operand) {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        }
        let resource = match unique_classified_match(page_resources, &operand) {
            ResourceMatch::Unique(resource) => resource,
            // Never first-win a semantic duplicate; ambiguity fails closed.
            ResourceMatch::Multiple | ResourceMatch::LiteralPoison => {
                flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
                continue;
            }
            ResourceMatch::None => {
                flags.add(ExtGStateUnsafeFlags::UNRESOLVED);
                continue;
            }
        };
        if resource.is_overprint_active() {
            flags.add(ExtGStateUnsafeFlags::OVERPRINT);
        }
        if resource.is_transparency_active() {
            flags.add(ExtGStateUnsafeFlags::TRANSPARENCY);
        }
        if resource.has_unresolved_or_unclassified_safety_param() {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
        }
    }
}

fn gs_operand_name<'a>(
    operands: &[presslint_syntax::OperandRecord],
    decoded: &'a [u8],
    tokens: &'a [Token],
) -> Option<&'a [u8]> {
    let [operand] = operands else {
        return None;
    };
    let [token_ref] = operand.tokens.as_slice() else {
        return None;
    };
    let token = &tokens[token_ref.token_index];
    if token.kind != TokenKind::Name {
        return None;
    }
    token.source_bytes(decoded)?.strip_prefix(b"/")
}

/// Result of matching a decoded `gs` operand against the classified resources.
enum ResourceMatch<'a> {
    /// No classified resource decodes to the operand's semantic name.
    None,
    /// Exactly one classified resource matches semantically.
    Unique(&'a ClassifiedExtGStateResource),
    /// Two or more classified resources decode to the same semantic name.
    Multiple,
    /// An undecodable classified name has the operand's decoded byte spelling.
    LiteralPoison,
}

/// Relationship between one raw report name and a strictly decoded operand.
enum ResourceNameMatch {
    None,
    Semantic,
    LiteralPoison,
}

/// Compare one raw report name with a decoded operand. Strictly undecodable
/// names retain their literal spelling as a bounded poison key: a permissive
/// reader may treat the malformed `#` literally, so exact raw equality cannot
/// be discarded or accepted as a safe classified match.
fn resource_name_match(raw_name: &[u8], operand: &[u8]) -> ResourceNameMatch {
    match decode_pdf_name(raw_name) {
        Some(decoded) if decoded.as_ref() == operand => ResourceNameMatch::Semantic,
        None if raw_name == operand => ResourceNameMatch::LiteralPoison,
        Some(_) | None => ResourceNameMatch::None,
    }
}

/// Whether the classified `ExtGState` set is authoritative but incomplete: at
/// least one resource was classified while a namespace-level (nameless)
/// structural skip proves the coverage is partial. A `gs` naming a classified
/// resource then cannot be trusted, because the skip may hide a same-named
/// unsafe sibling.
fn has_incomplete_coverage(page_resources: &PageExtGStateResourcesInspection) -> bool {
    !page_resources.extgstates.is_empty()
        && page_resources
            .skipped
            .iter()
            .any(|skip| skip.resource_name.is_none())
}

/// Whether any named structural skip semantically matches the operand. A
/// matching skip is an unclassifiable collision and fails closed.
fn has_matching_skip(page_resources: &PageExtGStateResourcesInspection, operand: &[u8]) -> bool {
    page_resources.skipped.iter().any(|skip| {
        skip.resource_name.as_ref().is_some_and(|resource_name| {
            !matches!(
                resource_name_match(&resource_name.0, operand),
                ResourceNameMatch::None
            )
        })
    })
}

/// Require exactly one classified semantic match; two or more is an ambiguous
/// duplicate that must never first-win.
fn unique_classified_match<'a>(
    page_resources: &'a PageExtGStateResourcesInspection,
    operand: &[u8],
) -> ResourceMatch<'a> {
    let mut found: Option<&'a ClassifiedExtGStateResource> = None;
    for resource in &page_resources.extgstates {
        match resource_name_match(&resource.name.0, operand) {
            ResourceNameMatch::None => {}
            ResourceNameMatch::LiteralPoison => return ResourceMatch::LiteralPoison,
            ResourceNameMatch::Semantic => {
                if found.is_some() {
                    return ResourceMatch::Multiple;
                }
                found = Some(resource);
            }
        }
    }
    found.map_or(ResourceMatch::None, ResourceMatch::Unique)
}
