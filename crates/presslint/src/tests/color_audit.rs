// The dependency-free serde value harness is shared verbatim with the other
// inventory tests; this focused module re-includes it rather than duplicating a
// 700-line format shim.
#[allow(clippy::duplicate_mod)]
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

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

// --- graphics-state findings (page-scope declared `/ExtGState` state) ---

/// CMYK-only content that never invokes `gs`, so graphics-state cases add no
/// colour findings or gaps and prove the DECLARED-in-resources semantics.
const CMYK_CONTENT_NO_GS: &[u8] = b"0 0 0 1 k\n12 12 80 80 re\nf";

/// A single-page classic PDF whose page carries `resources` as its literal
/// `/Resources` dictionary body and one raw content stream.
fn page_with_resources_pdf(resources: &str, content: &[u8]) -> Vec<u8> {
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << {resources} >> /Contents 4 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content_object = stream_object(4, "", content);
    classic_pdf(&[CATALOG, PAGES, &page, &content_object])
}

/// A single-page classic PDF whose `/Resources /ExtGState` dictionary body is
/// `dict` (same fixture shape as the `extgstate_wiring` tests).
fn page_with_extgstate_pdf(dict: &str, content: &[u8]) -> Vec<u8> {
    page_with_resources_pdf(&format!("/ExtGState << {dict} >>"), content)
}

// The four flags mirror the pinned finding shape positionally
// (overprint/transparency/unresolved/unclassified); a builder would only
// restate the struct.
#[allow(clippy::fn_params_excessive_bools)]
fn page_finding(
    overprint: bool,
    transparency: bool,
    unresolved: bool,
    unclassified: bool,
) -> GraphicsStateFinding {
    GraphicsStateFinding {
        page: PageIndex(0),
        source: GraphicsStateFindingSource::PageExtGState,
        overprint,
        transparency,
        unresolved,
        unclassified,
    }
}

fn group_finding(transparency: bool, unclassified: bool) -> GraphicsStateFinding {
    GraphicsStateFinding {
        page: PageIndex(0),
        source: GraphicsStateFindingSource::PageTransparencyGroup,
        overprint: false,
        transparency,
        unresolved: false,
        unclassified,
    }
}

fn audit_extgstate_page(dict: &str) -> Result<ColorUsageAudit, String> {
    audit_color_usage(&page_with_extgstate_pdf(dict, CMYK_CONTENT_NO_GS), 1024)
        .map_err(|error| format!("{error:?}"))
}

#[test]
fn graphics_state_finding_serde_shape_is_pinned() -> Result<(), String> {
    let finding = GraphicsStateFinding {
        page: PageIndex(2),
        source: GraphicsStateFindingSource::PageExtGState,
        overprint: true,
        transparency: false,
        unresolved: true,
        unclassified: false,
    };

    let value = serde_value(&finding).map_err(|error| error.to_string())?;
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(2)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_ext_g_state".to_string()),
            ),
            ("overprint".to_string(), TestSerdeValue::Bool(true)),
            ("transparency".to_string(), TestSerdeValue::Bool(false)),
            ("unresolved".to_string(), TestSerdeValue::Bool(true)),
            ("unclassified".to_string(), TestSerdeValue::Bool(false)),
        ])
    );
    round_trip(&finding)?;

    // The form-scope variant is a declared contract only in this slice: its
    // serde string is pinned here, but nothing emits it yet.
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::FormExtGState)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("form_ext_g_state".to_string())
    );
    round_trip(&GraphicsStateFindingSource::FormExtGState)?;
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::PageTransparencyGroup)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("page_transparency_group".to_string())
    );
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::FormTransparencyGroup)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("form_transparency_group".to_string())
    );
    round_trip(&GraphicsStateFindingSource::PageTransparencyGroup)?;
    round_trip(&GraphicsStateFindingSource::FormTransparencyGroup)?;

    // The two additive coverage-gap kinds are shape-locked the same way.
    assert_eq!(
        serde_value(&CoverageGapKind::ExtGStateResourceInspectionError)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("ext_g_state_resource_inspection_error".to_string())
    );
    assert_eq!(
        serde_value(&CoverageGapKind::ExtGStateResourceSkipped)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("ext_g_state_resource_skipped".to_string())
    );
    round_trip(&CoverageGapKind::ExtGStateResourceSkipped)?;
    assert_eq!(
        serde_value(&CoverageGapKind::TransparencyGroupInspectionError)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("transparency_group_inspection_error".to_string())
    );
    assert_eq!(
        serde_value(&CoverageGapKind::TransparencyGroupSkipped)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("transparency_group_skipped".to_string())
    );
    round_trip(&CoverageGapKind::TransparencyGroupSkipped)?;
    Ok(())
}

#[test]
fn audit_without_findings_omits_the_field_and_old_json_deserializes() -> Result<(), String> {
    // The synthetic pure-build path carries no graphics-state pass at all, so
    // the vec is empty and `skip_serializing_if` omits the key: every existing
    // pinned audit JSON stays byte-identical.
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::DeviceCmyk)],
        )],
        vec![inventoried_page(0, 1)],
    );
    let audit = build_color_usage_audit(inventory);
    assert!(audit.graphics_state_findings.is_empty());

    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    assert!(
        fields
            .iter()
            .all(|(key, _)| key != "graphics_state_findings")
    );

    // The serialized map WITHOUT the key is exactly an old-format report: it
    // must deserialize through `#[serde(default)]`.
    let decoded: ColorUsageAudit = from_serde_value(value).map_err(|error| error.to_string())?;
    assert!(decoded.graphics_state_findings.is_empty());
    assert_eq!(&decoded, &audit);
    Ok(())
}

#[test]
fn finding_bearing_audit_pins_the_graphics_state_findings_entry() -> Result<(), String> {
    // Content with no colour operators: the finding comes from the DECLARED
    // resources alone, and the audit then carries no f64 colour components the
    // dependency-free serde harness cannot model.
    let source = page_with_extgstate_pdf("/GS0 << /OP true /BM /Multiply >>", b"q\nQ");
    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    let (_, findings) = fields
        .iter()
        .find(|(key, _)| key == "graphics_state_findings")
        .ok_or_else(|| "graphics_state_findings key should be present".to_string())?;
    assert_eq!(
        findings,
        &TestSerdeValue::Seq(vec![TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(0)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_ext_g_state".to_string()),
            ),
            ("overprint".to_string(), TestSerdeValue::Bool(true)),
            ("transparency".to_string(), TestSerdeValue::Bool(true)),
            ("unresolved".to_string(), TestSerdeValue::Bool(false)),
            ("unclassified".to_string(), TestSerdeValue::Bool(false)),
        ])])
    );
    round_trip(&audit)?;
    Ok(())
}

#[test]
fn page_transparency_group_reports_transparency_finding() -> Result<(), String> {
    let mut page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << >> ".to_vec();
    page.extend_from_slice(
        b"/Group << /S /Transparency /CS /DeviceCMYK /I true /K false >> /Contents 4 0 R >>\nendobj\n",
    );
    let content_object = stream_object(4, "", b"q\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &content_object]);
    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        audit.graphics_state_findings,
        vec![group_finding(true, false)]
    );
    assert!(audit.coverage_gaps.is_empty());
    Ok(())
}

#[test]
fn malformed_page_group_reports_coverage_gap() -> Result<(), String> {
    let mut page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << >> ".to_vec();
    page.extend_from_slice(b"/Group 42 /Contents 4 0 R >>\nendobj\n");
    let content_object = stream_object(4, "", b"q\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &content_object]);
    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert!(audit.graphics_state_findings.is_empty());
    assert!(audit.coverage_gaps.iter().any(|gap| {
        gap.kind == CoverageGapKind::TransparencyGroupSkipped && gap.page == Some(PageIndex(0))
    }));
    Ok(())
}

#[test]
fn op_true_and_opm_one_aggregate_into_one_overprint_finding() -> Result<(), String> {
    // Two resources on one page aggregate into ONE finding; the content never
    // invokes `gs`, proving declared-in-resources presence counts.
    let audit = audit_extgstate_page("/GS0 << /OP true >> /GS1 << /OPM 1 >>")?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(
        audit.graphics_state_findings,
        vec![page_finding(true, false, false, false)]
    );
    Ok(())
}

#[test]
fn non_normal_blend_mode_and_non_opaque_alpha_report_transparency() -> Result<(), String> {
    let blend = audit_extgstate_page("/GS0 << /BM /Multiply >>")?;
    assert_eq!(
        blend.graphics_state_findings,
        vec![page_finding(false, true, false, false)]
    );

    let alpha = audit_extgstate_page("/GS0 << /CA 0.5 >>")?;
    assert_eq!(alpha.status, ColorAuditStatus::Complete);
    assert_eq!(
        alpha.graphics_state_findings,
        vec![page_finding(false, true, false, false)]
    );
    Ok(())
}

#[test]
fn unresolved_entry_is_an_unresolved_finding_not_a_gap() -> Result<(), String> {
    // `/GS0` is an indirect reference to a missing object: the env surfaces it
    // all-`Unresolved`, so the fact is a finding flag and NOT a coverage gap.
    let audit = audit_extgstate_page("/GS0 99 0 R")?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(
        audit.graphics_state_findings,
        vec![page_finding(false, false, true, false)]
    );
    Ok(())
}

#[test]
fn unclassified_keys_or_values_alone_report_an_unclassified_finding() -> Result<(), String> {
    // A harmless non-safety key (`/LW`) alone: partial classification is worth
    // surfacing, so the finding exists with ONLY `unclassified` set.
    let keys = audit_extgstate_page("/GS0 << /LW 2 >>")?;
    assert_eq!(keys.status, ColorAuditStatus::Complete);
    assert_eq!(
        keys.graphics_state_findings,
        vec![page_finding(false, false, false, true)]
    );

    // A safety key with an unclassifiable VALUE (`/op` must be a boolean).
    let values = audit_extgstate_page("/GS0 << /op 3 >>")?;
    assert_eq!(
        values.graphics_state_findings,
        vec![page_finding(false, false, false, true)]
    );
    Ok(())
}

#[test]
fn all_default_or_absent_extgstate_produces_no_finding() -> Result<(), String> {
    // Every classified parameter written with its trigger-free value.
    let defaults = audit_extgstate_page(
        "/GS0 << /op false /OP false /OPM 0 /ca 1.0 /BM /Normal /SMask /None >>",
    )?;
    assert_eq!(defaults.status, ColorAuditStatus::Complete);
    assert!(defaults.graphics_state_findings.is_empty());
    assert!(defaults.coverage_gaps.is_empty());

    // No `/ExtGState` (and no `/Resources`) at all: absence is not a finding
    // and not a gap.
    let absent = audit_color_usage(&single_page_pdf(b"", CMYK_FILL_CONTENT), 1024)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(absent.status, ColorAuditStatus::Complete);
    assert!(absent.graphics_state_findings.is_empty());
    assert!(absent.coverage_gaps.is_empty());
    Ok(())
}

#[test]
fn extgstate_inspection_failure_is_a_gap_not_a_finding() -> Result<(), String> {
    // A present but non-dictionary `/ExtGState` hides the whole scope from the
    // finding derivation: a page-anchored gap, no finding.
    let source = page_with_resources_pdf("/ExtGState [ ]", CMYK_CONTENT_NO_GS);
    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert!(audit.graphics_state_findings.is_empty());
    assert_eq!(audit.coverage_gaps.len(), 1);
    let gap = &audit.coverage_gaps[0];
    assert_eq!(gap.kind, CoverageGapKind::ExtGStateResourceSkipped);
    assert_eq!(gap.page, Some(PageIndex(0)));
    assert_eq!(gap.object, None);
    round_trip(gap)?;

    // A pass that cannot BEGIN at all yields one document-anchored
    // inspection-error gap and no findings.
    let scan = scan_document_graphics_state(b"not a pdf");
    assert!(scan.findings.is_empty());
    assert_eq!(scan.coverage_gaps.len(), 1);
    assert_eq!(
        scan.coverage_gaps[0].kind,
        CoverageGapKind::ExtGStateResourceInspectionError
    );
    assert_eq!(scan.coverage_gaps[0].page, None);
    Ok(())
}

#[test]
fn duplicate_name_shadowed_by_a_classified_entry_is_a_gap() -> Result<(), String> {
    // The first `/GS0` classifies (overprint finding); the duplicate `/GS0` is
    // dropped by the env mapping, so its state is invisible to the derivation:
    // that separate fact is a coverage gap alongside the finding.
    let audit = audit_extgstate_page("/GS0 << /OP true >> /GS0 99 0 R")?;

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert_eq!(
        audit.graphics_state_findings,
        vec![page_finding(true, false, false, false)]
    );
    assert_eq!(audit.coverage_gaps.len(), 1);
    assert_eq!(
        audit.coverage_gaps[0].kind,
        CoverageGapKind::ExtGStateResourceSkipped
    );
    assert_eq!(audit.coverage_gaps[0].page, Some(PageIndex(0)));
    Ok(())
}
