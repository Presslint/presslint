//! Request-atomic clone commit transaction, held one step short of the
//! writer: the successful staged export's fresh bodies PLUS one corroborated
//! planned dirty page object per affected page, materialized together or not
//! at all.
//!
//! The batch exists so no intermediate revision can ever contain clone
//! bodies without every page retarget (dark unreachable objects a
//! consolidating rewriter would silently discard). Production builds the
//! batch immediately after the staged export and deliberately DROPS it:
//! emitted product bytes stay byte-identical to the staged-export-only
//! behaviour, and no fresh object or page-retarget object is handed to any
//! production writer. The activation slice consumes exactly this family
//! through a crate-private dirty+fresh writer bridge.
//!
//! # Corroboration (all must hold, fail-closed)
//!
//! The staged body count must cover the planned member total exactly; every
//! planned set must carry its retained page ownership proof
//! (`InPlaceMutation` over the sole `/Parent`, proven before reservation)
//! and at least one retarget site; every root must map exactly once through
//! its SET-LOCAL source-to-fresh pairs to a generation-zero fresh identity
//! (no request-global map — pikepdf#271 class); sets are grouped by EXACT
//! page identity (ordinal, leaf reference, retained uncompressed offset) and
//! distinct groups must anchor distinct page objects; every page is
//! re-resolved ONCE at its retained offset with a corroborated header and
//! readable dictionary; every site key/value range must sit inside that
//! dictionary, the key bytes must still spell the retained entry name, and
//! the value must parse exactly and wholly as the expected old target;
//! duplicate or overlapping sites refuse.
//!
//! # Materialization
//!
//! The page dictionary is copied ONCE per unique proven page, into one
//! exactly sized buffer: after the overlap refusal the ascending disjoint
//! site value ranges partition the dictionary into verbatim segments and
//! per-site replacements, materialized in one sequential pass with checked
//! offset arithmetic — linear in the dictionary and replacement bytes
//! regardless of site count. ONLY each site's value range becomes the mapped
//! generation-zero fresh identity — every non-target byte stays verbatim.
//! One existing [`PlannedDirtyObject`] with
//! [`MutationBoundary::DictionaryEntry`] `Replace`/`ExistingValue`
//! boundaries (carrying the retained page ownership proof) is produced per
//! page, in deterministic plan order.
//!
//! # Atomicity
//!
//! Any failure returns one typed [`CloneCommitRefusal`] and discards the
//! ENTIRE batch — moved staged bodies included. No prefix/middle/suffix
//! salvage, no second reservation, no per-set retry.

// Crate-private commit mechanics inside a private module: `pub(crate)` is
// the deliberate crate-wide seam the pipeline and tests consume.
#![allow(clippy::redundant_pub_crate)]

use std::collections::BTreeSet;
use std::ops::Range;

use presslint_actions::{
    DictionaryEntryOp, DictionaryValueLocator, MutationBoundary, PlannedDirtyObject,
    PlannedValueProvenance,
};
use presslint_pdf::{
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectRef,
    inspect_indirect_object_dictionary, parse_indirect_reference,
};
use presslint_types::{ByteRange, PdfName};

use super::{
    CloneSetOutcome, CloneSetPageIdentity, CloneSetRetargetSite, FormCloneSet, FormCloneSetPlan,
};
use crate::writer::FreshObjectBytes;

/// The fully validated request-atomic clone commit transaction: staged fresh
/// bodies, page-root retargets, and the compact per-set metadata the
/// activation slice needs. No independent vector escapes this batch; the
/// staged bodies are MOVED in, never copied.
#[derive(Debug)]
pub(crate) struct CloneCommitBatch {
    /// Validated fresh bodies moved from the staged export, in plan order
    /// (set order, then member source-reference order).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fresh_objects: Vec<FreshObjectBytes>,
    /// One corroborated dirty page object per affected page, in plan order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) page_retargets: Vec<PlannedDirtyObject>,
    /// Compact per-set commit metadata, in plan order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) sets: Vec<CommittedCloneSet>,
}

/// Compact metadata for one committed clone set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommittedCloneSet {
    /// Exact anchoring page identity.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) page: CloneSetPageIdentity,
    /// Source root Form reference.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) root: IndirectRef,
    /// The root's mapped generation-zero fresh identity (set-local mapping;
    /// page-specific sets for one shared source keep distinct identities).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fresh_root: IndirectRef,
    /// Retargeted `/XObject` entry count on this set's page.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) retarget_sites: usize,
}

/// Typed reason the ENTIRE clone commit batch was discarded, fail-closed.
/// Exactly the FIRST refusal in deterministic plan/site order is retained.
/// This class is distinct from the closure, reservation, and export refusal
/// classes: a commit failure never masquerades as any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneCommitRefusal {
    /// The staged fresh-body count does not cover the planned member total
    /// exactly (defensive: the export builds the batch in plan order).
    StagedBatchMisaligned {
        /// Planned member total across every planned set.
        #[cfg_attr(not(test), allow(dead_code))]
        expected: usize,
        /// Staged fresh bodies supplied.
        #[cfg_attr(not(test), allow(dead_code))]
        found: usize,
    },
    /// A planned set lacks its retained page ownership proof, or the proof
    /// does not name this page with `InPlaceMutation` (defensive: plan
    /// admission requires the proof before reservation).
    PageOwnershipMissing {
        /// The page whose proof is missing or inconsistent.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// A planned set carries no retarget site (defensive: seeding always
    /// records at least one witnessed binding entry).
    NoRetargetSites {
        /// The set root.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// The set root does not map exactly once through its set-local
    /// source-to-fresh pairs.
    RootMappingInvalid {
        /// The set root.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// The root's mapped fresh identity carries a nonzero generation.
    FreshRootNotGenerationZero {
        /// The offending fresh identity.
        #[cfg_attr(not(test), allow(dead_code))]
        fresh: IndirectRef,
    },
    /// Two distinct page-identity groups anchor the same page object
    /// (defensive: the pre-reservation page proof refuses repeated leaves).
    PageIdentityCollision {
        /// The colliding page reference.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// The page did not re-resolve at its retained uncompressed offset with
    /// the retained header identity and a readable dictionary.
    PageResolutionMismatch {
        /// The page that failed to re-resolve.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// A site key/value range does not sit inside the re-inspected page
    /// dictionary in key-then-value order.
    SiteOutsideDictionary {
        /// The page whose site fell outside its dictionary.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// A site's key bytes no longer spell the retained entry name.
    SiteKeyMismatch {
        /// The page whose site key failed to corroborate.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// A site's value range does not parse exactly and wholly as the
    /// expected old target reference.
    SiteValueMismatch {
        /// The page whose site value failed to corroborate.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// A site's expected target is not its set's root (defensive: seeding
    /// groups sites by witnessed target).
    SiteTargetNotRoot {
        /// The set root the site should have named.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// Two sites on one page duplicate or overlap value ranges.
    SiteOverlap {
        /// The page carrying the colliding sites.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
    /// Checked splice offset arithmetic failed (defensive; never expected
    /// after the in-dictionary range corroboration).
    SpliceArithmetic {
        /// The page whose splice arithmetic failed.
        #[cfg_attr(not(test), allow(dead_code))]
        page: IndirectRef,
    },
}

/// Build the request-atomic commit batch over every planned set, in plan
/// order, or discard everything — moved staged bodies included — with one
/// typed refusal.
pub(crate) fn build_clone_commit_batch(
    input: &[u8],
    plan: &FormCloneSetPlan,
    fresh_objects: Vec<FreshObjectBytes>,
) -> Result<CloneCommitBatch, CloneCommitRefusal> {
    let planned_total = plan.planned_member_total();
    if planned_total != fresh_objects.len() {
        return Err(CloneCommitRefusal::StagedBatchMisaligned {
            expected: planned_total,
            found: fresh_objects.len(),
        });
    }

    // Group planned sets by EXACT page identity in plan order (sets are
    // already sorted by page ordinal then root, so groups are consecutive).
    let mut summaries = Vec::new();
    let mut groups: Vec<(CloneSetPageIdentity, Vec<(&FormCloneSet, IndirectRef)>)> = Vec::new();
    for set in &plan.sets {
        let CloneSetOutcome::Planned {
            source_to_fresh, ..
        } = &set.outcome
        else {
            continue;
        };
        let fresh_root = validate_set_commit(set, source_to_fresh)?;
        summaries.push(CommittedCloneSet {
            page: set.page,
            root: set.root,
            fresh_root,
            retarget_sites: set.retarget_sites.len(),
        });
        match groups.last_mut() {
            Some((identity, sets)) if *identity == set.page => sets.push((set, fresh_root)),
            _ => groups.push((set.page, vec![(set, fresh_root)])),
        }
    }

    // Distinct groups must anchor distinct page objects, or two dirty page
    // objects would collide on one identity.
    let mut group_pages = BTreeSet::new();
    for (identity, _) in &groups {
        if !group_pages.insert(identity.reference) {
            return Err(CloneCommitRefusal::PageIdentityCollision {
                page: identity.reference,
            });
        }
    }

    let mut page_retargets = Vec::with_capacity(groups.len());
    for (identity, sets) in &groups {
        page_retargets.push(retarget_page(input, *identity, sets)?);
    }

    Ok(CloneCommitBatch {
        fresh_objects,
        page_retargets,
        sets: summaries,
    })
}

/// Validate one planned set's commit prerequisites and resolve the root's
/// set-local generation-zero fresh identity.
fn validate_set_commit(
    set: &FormCloneSet,
    source_to_fresh: &[(IndirectRef, IndirectRef)],
) -> Result<IndirectRef, CloneCommitRefusal> {
    let ownership_proven = set.page_ownership.as_ref().is_some_and(|decision| {
        decision.target == set.page.reference
            && decision.disposition == IndirectObjectEditDisposition::InPlaceMutation
    });
    if !ownership_proven {
        return Err(CloneCommitRefusal::PageOwnershipMissing {
            page: set.page.reference,
        });
    }
    if set.retarget_sites.is_empty() {
        return Err(CloneCommitRefusal::NoRetargetSites { root: set.root });
    }
    let mut mapped = source_to_fresh
        .iter()
        .filter(|(source, _)| *source == set.root);
    let Some((_, fresh_root)) = mapped.next() else {
        return Err(CloneCommitRefusal::RootMappingInvalid { root: set.root });
    };
    if mapped.next().is_some() {
        return Err(CloneCommitRefusal::RootMappingInvalid { root: set.root });
    }
    if fresh_root.generation != 0 {
        return Err(CloneCommitRefusal::FreshRootNotGenerationZero { fresh: *fresh_root });
    }
    Ok(*fresh_root)
}

/// Re-resolve one proven page once, corroborate every site inside its
/// dictionary, and materialize the single dirty page object with one exactly
/// sized single-pass segment copy and one `DictionaryEntry` boundary per
/// site.
fn retarget_page(
    input: &[u8],
    identity: CloneSetPageIdentity,
    sets: &[(&FormCloneSet, IndirectRef)],
) -> Result<PlannedDirtyObject, CloneCommitRefusal> {
    let page = identity.reference;
    let Ok(dictionary) = inspect_indirect_object_dictionary(input, identity.object_byte_offset)
    else {
        return Err(CloneCommitRefusal::PageResolutionMismatch { page });
    };
    if dictionary.reference != page {
        return Err(CloneCommitRefusal::PageResolutionMismatch { page });
    }
    let dict_open = dictionary.dictionary_open_byte_offset;
    let dict_end = dictionary.after_dictionary_close_byte_offset;

    let mut splices: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
    let mut boundaries = Vec::new();
    for (set, fresh_root) in sets {
        let ownership = set
            .page_ownership
            .clone()
            .ok_or(CloneCommitRefusal::PageOwnershipMissing { page })?;
        let replacement = format!("{} 0 R", fresh_root.object_number).into_bytes();
        for site in &set.retarget_sites {
            corroborate_site(input, dict_open, dict_end, set.root, site, page)?;
            splices.push((
                site.value_range.start..site.value_range.end,
                replacement.clone(),
            ));
            boundaries.push(site_boundary(page, site, &ownership, set.root));
        }
    }
    // One sort by (start, end) serves both halves: adjacent-pair comparison
    // refuses duplicate or overlapping value ranges, and the surviving
    // ascending disjoint order is exactly what the segment copy consumes.
    splices.sort_unstable_by_key(|(range, _)| (range.start, range.end));
    for pair in splices.windows(2) {
        if pair[1].0.start < pair[0].0.end || pair[0].0 == pair[1].0 {
            return Err(CloneCommitRefusal::SiteOverlap { page });
        }
    }

    // Copy the page dictionary ONCE into one exactly sized buffer: after the
    // overlap refusal the ascending disjoint site ranges partition the
    // dictionary into verbatim segments and per-site replacements, so one
    // sequential pass materializes the body without moving any tail —
    // linear in the dictionary and replacement bytes regardless of site
    // count. Checked arithmetic throughout; a slice miss fails closed.
    let arithmetic = || CloneCommitRefusal::SpliceArithmetic { page };
    let mut final_len = dict_end.checked_sub(dict_open).ok_or_else(arithmetic)?;
    for (range, replacement) in &splices {
        final_len = final_len
            .checked_sub(range.len())
            .and_then(|kept| kept.checked_add(replacement.len()))
            .ok_or_else(arithmetic)?;
    }
    let mut body = Vec::with_capacity(final_len);
    let mut cursor = dict_open;
    for (range, replacement) in &splices {
        let verbatim = input.get(cursor..range.start).ok_or_else(arithmetic)?;
        body.extend_from_slice(verbatim);
        body.extend_from_slice(replacement);
        cursor = range.end;
    }
    body.extend_from_slice(input.get(cursor..dict_end).ok_or_else(arithmetic)?);
    if body.len() != final_len {
        return Err(arithmetic());
    }

    Ok(PlannedDirtyObject {
        reference: page,
        boundaries,
        body_bytes: body,
    })
}

/// Corroborate one retained retarget site against the re-inspected page
/// dictionary: in-dictionary key-then-value ranges, exact retained key
/// spelling, and a value that parses exactly and wholly as the expected old
/// target — which must be this set's root.
fn corroborate_site(
    input: &[u8],
    dict_open: usize,
    dict_end: usize,
    root: IndirectRef,
    site: &CloneSetRetargetSite,
    page: IndirectRef,
) -> Result<(), CloneCommitRefusal> {
    let key = site.key_range;
    let value = site.value_range;
    let in_dictionary = dict_open <= key.start
        && key.start < key.end
        && key.end <= value.start
        && value.start < value.end
        && value.end <= dict_end;
    if !in_dictionary {
        return Err(CloneCommitRefusal::SiteOutsideDictionary { page });
    }

    let key_matches = input
        .get(key.start..key.end)
        .and_then(|raw| raw.strip_prefix(b"/"))
        .is_some_and(|raw_name| raw_name == site.name.0.as_slice());
    if !key_matches {
        return Err(CloneCommitRefusal::SiteKeyMismatch { page });
    }

    let Ok(parsed) = parse_indirect_reference(input, value.start) else {
        return Err(CloneCommitRefusal::SiteValueMismatch { page });
    };
    if parsed.reference != site.expected_target
        || parsed.reference_range.start != value.start
        || parsed.reference_range.end != value.end
    {
        return Err(CloneCommitRefusal::SiteValueMismatch { page });
    }
    if site.expected_target != root {
        return Err(CloneCommitRefusal::SiteTargetNotRoot { root });
    }
    Ok(())
}

/// Build one site's `Replace`/`ExistingValue` dictionary-entry boundary,
/// carrying the retained page ownership proof and the clone provenance.
fn site_boundary(
    page: IndirectRef,
    site: &CloneSetRetargetSite,
    ownership: &IndirectObjectEditDecision,
    root: IndirectRef,
) -> MutationBoundary {
    MutationBoundary::DictionaryEntry {
        target: page,
        key: PdfName(site.name.0.clone()),
        op: DictionaryEntryOp::Replace,
        value_locator: DictionaryValueLocator::ExistingValue {
            key_range: ByteRange {
                start: site.key_range.start,
                end: site.key_range.end,
            },
            value_range: ByteRange {
                start: site.value_range.start,
                end: site.value_range.end,
            },
        },
        ownership: ownership.clone(),
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: root },
    }
}
