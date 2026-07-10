//! Exact logical page-content bytes with physical occurrence provenance.

use std::collections::BTreeMap;

use presslint_paint::{ColorSpaceEnv, DecodedRange, PaintProgram};
use presslint_pdf::{IndirectObjectEditDisposition, IndirectRef};
use presslint_syntax::{OperatorRecord, Token, TokenKind, assemble_operators, tokenize};
use presslint_types::ByteRange;

/// One physical `/Contents` occurrence supplied in source order.
pub struct OccurrenceInput<'a> {
    pub stream_ordinal: usize,
    pub content_object: IndirectRef,
    pub decoded: &'a [u8],
    pub disposition: IndirectObjectEditDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSplice {
    pub range: ByteRange,
    pub replacement: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalObjectPlan {
    pub content_object: IndirectRef,
    pub splices: Vec<LocalSplice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalizedRange {
    pub occurrence_index: usize,
    pub stream_ordinal: usize,
    pub content_object: IndirectRef,
    pub local_range: ByteRange,
    pub disposition: IndirectObjectEditDisposition,
}

#[derive(Debug, Clone)]
struct Occurrence {
    stream_ordinal: usize,
    content_object: IndirectRef,
    logical_range: ByteRange,
    disposition: IndirectObjectEditDisposition,
}

/// One exact decoded page sequence, parsed globally, with ordered physical spans.
pub struct PageContentSequence {
    logical: Vec<u8>,
    tokens: Vec<Token>,
    records: Vec<OperatorRecord>,
    occurrences: Vec<Occurrence>,
}

impl PageContentSequence {
    pub(crate) fn new(inputs: &[OccurrenceInput<'_>], cap: usize) -> Option<Self> {
        let total = inputs.iter().try_fold(0usize, |total, input| {
            total
                .checked_add(input.decoded.len())
                .filter(|size| *size <= cap)
        })?;
        let mut logical = Vec::with_capacity(total);
        let mut occurrences = Vec::with_capacity(inputs.len());
        for input in inputs {
            let start = logical.len();
            logical.extend_from_slice(input.decoded);
            occurrences.push(Occurrence {
                stream_ordinal: input.stream_ordinal,
                content_object: input.content_object,
                logical_range: ByteRange {
                    start,
                    end: logical.len(),
                },
                disposition: input.disposition,
            });
        }
        Self::parse(logical, occurrences)
    }

    fn parse(logical: Vec<u8>, occurrences: Vec<Occurrence>) -> Option<Self> {
        let tokens = tokenize(&logical).ok()?;
        validate_token_coverage(&tokens, logical.len())?;
        validate_boundaries(&tokens, &occurrences)?;
        let records = assemble_operators(&tokens).ok()?.records;
        Some(Self {
            logical,
            tokens,
            records,
            occurrences,
        })
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.logical
    }

    pub(crate) fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub(crate) fn records(&self) -> &[OperatorRecord] {
        &self.records
    }

    pub const fn occurrence_count(&self) -> usize {
        self.occurrences.len()
    }

    pub(crate) fn occurrence_object(&self, index: usize) -> Option<IndirectRef> {
        self.occurrences.get(index).map(|item| item.content_object)
    }

    pub(crate) fn occurrence_disposition(
        &self,
        index: usize,
    ) -> Option<IndirectObjectEditDisposition> {
        self.occurrences.get(index).map(|item| item.disposition)
    }

    pub(crate) fn localize(&self, range: DecodedRange) -> Option<LocalizedRange> {
        self.localize_bytes(range.into_byte_range())
    }

    pub(crate) fn localize_bytes(&self, range: ByteRange) -> Option<LocalizedRange> {
        if range.start >= range.end || range.end > self.logical.len() {
            return None;
        }
        self.occurrences
            .iter()
            .enumerate()
            .find(|(_, occurrence)| {
                range.start >= occurrence.logical_range.start
                    && range.end <= occurrence.logical_range.end
            })
            .map(|(occurrence_index, occurrence)| LocalizedRange {
                occurrence_index,
                stream_ordinal: occurrence.stream_ordinal,
                content_object: occurrence.content_object,
                local_range: ByteRange {
                    start: range.start - occurrence.logical_range.start,
                    end: range.end - occurrence.logical_range.start,
                },
                disposition: occurrence.disposition,
            })
    }

    /// Reconcile complete plans for repeated physical references.
    pub(crate) fn reconcile(
        &self,
        mut plans: Vec<Vec<LocalSplice>>,
    ) -> Option<Vec<PhysicalObjectPlan>> {
        if plans.len() != self.occurrences.len() {
            return None;
        }
        for plan in &mut plans {
            plan.sort_by_key(|splice| (splice.range.start, splice.range.end));
            if plan
                .iter()
                .any(|splice| splice.range.start >= splice.range.end)
                || plan
                    .windows(2)
                    .any(|pair| pair[0].range.end > pair[1].range.start)
            {
                return None;
            }
        }
        let mut first_by_object: BTreeMap<IndirectRef, usize> = BTreeMap::new();
        let mut reconciled = Vec::new();
        for (index, occurrence) in self.occurrences.iter().enumerate() {
            if let Some(first) = first_by_object.get(&occurrence.content_object) {
                if plans[*first] != plans[index] {
                    return None;
                }
                continue;
            }
            first_by_object.insert(occurrence.content_object, index);
            reconciled.push(PhysicalObjectPlan {
                content_object: occurrence.content_object,
                splices: plans[index].clone(),
            });
        }
        Some(reconciled)
    }

    /// Rebuild and globally validate the edited logical sequence.
    pub(crate) fn validate_edited(
        self,
        decoded_by_object: &BTreeMap<IndirectRef, &[u8]>,
        cap: usize,
    ) -> bool {
        let occurrences = self.occurrences;
        drop(self.logical);
        drop(self.tokens);
        drop(self.records);
        let mut inputs = Vec::with_capacity(occurrences.len());
        for occurrence in &occurrences {
            let Some(decoded) = decoded_by_object.get(&occurrence.content_object) else {
                return false;
            };
            inputs.push(OccurrenceInput {
                stream_ordinal: occurrence.stream_ordinal,
                content_object: occurrence.content_object,
                decoded,
                disposition: occurrence.disposition,
            });
        }
        let Some(edited) = Self::new(&inputs, cap) else {
            return false;
        };
        PaintProgram::new(edited.bytes(), edited.records(), ColorSpaceEnv::empty())
            .ops()
            .all(|op| op.is_ok())
    }
}

fn validate_token_coverage(tokens: &[Token], len: usize) -> Option<()> {
    let mut cursor = 0usize;
    for token in tokens {
        if token.range.start != cursor || token.range.end < token.range.start {
            return None;
        }
        cursor = token.range.end;
    }
    (cursor == len).then_some(())
}

fn validate_boundaries(tokens: &[Token], occurrences: &[Occurrence]) -> Option<()> {
    for boundary in occurrences.iter().take(occurrences.len().saturating_sub(1)) {
        let offset = boundary.logical_range.end;
        if let Some(token) = tokens
            .iter()
            .find(|token| token.range.start < offset && offset < token.range.end)
        {
            if !matches!(token.kind, TokenKind::Trivia(_)) {
                return None;
            }
        }
    }
    Some(())
}
