use presslint_types::{ByteRange, ColorSpace, PdfName};

use super::{json, walk};
use crate::{DecodedRange, GraphicsWalkErrorKind, PaintOpKind, PathPaintKind};

#[test]
fn gs_operator_emits_set_ext_g_state_event() -> Result<(), String> {
    let events = walk(b"/GS1 gs").map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        events[0].kind,
        PaintOpKind::SetExtGState {
            name: PdfName(b"GS1".to_vec()),
        }
    );
    assert_eq!(events[0].index, 0);
    assert_eq!(
        events[0].record_range,
        DecodedRange::new(ByteRange { start: 0, end: 7 })
    );
    assert_eq!(
        events[0].operator_range,
        DecodedRange::new(ByteRange { start: 5, end: 7 })
    );
    // `gs` only surfaces the invocation; the snapshot is left at the page default.
    assert_eq!(
        events[0].state.as_ref(),
        &crate::GraphicsStateSnapshot::page_default()
    );
    Ok(())
}

#[test]
fn set_ext_g_state_serializes_with_snake_case_kind_tag() -> Result<(), json::JsonError> {
    use serde::Serialize;

    // Pin whatever the `rename_all = "snake_case"` attribute actually emits for
    // the variant tag, rather than hand-asserting the string elsewhere.
    let kind = PaintOpKind::SetExtGState {
        name: PdfName(b"GS1".to_vec()),
    };
    let encoded = kind.serialize(json::JsonSerializer)?;

    assert_eq!(
        encoded,
        json::Json::object([
            ("kind", json::Json::string("set_ext_g_state")),
            (
                "name",
                json::Json::array(
                    b"GS1"
                        .iter()
                        .copied()
                        .map(|byte| json::Json::U32(u32::from(byte))),
                ),
            ),
        ])
    );
    Ok(())
}

#[test]
fn malformed_gs_operand_count_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"gs") else {
        return Err("gs with no name operand should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedOperandCount {
            operator: b"gs".to_vec(),
            expected: 1,
            got: 0,
        }
    );
    Ok(())
}

#[test]
fn malformed_gs_name_operand_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"42 gs") else {
        return Err("gs with non-name operand should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedNameOperand {
            operator: b"gs".to_vec(),
            operand_index: 0,
        }
    );
    assert_eq!(err.range, DecodedRange::new(ByteRange { start: 0, end: 2 }));
    Ok(())
}

#[test]
fn gs_invocation_across_save_restore_does_not_corrupt_state() -> Result<(), String> {
    // `gs` leaves the snapshot untouched, so invoking it inside a `q`...`Q`
    // block must not perturb the saved state that `Q` restores.
    let events = walk(b"1 0 0 rg q /GS1 gs Q f").map_err(|error| format!("{error:?}"))?;

    // Event order: `rg`(0) `q`(1) `gs`(2) `Q`(3) `f`(4).
    assert_eq!(
        events[2].kind,
        PaintOpKind::SetExtGState {
            name: PdfName(b"GS1".to_vec()),
        }
    );

    let final_event = events.last().ok_or("missing final event")?;
    assert_eq!(
        final_event.kind,
        PaintOpKind::PathPaint {
            paint: PathPaintKind::FillNonzero,
        }
    );
    assert_eq!(
        final_event.state.nonstroking_color.space,
        ColorSpace::DeviceRgb
    );
    assert_eq!(
        final_event.state.nonstroking_color.components,
        vec![1.0, 0.0, 0.0]
    );
    Ok(())
}
