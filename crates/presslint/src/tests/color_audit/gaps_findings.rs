//! Coverage-gap and RGB-finding classification over synthetic inventories:
//! which skips count as gaps, which observations become findings, and the
//! deterministic gap ordering.

use super::*;

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
