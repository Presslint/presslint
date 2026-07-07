use presslint_color::{
    ObservedOutputIntent, OutputIntentDecision, OutputIntentPolicy, OutputIntentSubtype,
    resolve_output_intent_policy,
};
use presslint_pdf::{
    OutputIntentsInspectionError, PdfOutputIntentFact, PdfOutputIntentSubtype,
    inspect_document_output_intents,
};
use serde::{Deserialize, Serialize};

/// Report-only output-intent eligibility resolved through `presslint-color`.
///
/// `observed` contains only fully classified catalog facts. Malformed or
/// unsupported PDF-side entries remain `presslint-pdf` inspection skips and are
/// not promoted into color-policy observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputIntentEligibility {
    /// Fully classified output intents observed in catalog order.
    pub observed: Vec<ObservedOutputIntent>,
    /// Policy decision for the observed output-intent facts.
    pub decision: OutputIntentDecision,
}

/// Map neutral PDF-side output-intent facts into color-policy observations.
///
/// This bridge lives in the umbrella crate so `presslint-pdf` stays independent
/// from `presslint-color`. PDF-side skips remain diagnostics in the PDF report;
/// this helper maps only fully classified facts.
#[must_use]
pub fn observed_output_intents_from_pdf<'a>(
    facts: impl IntoIterator<Item = &'a PdfOutputIntentFact>,
) -> Vec<ObservedOutputIntent> {
    facts
        .into_iter()
        .map(|fact| ObservedOutputIntent {
            subtype: map_subtype(fact.subtype),
            output_condition_identifier: fact.output_condition_identifier.clone(),
        })
        .collect()
}

/// Resolve an output-intent policy against already observed facts.
#[must_use]
pub fn resolve_output_intent_eligibility<I>(
    policy: &OutputIntentPolicy,
    observed: I,
) -> OutputIntentEligibility
where
    I: IntoIterator<Item = ObservedOutputIntent>,
{
    let observed = observed.into_iter().collect::<Vec<_>>();
    let decision = resolve_output_intent_policy(policy, observed.iter().cloned());
    OutputIntentEligibility { observed, decision }
}

/// Inspect PDF catalog output intents and resolve them against a policy.
///
/// This is read-only: it writes no bytes, retains no ICC/profile data, and maps
/// only classified PDF-side facts into the color-policy layer.
///
/// # Errors
///
/// Returns `presslint-pdf`'s document-access error when the catalog cannot be
/// reached. Malformed or unsupported output-intent entries are structured skips
/// in the PDF inspection layer and simply do not appear in `observed`.
pub fn evaluate_pdf_output_intent_eligibility(
    input: &[u8],
    policy: &OutputIntentPolicy,
) -> Result<OutputIntentEligibility, OutputIntentsInspectionError> {
    let inspection = inspect_document_output_intents(input)?;
    Ok(resolve_output_intent_eligibility(
        policy,
        observed_output_intents_from_pdf(&inspection.output_intents),
    ))
}

const fn map_subtype(subtype: PdfOutputIntentSubtype) -> OutputIntentSubtype {
    match subtype {
        PdfOutputIntentSubtype::GtsPdfx => OutputIntentSubtype::GtsPdfx,
        PdfOutputIntentSubtype::GtsPdfa1 => OutputIntentSubtype::GtsPdfa1,
        PdfOutputIntentSubtype::IsoPdfe1 => OutputIntentSubtype::IsoPdfe1,
    }
}

#[cfg(test)]
mod tests {
    use presslint_color::{
        NamedOutputCondition, OutputIntentPolicy, OutputIntentRejection, OutputIntentTarget,
    };
    use presslint_pdf::{
        DestOutputProfileFact, PdfOutputIntentFact, SkippedOutputIntentReason,
        inspect_document_output_intents,
    };

    use super::*;

    #[test]
    fn color_environment_maps_pdf_output_intent_facts_to_color_observations() {
        let facts = vec![
            fact(PdfOutputIntentSubtype::GtsPdfx, "CGATS TR 001"),
            fact(PdfOutputIntentSubtype::GtsPdfa1, "sRGB"),
            fact(PdfOutputIntentSubtype::IsoPdfe1, "Engineering"),
        ];

        let observed = observed_output_intents_from_pdf(&facts);

        assert_eq!(observed.len(), 3);
        assert_eq!(observed[0].subtype, OutputIntentSubtype::GtsPdfx);
        assert_eq!(observed[0].output_condition_identifier, "CGATS TR 001");
        assert_eq!(observed[1].subtype, OutputIntentSubtype::GtsPdfa1);
        assert_eq!(observed[2].subtype, OutputIntentSubtype::IsoPdfe1);
    }

    #[test]
    fn output_intent_eligibility_resolves_observed_facts() {
        let policy = fogra51_policy();
        let eligibility = resolve_output_intent_eligibility(
            &policy,
            [ObservedOutputIntent {
                subtype: OutputIntentSubtype::GtsPdfx,
                output_condition_identifier: "FOGRA39".to_string(),
            }],
        );

        assert_eq!(eligibility.observed.len(), 1);
        assert!(matches!(
            eligibility.decision,
            OutputIntentDecision::ConflictsWithExisting { .. }
        ));
    }

    #[test]
    fn pdf_output_intent_eligibility_ignores_skipped_entries() -> Result<(), String> {
        let source = output_intent_pdf(b"<< /S /Foo /OutputConditionIdentifier (FOO) >>");

        let inspection = inspect_document_output_intents(&source)
            .map_err(|error| format!("output-intent inspection should succeed: {error:?}"))?;
        assert!(inspection.output_intents.is_empty());
        assert!(matches!(
            inspection.skipped[0].reason,
            SkippedOutputIntentReason::UnsupportedSubtype { .. }
        ));

        let eligibility =
            evaluate_pdf_output_intent_eligibility(&source, &OutputIntentPolicy::RequireExisting)
                .map_err(|error| format!("eligibility should resolve: {error:?}"))?;
        assert!(eligibility.observed.is_empty());
        assert_eq!(
            eligibility.decision,
            OutputIntentDecision::Rejected {
                rejection: OutputIntentRejection::NoExistingIntent
            }
        );
        Ok(())
    }

    #[test]
    fn pdf_output_intent_eligibility_rejects_require_existing_when_absent() -> Result<(), String> {
        let eligibility = evaluate_pdf_output_intent_eligibility(
            &no_output_intent_pdf(),
            &OutputIntentPolicy::RequireExisting,
        )
        .map_err(|error| format!("eligibility should resolve: {error:?}"))?;

        assert!(eligibility.observed.is_empty());
        assert_eq!(
            eligibility.decision,
            OutputIntentDecision::Rejected {
                rejection: OutputIntentRejection::NoExistingIntent
            }
        );
        Ok(())
    }

    #[test]
    fn preserve_policy_is_neutral_with_observed_output_intents() -> Result<(), String> {
        let source = output_intent_pdf(b"<< /S /GTS_PDFX /OutputConditionIdentifier (FOGRA51) >>");
        let eligibility =
            evaluate_pdf_output_intent_eligibility(&source, &OutputIntentPolicy::Preserve)
                .map_err(|error| format!("eligibility should resolve: {error:?}"))?;

        assert_eq!(eligibility.observed.len(), 1);
        assert_eq!(eligibility.decision, OutputIntentDecision::Preserve);
        Ok(())
    }

    fn fact(
        subtype: PdfOutputIntentSubtype,
        output_condition_identifier: &str,
    ) -> PdfOutputIntentFact {
        PdfOutputIntentFact {
            index: 0,
            subtype,
            output_condition_identifier: output_condition_identifier.to_string(),
            dest_output_profile: DestOutputProfileFact {
                present: false,
                reference: None,
                value_kind: None,
            },
        }
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

    fn output_intent_pdf(intent: &[u8]) -> Vec<u8> {
        let mut catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ ".to_vec();
        catalog.extend_from_slice(intent);
        catalog.extend_from_slice(b" ] >>\nendobj\n");
        let pages = b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n";
        classic_pdf(&[&catalog, pages])
    }

    fn no_output_intent_pdf() -> Vec<u8> {
        let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
        let pages = b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n";
        classic_pdf(&[catalog, pages])
    }

    fn classic_pdf(objects: &[&[u8]]) -> Vec<u8> {
        let mut source = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for object in objects {
            offsets.push(source.len());
            source.extend_from_slice(object);
        }
        let xref_offset = source.len();
        let object_count = objects.len() + 1;
        source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
        source.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        source.extend_from_slice(
            format!(
                "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
            )
            .as_bytes(),
        );
        source
    }
}
