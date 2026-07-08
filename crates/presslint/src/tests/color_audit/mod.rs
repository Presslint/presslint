// The dependency-free serde value harness is shared verbatim with the other
// inventory tests; this focused module re-includes it rather than duplicating a
// 700-line format shim.
#[allow(clippy::duplicate_mod)]
#[path = "../../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

mod gaps_findings;
mod graphics_state;
mod scan_counts;
mod serde_shape;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use presslint_color::{
    NamedOutputCondition, OutputIntentPolicy, OutputIntentSubtype, OutputIntentTarget,
};

use super::form_inventory::{CATALOG, PAGES, classic_pdf, stream_object};
use super::single_page_pdf;
use crate::color_audit::build_color_usage_audit;
use crate::graphics_state_findings::scan_document_graphics_state;
use crate::inventory::{Inventory, InventoryEntry};
use crate::{
    ColorAuditStatus, ColorObservation, ColorSpace, ColorUsage, ColorUsageAudit, ContentScope,
    CoverageGap, CoverageGapKind, GraphicsStateFinding, GraphicsStateFindingSource, ObjectId,
    ObjectKind, PageColorUsage, PageIndex, PdfInventory, PdfInventoryError, PdfInventoryPage,
    PdfInventoryPageResult, PdfInventorySkip, PdfName, Provenance, RgbFinding,
    SkippedFormInventory, SkippedFormInventoryReason, audit_color_usage,
    audit_color_usage_with_output_intent_policy,
};

const CMYK_FILL_CONTENT: &[u8] = b"q\n0 0 0 1 k\n12 12 80 80 re\nf\nQ";
const GRAY_STROKE_CONTENT: &[u8] = b"q\n0.5 G\n12 12 80 80 re\nS\nQ";

fn round_trip<T>(value: &T) -> Result<(), String>
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_value(value).map_err(|error| error.to_string())?;
    let decoded: T = from_serde_value(encoded).map_err(|error| error.to_string())?;
    assert_eq!(&decoded, value);
    Ok(())
}

fn observation(usage: ColorUsage, space: ColorSpace) -> ColorObservation {
    ColorObservation {
        usage,
        space,
        components: Vec::new(),
        spot_name: None,
        spot_names: Vec::new(),
        source: None,
    }
}

fn spot_observation(space: ColorSpace, name: &[u8]) -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Fill,
        space,
        components: Vec::new(),
        spot_name: Some(PdfName(name.to_vec())),
        spot_names: Vec::new(),
        source: None,
    }
}

fn multi_spot_observation(space: ColorSpace, names: &[&[u8]]) -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Fill,
        space,
        components: Vec::new(),
        spot_name: names.first().map(|name| PdfName((*name).to_vec())),
        spot_names: names.iter().map(|name| PdfName((*name).to_vec())).collect(),
        source: None,
    }
}

fn entry(
    page: u32,
    sequence: u32,
    kind: ObjectKind,
    colors: Vec<ColorObservation>,
) -> InventoryEntry {
    InventoryEntry {
        id: ObjectId {
            page: PageIndex(page),
            sequence,
            digest: [0u8; 32],
        },
        kind,
        provenance: Provenance {
            page: PageIndex(page),
            scope: ContentScope::Page,
            range: None,
            invocation: None,
        },
        bounds: None,
        colors,
        capabilities: Vec::new(),
    }
}

fn inventoried_page(page: u32, entry_count: usize) -> PdfInventoryPage {
    inventoried_page_with_form_skipped(page, entry_count, Vec::new())
}

fn inventoried_page_with_form_skipped(
    page: u32,
    entry_count: usize,
    form_skipped: Vec<SkippedFormInventory>,
) -> PdfInventoryPage {
    PdfInventoryPage {
        page_index: PageIndex(page),
        result: PdfInventoryPageResult::Inventoried {
            entry_count,
            form_skipped,
        },
        image_xobjects: Vec::new(),
        xobject_resource_skipped: Vec::new(),
        color_space_resource_skipped: Vec::new(),
    }
}

fn skipped_page(page: u32) -> PdfInventoryPage {
    PdfInventoryPage {
        page_index: PageIndex(page),
        result: PdfInventoryPageResult::Skipped {
            reason: PdfInventorySkip::NoContentStreams,
        },
        image_xobjects: Vec::new(),
        xobject_resource_skipped: Vec::new(),
        color_space_resource_skipped: Vec::new(),
    }
}

fn synthetic_inventory(entries: Vec<InventoryEntry>, pages: Vec<PdfInventoryPage>) -> PdfInventory {
    PdfInventory {
        byte_len: 0,
        inventory: Inventory { entries },
        xobject_resource_error: None,
        color_space_resource_error: None,
        pages,
    }
}

fn budget_form_skip() -> SkippedFormInventory {
    SkippedFormInventory {
        name: PdfName(b"Fm".to_vec()),
        reference: crate::pdf::IndirectRef {
            object_number: 9,
            generation: 0,
        },
        object_byte_offset: 0,
        reason: SkippedFormInventoryReason::BudgetExhausted { max_expansions: 3 },
    }
}

fn resource_inspection_error() -> crate::pdf::DocumentPageXObjectResourcesInspectionError {
    crate::pdf::DocumentPageXObjectResourcesInspectionError {
        root_node_byte_offset: 0,
        byte_len: 0,
        error: crate::pdf::PageTreeKidTargetsInspectionError {
            byte_offset: 0,
            byte_len: 0,
            node_header_byte_offset: None,
            error_byte_offset: None,
            reason: crate::pdf::PageTreeKidTargetsInspectionRejection::PageTreeKids {
                kids_reason: crate::pdf::PageTreeKidsInspectionRejection::PageTreeNode {
                    node_reason: crate::pdf::PageTreeNodeInspectionRejection::MissingKids,
                },
            },
        },
    }
}

fn page_with_resources_pdf(resources: &str, content: &[u8]) -> Vec<u8> {
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << {resources} >> /Contents 4 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content_object = stream_object(4, "", content);
    classic_pdf(&[CATALOG, PAGES, &page, &content_object])
}

fn page_with_extgstate_pdf(dict: &str, content: &[u8]) -> Vec<u8> {
    page_with_resources_pdf(&format!("/ExtGState << {dict} >>"), content)
}

fn space_count(summary: &crate::ColorUsageSummary, space: &ColorSpace) -> usize {
    summary
        .color_space_counts
        .iter()
        .find(|count| &count.color_space == space)
        .map_or(0, |count| count.count)
}

fn usage_count(summary: &crate::ColorUsageSummary, usage: ColorUsage) -> usize {
    summary
        .color_usage_counts
        .iter()
        .find(|count| count.usage == usage)
        .map_or(0, |count| count.count)
}

fn kind_count(summary: &crate::ColorUsageSummary, kind: ObjectKind) -> usize {
    summary
        .object_kind_counts
        .iter()
        .find(|count| count.kind == kind)
        .map_or(0, |count| count.count)
}
