// The dependency-free serde value harness is shared with the other inventory
// tests; this module uses it only for shapes that avoid f64 colour components.
#[allow(clippy::duplicate_mod)]
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use super::form_inventory::{CATALOG, PAGES, classic_pdf, stream_object};
use crate::color_audit::build_color_usage_audit;
use crate::inventory::Inventory;
use crate::pdf::DefaultColorSpaceKind;
use crate::{
    ColorAuditStatus, ColorSpace, ColorUsageAudit, ColorUsageSummary, DefaultColorSpaceFinding,
    DefaultColorSpaceFindingSource, ObjectKind, PageColorUsage, PageIndex, PdfInventory,
    PdfInventoryPage, PdfInventoryPageResult, PdfName, audit_color_usage,
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
