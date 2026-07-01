// The dependency-free serde value harness is shared verbatim with the other
// inventory tests; this focused module re-includes it rather than duplicating a
// 700-line format shim.
#[allow(clippy::duplicate_mod)]
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{from_serde_value, serde_value};

use super::{classic_pdf, single_page_pdf};
use crate::inventory::{Inventory, InventoryEntry};
use crate::preflight::build_no_rgb_report;
use crate::{
    ColorObservation, ColorSpace, ColorUsage, ContentScope, ObjectId, ObjectKind, PageIndex,
    PdfInventory, PdfInventoryError, PdfInventoryPage, PdfInventoryPageResult, PdfInventorySkip,
    PreflightCheck, PreflightFinding, PreflightReason, PreflightReport, PreflightSeverity,
    PreflightStatus, Provenance, SkippedFormInventory, SkippedFormInventoryReason,
    check_no_rgb_in_print,
};

const CMYK_FILL_CONTENT: &[u8] = b"q\n0 0 0 1 k\n12 12 80 80 re\nf\nQ";
const GRAY_FILL_CONTENT: &[u8] = b"q\n0.5 g\n12 12 80 80 re\nf\nQ";
const RGB_FILL_CONTENT: &[u8] = b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ";

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
        xobject_resource_skipped: Vec::new(),
    }
}

fn skipped_page(page: u32) -> PdfInventoryPage {
    PdfInventoryPage {
        page_index: PageIndex(page),
        result: PdfInventoryPageResult::Skipped {
            reason: PdfInventorySkip::NoContentStreams,
        },
        xobject_resource_skipped: Vec::new(),
    }
}

fn synthetic_inventory(entries: Vec<InventoryEntry>, pages: Vec<PdfInventoryPage>) -> PdfInventory {
    PdfInventory {
        byte_len: 0,
        inventory: Inventory { entries },
        xobject_resource_error: None,
        pages,
    }
}

/// Two-page classic PDF: page 0 carries `page0_content` (raw), page 1 carries a
/// content stream declared with `page1_dict_suffix` (e.g. an unsupported
/// filter) so the bridge skips it.
fn two_page_pdf(page0_content: &[u8], page1_dict_suffix: &[u8], page1_content: &[u8]) -> Vec<u8> {
    let mut content0 = Vec::new();
    content0.extend_from_slice(b"5 0 obj\n<< /Length ");
    content0.extend_from_slice(page0_content.len().to_string().as_bytes());
    content0.extend_from_slice(b" >>\nstream\n");
    content0.extend_from_slice(page0_content);
    content0.extend_from_slice(b"\nendstream\nendobj\n");

    let mut content1 = Vec::new();
    content1.extend_from_slice(b"6 0 obj\n<< /Length ");
    content1.extend_from_slice(page1_content.len().to_string().as_bytes());
    content1.extend_from_slice(page1_dict_suffix);
    content1.extend_from_slice(b" >>\nstream\n");
    content1.extend_from_slice(page1_content);
    content1.extend_from_slice(b"\nendstream\nendobj\n");

    classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n",
        &content0,
        &content1,
    ])
}

fn stream_object(number: u32, dict_extra: &str, data: &[u8]) -> Vec<u8> {
    let mut object = format!(
        "{number} 0 obj\n<< /Length {}{} >>\nstream\n",
        data.len(),
        dict_extra
    )
    .into_bytes();
    object.extend_from_slice(data);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

fn page_with_xobjects_object(xobjects: &str, contents: u32) -> Vec<u8> {
    format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << {xobjects} >> >> /Contents {contents} 0 R >>\nendobj\n"
    )
    .into_bytes()
}

fn form_xobject(number: u32, xobjects: &str, content: &[u8]) -> Vec<u8> {
    let resources = if xobjects.is_empty() {
        String::new()
    } else {
        format!(" /Resources << /XObject << {xobjects} >> >>")
    };
    stream_object(
        number,
        &format!(" /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]{resources}"),
        content,
    )
}

fn nested_form_pdf(page_content: &[u8], parent_content: &[u8], leaf_content: &[u8]) -> Vec<u8> {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", parent_content);
    let form_b = form_xobject(5, "", leaf_content);
    let page_content = stream_object(6, "", page_content);
    classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        &page,
        &form_a,
        &form_b,
        &page_content,
    ])
}

fn max_depth_form_pdf() -> Vec<u8> {
    let page = page_with_xobjects_object("/F0 4 0 R", 13);
    let mut objects = vec![
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n".to_vec(),
        page,
    ];
    for index in 0..9 {
        let object_number = 4 + index;
        let content = if index == 8 {
            b"0 0 0 1 k\n0 0 10 10 re\nf".to_vec()
        } else {
            format!("/F{} Do", index + 1).into_bytes()
        };
        let resources = if index == 8 {
            String::new()
        } else {
            format!("/F{} {} 0 R", index + 1, object_number + 1)
        };
        objects.push(form_xobject(object_number, &resources, &content));
    }
    objects.push(stream_object(13, "", b"/F0 Do"));
    let refs = objects.iter().map(Vec::as_slice).collect::<Vec<_>>();
    classic_pdf(&refs)
}

#[test]
fn device_rgb_page_fails_with_error_finding() -> Result<(), PdfInventoryError> {
    let source = single_page_pdf(b"", RGB_FILL_CONTENT);

    let report = check_no_rgb_in_print(&source, 1024)?;

    assert_eq!(report.check, PreflightCheck::NoRgbInPrint);
    assert_eq!(report.status, PreflightStatus::Fail);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.severity, PreflightSeverity::Error);
    assert_eq!(finding.reason, PreflightReason::RgbDeviceColor);
    assert_eq!(finding.page, PageIndex(0));
    assert_eq!(finding.entry_index, Some(0));
    assert_eq!(finding.kind, Some(ObjectKind::Vector));
    assert_eq!(finding.usage, Some(ColorUsage::Fill));
    assert_eq!(finding.color_space, Some(ColorSpace::DeviceRgb));
    assert_eq!(
        finding.object.as_ref(),
        Some(&report.inventory.inventory.entries[0].id)
    );
    Ok(())
}

#[test]
fn device_rgb_inside_walked_nested_form_fails() -> Result<(), PdfInventoryError> {
    let source = nested_form_pdf(
        b"0 0 0 1 k\n0 0 20 20 re\nf\n/A Do",
        b"/B Do",
        b"1 0 0 rg\n0 0 20 20 re\nf",
    );

    let report = check_no_rgb_in_print(&source, 4096)?;

    assert_eq!(report.status, PreflightStatus::Fail);
    assert!(report.findings.iter().any(|finding| {
        finding.reason == PreflightReason::RgbDeviceColor
            && finding.page == PageIndex(0)
            && finding.kind == Some(ObjectKind::Vector)
            && finding.color_space == Some(ColorSpace::DeviceRgb)
    }));
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.reason == PreflightReason::CoverageIncomplete)
    );
    Ok(())
}

#[test]
fn pure_cmyk_page_passes_with_no_findings() -> Result<(), PdfInventoryError> {
    let source = single_page_pdf(b"", CMYK_FILL_CONTENT);

    let report = check_no_rgb_in_print(&source, 1024)?;

    assert_eq!(report.status, PreflightStatus::Pass);
    assert!(report.findings.is_empty());
    assert_eq!(report.inventory.inventory.len(), 1);
    Ok(())
}

#[test]
fn cmyk_only_walked_page_and_nested_form_tree_passes() -> Result<(), PdfInventoryError> {
    let source = nested_form_pdf(
        b"0 0 0 1 k\n0 0 20 20 re\nf\n/A Do",
        b"0 0 0 1 k\n0 0 10 10 re\nf\n/B Do",
        b"0 0 0 1 k\n0 0 5 5 re\nf",
    );

    let report = check_no_rgb_in_print(&source, 4096)?;

    assert_eq!(report.status, PreflightStatus::Pass);
    assert!(report.findings.is_empty());
    Ok(())
}

#[test]
fn pure_gray_page_passes_with_no_findings() -> Result<(), PdfInventoryError> {
    // Pin the `DeviceGray` half of the `DeviceCmyk | DeviceGray => None` arm:
    // a pure DeviceGray page is pass-compatible just like the CMYK case above.
    let source = single_page_pdf(b"", GRAY_FILL_CONTENT);

    let report = check_no_rgb_in_print(&source, 1024)?;

    assert_eq!(report.status, PreflightStatus::Pass);
    assert!(report.findings.is_empty());
    assert_eq!(report.inventory.inventory.len(), 1);
    Ok(())
}

#[test]
fn unmodeled_color_space_needs_review() {
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::Separation)],
        )],
        vec![inventoried_page(0, 1)],
    );

    let report = build_no_rgb_report(inventory);

    assert_eq!(report.status, PreflightStatus::NeedsReview);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!(finding.severity, PreflightSeverity::Review);
    assert_eq!(
        finding.reason,
        PreflightReason::UnmodeledOrUnresolvedColorSpace
    );
    assert_eq!(finding.color_space, Some(ColorSpace::Separation));
    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.severity != PreflightSeverity::Error)
    );
}

#[test]
fn resource_color_space_needs_review() {
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(
                ColorUsage::Stroke,
                ColorSpace::Resource(crate::PdfName(b"CS0".to_vec())),
            )],
        )],
        vec![inventoried_page(0, 1)],
    );

    let report = build_no_rgb_report(inventory);

    assert_eq!(report.status, PreflightStatus::NeedsReview);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].reason,
        PreflightReason::UnmodeledOrUnresolvedColorSpace
    );
}

#[test]
fn skipped_page_needs_review_even_when_inventoried_content_is_cmyk() -> Result<(), PdfInventoryError>
{
    let source = two_page_pdf(
        CMYK_FILL_CONTENT,
        b" /Filter /ASCIIHexDecode",
        CMYK_FILL_CONTENT,
    );

    let report = check_no_rgb_in_print(&source, 1024)?;

    // Page 0 is pure CMYK (no finding); page 1 is skipped by the unsupported
    // filter and must surface as a coverage review, not a hard error.
    assert_eq!(report.status, PreflightStatus::NeedsReview);
    let coverage: Vec<&PreflightFinding> = report
        .findings
        .iter()
        .filter(|finding| finding.reason == PreflightReason::CoverageIncomplete)
        .collect();
    assert_eq!(coverage.len(), 1);
    assert_eq!(coverage[0].page, PageIndex(1));
    assert_eq!(coverage[0].severity, PreflightSeverity::Review);
    assert_eq!(coverage[0].object, None);
    assert_eq!(coverage[0].entry_index, None);
    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.severity != PreflightSeverity::Error)
    );
    Ok(())
}

#[test]
fn max_depth_form_skip_needs_review() -> Result<(), String> {
    let source = max_depth_form_pdf();

    let report = check_no_rgb_in_print(&source, 4096).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.status, PreflightStatus::NeedsReview);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].reason,
        PreflightReason::CoverageIncomplete
    );
    let PdfInventoryPageResult::Inventoried { form_skipped, .. } =
        &report.inventory.pages[0].result
    else {
        return Err("page should be inventoried".to_string());
    };
    assert_eq!(form_skipped.len(), 1);
    assert!(matches!(
        form_skipped[0].reason,
        SkippedFormInventoryReason::MaxDepth { max_depth: 8 }
    ));
    Ok(())
}

#[test]
fn budget_exhausted_form_skip_needs_review() {
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::DeviceCmyk)],
        )],
        vec![inventoried_page_with_form_skipped(
            0,
            1,
            vec![SkippedFormInventory {
                name: crate::PdfName(b"Fm".to_vec()),
                reference: crate::pdf::IndirectRef {
                    object_number: 9,
                    generation: 0,
                },
                object_byte_offset: 0,
                reason: SkippedFormInventoryReason::BudgetExhausted { max_expansions: 3 },
            }],
        )],
    );

    let report = build_no_rgb_report(inventory);

    assert_eq!(report.status, PreflightStatus::NeedsReview);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].reason,
        PreflightReason::CoverageIncomplete
    );
    assert_eq!(report.findings[0].page, PageIndex(0));
}

#[test]
fn image_unknown_produces_coverage_signal_but_walked_form_does_not() -> Result<(), String> {
    let source = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Length 13 >>\nstream\n/Im Do /Fm Do\nendstream\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n<< /Type /XObject /Subtype /Form /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = check_no_rgb_in_print(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.status, PreflightStatus::NeedsReview);
    assert_eq!(report.findings.len(), 1);
    // Image observation modeled as Unknown -> image coverage anchored to entry 0.
    assert_eq!(
        report.findings[0].reason,
        PreflightReason::CoverageIncomplete
    );
    assert_eq!(report.findings[0].kind, Some(ObjectKind::Image));
    assert_eq!(report.findings[0].usage, Some(ColorUsage::Image));
    assert_eq!(report.findings[0].color_space, Some(ColorSpace::Unknown));
    Ok(())
}

#[test]
fn findings_are_emitted_in_document_page_entry_order() {
    let inventory = synthetic_inventory(
        vec![
            entry(
                1,
                0,
                ObjectKind::Vector,
                vec![observation(ColorUsage::Fill, ColorSpace::DeviceRgb)],
            ),
            entry(1, 1, ObjectKind::FormXObject, Vec::new()),
        ],
        vec![skipped_page(0), inventoried_page(1, 2)],
    );

    let report = build_no_rgb_report(inventory);

    assert_eq!(report.status, PreflightStatus::Fail);
    // Skipped page 0 first, then page 1's entries in content order.
    assert_eq!(report.findings.len(), 2);
    assert_eq!(report.findings[0].page, PageIndex(0));
    assert_eq!(
        report.findings[0].reason,
        PreflightReason::CoverageIncomplete
    );
    assert_eq!(report.findings[1].reason, PreflightReason::RgbDeviceColor);
    assert_eq!(report.findings[1].entry_index, Some(0));
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
                vec![observation(ColorUsage::Stroke, ColorSpace::Separation)],
            ),
            entry(
                1,
                2,
                ObjectKind::Image,
                vec![observation(ColorUsage::Image, ColorSpace::Unknown)],
            ),
            entry(1, 3, ObjectKind::FormXObject, Vec::new()),
        ],
        vec![
            skipped_page(0),
            inventoried_page_with_form_skipped(
                1,
                4,
                vec![SkippedFormInventory {
                    name: crate::PdfName(b"Fm".to_vec()),
                    reference: crate::pdf::IndirectRef {
                        object_number: 9,
                        generation: 0,
                    },
                    object_byte_offset: 0,
                    reason: SkippedFormInventoryReason::BudgetExhausted {
                        max_expansions: 256,
                    },
                }],
            ),
        ],
    );

    let report = build_no_rgb_report(inventory);

    // The hand-built report exercises every reason and both Option shapes.
    assert_eq!(report.status, PreflightStatus::Fail);
    assert_eq!(report.findings.len(), 5);

    round_trip::<PreflightReport>(&report)?;
    round_trip(&report.check)?;
    round_trip(&report.status)?;
    for finding in &report.findings {
        round_trip::<PreflightFinding>(finding)?;
        round_trip::<PreflightSeverity>(&finding.severity)?;
        round_trip::<PreflightReason>(&finding.reason)?;
    }
    Ok(())
}
