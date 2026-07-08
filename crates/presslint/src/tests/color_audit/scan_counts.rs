//! Document/page counting behaviour: PDF-backed smoke tests, deterministic
//! per-page/document summaries, and spot-name collection.

use super::*;

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
    // (defensively) carries a stray spot name must be ignored. Empty
    // `spot_names` falls back to legacy `spot_name` for old observations.
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
                    multi_spot_observation(ColorSpace::DeviceN, &[b"Cut", b"Varnish", b"All"]),
                    spot_observation(ColorSpace::Separation, b"Cut"),
                    // Not Separation/DeviceN: this spot name must be dropped.
                    ColorObservation {
                        usage: ColorUsage::Fill,
                        space: ColorSpace::DeviceCmyk,
                        components: Vec::new(),
                        spot_name: Some(PdfName(b"AAAA".to_vec())),
                        spot_names: Vec::new(),
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
    assert_eq!(
        names,
        vec![&b"All"[..], b"Cut", b"Pantone 300 C", b"Varnish"]
    );
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
