use std::borrow::Cow;

pub fn count_leading_digits(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

pub fn parse_usize_decimal(bytes: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    for byte in bytes {
        let digit = usize::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

pub fn parse_u64_decimal(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for byte in bytes {
        let digit = u64::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

/// Components of an `N G <keyword>` shape parsed at a caller-supplied offset.
///
/// Shared by the indirect-object-header and indirect-reference helpers, which
/// differ only in the trailing keyword (`obj` vs `R`), the scan window size, and
/// the public report/error types they wrap these components in.
pub struct ObjectReferenceShape {
    pub object_number: u32,
    pub generation: u16,
    /// Offset where the shape begins after optional leading PDF whitespace.
    pub reference_byte_offset: usize,
    /// Offset immediately after the trailing keyword.
    pub after_keyword_offset: usize,
}

/// Reason an `N G <keyword>` shape parse failed.
pub enum ObjectReferenceShapeRejection {
    OffsetOutOfBounds,
    Malformed,
    ObjectNumberOutOfRange,
    GenerationOutOfRange,
}

/// Failure of [`parse_object_reference_shape`], carrying the offset of the
/// malformed construct when one is known.
pub struct ObjectReferenceShapeError {
    pub error_byte_offset: Option<usize>,
    pub reason: ObjectReferenceShapeRejection,
}

/// Parse an `object-number generation keyword` shape at `byte_offset`.
///
/// Skips optional leading PDF whitespace, bounds the scan to a fixed
/// `scan_limit` leading window, and validates the trailing `keyword` with the
/// shared keyword-boundary rule so trailing bytes (e.g. `Robot`) are rejected.
/// It retains no input bytes and reports only the parsed numbers and structural
/// offsets.
pub fn parse_object_reference_shape(
    input: &[u8],
    byte_offset: usize,
    scan_limit: usize,
    keyword: &[u8],
) -> Result<ObjectReferenceShape, ObjectReferenceShapeError> {
    if byte_offset >= input.len() {
        return Err(ObjectReferenceShapeError {
            error_byte_offset: None,
            reason: ObjectReferenceShapeRejection::OffsetOutOfBounds,
        });
    }

    let window_end = byte_offset.saturating_add(scan_limit).min(input.len());
    let window = &input[byte_offset..window_end];
    let leading_whitespace = skip_whitespace(window);
    let reference_byte_offset = byte_offset + leading_whitespace;
    let content = &window[leading_whitespace..];

    let object_digits = count_leading_digits(content);
    if object_digits == 0 {
        return Err(malformed_shape(reference_byte_offset));
    }
    let after_object = &content[object_digits..];

    let object_generation_gap = skip_whitespace(after_object);
    if object_generation_gap == 0 {
        return Err(malformed_shape(reference_byte_offset));
    }
    let generation_field = &after_object[object_generation_gap..];

    let generation_digits = count_leading_digits(generation_field);
    if generation_digits == 0 {
        return Err(malformed_shape(
            reference_byte_offset + object_digits + object_generation_gap,
        ));
    }
    let after_generation = &generation_field[generation_digits..];

    let generation_keyword_gap = skip_whitespace(after_generation);
    if generation_keyword_gap == 0 {
        return Err(malformed_shape(
            reference_byte_offset + object_digits + object_generation_gap + generation_digits,
        ));
    }
    let keyword_offset = reference_byte_offset
        + object_digits
        + object_generation_gap
        + generation_digits
        + generation_keyword_gap;
    let keyword_content = &after_generation[generation_keyword_gap..];
    if consume_keyword(keyword_content, keyword).is_none() {
        return Err(malformed_shape(keyword_offset));
    }

    let object_number = parse_u64_decimal(&content[..object_digits])
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ObjectReferenceShapeError {
            error_byte_offset: Some(reference_byte_offset),
            reason: ObjectReferenceShapeRejection::ObjectNumberOutOfRange,
        })?;
    let generation = parse_u64_decimal(&generation_field[..generation_digits])
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(ObjectReferenceShapeError {
            error_byte_offset: Some(reference_byte_offset + object_digits + object_generation_gap),
            reason: ObjectReferenceShapeRejection::GenerationOutOfRange,
        })?;

    Ok(ObjectReferenceShape {
        object_number,
        generation,
        reference_byte_offset,
        after_keyword_offset: keyword_offset + keyword.len(),
    })
}

const fn malformed_shape(error_byte_offset: usize) -> ObjectReferenceShapeError {
    ObjectReferenceShapeError {
        error_byte_offset: Some(error_byte_offset),
        reason: ObjectReferenceShapeRejection::Malformed,
    }
}

pub fn consume_keyword(bytes: &[u8], keyword: &[u8]) -> Option<usize> {
    let after_keyword = bytes.strip_prefix(keyword)?;
    if after_keyword
        .first()
        .is_some_and(|byte| !is_pdf_whitespace(*byte) && !is_pdf_delimiter(*byte))
    {
        return None;
    }
    Some(keyword.len())
}

pub fn consume_line_end(input: &[u8], mut cursor: usize, allow_now: bool) -> Option<usize> {
    let mut allow_line_end = allow_now;
    while let Some(byte) = input.get(cursor) {
        match *byte {
            b'\r' if allow_line_end || input.get(cursor + 1) == Some(&b'\n') => {
                let after_cr = cursor + 1;
                return Some(if input.get(after_cr) == Some(&b'\n') {
                    after_cr + 1
                } else {
                    after_cr
                });
            }
            b'\n' if allow_line_end => return Some(cursor + 1),
            byte if is_pdf_whitespace(byte) && !matches!(byte, b'\r' | b'\n') => {
                cursor += 1;
                allow_line_end = true;
            }
            _ => return None,
        }
    }
    None
}

pub fn skip_whitespace(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| !is_pdf_whitespace(*byte))
        .unwrap_or(bytes.len())
}

pub const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

pub const fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Decode the bytes of one PDF name object after its leading `/`.
///
/// The unescaped fast path borrows the input. An escaped name allocates at most
/// one output buffer whose capacity is bounded by the raw name length. Every
/// `#` shall be followed by exactly two hexadecimal digits; truncated,
/// non-hexadecimal, and null-producing escapes fail closed. A literal null byte
/// is rejected for the same reason (ISO 32000-1 §7.3.5).
pub fn decode_pdf_name(name: &[u8]) -> Option<Cow<'_, [u8]>> {
    if !name.contains(&b'#') {
        return (!name.contains(&0)).then_some(Cow::Borrowed(name));
    }

    let mut decoded = Vec::with_capacity(name.len());
    let mut cursor = 0;
    while cursor < name.len() {
        if name[cursor] != b'#' {
            if name[cursor] == 0 {
                return None;
            }
            decoded.push(name[cursor]);
            cursor += 1;
            continue;
        }
        let high = hex_value(*name.get(cursor + 1)?)?;
        let low = hex_value(*name.get(cursor + 2)?)?;
        let byte = (high << 4) | low;
        if byte == 0 {
            return None;
        }
        decoded.push(byte);
        cursor += 3;
    }
    Some(Cow::Owned(decoded))
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn skip_name(input: &[u8], start: usize, limit: usize) -> usize {
    let mut cursor = start + 1;
    while cursor < limit && !is_pdf_whitespace(input[cursor]) && !is_pdf_delimiter(input[cursor]) {
        cursor += 1;
    }
    cursor
}

pub fn skip_scalar_token(input: &[u8], start: usize, limit: usize) -> usize {
    let mut cursor = start;
    while cursor < limit && !is_pdf_whitespace(input[cursor]) && !is_pdf_delimiter(input[cursor]) {
        cursor += 1;
    }
    if cursor == start { cursor + 1 } else { cursor }
}

pub fn skip_whitespace_and_comments(input: &[u8], mut cursor: usize, limit: usize) -> usize {
    loop {
        cursor += skip_whitespace(&input[cursor..limit]);
        if cursor < limit && input[cursor] == b'%' {
            cursor = skip_comment(input, cursor).min(limit);
            continue;
        }
        return cursor;
    }
}

/// Skip a literal string `( ... )` opaque span starting at its opening `(`.
///
/// Returns the exclusive byte offset just past the matching `)`, honoring `\`
/// escapes (the byte after a backslash never affects paren depth) and balanced
/// unescaped parentheses. Returns `None` if the string is unterminated before
/// EOF. Inner bytes are not decoded.
pub fn skip_literal_string(input: &[u8], open: usize) -> Option<usize> {
    let mut cursor = open + 1;
    let mut depth: usize = 1;
    while let Some(&byte) = input.get(cursor) {
        match byte {
            b'\\' => cursor += 2,
            b'(' => {
                depth += 1;
                cursor += 1;
            }
            b')' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

/// Skip a hex string `< ... >` opaque span starting at its opening `<`.
///
/// Returns the exclusive byte offset just past the closing `>`, or `None` if
/// the hex string is unterminated before EOF. Inner bytes are not decoded or
/// validated.
pub fn skip_hex_string(input: &[u8], open: usize) -> Option<usize> {
    let mut cursor = open + 1;
    while let Some(&byte) = input.get(cursor) {
        cursor += 1;
        if byte == b'>' {
            return Some(cursor);
        }
    }
    None
}

/// Skip a `%` comment to the end of its line.
///
/// Returns the byte offset of the terminating end-of-line byte (or EOF). The
/// terminating `\r`/`\n` byte itself is not consumed.
pub fn skip_comment(input: &[u8], start: usize) -> usize {
    let mut cursor = start;
    while let Some(&byte) = input.get(cursor) {
        if matches!(byte, b'\r' | b'\n') {
            break;
        }
        cursor += 1;
    }
    cursor
}

pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}
