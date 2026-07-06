// The dependency-free serde value harness is shared verbatim with the other
// inventory tests; this focused module re-includes it rather than duplicating a
// 700-line format shim.
#[allow(clippy::duplicate_mod)]
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{from_serde_value, serde_value};

use super::single_page_pdf;
use crate::color_audit::build_color_usage_audit;
use crate::inventory::{Inventory, InventoryEntry};
use crate::{
    ColorAuditStatus, ColorObservation, ColorSpace, ColorUsage, ColorUsageAudit, ContentScope,
    CoverageGap, CoverageGapKind, ObjectId, ObjectKind, PageColorUsage, PageIndex, PdfInventory,
    PdfInventoryError, PdfInventoryPage, PdfInventoryPageResult, PdfInventorySkip, PdfName,
    Provenance, RgbFinding, SkippedFormInventory, SkippedFormInventoryReason, audit_color_usage,
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
        source: None,
    }
}

fn spot_observation(space: ColorSpace, name: &[u8]) -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Fill,
        space,
        components: Vec::new(),
        spot_name: Some(PdfName(name.to_vec())),
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

#[test]
fn clean_cmyk_page_audits_complete_via_pdf() -> Result<(), PdfInventoryError> {
    // PDF-backed smoke test through the real `audit_color_usage` entry point.
    let source = single_page_pdf(b"", CMYK_FILL_CONTENT);

    let audit = audit_color_usage(&source, 1024)?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert!(audit.rgb_findings.is_empty());
    assert!(audit.spot_names.is_empty());
    assert_eq!(audit.pages.len(), 1);
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceCmyk), 1);
    assert_eq!(usage_count(&audit.document, ColorUsage::Fill), 1);
    assert_eq!(kind_count(&audit.document, ObjectKind::Vector), 1);
    Ok(())
}

#[test]
fn clean_gray_page_audits_complete_via_pdf() -> Result<(), PdfInventoryError> {
    let source = single_page_pdf(b"", GRAY_STROKE_CONTENT);

    let audit = audit_color_usage(&source, 1024)?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceGray), 1);
    assert_eq!(usage_count(&audit.document, ColorUsage::Stroke), 1);
    Ok(())
}

#[test]
fn device_rgb_is_a_finding_not_a_coverage_gap() {
    // A `DeviceRGB` observation is fully classified (a modeled device space), so
    // it is reported as an explicit RGB finding and does NOT make the audit
    // incomplete on its own.
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::DeviceRgb)],
        )],
        vec![inventoried_page(0, 1)],
    );

    let audit = build_color_usage_audit(inventory);

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(audit.rgb_findings.len(), 1);
    let finding = &audit.rgb_findings[0];
    assert_eq!(finding.page, PageIndex(0));
    assert_eq!(finding.entry_index, 0);
    assert_eq!(finding.kind, ObjectKind::Vector);
    assert_eq!(finding.usage, ColorUsage::Fill);
    assert_eq!(
        finding.object,
        audit.inventory.inventory.entries[0].id.clone()
    );
}

#[test]
fn per_page_and_document_counts_are_deterministic() {
    // Page 0: one CMYK-fill vector + one RGB-stroke vector.
    // Page 1: one gray-fill vector.
    let inventory = synthetic_inventory(
        vec![
            entry(
                0,
                0,
                ObjectKind::Vector,
                vec![observation(ColorUsage::Fill, ColorSpace::DeviceCmyk)],
            ),
            entry(
                0,
                1,
                ObjectKind::Vector,
                vec![observation(ColorUsage::Stroke, ColorSpace::DeviceRgb)],
            ),
            entry(
                1,
                2,
                ObjectKind::Vector,
                vec![observation(ColorUsage::Fill, ColorSpace::DeviceGray)],
            ),
        ],
        vec![inventoried_page(0, 2), inventoried_page(1, 1)],
    );

    let audit = build_color_usage_audit(inventory);

    // Document totals: 3 observations, 3 vector entries.
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceCmyk), 1);
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceRgb), 1);
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceGray), 1);
    assert_eq!(usage_count(&audit.document, ColorUsage::Fill), 2);
    assert_eq!(usage_count(&audit.document, ColorUsage::Stroke), 1);
    assert_eq!(kind_count(&audit.document, ObjectKind::Vector), 3);

    // Per-page split follows each page's contiguous entry run.
    assert_eq!(audit.pages.len(), 2);
    assert_eq!(audit.pages[0].page, PageIndex(0));
    assert_eq!(
        space_count(&audit.pages[0].summary, &ColorSpace::DeviceCmyk),
        1
    );
    assert_eq!(
        space_count(&audit.pages[0].summary, &ColorSpace::DeviceRgb),
        1
    );
    assert_eq!(kind_count(&audit.pages[0].summary, ObjectKind::Vector), 2);
    assert_eq!(audit.pages[1].page, PageIndex(1));
    assert_eq!(
        space_count(&audit.pages[1].summary, &ColorSpace::DeviceGray),
        1
    );
    assert_eq!(kind_count(&audit.pages[1].summary, ObjectKind::Vector), 1);

    // Color-space counts are emitted in the fixed variant order
    // (Gray < Rgb < Cmyk), independent of observation order.
    let order: Vec<&ColorSpace> = audit
        .document
        .color_space_counts
        .iter()
        .map(|count| &count.color_space)
        .collect();
    assert_eq!(
        order,
        vec![
            &ColorSpace::DeviceGray,
            &ColorSpace::DeviceRgb,
            &ColorSpace::DeviceCmyk
        ]
    );
}

#[test]
fn spot_names_are_deduplicated_and_sorted_by_raw_bytes() {
    // Separation/DeviceN observations contribute spot names; duplicates collapse
    // and the result is sorted by raw name bytes. A DeviceCMYK observation that
    // (defensively) carries a stray spot name must be ignored.
    let inventory = synthetic_inventory(
        vec![
            entry(
                0,
                0,
                ObjectKind::Vector,
                vec![
                    spot_observation(ColorSpace::Separation, b"Pantone 300 C"),
                    spot_observation(ColorSpace::DeviceN, b"All"),
                ],
            ),
            entry(
                0,
                1,
                ObjectKind::Vector,
                vec![
                    spot_observation(ColorSpace::Separation, b"All"),
                    spot_observation(ColorSpace::Separation, b"Cut"),
                    // Not Separation/DeviceN: this spot name must be dropped.
                    ColorObservation {
                        usage: ColorUsage::Fill,
                        space: ColorSpace::DeviceCmyk,
                        components: Vec::new(),
                        spot_name: Some(PdfName(b"AAAA".to_vec())),
                        source: None,
                    },
                ],
            ),
        ],
        vec![inventoried_page(0, 2)],
    );

    let audit = build_color_usage_audit(inventory);

    let names: Vec<&[u8]> = audit
        .spot_names
        .iter()
        .map(|name| name.0.as_slice())
        .collect();
    assert_eq!(names, vec![&b"All"[..], b"Cut", b"Pantone 300 C"]);
}

#[test]
fn skipped_page_and_unmodeled_space_make_audit_incomplete() {
    let inventory = synthetic_inventory(
        vec![entry(
            1,
            0,
            ObjectKind::Vector,
            // `Lab` is still an unmodeled space after resource colour-space
            // tracking (only `IccBased`/`Separation`/`DeviceN` became modeled),
            // so it still exercises the `UnmodeledColorSpace` gap path.
            vec![observation(ColorUsage::Fill, ColorSpace::Lab)],
        )],
        vec![skipped_page(0), inventoried_page(1, 1)],
    );

    let audit = build_color_usage_audit(inventory);

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    // Skipped page 0 first, then the unmodeled-space gap on page 1.
    assert_eq!(audit.coverage_gaps.len(), 2);
    assert_eq!(audit.coverage_gaps[0].kind, CoverageGapKind::SkippedPage);
    assert_eq!(audit.coverage_gaps[0].page, Some(PageIndex(0)));
    assert_eq!(audit.coverage_gaps[0].object, None);

    let unmodeled = &audit.coverage_gaps[1];
    assert_eq!(unmodeled.kind, CoverageGapKind::UnmodeledColorSpace);
    assert_eq!(unmodeled.page, Some(PageIndex(1)));
    assert_eq!(unmodeled.entry_index, Some(0));
    assert_eq!(unmodeled.kind_of_object, Some(ObjectKind::Vector));
    assert_eq!(unmodeled.usage, Some(ColorUsage::Fill));
    assert_eq!(unmodeled.color_space, Some(ColorSpace::Lab));
}

#[test]
fn image_unknown_form_skip_resource_skip_and_error_are_gaps() {
    let mut page = inventoried_page_with_form_skipped(0, 1, vec![budget_form_skip()]);
    // A present `XObject` target with no `/Subtype` is unclassifiable, so it is
    // a genuine coverage gap (unlike a page that simply has no resources).
    page.xobject_resource_skipped
        .push(crate::pdf::SkippedPageXObjectResource {
            page_object_byte_offset: 0,
            resource_name: Some(crate::pdf::PdfName(b"Im0".to_vec())),
            reason: crate::pdf::SkippedPageXObjectResourceReason::MissingSubtype {
                object_byte_offset: 0,
            },
        });
    let mut inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Image,
            vec![observation(ColorUsage::Image, ColorSpace::Unknown)],
        )],
        vec![page],
    );
    inventory.xobject_resource_error = Some(resource_inspection_error());

    let audit = build_color_usage_audit(inventory);

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    let kinds: Vec<CoverageGapKind> = audit.coverage_gaps.iter().map(|gap| gap.kind).collect();
    // Page-scope resource skip, then image-color gap, then form-expansion skip,
    // then the document-level resource-inspection error last.
    assert_eq!(
        kinds,
        vec![
            CoverageGapKind::PageResourceSkipped,
            CoverageGapKind::ImageColorUndecoded,
            CoverageGapKind::FormExpansionSkipped,
            CoverageGapKind::ResourceInspectionError,
        ]
    );

    let image_gap = &audit.coverage_gaps[1];
    assert_eq!(image_gap.kind_of_object, Some(ObjectKind::Image));
    assert_eq!(image_gap.usage, Some(ColorUsage::Image));
    assert_eq!(image_gap.color_space, Some(ColorSpace::Unknown));

    let error_gap = &audit.coverage_gaps[3];
    assert_eq!(error_gap.page, None);
    assert_eq!(error_gap.object, None);
}

#[test]
fn missing_resources_skip_is_not_a_coverage_gap() {
    // A page that declares no `/Resources`/`/XObject` has no XObject color to
    // miss, so the benign skip must not make the audit incomplete.
    let mut page = inventoried_page(0, 1);
    page.xobject_resource_skipped
        .push(crate::pdf::SkippedPageXObjectResource {
            page_object_byte_offset: 0,
            resource_name: None,
            reason: crate::pdf::SkippedPageXObjectResourceReason::MissingResources,
        });
    page.xobject_resource_skipped
        .push(crate::pdf::SkippedPageXObjectResource {
            page_object_byte_offset: 0,
            resource_name: None,
            reason: crate::pdf::SkippedPageXObjectResourceReason::MissingXObject,
        });
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::DeviceCmyk)],
        )],
        vec![page],
    );

    let audit = build_color_usage_audit(inventory);

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
}

#[test]
fn report_serde_round_trips_all_shapes() -> Result<(), String> {
    let inventory = synthetic_inventory(
        vec![
            entry(
                1,
                0,
                ObjectKind::Vector,
                vec![observation(ColorUsage::Fill, ColorSpace::DeviceRgb)],
            ),
            entry(
                1,
                1,
                ObjectKind::Vector,
                vec![spot_observation(ColorSpace::Separation, b"Spot")],
            ),
            entry(
                1,
                2,
                ObjectKind::Image,
                vec![observation(ColorUsage::Image, ColorSpace::Unknown)],
            ),
        ],
        vec![
            skipped_page(0),
            inventoried_page_with_form_skipped(1, 3, vec![budget_form_skip()]),
        ],
    );

    let audit = build_color_usage_audit(inventory);

    // The hand-built report exercises a finding, spot name, and every gap kind
    // reachable from entries/pages.
    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert_eq!(audit.rgb_findings.len(), 1);
    assert_eq!(audit.spot_names.len(), 1);

    round_trip::<ColorUsageAudit>(&audit)?;
    round_trip(&audit.status)?;
    round_trip(&audit.document)?;
    for page in &audit.pages {
        round_trip::<PageColorUsage>(page)?;
    }
    for finding in &audit.rgb_findings {
        round_trip::<RgbFinding>(finding)?;
    }
    for gap in &audit.coverage_gaps {
        round_trip::<CoverageGap>(gap)?;
        round_trip::<CoverageGapKind>(&gap.kind)?;
    }
    Ok(())
}

#[test]
fn rgb_page_through_pdf_reports_finding_and_stays_complete() -> Result<(), PdfInventoryError> {
    let source = single_page_pdf(b"", b"q\n1 0 0 rg\n0 0 9 9 re\nf\nQ");

    let audit = audit_color_usage(&source, 1024)?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert_eq!(audit.rgb_findings.len(), 1);
    assert_eq!(audit.rgb_findings[0].usage, ColorUsage::Fill);
    assert_eq!(space_count(&audit.document, &ColorSpace::DeviceRgb), 1);
    Ok(())
}
