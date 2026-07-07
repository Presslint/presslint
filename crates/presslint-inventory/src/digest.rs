use presslint_types::{
    ByteRange, ColorObservation, ColorSpace, ColorUsage, ContentScope, InvocationPath, PageIndex,
    PdfName,
};

use presslint_paint::{PaintOp, PathPaintKind, TextRenderingMode, TextShowOperator};

pub fn vector_object_digest(
    page: PageIndex,
    sequence: u32,
    scope: &ContentScope,
    path: &InvocationPath,
    event: &PaintOp,
    paint: PathPaintKind,
    colors: &[ColorObservation],
) -> [u8; 32] {
    let mut digest = StableDigest::new();
    digest.push_bytes(b"presslint.vector.v3");
    push_event_header(&mut digest, page, sequence, scope, path, event);
    digest.push_u8(path_paint_tag(paint));
    for color in colors {
        digest.push_color_observation(color);
    }
    digest.finish()
}

#[allow(clippy::too_many_arguments)]
pub fn text_object_digest(
    page: PageIndex,
    sequence: u32,
    scope: &ContentScope,
    path: &InvocationPath,
    event: &PaintOp,
    operator: TextShowOperator,
    rendering_mode: TextRenderingMode,
    colors: &[ColorObservation],
) -> [u8; 32] {
    let mut digest = StableDigest::new();
    digest.push_bytes(b"presslint.text.v3");
    push_event_header(&mut digest, page, sequence, scope, path, event);
    digest.push_u8(text_show_operator_tag(operator));
    digest.push_text_rendering_mode(rendering_mode);
    for color in colors {
        digest.push_color_observation(color);
    }
    digest.finish()
}

pub fn image_object_digest(
    page: PageIndex,
    sequence: u32,
    scope: &ContentScope,
    path: &InvocationPath,
    event: &PaintOp,
    name: &PdfName,
    colors: &[ColorObservation],
) -> [u8; 32] {
    let mut digest = StableDigest::new();
    digest.push_bytes(b"presslint.image.v3");
    push_event_header(&mut digest, page, sequence, scope, path, event);
    digest.push_bytes(&name.0);
    for color in colors {
        digest.push_color_observation(color);
    }
    digest.finish()
}

pub fn form_object_digest(
    page: PageIndex,
    sequence: u32,
    scope: &ContentScope,
    path: &InvocationPath,
    event: &PaintOp,
    name: &PdfName,
) -> [u8; 32] {
    let mut digest = StableDigest::new();
    digest.push_bytes(b"presslint.form.v3");
    push_event_header(&mut digest, page, sequence, scope, path, event);
    digest.push_bytes(&name.0);
    digest.finish()
}

/// Push the identity header shared by every object digest (identity v3): page,
/// FINAL flat sequence, lexical scope, then the invocation path (length, then
/// each frame ordinal + resource-name bytes from the page inward), then the
/// event's record index and its record and operator ranges.
///
/// The invocation path folds the form-call chain of the paint instance into the
/// identity so distinct invocations of one shared form get distinct digests; a
/// page-level entry carries an empty path (length 0), which keeps the header
/// uniform across page and form entries.
///
/// The two paint ranges are decoded-buffer typed; the unwrap here is the
/// explicit identity-only seam, so digest input order and values match the
/// decoded-range bytes exactly.
fn push_event_header(
    digest: &mut StableDigest,
    page: PageIndex,
    sequence: u32,
    scope: &ContentScope,
    path: &InvocationPath,
    event: &PaintOp,
) {
    digest.push_u32(page.0);
    digest.push_u32(sequence);
    digest.push_scope(scope);
    digest.push_u32(usize_to_u32(path.frames.len()));
    for frame in &path.frames {
        digest.push_u32(frame.ordinal);
        digest.push_bytes(&frame.name.0);
    }
    digest.push_usize(event.index);
    digest.push_range(event.record_range.into_byte_range());
    digest.push_range(event.operator_range.into_byte_range());
}

#[derive(Debug, Clone)]
struct StableDigest {
    lanes: [u64; 4],
}

impl StableDigest {
    const fn new() -> Self {
        Self {
            lanes: [
                0xcbf2_9ce4_8422_2325,
                0x8422_2325_cbf2_9ce4,
                0x9e37_79b1_85eb_ca87,
                0xc2b2_ae3d_27d4_eb4f,
            ],
        }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        self.push_usize(bytes.len());
        for byte in bytes {
            self.push_u8(*byte);
        }
    }

    fn push_u8(&mut self, value: u8) {
        for (index, lane) in self.lanes.iter_mut().enumerate() {
            *lane ^= u64::from(value).wrapping_add((index as u64) << 8);
            *lane = lane.wrapping_mul(0x0100_0000_01b3);
            *lane ^= *lane >> 32;
        }
    }

    fn push_u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.push_u8(byte);
        }
    }

    fn push_u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.push_u8(byte);
        }
    }

    fn push_usize(&mut self, value: usize) {
        self.push_u64(usize_to_u64(value));
    }

    fn push_f64(&mut self, value: f64) {
        self.push_u64(value.to_bits());
    }

    fn push_range(&mut self, range: ByteRange) {
        self.push_usize(range.start);
        self.push_usize(range.end);
    }

    fn push_scope(&mut self, scope: &ContentScope) {
        match scope {
            ContentScope::Page => self.push_u8(0),
            ContentScope::FormXObject { name } => {
                self.push_u8(1);
                self.push_bytes(&name.0);
            }
            ContentScope::AnnotationAppearance => self.push_u8(2),
        }
    }

    fn push_color_observation(&mut self, color: &ColorObservation) {
        self.push_u8(color_usage_tag(color.usage));
        self.push_u8(color_space_tag(&color.space));
        if let ColorSpace::Resource(name) = &color.space {
            self.push_bytes(&name.0);
        }
        self.push_usize(color.components.len());
        for component in &color.components {
            self.push_f64(*component);
        }
        match &color.spot_name {
            Some(name) => {
                self.push_u8(1);
                self.push_bytes(&name.0);
            }
            None => self.push_u8(0),
        }
        // Compatibility: identity v3 deliberately ignores the additive
        // `spot_names` list and continues to hash only legacy `spot_name`.
        // Including second+ DeviceN colorants is a future identity-v4 choice.
        match color.source {
            Some(range) => {
                self.push_u8(1);
                self.push_range(range);
            }
            None => self.push_u8(0),
        }
    }

    fn push_text_rendering_mode(&mut self, mode: TextRenderingMode) {
        match mode {
            TextRenderingMode::Fill => self.push_u8(0),
            TextRenderingMode::Stroke => self.push_u8(1),
            TextRenderingMode::FillThenStroke => self.push_u8(2),
            TextRenderingMode::Invisible => self.push_u8(3),
            TextRenderingMode::Unsupported { value } => {
                self.push_u8(4);
                self.push_i32(value);
            }
        }
    }

    fn push_i32(&mut self, value: i32) {
        for byte in value.to_le_bytes() {
            self.push_u8(byte);
        }
    }

    fn finish(self) -> [u8; 32] {
        let mut out = [0; 32];
        for (chunk, lane) in out.chunks_exact_mut(8).zip(self.lanes) {
            chunk.copy_from_slice(&lane.to_le_bytes());
        }
        out
    }
}

pub fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

const fn path_paint_tag(paint: PathPaintKind) -> u8 {
    match paint {
        PathPaintKind::Stroke => 0,
        PathPaintKind::CloseAndStroke => 1,
        PathPaintKind::FillNonzero => 2,
        PathPaintKind::FillObsolete => 3,
        PathPaintKind::FillEvenOdd => 4,
        PathPaintKind::FillAndStrokeNonzero => 5,
        PathPaintKind::FillAndStrokeEvenOdd => 6,
        PathPaintKind::CloseFillAndStrokeNonzero => 7,
        PathPaintKind::CloseFillAndStrokeEvenOdd => 8,
        PathPaintKind::EndPath => 9,
    }
}

const fn text_show_operator_tag(operator: TextShowOperator) -> u8 {
    match operator {
        TextShowOperator::ShowText => 0,
        TextShowOperator::ShowTextAdjusted => 1,
        TextShowOperator::MoveNextLineAndShowText => 2,
        TextShowOperator::SetSpacingMoveNextLineAndShowText => 3,
    }
}

const fn color_usage_tag(usage: ColorUsage) -> u8 {
    match usage {
        ColorUsage::Fill => 0,
        ColorUsage::Stroke => 1,
        ColorUsage::Image => 2,
        ColorUsage::Shading => 3,
    }
}

const fn color_space_tag(space: &ColorSpace) -> u8 {
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
