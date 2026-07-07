use presslint::{
    ColorAuditStatus, ColorUsageAudit, ColorUsageSummary, PageColorUsage, PdfInventory,
    PdfInventoryPage, PdfInventoryPageResult, inventory::Inventory, pdf::IndirectRef,
};
use presslint_types::PageIndex;
use presslint_write::{
    ConvertContentColorsOutput, ConvertPageSkip, ConvertPageSkipReason, ConvertedPage,
    LinkConversionCounts, OperatorSkipCounts,
};
use serde_json::Value;

use crate::report::{RunReport, convert_warnings, render_audit_human, render_convert_human};

#[test]
fn convert_report_warns_on_zero_conversion_and_skips() {
    let output = ConvertContentColorsOutput {
        bytes: b"%PDF".to_vec(),
        converted: vec![ConvertedPage {
            page_index: PageIndex(0),
            content_objects: vec![IndirectRef {
                object_number: 4,
                generation: 0,
            }],
            operators_converted: 0,
            black_preserved: 0,
            operator_skips: OperatorSkipCounts {
                no_matching_link: 2,
                selector_excluded: 1,
                ..OperatorSkipCounts::default()
            },
            links: Vec::<LinkConversionCounts>::new(),
        }],
        skipped: vec![ConvertPageSkip {
            page_index: PageIndex(2),
            content_object: None,
            reason: ConvertPageSkipReason::NoContentStream,
        }],
    };

    let warnings = convert_warnings(&output);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("zero operators converted"))
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("coverage gaps or skips"))
    );
}

#[test]
fn human_convert_report_surfaces_page_coverage_counts() {
    let output = ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![ConvertedPage {
            page_index: PageIndex(1),
            content_objects: vec![IndirectRef {
                object_number: 7,
                generation: 0,
            }],
            operators_converted: 3,
            black_preserved: 1,
            operator_skips: OperatorSkipCounts {
                no_matching_link: 4,
                selector_excluded: 2,
                ..OperatorSkipCounts::default()
            },
            links: Vec::<LinkConversionCounts>::new(),
        }],
        skipped: Vec::new(),
    };
    let mut rendered = Vec::new();

    render_convert_human(&mut rendered, &output, &[]).unwrap();
    let rendered = String::from_utf8(rendered).unwrap();

    assert!(rendered.contains("operators converted: 3"));
    assert!(rendered.contains("black preserved: 1"));
    assert!(rendered.contains("no matching link: 4"));
    assert!(rendered.contains("selector excluded: 2"));
    assert!(rendered.contains("page 2: converted=3 black_preserved=1"));
}

#[test]
fn json_convert_report_wraps_library_output_and_omits_pdf_bytes() {
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: b"%PDF bytes must not appear in JSON".to_vec(),
        converted: vec![ConvertedPage {
            page_index: PageIndex(0),
            content_objects: vec![IndirectRef {
                object_number: 4,
                generation: 0,
            }],
            operators_converted: 2,
            black_preserved: 1,
            operator_skips: OperatorSkipCounts::default(),
            links: Vec::<LinkConversionCounts>::new(),
        }],
        skipped: Vec::new(),
    });

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let result = &rendered["result"];

    assert_eq!(rendered["command"], "convert");
    assert_eq!(result["kind"], "convert");
    assert!(result.get("library_output").is_some());
    assert!(result["library_output"].get("converted").is_some());
    assert!(result["library_output"].get("skipped").is_some());
    assert!(result["library_output"].get("bytes").is_none());
    let converted = &result["library_output"]["converted"][0];
    assert!(converted.get("content_objects").is_some());
    assert!(converted.get("content_object").is_none());
}

#[test]
fn json_audit_report_wraps_library_output_and_reports_decode_cap() {
    let report = RunReport::audit(synthetic_audit(), 512 * 1024 * 1024);

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let result = &rendered["result"];

    assert_eq!(rendered["command"], "audit");
    assert_eq!(result["kind"], "audit");
    assert_eq!(result["max_decoded_stream_bytes"], 512 * 1024 * 1024);
    assert_eq!(result["library_output"]["status"], "complete");
    assert!(result["library_output"].get("inventory").is_some());
}

#[test]
fn human_audit_report_surfaces_default_color_space_count() {
    let audit = synthetic_audit();
    let mut rendered = Vec::new();

    render_audit_human(&mut rendered, &audit, 512 * 1024 * 1024, &[]).unwrap();
    let rendered = String::from_utf8(rendered).unwrap();

    assert!(rendered.contains("default color-space findings: 0"));
}

fn synthetic_audit() -> ColorUsageAudit {
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
