use presslint_color::{ObservedOutputIntent, OutputIntentSubtype};
use presslint_pdf::{PdfOutputIntentFact, PdfOutputIntentSubtype};

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

const fn map_subtype(subtype: PdfOutputIntentSubtype) -> OutputIntentSubtype {
    match subtype {
        PdfOutputIntentSubtype::GtsPdfx => OutputIntentSubtype::GtsPdfx,
        PdfOutputIntentSubtype::GtsPdfa1 => OutputIntentSubtype::GtsPdfa1,
        PdfOutputIntentSubtype::IsoPdfe1 => OutputIntentSubtype::IsoPdfe1,
    }
}

#[cfg(test)]
mod tests {
    use presslint_pdf::{DestOutputProfileFact, PdfOutputIntentFact};

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
}
