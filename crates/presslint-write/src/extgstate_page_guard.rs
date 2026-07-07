//! Precision page-level `ExtGState` guard for the device-colour converter.
//!
//! A page's content streams share graphics state, so one unsafe `gs` activation
//! poisons the whole page. This guard scans already-decoded streams and checks
//! only the resources that are actually named by `gs`. Declared-but-unused
//! resources, and resources with only harmless unclassified keys such as `/LW`,
//! do not block conversion.
//!
//! Known residual: transparency groups (`/Group`) can make colour conversion
//! unsafe without any `gs` operator. That belongs to a later graphics-state
//! slice; this guard intentionally covers only page `ExtGState` activation.

use presslint_pdf::{ClassifiedExtGStateResource, PageExtGStateResourcesInspection};
use presslint_syntax::{Token, TokenKind, assemble_operators, tokenize};

use crate::content_edit_pipeline::PipelineSkipReason;

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
    decoded_streams: &[Vec<u8>],
) -> Option<PipelineSkipReason> {
    let mut flags = ExtGStateUnsafeFlags::default();
    for decoded in decoded_streams {
        scan_stream(decoded, page_resources, &mut flags);
    }
    if flags.gs_count == 0 || flags.is_empty() {
        None
    } else {
        Some(flags.into_skip_reason())
    }
}

fn scan_stream(
    decoded: &[u8],
    page_resources: Option<&PageExtGStateResourcesInspection>,
    flags: &mut ExtGStateUnsafeFlags,
) {
    let Ok(tokens) = tokenize(decoded) else {
        return;
    };
    let Ok(assembled) = assemble_operators(&tokens) else {
        return;
    };

    for record in assembled.records {
        if tokens[record.operator.token_index].source_bytes(decoded) != Some(GS_OPERATOR) {
            continue;
        }
        flags.gs_count = flags.gs_count.saturating_add(1);
        let Some(name) = gs_operand_name(&record.operands, decoded, &tokens) else {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        };
        let Some(page_resources) = page_resources else {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        };
        if has_skipped_resource_name(page_resources, name) {
            flags.add(ExtGStateUnsafeFlags::UNCLASSIFIED);
            continue;
        }
        let Some(resource) = find_resource(page_resources, name) else {
            flags.add(ExtGStateUnsafeFlags::UNRESOLVED);
            continue;
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

fn has_skipped_resource_name(
    page_resources: &PageExtGStateResourcesInspection,
    name: &[u8],
) -> bool {
    page_resources.skipped.iter().any(|skip| {
        skip.resource_name
            .as_ref()
            .is_some_and(|resource_name| resource_name.0.as_slice() == name)
    })
}

fn find_resource<'a>(
    page_resources: &'a PageExtGStateResourcesInspection,
    name: &[u8],
) -> Option<&'a ClassifiedExtGStateResource> {
    page_resources
        .extgstates
        .iter()
        .find(|resource| resource.name.0.as_slice() == name)
}
