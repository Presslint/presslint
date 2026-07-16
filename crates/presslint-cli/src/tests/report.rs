use presslint::{
    ColorAuditStatus, ColorUsageAudit, ColorUsageSummary, PageColorUsage, PdfInventory,
    PdfInventoryPage, PdfInventoryPageResult, inventory::Inventory, pdf::IndirectRef,
};
use presslint_types::PageIndex;
use presslint_write::{
    ConvertContentColorsOutput, ConvertPageSkip, ConvertPageSkipReason, ConvertedPage,
    FormXObjectRefusalCounts, LinkConversionCounts, OperatorSkipCounts,
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
            resource_alias_setters_eligible: 0,
            resource_alias_setters_ineligible: 0,
            resource_alias_candidates_converted: 0,
            resource_alias_candidates_refused: 0,
            operator_skips: OperatorSkipCounts {
                no_matching_link: 2,
                selector_excluded: 1,
                ..OperatorSkipCounts::default()
            },
            form_xobject_refusal_counts: FormXObjectRefusalCounts::default(),
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
            resource_alias_setters_eligible: 5,
            resource_alias_setters_ineligible: 2,
            resource_alias_candidates_converted: 4,
            resource_alias_candidates_refused: 3,
            operator_skips: OperatorSkipCounts {
                no_matching_link: 4,
                selector_excluded: 2,
                default_color_space_unsafe: 3,
                ..OperatorSkipCounts::default()
            },
            form_xobject_refusal_counts: FormXObjectRefusalCounts::default(),
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
    assert!(rendered.contains("alias setters eligible: 5"));
    assert!(rendered.contains("alias setters ineligible: 2"));
    assert!(rendered.contains("alias candidates converted: 4"));
    assert!(rendered.contains("alias candidates refused: 3"));
    assert!(rendered.contains("default color-space unsafe: 3"));
    assert!(rendered.contains(
        "page 2: converted=3 black_preserved=1 no_matching_link=4 selector_excluded=2 alias_eligible=5 alias_ineligible=2 alias_converted=4 alias_refused=3 default_unsafe=3"
    ));
}

#[test]
fn default_color_space_unsafe_is_a_deterministic_coverage_warning() {
    let output = ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![converted_page_with_counts(0, 0, 2)],
        skipped: Vec::new(),
    };

    assert_eq!(
        convert_warnings(&output),
        vec![
            "coverage gaps or skips observed: no_matching_link=0 selector_excluded=0 invalid_operands=0 alias_candidates_refused=0 default_color_space_unsafe=2 skipped_pages=0"
                .to_owned()
        ]
    );
}

#[test]
fn alias_candidate_refusal_is_a_deterministic_coverage_warning() {
    let mut page = converted_page_with_counts(0, 0, 0);
    page.resource_alias_candidates_refused = 2;
    let output = ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![page],
        skipped: Vec::new(),
    };

    assert_eq!(
        convert_warnings(&output),
        vec![
            "coverage gaps or skips observed: no_matching_link=0 selector_excluded=0 invalid_operands=0 alias_candidates_refused=2 default_color_space_unsafe=0 skipped_pages=0"
                .to_owned()
        ]
    );
}

#[test]
fn human_convert_report_renders_zero_alias_and_default_totals_deterministically() {
    let output = ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![converted_page_with_counts(0, 0, 0)],
        skipped: Vec::new(),
    };
    let mut rendered = Vec::new();

    render_convert_human(&mut rendered, &output, &[]).unwrap();
    let rendered = String::from_utf8(rendered).unwrap();

    assert!(rendered.contains("alias setters eligible: 0"));
    assert!(rendered.contains("alias setters ineligible: 0"));
    assert!(rendered.contains("alias candidates converted: 0"));
    assert!(rendered.contains("alias candidates refused: 0"));
    assert!(rendered.contains("default color-space unsafe: 0"));
    assert!(rendered.contains(
        "alias_eligible=0 alias_ineligible=0 alias_converted=0 alias_refused=0 default_unsafe=0"
    ));
}

#[test]
fn json_convert_report_wraps_library_output_and_omits_pdf_bytes() {
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: b"%PDF bytes must not appear in JSON".to_vec(),
        converted: vec![converted_page_with_counts(0, 0, 0)],
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
fn json_convert_report_omits_zero_alias_and_default_counts() {
    // Additive counts at zero must not change the existing JSON shape.
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![converted_page_with_counts(0, 0, 0)],
        skipped: Vec::new(),
    });

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let converted = &rendered["result"]["library_output"]["converted"][0];

    assert!(converted.get("resource_alias_setters_eligible").is_none());
    assert!(converted.get("resource_alias_setters_ineligible").is_none());
    assert!(
        converted
            .get("resource_alias_candidates_converted")
            .is_none()
    );
    assert!(converted.get("resource_alias_candidates_refused").is_none());
    assert!(
        converted["operator_skips"]
            .get("default_color_space_unsafe")
            .is_none()
    );
    // The pre-existing skip counts keep their always-present shape.
    assert_eq!(converted["operator_skips"]["no_matching_link"], 0);
}

#[test]
fn json_convert_report_serializes_nonzero_alias_and_default_counts() {
    let mut page = converted_page_with_counts(5, 2, 3);
    page.resource_alias_candidates_converted = 4;
    page.resource_alias_candidates_refused = 1;
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![page],
        skipped: Vec::new(),
    });

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let converted = &rendered["result"]["library_output"]["converted"][0];

    assert_eq!(converted["resource_alias_setters_eligible"], 5);
    assert_eq!(converted["resource_alias_setters_ineligible"], 2);
    assert_eq!(converted["resource_alias_candidates_converted"], 4);
    assert_eq!(converted["resource_alias_candidates_refused"], 1);
    assert_eq!(converted["operator_skips"]["default_color_space_unsafe"], 3);
}

#[test]
fn json_convert_report_omits_form_xobject_refusal_counts_when_empty() {
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![converted_page_with_counts(0, 0, 0)],
        skipped: Vec::new(),
    });

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let converted = &rendered["result"]["library_output"]["converted"][0];

    assert!(converted.get("form_xobject_refusal_counts").is_none());
}

#[test]
fn json_convert_report_serializes_nonzero_form_xobject_refusal_counts() {
    let mut page = converted_page_with_counts(0, 0, 0);
    page.form_xobject_refusal_counts.raw_grammar = 2;
    page.form_xobject_refusal_counts.recursion_cycle = 1;
    let report = RunReport::convert(ConvertContentColorsOutput {
        bytes: Vec::new(),
        converted: vec![page],
        skipped: Vec::new(),
    });

    let rendered: Value = serde_json::from_str(&report.to_json_string().unwrap()).unwrap();
    let converted = &rendered["result"]["library_output"]["converted"][0];

    assert_eq!(converted["form_xobject_refusal_counts"]["raw_grammar"], 2);
    assert_eq!(
        converted["form_xobject_refusal_counts"]["recursion_cycle"],
        1
    );
    assert!(
        converted["form_xobject_refusal_counts"]
            .get("structural_preflight")
            .is_none()
    );
}

#[test]
fn older_converted_page_json_defaults_form_xobject_refusal_counts_to_empty() {
    let page = converted_page_with_counts(0, 0, 0);
    let value = serde_json::to_value(page).unwrap();
    assert!(value.get("form_xobject_refusal_counts").is_none());

    let restored: ConvertedPage = serde_json::from_value(value).unwrap();
    assert_eq!(
        restored.form_xobject_refusal_counts,
        FormXObjectRefusalCounts::default()
    );
}

#[test]
fn older_converted_page_json_defaults_alias_candidate_counts_to_zero() {
    let page = converted_page_with_counts(0, 0, 0);
    let mut value = serde_json::to_value(page).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("resource_alias_candidates_converted");
    object.remove("resource_alias_candidates_refused");

    let restored: ConvertedPage = serde_json::from_value(value).unwrap();

    assert_eq!(restored.resource_alias_candidates_converted, 0);
    assert_eq!(restored.resource_alias_candidates_refused, 0);
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
    assert!(rendered.contains("icc-based findings: 0"));
}

/// Public struct-literal shape lock for the additive per-page counts.
fn converted_page_with_counts(
    alias_eligible: usize,
    alias_ineligible: usize,
    default_unsafe: usize,
) -> ConvertedPage {
    ConvertedPage {
        page_index: PageIndex(0),
        content_objects: vec![IndirectRef {
            object_number: 4,
            generation: 0,
        }],
        operators_converted: 2,
        black_preserved: 1,
        resource_alias_setters_eligible: alias_eligible,
        resource_alias_setters_ineligible: alias_ineligible,
        resource_alias_candidates_converted: 0,
        resource_alias_candidates_refused: 0,
        operator_skips: OperatorSkipCounts {
            default_color_space_unsafe: default_unsafe,
            ..OperatorSkipCounts::default()
        },
        form_xobject_refusal_counts: FormXObjectRefusalCounts::default(),
        links: Vec::<LinkConversionCounts>::new(),
    }
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
        icc_based_findings: Vec::new(),
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
