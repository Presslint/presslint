use presslint_pdf::{IccProfileInspectionGap, encode_flate_stream};

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
    // A structurally valid 128-byte header with an unrecognized (`XYZ `) data
    // space, so the dictionary-level tests never incur a profile-header
    // descriptor finding and keep asserting the dictionary anomaly alone.
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"XYZ ",
        b"Lab ",
        true,
    );
    stream_object(object_number, dictionary_suffix, &header)
}

/// Build a 128-byte ICC header with the supplied fixed fields; other bytes stay
/// zero. When `acsp` is false the file-signature slot holds `junk` instead.
fn icc_header(
    size: u32,
    version: [u8; 4],
    class: &[u8],
    space: &[u8],
    pcs: &[u8],
    acsp: bool,
) -> Vec<u8> {
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&size.to_be_bytes());
    header[8..12].copy_from_slice(&version);
    header[12..16].copy_from_slice(class);
    header[16..20].copy_from_slice(space);
    header[20..24].copy_from_slice(pcs);
    header[36..40].copy_from_slice(if acsp { b"acsp" } else { b"junk" });
    header
}

/// A profile stream object `number 0` carrying `dict_suffix` in its dictionary
/// and the raw ICC `header` bytes as its stream body.
fn icc_profile_with_header(number: u32, dict_suffix: &str, header: &[u8]) -> Vec<u8> {
    stream_object(number, dict_suffix, header)
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

/// Collect only the descriptor (profile-header) findings, filtering out the
/// dictionary-level anomalies that a fixture may also carry.
fn descriptor_findings(source: &[u8]) -> Result<Vec<IccBasedFindingKind>, String> {
    let audit = audit_color_usage(source, 1024).map_err(|error| format!("{error:?}"))?;
    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    Ok(audit
        .icc_based_findings
        .into_iter()
        .map(|finding| finding.kind)
        .collect())
}

#[test]
fn component_count_mismatch_against_header_data_space() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"GRAY",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 3", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileComponentCountMismatch {
            n: 3,
            data_color_space_signature: *b"GRAY",
        }]
    );
    Ok(())
}

#[test]
fn coherent_cmyk_header_emits_no_descriptor_finding() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 4", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert!(descriptor_findings(&source)?.is_empty());
    Ok(())
}

#[test]
fn fclr_data_space_maps_to_fifteen_components() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"spac",
        b"FCLR",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 4", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileComponentCountMismatch {
            n: 4,
            data_color_space_signature: *b"FCLR",
        }]
    );
    Ok(())
}

#[test]
fn corrupt_acsp_emits_missing_signature_finding() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        false,
    );
    let profile = icc_profile_with_header(5, " /N 4", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileAcspMissing]
    );
    Ok(())
}

#[test]
fn declared_size_mismatch_emits_finding() -> Result<(), String> {
    let header = icc_header(
        999,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 4", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileDeclaredSizeMismatch {
            declared: 999,
            decoded_len: 128,
        }]
    );
    Ok(())
}

#[test]
fn disallowed_link_class_emits_finding() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"link",
        b"CMYK",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 4", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileClassDisallowed {
            profile_class_signature: *b"link",
        }]
    );
    Ok(())
}

#[test]
fn single_flate_profile_stream_parses() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        true,
    );
    let compressed = encode_flate_stream(&header, 4096).map_err(|error| format!("{error:?}"))?;
    let profile = icc_profile_with_header(5, " /N 4 /Filter /FlateDecode", &compressed);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert!(descriptor_findings(&source)?.is_empty());
    Ok(())
}

#[test]
fn truncated_profile_is_a_non_fatal_finding() -> Result<(), String> {
    let profile = icc_profile_with_header(5, " /N 4", &[0u8; 40]);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileHeaderTruncated { decoded_len: 40 }]
    );
    Ok(())
}

#[test]
fn unsupported_filter_profile_is_a_non_fatal_gap() -> Result<(), String> {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 4 /Filter /LZWDecode", &header);
    let source = page_with_color_space("/CS0 [ /ICCBased 5 0 R ]", b"q Q", &[&profile]);

    assert_eq!(
        descriptor_findings(&source)?,
        vec![IccBasedFindingKind::ProfileInspectionGap {
            reason: IccProfileInspectionGap::UnsupportedFilter,
        }]
    );
    Ok(())
}

#[test]
fn shared_profile_ref_inspects_once_but_emits_per_use_findings() -> Result<(), String> {
    // Both resources reference the same profile object; the descriptor is
    // inspected once (memoized) yet a finding is emitted per use-site anchor.
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"GRAY",
        b"Lab ",
        true,
    );
    let profile = icc_profile_with_header(5, " /N 3", &header);
    let source = page_with_color_space(
        "/CS0 [ /ICCBased 5 0 R ] /CS1 [ /ICCBased 5 0 R ]",
        b"q Q",
        &[&profile],
    );

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;
    assert_eq!(audit.status, ColorAuditStatus::Complete);
    let mismatch = IccBasedFindingKind::ProfileComponentCountMismatch {
        n: 3,
        data_color_space_signature: *b"GRAY",
    };
    assert_eq!(audit.icc_based_findings.len(), 2);
    assert_eq!(
        audit.icc_based_findings[0].resource_name,
        PdfName(b"CS0".to_vec())
    );
    assert_eq!(audit.icc_based_findings[0].kind, mismatch);
    assert_eq!(
        audit.icc_based_findings[1].resource_name,
        PdfName(b"CS1".to_vec())
    );
    assert_eq!(audit.icc_based_findings[1].kind, mismatch);
    Ok(())
}
