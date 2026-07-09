//! Serde round-trip and JSON-shape locks for the audit report, the pinned
//! graphics-state finding shape, and the optional output-intent eligibility
//! field.

use super::*;

fn output_intent_pdf(identifier: &str) -> Vec<u8> {
    let catalog = format!(
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ << /S /GTS_PDFX /OutputConditionIdentifier {identifier} >> ] >>\nendobj\n"
    )
    .into_bytes();
    let page =
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << >> /Contents 4 0 R >>\nendobj\n";
    let content_object = stream_object(4, "", b"q\nQ");
    classic_pdf(&[&catalog, PAGES, page, &content_object])
}

fn fogra51_policy() -> OutputIntentPolicy {
    OutputIntentPolicy::EnsureTarget {
        target: OutputIntentTarget::NamedCondition {
            condition: NamedOutputCondition {
                subtype: OutputIntentSubtype::GtsPdfx,
                output_condition_identifier: "FOGRA51".to_string(),
                registry_name: "https://example.test/registry".to_string(),
            },
        },
    }
}

fn has_eligibility_field(fields: &[(String, TestSerdeValue)]) -> bool {
    fields
        .iter()
        .any(|(key, _)| key == "output_intent_eligibility")
}

fn has_field(fields: &[(String, TestSerdeValue)], name: &str) -> bool {
    fields.iter().any(|(key, _)| key == name)
}

fn byte_seq(bytes: &[u8]) -> TestSerdeValue {
    TestSerdeValue::Seq(
        bytes
            .iter()
            .copied()
            .map(u64::from)
            .map(TestSerdeValue::U64)
            .collect(),
    )
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
                vec![spot_observation(ColorSpace::Separation, b"Spot")],
            ),
            entry(
                1,
                2,
                ObjectKind::Image,
                vec![observation(ColorUsage::Image, ColorSpace::Unknown)],
            ),
        ],
        vec![
            skipped_page(0),
            inventoried_page_with_form_skipped(1, 3, vec![budget_form_skip()]),
        ],
    );

    let audit = build_color_usage_audit(inventory);

    // The hand-built report exercises a finding, spot name, and every gap kind
    // reachable from entries/pages.
    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert_eq!(audit.rgb_findings.len(), 1);
    assert_eq!(audit.spot_names.len(), 1);

    round_trip::<ColorUsageAudit>(&audit)?;
    round_trip(&audit.status)?;
    round_trip(&audit.document)?;
    for page in &audit.pages {
        round_trip::<PageColorUsage>(page)?;
    }
    for finding in &audit.rgb_findings {
        round_trip::<RgbFinding>(finding)?;
    }
    for gap in &audit.coverage_gaps {
        round_trip::<CoverageGap>(gap)?;
        round_trip::<CoverageGapKind>(&gap.kind)?;
    }
    Ok(())
}

#[test]
fn graphics_state_finding_serde_shape_is_pinned() -> Result<(), String> {
    let finding = GraphicsStateFinding {
        page: PageIndex(2),
        source: GraphicsStateFindingSource::PageExtGState,
        overprint: true,
        transparency: false,
        unresolved: true,
        unclassified: false,
    };

    let value = serde_value(&finding).map_err(|error| error.to_string())?;
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(2)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_ext_g_state".to_string()),
            ),
            ("overprint".to_string(), TestSerdeValue::Bool(true)),
            ("transparency".to_string(), TestSerdeValue::Bool(false)),
            ("unresolved".to_string(), TestSerdeValue::Bool(true)),
            ("unclassified".to_string(), TestSerdeValue::Bool(false)),
        ])
    );
    round_trip(&finding)?;

    // The form-scope variant is a declared contract only in this slice: its
    // serde string is pinned here, but nothing emits it yet.
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::FormExtGState)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("form_ext_g_state".to_string())
    );
    round_trip(&GraphicsStateFindingSource::FormExtGState)?;
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::PageTransparencyGroup)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("page_transparency_group".to_string())
    );
    assert_eq!(
        serde_value(&GraphicsStateFindingSource::FormTransparencyGroup)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("form_transparency_group".to_string())
    );
    round_trip(&GraphicsStateFindingSource::PageTransparencyGroup)?;
    round_trip(&GraphicsStateFindingSource::FormTransparencyGroup)?;

    // The two additive coverage-gap kinds are shape-locked the same way.
    assert_eq!(
        serde_value(&CoverageGapKind::ExtGStateResourceInspectionError)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("ext_g_state_resource_inspection_error".to_string())
    );
    assert_eq!(
        serde_value(&CoverageGapKind::ExtGStateResourceSkipped)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("ext_g_state_resource_skipped".to_string())
    );
    round_trip(&CoverageGapKind::ExtGStateResourceSkipped)?;
    assert_eq!(
        serde_value(&CoverageGapKind::TransparencyGroupInspectionError)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("transparency_group_inspection_error".to_string())
    );
    assert_eq!(
        serde_value(&CoverageGapKind::TransparencyGroupSkipped)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::String("transparency_group_skipped".to_string())
    );
    round_trip(&CoverageGapKind::TransparencyGroupSkipped)?;
    Ok(())
}

#[test]
fn audit_without_findings_omits_the_field_and_old_json_deserializes() -> Result<(), String> {
    // The synthetic pure-build path carries no graphics-state pass at all, so
    // the vec is empty and `skip_serializing_if` omits the key: every existing
    // pinned audit JSON stays byte-identical.
    let inventory = synthetic_inventory(
        vec![entry(
            0,
            0,
            ObjectKind::Vector,
            vec![observation(ColorUsage::Fill, ColorSpace::DeviceCmyk)],
        )],
        vec![inventoried_page(0, 1)],
    );
    let audit = build_color_usage_audit(inventory);
    assert!(audit.graphics_state_findings.is_empty());
    assert!(audit.icc_based_findings.is_empty());

    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    assert!(
        fields
            .iter()
            .all(|(key, _)| key != "graphics_state_findings")
    );
    assert!(!has_field(fields, "icc_based_findings"));

    // The serialized map WITHOUT the key is exactly an old-format report: it
    // must deserialize through `#[serde(default)]`.
    let decoded: ColorUsageAudit = from_serde_value(value).map_err(|error| error.to_string())?;
    assert!(decoded.graphics_state_findings.is_empty());
    assert!(decoded.icc_based_findings.is_empty());
    assert_eq!(&decoded, &audit);
    Ok(())
}

#[test]
fn icc_based_finding_serde_shape_is_pinned() -> Result<(), String> {
    let finding = crate::IccBasedFinding {
        page: PageIndex(2),
        source: crate::IccBasedFindingSource::PageColorSpaceResource,
        resource_name: PdfName(b"CS0".to_vec()),
        profile_stream: Some(crate::pdf::IndirectRef {
            object_number: 5,
            generation: 0,
        }),
        n: Some(4),
        alternate_space: Some(ColorSpace::DeviceCmyk),
        kind: crate::IccBasedFindingKind::RangeArityMismatch {
            expected: 8,
            got: 6,
        },
    };

    let value = serde_value(&finding).map_err(|error| error.to_string())?;
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(2)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_color_space_resource".to_string()),
            ),
            ("resource_name".to_string(), byte_seq(b"CS0")),
            (
                "profile_stream".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::Map(vec![
                    ("object_number".to_string(), TestSerdeValue::U64(5)),
                    ("generation".to_string(), TestSerdeValue::U64(0)),
                ]))),
            ),
            (
                "n".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::U64(4))),
            ),
            (
                "alternate_space".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::String("device_cmyk".to_string(),))),
            ),
            (
                "kind".to_string(),
                TestSerdeValue::Map(vec![
                    (
                        "kind".to_string(),
                        TestSerdeValue::String("range_arity_mismatch".to_string()),
                    ),
                    ("expected".to_string(), TestSerdeValue::U64(8)),
                    ("got".to_string(), TestSerdeValue::U64(6)),
                ]),
            ),
        ])
    );
    round_trip(&finding)?;
    round_trip(&crate::IccBasedFindingKind::MissingOrMalformedN)?;
    round_trip(&crate::IccBasedFindingKind::AlternateComponentMismatch {
        n: 3,
        alternate_implied: 4,
    })?;
    round_trip(&crate::IccBasedFindingKind::AlternateUnclassified)?;
    round_trip(&crate::IccBasedFindingSource::DefaultColorSpace)?;
    Ok(())
}

#[test]
fn icc_based_profile_finding_kinds_serde_shapes_are_pinned() -> Result<(), String> {
    // Truncated header: tagged struct-variant with a single `decoded_len`.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileHeaderTruncated { decoded_len: 40 })
            .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![
            (
                "kind".to_string(),
                TestSerdeValue::String("profile_header_truncated".to_string()),
            ),
            ("decoded_len".to_string(), TestSerdeValue::U64(40)),
        ])
    );

    // Missing acsp: a unit-like tagged variant carrying only its `kind` tag.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileAcspMissing)
            .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![(
            "kind".to_string(),
            TestSerdeValue::String("profile_acsp_missing".to_string()),
        )])
    );

    // Declared-size mismatch: both size facts alongside the tag.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileDeclaredSizeMismatch {
            declared: 999,
            decoded_len: 128,
        })
        .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![
            (
                "kind".to_string(),
                TestSerdeValue::String("profile_declared_size_mismatch".to_string()),
            ),
            ("declared".to_string(), TestSerdeValue::U64(999)),
            ("decoded_len".to_string(), TestSerdeValue::U64(128)),
        ])
    );

    // Component-count mismatch: the raw four-byte signature serializes as a seq.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileComponentCountMismatch {
            n: 3,
            data_color_space_signature: *b"GRAY",
        })
        .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![
            (
                "kind".to_string(),
                TestSerdeValue::String("profile_component_count_mismatch".to_string()),
            ),
            ("n".to_string(), TestSerdeValue::U64(3)),
            ("data_color_space_signature".to_string(), byte_seq(b"GRAY"),),
        ])
    );

    // Disallowed class: raw class signature as a seq.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileClassDisallowed {
            profile_class_signature: *b"link",
        })
        .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![
            (
                "kind".to_string(),
                TestSerdeValue::String("profile_class_disallowed".to_string()),
            ),
            ("profile_class_signature".to_string(), byte_seq(b"link"),),
        ])
    );

    // Inspection gap: the nested pdf reason serializes as a plain snake_case
    // string under the `reason` field.
    assert_eq!(
        serde_value(&crate::IccBasedFindingKind::ProfileInspectionGap {
            reason: crate::pdf::IccProfileInspectionGap::UnsupportedFilter,
        })
        .map_err(|error| error.to_string())?,
        TestSerdeValue::Map(vec![
            (
                "kind".to_string(),
                TestSerdeValue::String("profile_inspection_gap".to_string()),
            ),
            (
                "reason".to_string(),
                TestSerdeValue::String("unsupported_filter".to_string()),
            ),
        ])
    );

    round_trip(&crate::IccBasedFindingKind::ProfileHeaderTruncated { decoded_len: 40 })?;
    round_trip(&crate::IccBasedFindingKind::ProfileAcspMissing)?;
    round_trip(&crate::IccBasedFindingKind::ProfileDeclaredSizeMismatch {
        declared: 999,
        decoded_len: 128,
    })?;
    round_trip(&crate::IccBasedFindingKind::ProfileComponentCountMismatch {
        n: 3,
        data_color_space_signature: *b"GRAY",
    })?;
    round_trip(&crate::IccBasedFindingKind::ProfileClassDisallowed {
        profile_class_signature: *b"link",
    })?;
    round_trip(&crate::IccBasedFindingKind::ProfileInspectionGap {
        reason: crate::pdf::IccProfileInspectionGap::ProfileObjectCompressed,
    })?;
    Ok(())
}

#[test]
fn finding_bearing_audit_pins_the_graphics_state_findings_entry() -> Result<(), String> {
    // Content with no colour operators: the finding comes from the DECLARED
    // resources alone, and the audit then carries no f64 colour components the
    // dependency-free serde harness cannot model.
    let source = page_with_extgstate_pdf("/GS0 << /OP true /BM /Multiply >>", b"q\nQ");
    let audit = audit_color_usage(&source, 1024).map_err(|error| format!("{error:?}"))?;

    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    let (_, findings) = fields
        .iter()
        .find(|(key, _)| key == "graphics_state_findings")
        .ok_or_else(|| "graphics_state_findings key should be present".to_string())?;
    assert_eq!(
        findings,
        &TestSerdeValue::Seq(vec![TestSerdeValue::Map(vec![
            ("page".to_string(), TestSerdeValue::U64(0)),
            (
                "source".to_string(),
                TestSerdeValue::String("page_ext_g_state".to_string()),
            ),
            ("overprint".to_string(), TestSerdeValue::Bool(true)),
            ("transparency".to_string(), TestSerdeValue::Bool(true)),
            ("unresolved".to_string(), TestSerdeValue::Bool(false)),
            ("unclassified".to_string(), TestSerdeValue::Bool(false)),
        ])])
    );
    round_trip(&audit)?;
    Ok(())
}

#[test]
fn output_intent_eligibility_field_is_optional_and_policy_gated() -> Result<(), String> {
    let audit = audit_color_usage(&single_page_pdf(b"", b"q\nQ"), 1024)
        .map_err(|error| format!("{error:?}"))?;
    assert!(audit.output_intent_eligibility.is_none());
    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    assert!(!has_eligibility_field(fields));
    let decoded: ColorUsageAudit = from_serde_value(value).map_err(|error| error.to_string())?;
    assert!(decoded.output_intent_eligibility.is_none());

    let audit = audit_color_usage_with_output_intent_policy(
        &output_intent_pdf("(FOGRA51)"),
        1024,
        &fogra51_policy(),
    )
    .map_err(|error| format!("{error:?}"))?;
    assert!(audit.output_intent_eligibility.is_some());
    let value = serde_value(&audit).map_err(|error| error.to_string())?;
    let TestSerdeValue::Map(fields) = &value else {
        return Err("audit should serialize as a map".to_string());
    };
    assert!(has_eligibility_field(fields));
    round_trip(&audit)?;
    Ok(())
}
