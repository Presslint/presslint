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

const fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
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
