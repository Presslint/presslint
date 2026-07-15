//! Bounded Form-local `/Resources /XObject` authority and exact Image/stencil
//! target corroboration.
//!
//! [`FormLocalXObjectAuthority`] is the ONE new private domain abstraction of
//! the T189 slice. It is built at most once per analyzed Form compute, and only
//! when a syntactically valid `Do` is actually present, from the Form's OWN
//! `/Resources /XObject` — never page or caller fallback, never merged scopes.
//! It answers exactly one question per raw `Do` operand spelling: what does
//! painting this named `XObject` do to the CURRENT graphics-state colour inside
//! the analyzed Form?
//!
//! The map value semantics mirror the sanctioned shape: a present
//! `Some((target, effect))` is ONE unambiguous exact typed binding; a per-name
//! `None` is collision/named-skip/uncertain-target poison; the namespace-wide
//! poison flag makes no `Do` admissible at all. `/Subtype /Form` targets are
//! RETAINED with their full target tuple; the authority's single
//! [`FormLocalXObjectAuthority::resolve`] returns that tuple with its effect so
//! the parent analyzer's bounded recursion (T190) can descend at the invoking
//! `Do`. A Form tuple is retained as exact only after the SAME identity
//! corroboration the Image path applies (reference/generation/reached-offset
//! re-resolution, an exact reinspected dictionary header, and canonical
//! semantically unique `/Subtype /Form`); an uncorroborated Form target is
//! per-name poison.
//!
//! Before any structural report fact is trusted, the Form's own `/Resources`
//! and `/XObject` keys must be canonical and semantically unique in
//! source-addressable dictionaries. Before an Image binding is published, the
//! target itself is re-corroborated: its reference re-resolves to the reached
//! generation and byte offset, its dictionary is reinspected at that exact
//! offset, its `/Subtype` is canonically and uniquely `/Image`, its
//! `/ImageMask` authority is canonical (including exact absence), and no
//! substitution/optional-content/external escape (`/Alternates`, `/OPI`,
//! `/OC`, `/F`, `/Ref`) is semantically present. Only then is the shipped page
//! ordinary/stencil classifier consulted; a `Stencil` verdict additionally
//! requires canonical semantic uniqueness of the `/Width`, `/Height`,
//! `/BitsPerComponent`, and `/ColorSpace` gate keys the raw-key metadata read.
//!
//! The authority retains no source bytes, dictionaries, streams, tokens,
//! records, or image data — only owned decoded names, small target tuples, and
//! the poison state — and it is dropped with the walk once the analyzed Form's
//! depth-indexed effect is cached. Total classified targets plus skips are
//! capped at 256 before the deterministic `BTreeMap` is populated; excess
//! poisons the namespace. Image sample data is never read or decoded.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use presslint_pdf::{
    DictionaryEntrySpan, DictionaryValueKind, ObjectLookup, PageXObjectResourceTarget,
    SkippedPageXObjectResourceReason, inspect_form_xobject_resources,
    inspect_image_xobject_metadata, inspect_indirect_object_dictionary,
};

use super::{
    canonical_unique_authority_entry, corroborates, has_canonical_form_resource_dictionary,
};
use crate::page_xobject_policy::{PageXObjectEffect, classify_image, decode_pdf_name};

/// Fixed cap on classified Form-local `XObject` targets plus skip facts
/// consulted for one analyzed Form's decoded-name authority. Beyond this, the
/// whole namespace is poisoned Unknown before the writer map is populated.
const MAX_XOBJECT_FACTS: usize = 256;

/// Image dictionary keys whose semantic presence puts an invoked target outside
/// this slice's execution envelope: `/Alternates` and `/OPI` supply
/// substitution semantics, `/OC` optional content may suppress the invocation,
/// and `/F`/`/Ref` substitute external or imported data for the local stream.
const FORBIDDEN_IMAGE_KEYS: [&[u8]; 5] = [b"Alternates", b"OPI", b"OC", b"F", b"Ref"];

/// Stencil-gate keys whose raw structural facts must be corroborated as
/// canonical and semantically unique before `Stencil` may be published: the
/// shared metadata inspector reads them by raw key, so an escaped or duplicated
/// spelling could otherwise hide a disqualifying value.
const STENCIL_GATE_KEYS: [&[u8]; 4] = [b"Width", b"Height", b"BitsPerComponent", b"ColorSpace"];

/// Bounded decoded-name authority over one analyzed Form's own
/// `/Resources /XObject` bindings.
pub(super) struct FormLocalXObjectAuthority {
    /// Decoded name -> one unambiguous exact typed binding (`Some`), or
    /// collision/named-skip/uncertain-target poison (`None`). Corroborated
    /// Form bindings keep their exact target tuple for the bounded nested
    /// descent the parent analyzer runs at an invoking `Do`.
    targets: BTreeMap<Vec<u8>, Option<(PageXObjectResourceTarget, PageXObjectEffect)>>,
    /// Literal spellings of undecodable classified/skipped resource names. A
    /// valid operand decoding to one of these spellings can never be proven
    /// distinct from it; unrelated malformed names remain isolated.
    literal_poison: BTreeSet<Vec<u8>>,
    /// Namespace-wide poison: a nameless uncertain skip, a fact-cap overflow,
    /// or ambiguous `/Resources`/`/XObject` authority makes no `Do` admissible.
    poison_all: bool,
}

impl FormLocalXObjectAuthority {
    /// Build the authority for one analyzed Form from its own classified
    /// `/Resources /XObject` report, corroborating each Image target's exact
    /// identity and semantic dictionary authority before its ordinary/stencil
    /// effect is published.
    pub(super) fn from_form(input: &[u8], lookup: ObjectLookup<'_>, reached_offset: usize) -> Self {
        if !has_canonical_form_resource_dictionary(input, lookup, reached_offset, b"XObject") {
            return Self::poisoned();
        }
        let inspection = inspect_form_xobject_resources(input, lookup, reached_offset);
        let facts = inspection.image_xobjects.len()
            + inspection.form_xobjects.len()
            + inspection.skipped.len();
        if facts > MAX_XOBJECT_FACTS {
            return Self::poisoned();
        }

        let mut targets: BTreeMap<Vec<u8>, Option<(PageXObjectResourceTarget, PageXObjectEffect)>> =
            BTreeMap::new();
        let mut literal_poison = BTreeSet::new();
        let mut poison_all = false;

        for (targets_to_classify, form_subtype) in [
            (inspection.image_xobjects, false),
            (inspection.form_xobjects, true),
        ] {
            for target in targets_to_classify {
                let Some(decoded) = decode_pdf_name(&target.name.0) else {
                    literal_poison.insert(target.name.0);
                    continue;
                };
                match targets.entry(decoded.into_owned()) {
                    // Two raw spellings decoding to one semantic name cannot
                    // be told apart at invocation time: the name is poisoned,
                    // never first-win.
                    Entry::Occupied(mut slot) => {
                        slot.insert(None);
                    }
                    Entry::Vacant(slot) => {
                        let effect = if form_subtype {
                            // Corroborated exact Form targets are retained for
                            // the bounded nested descent at an invoking `Do`.
                            corroborate_retained_form(input, lookup, &target)
                        } else {
                            classify_invoked_image(input, lookup, &target)
                        };
                        slot.insert(effect.map(|effect| (target, effect)));
                    }
                }
            }
        }

        for skip in inspection.skipped {
            match skip.reason {
                // A proven-absent `/Resources` or `/XObject` is not
                // uncertainty (the canonical authority preflight corroborated
                // the raw fact): an invoked name simply finds no binding.
                SkippedPageXObjectResourceReason::MissingResources
                | SkippedPageXObjectResourceReason::MissingXObject => continue,
                _ => {}
            }
            match skip.resource_name {
                None => poison_all = true,
                Some(name) => match decode_pdf_name(&name.0) {
                    // A named skip always overrides a same-name classified
                    // target and poisons only its decoded name.
                    Some(decoded) => {
                        targets.insert(decoded.into_owned(), None);
                    }
                    None => {
                        literal_poison.insert(name.0);
                    }
                },
            }
        }

        Self {
            targets,
            literal_poison,
            poison_all,
        }
    }

    /// Resolve one raw `Do` operand spelling to its unambiguous exact target
    /// and proven colour effect with one decoded-name lookup.
    ///
    /// `None` is a fail-closed refusal: an undecodable operand, the poisoned
    /// namespace, a literal-poison collision, an unresolved name, or a
    /// per-name poisoned binding. A returned effect is only ever
    /// [`PageXObjectEffect::OrdinaryImage`], [`PageXObjectEffect::Stencil`],
    /// or [`PageXObjectEffect::Form`]. Form retention proved exact identity and
    /// canonical semantically unique `/Subtype /Form`; the bounded recursion
    /// re-runs the complete root admission on the returned tuple.
    pub(super) fn resolve(
        &self,
        raw: &[u8],
    ) -> Option<(&PageXObjectResourceTarget, PageXObjectEffect)> {
        if self.poison_all {
            return None;
        }
        let decoded = decode_pdf_name(raw)?;
        if self.literal_poison.contains(decoded.as_ref()) {
            return None;
        }
        let (target, effect) = self.targets.get(decoded.as_ref())?.as_ref()?;
        Some((target, *effect))
    }

    /// The namespace-poisoned authority: no `Do` is admissible.
    const fn poisoned() -> Self {
        Self {
            targets: BTreeMap::new(),
            literal_poison: BTreeSet::new(),
            poison_all: true,
        }
    }
}

/// Corroborate one classified Image target's exact identity and semantic
/// dictionary authority, then fold it through the shipped page
/// ordinary/stencil classifier. `None` is an uncertain target that poisons
/// only its own decoded name.
///
/// This reads dictionary structure only: no stream body, image sample, or
/// filter data is touched, so `/Mask`, `/SMask`, `/Decode`, `/Interpolate`,
/// `/Intent`, and a JPX-style missing `/ColorSpace` never disturb an ordinary
/// Image's lane neutrality while the `/Subtype`/`/ImageMask` authority is
/// exact.
fn classify_invoked_image(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    target: &PageXObjectResourceTarget,
) -> Option<PageXObjectEffect> {
    let entries = corroborated_target_entries(input, lookup, target)?;
    let entries = entries.as_slice();
    if !canonical_subtype_is(input, entries, b"/Image") {
        return None;
    }
    // `/ImageMask` authority must be canonical and semantically unique,
    // INCLUDING exact absence, before the raw-key metadata fact is trusted.
    canonical_unique_authority_entry(input, entries, b"ImageMask").ok()?;
    for key in FORBIDDEN_IMAGE_KEYS {
        if canonical_unique_authority_entry(input, entries, key)
            .ok()?
            .is_some()
        {
            return None;
        }
    }
    let metadata = inspect_image_xobject_metadata(input, entries);
    match classify_image(Some(&metadata)) {
        PageXObjectEffect::OrdinaryImage => Some(PageXObjectEffect::OrdinaryImage),
        PageXObjectEffect::Stencil => {
            // The raw stencil gates (positive dimensions, `/BitsPerComponent`
            // absent or exactly 1, no `/ColorSpace`) hold only if each key's
            // semantic authority is canonical and unique.
            for key in STENCIL_GATE_KEYS {
                canonical_unique_authority_entry(input, entries, key).ok()?;
            }
            Some(PageXObjectEffect::Stencil)
        }
        _ => None,
    }
}

/// Corroborate one classified Form target's exact identity and canonical,
/// semantically unique `/Subtype /Form` authority before its exact tuple is
/// retained for the bounded nested descent. `None` is an uncertain target that
/// poisons only its own decoded name. The deeper dictionary preflight runs at
/// analysis time, when the recursion enters the retained tuple through the
/// complete root admission path.
fn corroborate_retained_form(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    target: &PageXObjectResourceTarget,
) -> Option<PageXObjectEffect> {
    let entries = corroborated_target_entries(input, lookup, target)?;
    if !canonical_subtype_is(input, &entries, b"/Form") {
        return None;
    }
    Some(PageXObjectEffect::Form)
}

/// Re-corroborate one classified target's exact identity before its report
/// facts are trusted: the reference re-resolves to the reached generation and
/// byte offset, and the reinspected dictionary at that exact offset carries
/// the same object header reference the resource entry named.
fn corroborated_target_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    target: &PageXObjectResourceTarget,
) -> Option<Vec<DictionaryEntrySpan>> {
    if !corroborates(lookup, target.reference, target.object_byte_offset) {
        return None;
    }
    let dictionary = inspect_indirect_object_dictionary(input, target.object_byte_offset).ok()?;
    if dictionary.reference != target.reference {
        return None;
    }
    Some(dictionary.entries)
}

/// Test-only probe for constructing a stale report tuple whose reached
/// dictionary still carries the expected object header. Production authority
/// reaches this exact helper only through inspector-produced targets.
#[cfg(test)]
pub fn xobject_target_identity_corroborates_for_test(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    target: &PageXObjectResourceTarget,
) -> bool {
    corroborated_target_entries(input, lookup, target).is_some()
}

/// Whether the target dictionary's `/Subtype` is canonical, semantically
/// unique, and exactly the expected direct name. The raw report already
/// matched a raw `/Subtype`; this refuses any escaped alias that could supply
/// a second, different subtype.
fn canonical_subtype_is(input: &[u8], entries: &[DictionaryEntrySpan], expected: &[u8]) -> bool {
    let Ok(Some(subtype)) = canonical_unique_authority_entry(input, entries, b"Subtype") else {
        return false;
    };
    subtype.value_kind == DictionaryValueKind::Name
        && input.get(subtype.value_range.start..subtype.value_range.end) == Some(expected)
}
