// The dependency-free serde value harness is shared with the other inventory
// tests; this module uses it only for shapes that avoid f64 colour components.
#[allow(clippy::duplicate_mod)]
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use super::form_inventory::{
    CATALOG, PAGES, classic_pdf, page_with_xobjects_object, stream_object,
};
use crate::color_audit::build_color_usage_audit;
use crate::inventory::Inventory;
use crate::pdf::DefaultColorSpaceKind;
use crate::{
    ColorAuditStatus, ColorSpace, ColorUsageAudit, ColorUsageSummary, DefaultColorSpaceFinding,
    DefaultColorSpaceFindingSource, InvocationFrame, InvocationPath, ObjectKind, PageColorUsage,
    PageIndex, PdfInventory, PdfInventoryPage, PdfInventoryPageResult, PdfName, audit_color_usage,
};

const RGB_FILL: &[u8] = b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ";
const CMYK_FILL: &[u8] = b"q\n0 0 0 1 k\n12 12 80 80 re\nf\nQ";

fn round_trip<T>(value: &T) -> Result<(), String>
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_value(value).map_err(|error| error.to_string())?;
    let decoded: T = from_serde_value(encoded).map_err(|error| error.to_string())?;
    assert_eq!(&decoded, value);
    Ok(())
}

fn page_with_color_space_resources(color_space_entries: &str, content: &[u8]) -> Vec<u8> {
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << {color_space_entries} >> >> /Contents 4 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content_object = stream_object(4, "", content);
    let icc_profile = stream_object(5, " /N 3", b"");
    classic_pdf(&[CATALOG, PAGES, &page, &content_object, &icc_profile])
}

fn page_without_color_space_resources(content: &[u8]) -> Vec<u8> {
    let page =
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << >> /Contents 4 0 R >>\nendobj\n";
    let content_object = stream_object(4, "", content);
    classic_pdf(&[CATALOG, PAGES, page, &content_object])
}

fn form_xobject_with_resources(number: u32, resources: &str, content: &[u8]) -> Vec<u8> {
    stream_object(
        number,
        &format!(
            " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << {resources} >>"
        ),
        content,
    )
}

fn invocation_path(frames: &[(u32, &[u8])]) -> InvocationPath {
    InvocationPath {
        frames: frames
            .iter()
            .map(|(ordinal, name)| InvocationFrame {
                ordinal: *ordinal,
                name: PdfName((*name).to_vec()),
            })
            .collect(),
    }
}

fn empty_audit() -> ColorUsageAudit {
    ColorUsageAudit {
        status: ColorAuditStatus::Complete,
        document: empty_summary(),
        pages: vec![PageColorUsage {
            page: PageIndex(0),
            summary: empty_summary(),
        }],
        spot_names: Vec::new(),
        rgb_findings: Vec::new(),
        graphics_state_findings: Vec::new(),
        default_color_space_findings: Vec::new(),
        output_intent_eligibility: None,
        coverage_gaps: Vec::new(),
        inventory: PdfInventory {
            byte_len: 0,
            inventory: Inventory {
                entries: Vec::new(),
            },
            xobject_resource_error: None,
            color_space_resource_error: None,
            pages: vec![PdfInventoryPage {
                page_index: PageIndex(0),
                result: PdfInventoryPageResult::Inventoried {
                    entry_count: 0,
                    form_skipped: Vec::new(),
                },
                image_xobjects: Vec::new(),
                xobject_resource_skipped: Vec::new(),
                color_space_resource_skipped: Vec::new(),
            }],
        },
    }
}

fn empty_summary() -> ColorUsageSummary {
    ColorUsageSummary {
        color_space_counts: Vec::new(),
        color_usage_counts: Vec::new(),
        object_kind_counts: Vec::new(),
    }
}

#[test]
fn non_trivial_default_rgb_with_matching_device_rgb_emits_one_finding() -> Result<(), String> {
    let source = page_with_color_space_resources("/DefaultRGB [ /ICCBased 5 0 R ]", RGB_FILL);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert_eq!(audit.rgb_findings.len(), 1);
    assert_eq!(
        audit.default_color_space_findings,
        vec![DefaultColorSpaceFinding {
            page: PageIndex(0),
            source: DefaultColorSpaceFindingSource::PageDefaultColorSpace,
            default: DefaultColorSpaceKind::DefaultRgb,
            device_space: ColorSpace::DeviceRgb,
            replacement_space: ColorSpace::IccBased,
            replacement_component_count: Some(3),
            replacement_spot_names: Vec::new(),
            matching_device_observation_count: 1,
            form_name: None,
            invocation: None,
        }]
    );
    assert_eq!(
        audit.inventory.inventory.entries[0].kind,
        ObjectKind::Vector
    );
    assert_eq!(
        audit.inventory.inventory.entries[0].colors[0].space,
        ColorSpace::DeviceRgb
    );
    Ok(())
}

#[test]
fn no_default_emits_no_default_color_space_finding() -> Result<(), String> {
    let source = page_without_color_space_resources(RGB_FILL);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.default_color_space_findings.is_empty());
    assert!(audit.coverage_gaps.is_empty());
    Ok(())
}

#[test]
fn trivial_same_family_default_emits_no_finding() -> Result<(), String> {
    let source = page_with_color_space_resources("/DefaultCMYK /DeviceCMYK", CMYK_FILL);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.default_color_space_findings.is_empty());
    assert!(audit.coverage_gaps.is_empty());
    Ok(())
}

#[test]
fn non_trivial_default_without_matching_device_observation_emits_no_finding() -> Result<(), String>
{
    let source = page_with_color_space_resources("/DefaultRGB [ /ICCBased 5 0 R ]", CMYK_FILL);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.default_color_space_findings.is_empty());
    assert!(audit.coverage_gaps.is_empty());
    Ok(())
}

#[test]
fn indexed_default_rgb_reports_indexed_replacement_without_conversion() -> Result<(), String> {
    let source = page_with_color_space_resources(
        "/DefaultRGB [ /Indexed /DeviceRGB 255 <000102> ]",
        RGB_FILL,
    );

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.coverage_gaps.is_empty());
    assert_eq!(
        audit.default_color_space_findings,
        vec![DefaultColorSpaceFinding {
            page: PageIndex(0),
            source: DefaultColorSpaceFindingSource::PageDefaultColorSpace,
            default: DefaultColorSpaceKind::DefaultRgb,
            device_space: ColorSpace::DeviceRgb,
            replacement_space: ColorSpace::Indexed,
            replacement_component_count: Some(1),
            replacement_spot_names: Vec::new(),
            matching_device_observation_count: 1,
            form_name: None,
            invocation: None,
        }]
    );
    // The observation keeps its original device family and operands: the
    // Indexed default is REPORTED, never applied or converted.
    assert_eq!(
        audit.inventory.inventory.entries[0].colors[0].space,
        ColorSpace::DeviceRgb
    );
    assert_eq!(
        audit.inventory.inventory.entries[0].colors[0].components,
        vec![0.0, 0.0, 1.0]
    );
    Ok(())
}

#[test]
fn malformed_present_default_is_coverage_gap_not_finding() -> Result<(), String> {
    let source = page_with_color_space_resources("/DefaultRGB 7", RGB_FILL);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert!(audit.default_color_space_findings.is_empty());
    assert_eq!(audit.coverage_gaps.len(), 1);
    assert_eq!(
        audit.coverage_gaps[0].kind,
        crate::CoverageGapKind::DefaultColorSpaceSkipped
    );
    assert_eq!(audit.coverage_gaps[0].page, Some(PageIndex(0)));
    Ok(())
}

#[test]
fn malformed_present_default_without_device_observation_is_coverage_gap() -> Result<(), String> {
    let source = page_with_color_space_resources("/DefaultRGB 7", b"");

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert!(audit.document.color_space_counts.is_empty());
    assert!(audit.default_color_space_findings.is_empty());
    assert_eq!(audit.coverage_gaps.len(), 1);
    assert_eq!(
        audit.coverage_gaps[0].kind,
        crate::CoverageGapKind::DefaultColorSpaceSkipped
    );
    assert_eq!(audit.coverage_gaps[0].page, Some(PageIndex(0)));
    Ok(())
}

#[test]
fn default_color_space_finding_serde_shape_is_pinned() -> Result<(), String> {
    let finding = DefaultColorSpaceFinding {
        page: PageIndex(2),
        source: DefaultColorSpaceFindingSource::PageDefaultColorSpace,
        default: DefaultColorSpaceKind::DefaultCmyk,
        device_space: ColorSpace::DeviceCmyk,
        replacement_space: ColorSpace::DeviceN,
        replacement_component_count: Some(2),
        replacement_spot_names: vec![PdfName(b"Cyan".to_vec()), PdfName(b"Spot".to_vec())],
        matching_device_observation_count: 4,
        form_name: None,
        invocation: None,
    };

    let value = serde_value(&finding).map_err(|error| error.to_string())?;
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(2)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_default_color_space".to_string()),
            ),
            (
                "default".to_string(),
                TestSerdeValue::String("default_cmyk".to_string()),
            ),
            (
                "device_space".to_string(),
                TestSerdeValue::String("device_cmyk".to_string()),
            ),
            (
                "replacement_space".to_string(),
                TestSerdeValue::String("device_n".to_string()),
            ),
            (
                "replacement_component_count".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::U64(2))),
            ),
            (
                "replacement_spot_names".to_string(),
                TestSerdeValue::Seq(vec![
                    TestSerdeValue::Seq(vec![
                        TestSerdeValue::U64(u64::from(b'C')),
                        TestSerdeValue::U64(u64::from(b'y')),
                        TestSerdeValue::U64(u64::from(b'a')),
                        TestSerdeValue::U64(u64::from(b'n')),
                    ]),
                    TestSerdeValue::Seq(vec![
                        TestSerdeValue::U64(u64::from(b'S')),
                        TestSerdeValue::U64(u64::from(b'p')),
                        TestSerdeValue::U64(u64::from(b'o')),
                        TestSerdeValue::U64(u64::from(b't')),
                    ]),
                ]),
            ),
            (
                "matching_device_observation_count".to_string(),
                TestSerdeValue::U64(4),
            ),
        ])
    );
    round_trip(&finding)?;
    assert_eq!(
        serde_value(&crate::CoverageGapKind::DefaultColorSpaceInspectionError)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("default_color_space_inspection_error".to_string())
    );
    assert_eq!(
        serde_value(&crate::CoverageGapKind::DefaultColorSpaceSkipped)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("default_color_space_skipped".to_string())
    );
    Ok(())
}

#[test]
fn audit_without_default_findings_omits_field_and_old_json_deserializes() -> Result<(), String> {
    let audit = build_color_usage_audit(empty_audit().inventory);
    assert!(audit.default_color_space_findings.is_empty());

    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    assert!(
        fields
            .iter()
            .all(|(key, _)| key != "default_color_space_findings")
    );

    let decoded: ColorUsageAudit = from_serde_value(value).map_err(|error| error.to_string())?;
    assert!(decoded.default_color_space_findings.is_empty());
    assert_eq!(decoded, audit);
    Ok(())
}

#[test]
fn root_form_default_rgb_with_matching_form_rgb_emits_form_finding() -> Result<(), String> {
    let page = page_with_xobjects_object("/Fm 4 0 R", 5);
    let form = form_xobject_with_resources(
        4,
        "/ColorSpace << /DefaultRGB [ /ICCBased 6 0 R ] >>",
        RGB_FILL,
    );
    let page_content = stream_object(5, "", b"/Fm Do");
    let icc_profile = stream_object(6, " /N 3", b"");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content, &icc_profile]);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert_eq!(
        audit.default_color_space_findings,
        vec![DefaultColorSpaceFinding {
            page: PageIndex(0),
            source: DefaultColorSpaceFindingSource::FormDefaultColorSpace,
            default: DefaultColorSpaceKind::DefaultRgb,
            device_space: ColorSpace::DeviceRgb,
            replacement_space: ColorSpace::IccBased,
            replacement_component_count: Some(3),
            replacement_spot_names: Vec::new(),
            matching_device_observation_count: 1,
            form_name: Some(PdfName(b"Fm".to_vec())),
            invocation: Some(invocation_path(&[(0, b"Fm")])),
        }]
    );
    let Some(form_entry) = audit
        .inventory
        .inventory
        .entries
        .iter()
        .find(|entry| entry.provenance.invocation.is_some())
    else {
        return Err("missing form entry".to_string());
    };
    assert_eq!(form_entry.colors[0].space, ColorSpace::DeviceRgb);
    Ok(())
}

#[test]
fn page_direct_rgb_does_not_trigger_root_form_default_finding() -> Result<(), String> {
    let page = page_with_xobjects_object("/Fm 4 0 R", 5);
    let form = form_xobject_with_resources(
        4,
        "/ColorSpace << /DefaultRGB [ /ICCBased 6 0 R ] >>",
        CMYK_FILL,
    );
    let page_content = stream_object(5, "", b"0 0 1 rg\n1 1 10 10 re\nf\n/Fm Do");
    let icc_profile = stream_object(6, " /N 3", b"");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content, &icc_profile]);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert_eq!(audit.rgb_findings.len(), 1);
    assert!(audit.default_color_space_findings.is_empty());
    Ok(())
}

#[test]
fn shared_root_form_invoked_twice_emits_one_finding_per_invocation() -> Result<(), String> {
    let page = page_with_xobjects_object("/Fm 4 0 R", 5);
    let form = form_xobject_with_resources(
        4,
        "/ColorSpace << /DefaultRGB [ /ICCBased 6 0 R ] >>",
        RGB_FILL,
    );
    let page_content = stream_object(5, "", b"/Fm Do\n/Fm Do");
    let icc_profile = stream_object(6, " /N 3", b"");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content, &icc_profile]);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.default_color_space_findings.len(), 2);
    assert_eq!(
        audit.default_color_space_findings[0].invocation,
        Some(invocation_path(&[(0, b"Fm")]))
    );
    assert_eq!(
        audit.default_color_space_findings[1].invocation,
        Some(invocation_path(&[(1, b"Fm")]))
    );
    assert!(
        audit
            .default_color_space_findings
            .iter()
            .all(|finding| finding.matching_device_observation_count == 1)
    );
    Ok(())
}

#[test]
fn malformed_present_root_form_default_is_coverage_gap_without_matching_observation()
-> Result<(), String> {
    let page = page_with_xobjects_object("/Fm 4 0 R", 5);
    let form = form_xobject_with_resources(4, "/ColorSpace << /DefaultRGB 7 >>", b"");
    let page_content = stream_object(5, "", b"/Fm Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content]);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert!(audit.default_color_space_findings.is_empty());
    assert_eq!(audit.coverage_gaps.len(), 1);
    assert_eq!(
        audit.coverage_gaps[0].kind,
        crate::CoverageGapKind::DefaultColorSpaceSkipped
    );
    assert_eq!(audit.coverage_gaps[0].page, Some(PageIndex(0)));
    Ok(())
}

#[test]
fn nested_form_default_is_not_attributed_in_root_form_slice() -> Result<(), String> {
    let page = page_with_xobjects_object("/A 4 0 R", 7);
    let form_a = form_xobject_with_resources(4, "/XObject << /B 5 0 R >>", b"/B Do");
    let form_b = form_xobject_with_resources(
        5,
        "/ColorSpace << /DefaultRGB [ /ICCBased 6 0 R ] >>",
        RGB_FILL,
    );
    let icc_profile = stream_object(6, " /N 3", b"");
    let page_content = stream_object(7, "", b"/A Do");
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        &page,
        &form_a,
        &form_b,
        &icc_profile,
        &page_content,
    ]);

    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.status, ColorAuditStatus::Complete);
    assert!(audit.default_color_space_findings.is_empty());
    assert!(audit.inventory.inventory.entries.iter().any(|entry| {
        entry.provenance.invocation == Some(invocation_path(&[(0, b"A"), (0, b"B")]))
            && entry
                .colors
                .iter()
                .any(|color| color.space == ColorSpace::DeviceRgb)
    }));
    Ok(())
}

#[test]
fn form_default_color_space_finding_serde_shape_is_pinned() -> Result<(), String> {
    let finding = DefaultColorSpaceFinding {
        page: PageIndex(0),
        source: DefaultColorSpaceFindingSource::FormDefaultColorSpace,
        default: DefaultColorSpaceKind::DefaultRgb,
        device_space: ColorSpace::DeviceRgb,
        replacement_space: ColorSpace::IccBased,
        replacement_component_count: Some(3),
        replacement_spot_names: Vec::new(),
        matching_device_observation_count: 1,
        form_name: Some(PdfName(b"Fm".to_vec())),
        invocation: Some(invocation_path(&[(0, b"Fm")])),
    };

    let value = serde_value(&finding).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("finding should serialize as a map".to_string());
    };
    assert!(fields.iter().any(|(key, value)| {
        key == "source" && value == &TestSerdeValue::String("form_default_color_space".to_string())
    }));
    assert!(fields.iter().any(|(key, _)| key == "form_name"));
    assert!(fields.iter().any(|(key, _)| key == "invocation"));
    round_trip(&finding)?;
    Ok(())
}
