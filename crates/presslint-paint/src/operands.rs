use presslint_syntax::OperatorRecord;
use presslint_types::{ByteRange, ColorSpace, PdfName};

use crate::walker::{GraphicsColor, GraphicsWalkError, GraphicsWalkErrorKind};

pub fn checked_source(
    source: &[u8],
    range: ByteRange,
    error_range: ByteRange,
) -> Result<&[u8], GraphicsWalkError> {
    source.get(range.start..range.end).ok_or_else(|| {
        GraphicsWalkError::new(GraphicsWalkErrorKind::InvalidSourceRange, error_range)
    })
}

pub fn device_color(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
    space: ColorSpace,
    count: usize,
) -> Result<GraphicsColor, GraphicsWalkError> {
    Ok(GraphicsColor::new(
        space,
        numeric_operands_vec(source, operator, record, count)?,
    ))
}

/// Parse the operands of `sc`/`scn`/`SC`/`SCN`: a variable count of numeric
/// components with an optional trailing PDF name (pattern colour).
///
/// Numeric components must all precede the optional trailing name; a name in any
/// other position, or any other operand shape, is a malformed operand. Returns
/// the parsed components in source order plus the pattern name when present.
pub fn color_operands(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
) -> Result<(Vec<f64>, Option<PdfName>), GraphicsWalkError> {
    let mut components = Vec::with_capacity(record.operands.len());
    let mut pattern_name = None;
    for (operand_index, operand) in record.operands.iter().enumerate() {
        if pattern_name.is_some() {
            // No operand may follow the trailing pattern name.
            return Err(malformed_numeric(operator, operand_index, operand.range));
        }
        let bytes = checked_source(source, operand.range, operand.range)?;
        if operand.tokens.len() == 1 && bytes.first() == Some(&b'/') {
            if bytes.len() <= 1 {
                return Err(GraphicsWalkError::new(
                    GraphicsWalkErrorKind::MalformedNameOperand {
                        operator: operator.to_vec(),
                        operand_index,
                    },
                    operand.range,
                ));
            }
            pattern_name = Some(PdfName(bytes[1..].to_vec()));
            continue;
        }
        components.push(parse_finite_number(
            operator,
            operand_index,
            operand,
            bytes,
        )?);
    }
    Ok((components, pattern_name))
}

fn malformed_numeric(operator: &[u8], operand_index: usize, range: ByteRange) -> GraphicsWalkError {
    GraphicsWalkError::new(
        GraphicsWalkErrorKind::MalformedNumericOperand {
            operator: operator.to_vec(),
            operand_index,
        },
        range,
    )
}

fn parse_finite_number(
    operator: &[u8],
    operand_index: usize,
    operand: &presslint_syntax::OperandRecord,
    bytes: &[u8],
) -> Result<f64, GraphicsWalkError> {
    if operand.tokens.len() != 1 {
        return Err(malformed_numeric(operator, operand_index, operand.range));
    }
    let Ok(text) = core::str::from_utf8(bytes) else {
        return Err(malformed_numeric(operator, operand_index, operand.range));
    };
    let Ok(value) = text.parse::<f64>() else {
        return Err(malformed_numeric(operator, operand_index, operand.range));
    };
    if !value.is_finite() {
        return Err(GraphicsWalkError::new(
            GraphicsWalkErrorKind::NonFiniteNumericOperand {
                operator: operator.to_vec(),
                operand_index,
            },
            operand.range,
        ));
    }
    Ok(value)
}

pub fn expect_operands(
    operator: &[u8],
    record: &OperatorRecord,
    expected: usize,
) -> Result<(), GraphicsWalkError> {
    let got = record.operands.len();
    if got == expected {
        Ok(())
    } else {
        Err(GraphicsWalkError::new(
            GraphicsWalkErrorKind::MalformedOperandCount {
                operator: operator.to_vec(),
                expected,
                got,
            },
            record.range,
        ))
    }
}

pub fn numeric_operands(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
    expected: usize,
) -> Result<[f64; 6], GraphicsWalkError> {
    let operands = numeric_operands_vec(source, operator, record, expected)?;
    Ok([
        operands[0],
        operands[1],
        operands[2],
        operands[3],
        operands[4],
        operands[5],
    ])
}

pub fn integer_operand(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
) -> Result<i32, GraphicsWalkError> {
    let operands = numeric_operands_vec(source, operator, record, 1)?;
    let value = operands[0];
    if value.fract() != 0.0 || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(GraphicsWalkError::new(
            GraphicsWalkErrorKind::MalformedNumericOperand {
                operator: operator.to_vec(),
                operand_index: 0,
            },
            record.operands[0].range,
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(value as i32)
}

pub fn name_operand(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
) -> Result<PdfName, GraphicsWalkError> {
    expect_operands(operator, record, 1)?;
    let operand = &record.operands[0];
    if operand.tokens.len() != 1 {
        return Err(GraphicsWalkError::new(
            GraphicsWalkErrorKind::MalformedNameOperand {
                operator: operator.to_vec(),
                operand_index: 0,
            },
            operand.range,
        ));
    }
    let bytes = checked_source(source, operand.range, operand.range)?;
    if bytes.len() <= 1 || bytes[0] != b'/' {
        return Err(GraphicsWalkError::new(
            GraphicsWalkErrorKind::MalformedNameOperand {
                operator: operator.to_vec(),
                operand_index: 0,
            },
            operand.range,
        ));
    }
    Ok(PdfName(bytes[1..].to_vec()))
}

fn numeric_operands_vec(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
    expected: usize,
) -> Result<Vec<f64>, GraphicsWalkError> {
    expect_operands(operator, record, expected)?;
    record
        .operands
        .iter()
        .enumerate()
        .map(|(operand_index, operand)| {
            if operand.tokens.len() != 1 {
                return Err(GraphicsWalkError::new(
                    GraphicsWalkErrorKind::MalformedNumericOperand {
                        operator: operator.to_vec(),
                        operand_index,
                    },
                    operand.range,
                ));
            }
            let bytes = checked_source(source, operand.range, operand.range)?;
            let Ok(text) = core::str::from_utf8(bytes) else {
                return Err(GraphicsWalkError::new(
                    GraphicsWalkErrorKind::MalformedNumericOperand {
                        operator: operator.to_vec(),
                        operand_index,
                    },
                    operand.range,
                ));
            };
            let Ok(value) = text.parse::<f64>() else {
                return Err(GraphicsWalkError::new(
                    GraphicsWalkErrorKind::MalformedNumericOperand {
                        operator: operator.to_vec(),
                        operand_index,
                    },
                    operand.range,
                ));
            };
            if !value.is_finite() {
                return Err(GraphicsWalkError::new(
                    GraphicsWalkErrorKind::NonFiniteNumericOperand {
                        operator: operator.to_vec(),
                        operand_index,
                    },
                    operand.range,
                ));
            }
            Ok(value)
        })
        .collect()
}

#[allow(clippy::suboptimal_flops)]
pub fn concat_matrix(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    let [a1, b1, c1, d1, e1, f1] = m;
    let [a2, b2, c2, d2, e2, f2] = n;
    [
        a1 * a2 + b1 * c2,
        a1 * b2 + b1 * d2,
        c1 * a2 + d1 * c2,
        c1 * b2 + d1 * d2,
        e1 * a2 + f1 * c2 + e2,
        e1 * b2 + f1 * d2 + f2,
    ]
}
