//! Private ownership proof adapter for content-stream object edits.

#![allow(clippy::redundant_pub_crate)]

use std::collections::BTreeMap;

use presslint_pdf::{
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult,
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectObjectOwnership,
    IndirectRef, ObjectConsumerIndexInspection, ObjectConsumerReferrer, SkippedObjectConsumerScan,
    decide_indirect_object_edit,
};

/// Combines exact direct `/Contents` owners with a document-wide exclusivity veto.
///
/// A typed `Page` referrer proves only that the target is somewhere in that
/// page's reachable subtree. It never substitutes for the direct `/Contents`
/// edge supplied to `decide_indirect_object_edit`. Because consumer paths are
/// deduplicated per typed user, this proves confinement to one page user, not
/// strict immediate-edge multiplicity within that page subtree.
pub(crate) struct ContentObjectOwnershipIndex {
    direct_owners: BTreeMap<IndirectRef, Vec<IndirectRef>>,
    page_indices: BTreeMap<IndirectRef, usize>,
    typed_referrers: BTreeMap<IndirectRef, Vec<ObjectConsumerReferrer>>,
    globally_complete: bool,
}

impl ContentObjectOwnershipIndex {
    pub(crate) fn new(
        pages: &[DocumentPageContentExtentInspection],
        consumers: ObjectConsumerIndexInspection,
    ) -> Self {
        let mut direct_owners: BTreeMap<IndirectRef, Vec<IndirectRef>> = BTreeMap::new();
        let mut page_indices = BTreeMap::new();
        for (page_index, page) in pages.iter().enumerate() {
            page_indices.insert(page.leaf.reference, page_index);
            // Only source-located leaf dictionaries supply editable direct edges.
            if let DocumentPageContentExtentResult::Inspected { contents, .. } = &page.result {
                for reference in &contents.contents {
                    direct_owners
                        .entry(reference.reference)
                        .or_default()
                        .push(page.leaf.reference);
                }
            }
        }

        let globally_complete = inspection_is_complete(&consumers);
        let typed_referrers = consumers
            .entries
            .into_iter()
            .filter(|entry| direct_owners.contains_key(&entry.target))
            .map(|entry| (entry.target, entry.referrers))
            .collect();

        Self {
            direct_owners,
            page_indices,
            typed_referrers,
            globally_complete,
        }
    }

    /// Return the direct occurrence count and the conservative edit decision.
    pub(crate) fn decide(&self, target: IndirectRef) -> (usize, IndirectObjectEditDecision) {
        let owners = self
            .direct_owners
            .get(&target)
            .map_or([].as_slice(), Vec::as_slice);
        let occurrences = owners.len();
        let mut decision = decide_indirect_object_edit(target, owners.iter().copied());

        let unique_direct_owner = owners
            .first()
            .filter(|owner| owners.iter().all(|candidate| candidate == *owner));
        let matching_page_is_exclusive =
            match (unique_direct_owner, self.typed_referrers.get(&target)) {
                (Some(owner), Some(referrers)) => {
                    let expected_index = self.page_indices.get(owner);
                    matches!(
                        (expected_index, referrers.as_slice()),
                        (
                            Some(expected_index),
                            [ObjectConsumerReferrer::Page { page_index, page }]
                        ) if page == owner && *page_index == *expected_index
                    )
                }
                _ => false,
            };

        if !self.globally_complete || !matching_page_is_exclusive {
            // The document-wide veto invalidates the narrow positive proof; keep
            // the public decision's ownership state consistent with its refusal.
            decision.ownership = IndirectObjectOwnership::Unproven;
            decision.disposition = IndirectObjectEditDisposition::PrivateCopy;
        }
        (occurrences, decision)
    }
}

pub(crate) fn inspection_is_complete(consumers: &ObjectConsumerIndexInspection) -> bool {
    consumers.truncations.is_empty()
        && consumers.unresolved_edges.is_empty()
        && consumers.skipped.iter().all(|skip| {
            matches!(
                skip,
                SkippedObjectConsumerScan::UnreferencedEntryUnresolvable { .. }
            )
        })
}
