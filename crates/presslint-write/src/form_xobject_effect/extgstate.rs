//! Bounded Form-local `/Resources /ExtGState` proven-neutral `gs` gate.
//!
//! [`FormLocalExtGStateAuthority`] is the ONE new private domain abstraction of
//! the T191 slice, mirroring the sanctioned `FormLocalXObjectAuthority` shape.
//! It is built at most once per analyzed Form compute, and only when a
//! syntactically valid `gs` is actually present, from the Form's OWN
//! `/Resources /ExtGState` — never page or caller fallback, never merged
//! scopes. It answers exactly one question per decoded `gs` operand spelling:
//! does the name resolve to exactly one classified `ExtGState` resource proven
//! colour-lane neutral and font-inert?
//!
//! It is a GATE, not a state machine: it never tracks which `gs` is in force
//! where. Neutrality of every activated entry makes activation order
//! irrelevant, so the single sentinel-seeded walk keeps running over the empty
//! `ExtGState` environment with no walker, lattice, cache, or budget change —
//! a proven-neutral `gs` is a no-op for the two-bit inherited-colour question.
//! Semantic validation therefore happens BEFORE the walk; the walker's empty
//! environment is compatibility-neutral and never stands in for validation.
//!
//! Before a classified entry reached through an INDIRECT `/ExtGState` value
//! may prove anything, its exact identity is corroborated the same way the
//! `XObject` authority corroborates its targets: the reference must re-resolve
//! through the current lookup to an in-use source-addressable object whose
//! generation matches AND whose object header at the resolved byte offset
//! identifies exactly the requested object. A malformed xref can bind `N G R`
//! to an offset holding a DIFFERENT object's neutral body while a repairing
//! reader locates — and would activate — another, unsafe object; classifying
//! the mispointed body would fabricate a false-neutral verdict. The classified
//! report does not carry the resolved reference, so the authority re-scans the
//! canonical raw `/ExtGState` entries and poisons the decoded name of every
//! indirect entry that fails this header-identity corroboration.
//!
//! Neutrality delegates entirely to the shipped classifier facts and requires
//! ALL of: no active overprint (ANY set `/OPM`, including `0`, is active), no
//! active transparency (`/CA`/`/ca` absent or exactly opaque, `/BM` absent or
//! Normal/Compatible, `/SMask` absent or `/None`), no unresolved or
//! unclassified safety parameter, `font_effect` exactly `Unset`, AND
//! `has_unclassified_keys == false` — the classifier documents that `Unset`
//! proves `/Font` absence only while that aggregate flag is false, so a benign
//! `/LW`-style dictionary deliberately refuses in this slice. This parameter
//! matrix matches the page-level `gs` guard entry-for-entry, and the
//! decoded-name matching discipline mirrors its semantics: a semantic
//! collision poisons the decoded name (never first-win), an undecodable
//! relevant spelling retains literal poison, a matching named skip poisons the
//! name, and a nameless skip or ambiguous authority poisons the namespace. An
//! unused unsafe declaration does not poison a used safe one.
//!
//! The authority retains only owned decoded names, per-name neutrality
//! verdicts, and the poison state — no source bytes, dictionaries, tokens,
//! streams, or classifier reports survive the compute — and it is dropped with
//! the gate once the analyzed Form's decision is made. Classified entries plus
//! skips are capped at 256 before the deterministic writer map is populated,
//! and distinct raw `gs` operand spellings are separately capped at 256;
//! either overflow is namespace Unknown.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use presslint_pdf::{
    ClassifiedExtGStateResource, DictionaryValueKind, ExtGStateFontEffect, ObjectLookup,
    ObjectLookupLocation, SkippedExtGStateResourceReason, inspect_dictionary_entries,
    inspect_form_extgstate_resources, inspect_indirect_object_dictionary, locate_xref_object,
    parse_indirect_reference,
};
use presslint_syntax::OperatorRecord;

use super::{
    canonical_form_resources_entries, canonical_unique_authority_entry,
    has_canonical_form_resource_dictionary,
};
use crate::{
    extgstate_page_guard::{ResourceNameMatch, resource_name_match},
    page_xobject_policy::decode_pdf_name,
};

/// Fixed cap on classified Form-local `ExtGState` entries plus skip facts
/// consulted for one analyzed Form's decoded-name authority. Beyond this, the
/// whole namespace is poisoned Unknown before the writer map is populated.
const MAX_EXTGSTATE_FACTS: usize = 256;

/// Fixed cap on distinct raw `gs` operand spellings validated for one analyzed
/// Form. This separately bounds the gate even when many escaped spellings
/// decode to one semantic resource name.
const MAX_EXTGSTATE_OPERAND_SPELLINGS: usize = 256;

/// Bounded decoded-name authority over one analyzed Form's own
/// `/Resources /ExtGState` bindings.
struct FormLocalExtGStateAuthority {
    /// Decoded name -> proven colour-lane neutral and font-inert (`true`), or
    /// classified-but-inadmissible / header-uncorroborated / collision /
    /// named-skip poison (`false`). Both refuse identically at the gate; the
    /// map only proves which names admit.
    entries: BTreeMap<Vec<u8>, bool>,
    /// Literal spellings of undecodable classified/skipped resource names. A
    /// valid operand decoding to one of these spellings can never be proven
    /// distinct from it; unrelated malformed names remain isolated.
    literal_poison: BTreeSet<Vec<u8>>,
    /// Namespace-wide poison: a nameless uncertain skip, a fact-cap overflow,
    /// or ambiguous `/Resources`/`/ExtGState` authority makes no `gs`
    /// admissible.
    poison_all: bool,
}

impl FormLocalExtGStateAuthority {
    /// Build the authority for one analyzed Form from its own classified
    /// `/Resources /ExtGState` report, corroborating the canonical raw
    /// authority keys — and each indirect entry's exact target header identity
    /// — before any structural fact is trusted.
    fn from_form(input: &[u8], lookup: ObjectLookup<'_>, reached_offset: usize) -> Self {
        if !has_canonical_form_resource_dictionary(input, lookup, reached_offset, b"ExtGState") {
            return Self::poisoned();
        }
        let inspection = inspect_form_extgstate_resources(input, lookup, reached_offset);
        if inspection.extgstates.len() + inspection.skipped.len() > MAX_EXTGSTATE_FACTS {
            return Self::poisoned();
        }
        let Ok(uncorroborated) =
            indirect_entries_failing_header_identity(input, lookup, reached_offset)
        else {
            return Self::poisoned();
        };

        let mut entries: BTreeMap<Vec<u8>, bool> = BTreeMap::new();
        let mut literal_poison = BTreeSet::new();
        let mut poison_all = false;

        for resource in inspection.extgstates {
            let Some(decoded) = decode_pdf_name(&resource.name.0) else {
                literal_poison.insert(resource.name.0);
                continue;
            };
            match entries.entry(decoded.into_owned()) {
                // Two raw spellings decoding to one semantic name cannot be
                // told apart at activation time: the name is poisoned, never
                // first-win.
                Entry::Occupied(mut slot) => {
                    slot.insert(false);
                }
                Entry::Vacant(slot) => {
                    // A mispointed indirect binding classified the wrong
                    // object's body: the name is poisoned regardless of the
                    // (false) neutrality that body would prove.
                    slot.insert(
                        !uncorroborated.contains(&resource.name.0) && entry_is_neutral(&resource),
                    );
                }
            }
        }

        for skip in inspection.skipped {
            match skip.reason {
                // A proven-absent `/Resources` or `/ExtGState` is not
                // uncertainty (the canonical authority preflight corroborated
                // the raw fact): an activated name simply finds no binding.
                SkippedExtGStateResourceReason::MissingExtGStateResources
                | SkippedExtGStateResourceReason::MissingExtGState => continue,
                _ => {}
            }
            match skip.resource_name {
                None => poison_all = true,
                Some(name) => match decode_pdf_name(&name.0) {
                    // A named skip always overrides a same-name classified
                    // entry and poisons only its decoded name.
                    Some(decoded) => {
                        entries.insert(decoded.into_owned(), false);
                    }
                    None => {
                        literal_poison.insert(name.0);
                    }
                },
            }
        }

        Self {
            entries,
            literal_poison,
            poison_all,
        }
    }

    /// Whether one raw `gs` operand spelling resolves to exactly one
    /// classified entry proven colour-lane neutral and font-inert, with one
    /// decoded-name lookup.
    ///
    /// `false` is a fail-closed refusal: an undecodable operand, the poisoned
    /// namespace, a literal-poison collision, an unresolved name, or a
    /// per-name poisoned/inadmissible binding.
    fn admits(&self, raw: &[u8]) -> bool {
        if self.poison_all {
            return false;
        }
        let Some(decoded) = decode_pdf_name(raw) else {
            return false;
        };
        if self.literal_poison.iter().any(|raw_name| {
            matches!(
                resource_name_match(raw_name, decoded.as_ref()),
                ResourceNameMatch::LiteralPoison
            )
        }) {
            return false;
        }
        self.entries.get(decoded.as_ref()).copied().unwrap_or(false)
    }

    /// The namespace-poisoned authority: no `gs` is admissible.
    const fn poisoned() -> Self {
        Self {
            entries: BTreeMap::new(),
            literal_poison: BTreeSet::new(),
            poison_all: true,
        }
    }
}

/// The exact neutrality predicate over one shipped classified entry. Every
/// conjunct delegates to the classifier facts; no parameter semantics are
/// re-derived here. `font_effect == Unset` proves `/Font` absence only while
/// `has_unclassified_keys` is false, so both gates are required.
fn entry_is_neutral(resource: &ClassifiedExtGStateResource) -> bool {
    !resource.is_overprint_active()
        && !resource.is_transparency_active()
        && !resource.has_unresolved_or_unclassified_safety_param()
        && resource.font_effect == ExtGStateFontEffect::Unset
        && !resource.has_unclassified_keys
}

/// Raw names of the Form's own canonical `/ExtGState` entries whose INDIRECT
/// value fails exact header-identity corroboration. The shipped classifier
/// resolves an indirect entry through the xref and classifies whatever
/// dictionary sits at the resolved offset; it does not corroborate that the
/// object header there identifies the requested object, so a malformed xref
/// binding `N G R` to a different object's neutral body would classify a
/// false neutral while a repairing reader may activate the real, unsafe
/// object. Direct dictionary entries involve no xref binding and are never in
/// the set. `Err(())` is unscannable authority and poisons the namespace.
fn indirect_entries_failing_header_identity(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
) -> Result<BTreeSet<Vec<u8>>, ()> {
    let Some(resources) = canonical_form_resources_entries(input, lookup, reached_offset)? else {
        return Ok(BTreeSet::new());
    };
    let Some(extgstate) = canonical_unique_authority_entry(input, &resources, b"ExtGState")? else {
        return Ok(BTreeSet::new());
    };
    // The canonical preflight already required a direct dictionary; anything
    // else here is defensively unscannable.
    if extgstate.value_kind != DictionaryValueKind::Dictionary {
        return Err(());
    }
    let dictionary =
        inspect_dictionary_entries(input, extgstate.value_range.start).map_err(|_| ())?;
    let mut failing = BTreeSet::new();
    for entry in dictionary.entries {
        if entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
            continue;
        }
        let raw_key = input
            .get(entry.key_range.start..entry.key_range.end)
            .ok_or(())?;
        let name = raw_key.strip_prefix(b"/").ok_or(())?;
        if !indirect_target_header_corroborates(input, lookup, entry.value_range.start) {
            failing.insert(name.to_vec());
        }
    }
    Ok(failing)
}

/// Whether one indirect `/ExtGState` entry value re-resolves through the
/// current lookup to an in-use source-addressable object whose generation
/// matches and whose object header at the resolved byte offset identifies
/// exactly the requested reference. Compressed, free, missing, mismatched, or
/// uninspectable targets fail closed.
fn indirect_target_header_corroborates(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    value_offset: usize,
) -> bool {
    let Ok(parsed) = parse_indirect_reference(input, value_offset) else {
        return false;
    };
    let reference = parsed.reference;
    let Ok(object_number) = usize::try_from(reference.object_number) else {
        return false;
    };
    let object_offset = match locate_xref_object(lookup, object_number) {
        ObjectLookupLocation::ClassicInUse {
            generation,
            byte_offset,
            ..
        }
        | ObjectLookupLocation::XrefStreamUncompressed {
            generation,
            byte_offset,
            ..
        } if generation == reference.generation => byte_offset,
        _ => return false,
    };
    inspect_indirect_object_dictionary(input, object_offset)
        .is_ok_and(|dictionary| dictionary.reference == reference)
}

/// Whether one record's operator token is `gs`.
fn is_extgstate_operator(record: &OperatorRecord, source: &[u8]) -> bool {
    matches!(
        source.get(record.operator.range.start..record.operator.range.end),
        Some(b"gs")
    )
}

/// The raw operand spelling (without the leading slash) of a `gs` record, or
/// `None` when the sole operand is not a well-formed single name.
fn extgstate_operand_name<'a>(record: &OperatorRecord, source: &'a [u8]) -> Option<&'a [u8]> {
    let [operand] = record.operands.as_slice() else {
        return None;
    };
    source
        .get(operand.range.start..operand.range.end)
        .and_then(|bytes| bytes.strip_prefix(b"/"))
}

/// Prove every executed `gs` in one decoded Form resolves, through the ONE
/// demand-built Form-local authority, to a classified entry proven colour-lane
/// neutral and font-inert.
///
/// The authority is built only after the first syntactically valid `gs` record
/// is found, so a Form without `gs` never inspects its own
/// `/Resources /ExtGState` — even a malformed one. Each DISTINCT raw operand
/// spelling is validated once through the authority; exceeding the spelling
/// cap, or any spelling that fails to prove, refuses the whole Form.
pub(super) fn proven_neutral_gs_activations(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
    records: &[OperatorRecord],
    decoded: &[u8],
) -> bool {
    let mut activations = records
        .iter()
        .filter(|record| is_extgstate_operator(record, decoded));
    let Some(first) = activations.next() else {
        return true;
    };
    let authority = FormLocalExtGStateAuthority::from_form(input, lookup, reached_offset);
    let mut seen: BTreeSet<&[u8]> = BTreeSet::new();
    for record in std::iter::once(first).chain(activations) {
        let Some(raw) = extgstate_operand_name(record, decoded) else {
            return false;
        };
        if seen.contains(raw) {
            continue;
        }
        if seen.len() == MAX_EXTGSTATE_OPERAND_SPELLINGS {
            return false;
        }
        seen.insert(raw);
        if !authority.admits(raw) {
            return false;
        }
    }
    true
}
