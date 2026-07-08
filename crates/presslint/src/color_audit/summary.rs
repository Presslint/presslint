//! Deterministic aggregation: the mutable count accumulator, the shared
//! [`bump`] linear probe, and the fixed variant orders behind the sorted
//! summary vectors.

use std::cmp::Ordering;

use presslint_types::{ColorObservation, ColorSpace, ColorUsage, ObjectKind};

use super::report::{ColorSpaceCount, ColorUsageCount, ColorUsageSummary, ObjectKindCount};

/// Mutable count accumulator finalized into sorted [`ColorUsageSummary`] vectors.
///
/// Color spaces, usages, and kinds are all counted with the shared [`bump`]
/// linear probe and sorted into a fixed variant order by [`finish`].
///
/// [`finish`]: SummaryAccumulator::finish
#[derive(Default)]
pub(super) struct SummaryAccumulator {
    color_spaces: Vec<(ColorSpace, usize)>,
    usages: Vec<(ColorUsage, usize)>,
    kinds: Vec<(ObjectKind, usize)>,
}

impl SummaryAccumulator {
    pub(super) fn add_observation(&mut self, observation: &ColorObservation) {
        bump(&mut self.color_spaces, &observation.space);
        bump(&mut self.usages, &observation.usage);
    }

    pub(super) fn add_entry_kind(&mut self, kind: ObjectKind) {
        bump(&mut self.kinds, &kind);
    }

    pub(super) fn finish(mut self) -> ColorUsageSummary {
        self.color_spaces
            .sort_by(|a, b| color_space_cmp(&a.0, &b.0));
        self.usages
            .sort_by_key(|(usage, _)| color_usage_rank(*usage));
        self.kinds.sort_by_key(|(kind, _)| object_kind_rank(*kind));
        ColorUsageSummary {
            color_space_counts: self
                .color_spaces
                .into_iter()
                .map(|(color_space, count)| ColorSpaceCount { color_space, count })
                .collect(),
            color_usage_counts: self
                .usages
                .into_iter()
                .map(|(usage, count)| ColorUsageCount { usage, count })
                .collect(),
            object_kind_counts: self
                .kinds
                .into_iter()
                .map(|(kind, count)| ObjectKindCount { kind, count })
                .collect(),
        }
    }
}

/// Increment the count for `value`, appending a fresh slot the first time it is
/// seen. One helper for every count kind (color space, usage, object kind):
/// distinct keys are few, so this linear probe is cheaper than a hash map and
/// needs no `Hash`/`Ord` bound on the key type.
pub(super) fn bump<T: Clone + PartialEq>(counts: &mut Vec<(T, usize)>, value: &T) {
    if let Some(slot) = counts.iter_mut().find(|(existing, _)| existing == value) {
        slot.1 += 1;
    } else {
        counts.push((value.clone(), 1));
    }
}

/// Total order over color spaces: by a fixed variant rank, breaking `Resource`
/// ties by raw name bytes. The key type does not implement `Ord`, so this makes
/// the summary deterministic.
fn color_space_cmp(a: &ColorSpace, b: &ColorSpace) -> Ordering {
    color_space_rank(a)
        .cmp(&color_space_rank(b))
        .then_with(|| match (a, b) {
            (ColorSpace::Resource(x), ColorSpace::Resource(y)) => x.cmp(y),
            _ => Ordering::Equal,
        })
}

const fn color_space_rank(space: &ColorSpace) -> u8 {
    match space {
        ColorSpace::DeviceGray => 0,
        ColorSpace::DeviceRgb => 1,
        ColorSpace::DeviceCmyk => 2,
        ColorSpace::IccBased => 3,
        ColorSpace::Lab => 4,
        ColorSpace::CalGray => 5,
        ColorSpace::CalRgb => 6,
        ColorSpace::Indexed => 7,
        ColorSpace::Separation => 8,
        ColorSpace::DeviceN => 9,
        ColorSpace::Pattern => 10,
        ColorSpace::Resource(_) => 11,
        ColorSpace::Unknown => 12,
    }
}

const fn color_usage_rank(usage: ColorUsage) -> u8 {
    match usage {
        ColorUsage::Fill => 0,
        ColorUsage::Stroke => 1,
        ColorUsage::Image => 2,
        ColorUsage::Shading => 3,
    }
}

const fn object_kind_rank(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Text => 0,
        ObjectKind::Vector => 1,
        ObjectKind::Image => 2,
        ObjectKind::FormXObject => 3,
        ObjectKind::Shading => 4,
        ObjectKind::Pattern => 5,
        ObjectKind::Annotation => 6,
    }
}
