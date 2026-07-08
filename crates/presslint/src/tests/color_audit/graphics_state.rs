//! Graphics-state findings (page-scope declared `/ExtGState` state and page
//! transparency groups): finding aggregation, default/absent behaviour, and
//! the skip-versus-gap boundary.

use super::*;

const CMYK_CONTENT_NO_GS: &[u8] = b"0 0 0 1 k\n12 12 80 80 re\nf";

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
