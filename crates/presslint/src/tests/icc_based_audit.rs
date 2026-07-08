use crate::{
    ColorAuditStatus, ColorSpace, IccBasedFinding, IccBasedFindingKind, IccBasedFindingSource,
    PageIndex, PdfName, audit_color_usage,
};

use super::form_inventory::{CATALOG, PAGES, classic_pdf, stream_object};

fn page_with_color_space(color_space_entries: &str, content: &[u8], profiles: &[&[u8]]) -> Vec<u8> {
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << {color_space_entries} >> >> /Contents 4 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content_object = stream_object(4, "", content);
    let mut objects = vec![CATALOG, PAGES, &page, &content_object];
    objects.extend_from_slice(profiles);
    classic_pdf(&objects)
}

fn icc_profile(object_number: u32, dictionary_suffix: &str) -> Vec<u8> {
    stream_object(object_number, dictionary_suffix, b"x")
}

fn first_icc_finding(source: &[u8]) -> Result<IccBasedFinding, String> {
    let audit = audit_color_usage(source, 1024).map_err(|error| format!("{error:?}"))?;
    audit
        .icc_based_findings
        .into_iter()
        .next()
        .ok_or_else(|| "expected one ICCBased finding".to_string())
}

#[test]
fn divergent_direct_range_emits_finding_without_status_gap() -> Result<(), String> {
    let profile = icc_profile(5, " /N 4 /Range [ 0 1 0 1 0 1 ]");
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ]",
        b"/CS0 cs 0 0 0 1 scn 0 0 50 50 re f",
        &[&profile],
    );

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(audit.icc_based_findings.len(), 1);
    assert_eq!(
        audit.inventory.inventory.entries[0].colors[0].space,
        ColorSpace::IccBased
    );
    assert_eq!(
        audit.icc_based_findings[0].kind,
        IccBasedFindingKind::RangeArityMismatch {
            expected: 8,
            got: 6,
        }
    );
    Ok(())
}

#[test]
fn device_alternate_component_mismatch_emits_finding() -> Result<(), String> {
    let profile = icc_profile(5, " /N 3 /Alternate /DeviceCMYK");
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ]",
        b"/CS0 cs 0 0 0 scn 0 0 50 50 re f",
        &[&profile],
    );

    let finding = first_icc_finding(&source)?;

    assert_eq!(finding.page, PageIndex(0));
    assert_eq!(
        finding.source,
        IccBasedFindingSource::PageColorSpaceResource
    );
    assert_eq!(finding.resource_name, PdfName(b"CS0".to_vec()));
    assert_eq!(
        finding.profile_stream,
        Some(crate::pdf::IndirectRef {
            object_number: 5,
            generation: 0
        })
    );
    assert_eq!(finding.n, Some(3));
    assert_eq!(finding.alternate_space, Some(ColorSpace::DeviceCmyk));
    assert_eq!(
        finding.kind,
        IccBasedFindingKind::AlternateComponentMismatch {
            n: 3,
            alternate_implied: 4,
        }
    );
    Ok(())
}

#[test]
fn coherent_device_alternate_emits_no_finding() -> Result<(), String> {
    let profile = icc_profile(5, " /N 3 /Alternate /DeviceRGB");
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ]",
        b"/CS0 cs 0 0 0 scn 0 0 50 50 re f",
        &[&profile],
    );

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.icc_based_findings.is_empty());
    Ok(())
}

#[test]
fn missing_n_emits_missing_or_malformed_n_without_status_gap() -> Result<(), String> {
    let profile = icc_profile(5, "");
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ]",
        b"/CS0 cs 0 scn 0 0 50 50 re f",
        &[&profile],
    );

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(audit.icc_based_findings.len(), 1);
    assert_eq!(
        audit.icc_based_findings[0].kind,
        IccBasedFindingKind::MissingOrMalformedN
    );
    Ok(())
}

#[test]
fn unclassified_alternate_emits_finding() -> Result<(), String> {
    let profile = icc_profile(5, " /N 3 /Alternate /CalRGB");
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ]",
        b"/CS0 cs 0 0 0 scn 0 0 50 50 re f",
        &[&profile],
    );

    let finding = first_icc_finding(&source)?;

    assert_eq!(finding.kind, IccBasedFindingKind::AlternateUnclassified);
    assert_eq!(finding.alternate_space, None);
    Ok(())
}

#[test]
fn default_scope_divergent_range_emits_default_attribution() -> Result<(), String> {
    let profile = icc_profile(5, " /N 4 /Range [ 0 1 0 1 0 1 ]");
    let source = page_with_color_space(
        "/DefaultCMYK [ /ICCBased 5 0 R ]",
        b"0 0 0 1 k 0 0 50 50 re f",
        &[&profile],
    );

    let finding = first_icc_finding(&source)?;

    assert_eq!(finding.source, IccBasedFindingSource::DefaultColorSpace);
    assert_eq!(finding.resource_name, PdfName(b"DefaultCMYK".to_vec()));
    assert_eq!(
        finding.kind,
        IccBasedFindingKind::RangeArityMismatch {
            expected: 8,
            got: 6,
        }
    );
    Ok(())
}
