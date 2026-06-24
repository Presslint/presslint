//! Byte-preserving content-stream syntax.

#![forbid(unsafe_code)]

use presslint_core::ByteRange;
use serde::{Deserialize, Serialize};

/// Lexical token with source byte range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// Token lexical class.
    pub kind: TokenKind,
    /// Source range in the content stream.
    pub range: ByteRange,
}

impl Token {
    /// Create a token with a lexical class and source range.
    #[must_use]
    pub const fn new(kind: TokenKind, range: ByteRange) -> Self {
        Self { kind, range }
    }

    /// Return this token's exact source bytes when the range is valid.
    #[must_use]
    pub fn source_bytes<'source>(&self, source: &'source [u8]) -> Option<&'source [u8]> {
        if self.range.start > self.range.end {
            return None;
        }
        source.get(self.range.start..self.range.end)
    }
}

/// Source-preserving PDF content-stream token class.
///
/// Variants classify lexical structure only. Parsed values remain in the
/// original byte stream and are addressed by each [`Token`]'s byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TokenKind {
    /// Whitespace or comment trivia.
    Trivia(TriviaKind),
    /// `true` or `false`.
    Boolean,
    /// `null`.
    Null,
    /// Integer or real numeric lexeme.
    Number(NumberKind),
    /// Literal or hexadecimal string.
    String(StringKind),
    /// PDF name beginning with `/`.
    Name,
    /// Structural delimiter.
    Delimiter(Delimiter),
    /// Object or stream keyword.
    Keyword(Keyword),
    /// Content-stream operator.
    Operator,
}

/// Non-semantic lexical trivia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriviaKind {
    /// One or more PDF whitespace bytes.
    Whitespace,
    /// `%` comment through end of line.
    Comment,
}

/// Numeric lexeme family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberKind {
    /// Integer number.
    Integer,
    /// Real number.
    Real,
}

/// String lexeme family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringKind {
    /// Parenthesized literal string.
    Literal,
    /// Angle-bracket hexadecimal string.
    Hexadecimal,
}

/// PDF syntax delimiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delimiter {
    /// `[`.
    ArrayOpen,
    /// `]`.
    ArrayClose,
    /// `<<`.
    DictionaryOpen,
    /// `>>`.
    DictionaryClose,
}

/// Keyword lexeme outside normal content operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keyword {
    /// `obj`.
    ObjectBegin,
    /// `endobj`.
    ObjectEnd,
    /// `stream`.
    StreamBegin,
    /// `endstream`.
    StreamEnd,
}

/// Tokenization failure with source range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenizeError {
    /// Error class.
    pub kind: TokenizeErrorKind,
    /// Source range that caused the error.
    pub range: ByteRange,
}

/// Structured tokenizer error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizeErrorKind {
    /// A literal string opened with `(` but did not close.
    UnterminatedLiteralString,
    /// A hexadecimal string opened with `<` but did not close.
    UnterminatedHexString,
}

/// Tokenize a PDF content byte stream into source-preserving lexical tokens.
///
/// This is a lexical scanner only. Token values remain encoded in the source
/// bytes and are addressed through each token's byte range.
///
/// # Errors
///
/// Returns a [`TokenizeError`] when a string token is unterminated.
pub fn tokenize(input: &[u8]) -> Result<Vec<Token>, TokenizeError> {
    let mut tokens = Vec::new();
    let mut offset = 0;

    while offset < input.len() {
        let byte = input[offset];
        if is_whitespace(byte) {
            let start = offset;
            offset += 1;
            while offset < input.len() && is_whitespace(input[offset]) {
                offset += 1;
            }
            tokens.push(Token::new(
                TokenKind::Trivia(TriviaKind::Whitespace),
                ByteRange { start, end: offset },
            ));
        } else if byte == b'%' {
            let start = offset;
            offset += 1;
            while offset < input.len() && input[offset] != b'\r' && input[offset] != b'\n' {
                offset += 1;
            }
            tokens.push(Token::new(
                TokenKind::Trivia(TriviaKind::Comment),
                ByteRange { start, end: offset },
            ));
        } else if byte == b'(' {
            let end = scan_literal_string(input, offset)?;
            tokens.push(Token::new(
                TokenKind::String(StringKind::Literal),
                ByteRange { start: offset, end },
            ));
            offset = end;
        } else if byte == b'<' && input.get(offset + 1) != Some(&b'<') {
            let end = scan_hex_string(input, offset)?;
            tokens.push(Token::new(
                TokenKind::String(StringKind::Hexadecimal),
                ByteRange { start: offset, end },
            ));
            offset = end;
        } else if let Some((kind, end)) = scan_delimiter(input, offset) {
            tokens.push(Token::new(
                TokenKind::Delimiter(kind),
                ByteRange { start: offset, end },
            ));
            offset = end;
        } else {
            let start = offset;
            offset += 1;
            while offset < input.len()
                && !is_whitespace(input[offset])
                && !is_delimiter(input[offset])
            {
                offset += 1;
            }
            tokens.push(Token::new(
                classify_regular_token(&input[start..offset]),
                ByteRange { start, end: offset },
            ));
        }
    }

    Ok(tokens)
}

fn scan_literal_string(input: &[u8], start: usize) -> Result<usize, TokenizeError> {
    let mut offset = start + 1;
    let mut depth = 1_u32;

    while offset < input.len() {
        match input[offset] {
            b'\\' => {
                offset = offset.saturating_add(2);
            }
            b'(' => {
                depth += 1;
                offset += 1;
            }
            b')' => {
                depth -= 1;
                offset += 1;
                if depth == 0 {
                    return Ok(offset);
                }
            }
            _ => offset += 1,
        }
    }

    Err(TokenizeError {
        kind: TokenizeErrorKind::UnterminatedLiteralString,
        range: ByteRange {
            start,
            end: input.len(),
        },
    })
}

fn scan_hex_string(input: &[u8], start: usize) -> Result<usize, TokenizeError> {
    let mut offset = start + 1;
    while offset < input.len() {
        if input[offset] == b'>' {
            return Ok(offset + 1);
        }
        offset += 1;
    }

    Err(TokenizeError {
        kind: TokenizeErrorKind::UnterminatedHexString,
        range: ByteRange {
            start,
            end: input.len(),
        },
    })
}

fn scan_delimiter(input: &[u8], offset: usize) -> Option<(Delimiter, usize)> {
    match input[offset] {
        b'[' => Some((Delimiter::ArrayOpen, offset + 1)),
        b']' => Some((Delimiter::ArrayClose, offset + 1)),
        b'<' if input.get(offset + 1) == Some(&b'<') => {
            Some((Delimiter::DictionaryOpen, offset + 2))
        }
        b'>' if input.get(offset + 1) == Some(&b'>') => {
            Some((Delimiter::DictionaryClose, offset + 2))
        }
        _ => None,
    }
}

fn classify_regular_token(bytes: &[u8]) -> TokenKind {
    match bytes {
        b"true" | b"false" => TokenKind::Boolean,
        b"null" => TokenKind::Null,
        b"obj" => TokenKind::Keyword(Keyword::ObjectBegin),
        b"endobj" => TokenKind::Keyword(Keyword::ObjectEnd),
        b"stream" => TokenKind::Keyword(Keyword::StreamBegin),
        b"endstream" => TokenKind::Keyword(Keyword::StreamEnd),
        _ if bytes.first() == Some(&b'/') => TokenKind::Name,
        _ if is_number_lexeme(bytes) => {
            if bytes.contains(&b'.') {
                TokenKind::Number(NumberKind::Real)
            } else {
                TokenKind::Number(NumberKind::Integer)
            }
        }
        _ => TokenKind::Operator,
    }
}

fn is_number_lexeme(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    let mut offset = 0;
    if matches!(bytes[offset], b'+' | b'-') {
        offset += 1;
    }
    if offset == bytes.len() {
        return false;
    }

    let mut saw_digit = false;
    let mut saw_dot = false;
    while offset < bytes.len() {
        match bytes[offset] {
            b'0'..=b'9' => saw_digit = true,
            b'.' if !saw_dot => saw_dot = true,
            _ => return false,
        }
        offset += 1;
    }

    saw_digit
}

const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Serialization failure for unmodified token streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializeError {
    /// A token range is reversed or outside the source bytes.
    InvalidRange {
        /// Token index in the provided token stream.
        token_index: usize,
        /// Invalid source range.
        range: ByteRange,
    },
    /// Token ranges do not form one contiguous source span.
    NonContiguousRange {
        /// Token index in the provided token stream.
        token_index: usize,
        /// Expected range start.
        expected_start: usize,
        /// Actual range start.
        actual_start: usize,
    },
}

/// Serialize an unmodified token stream by copying exact source bytes.
///
/// This function is intentionally limited to the unmodified path. It proves
/// that token ranges can re-emit the original byte stream without syntax
/// normalization.
///
/// # Errors
///
/// Returns a [`SerializeError`] if token ranges are invalid or non-contiguous.
pub fn serialize_tokens_unmodified(
    source: &[u8],
    tokens: &[Token],
) -> Result<Vec<u8>, SerializeError> {
    let mut output = Vec::with_capacity(source.len());
    let mut expected_start = 0;

    for (token_index, token) in tokens.iter().enumerate() {
        if token.range.start != expected_start {
            return Err(SerializeError::NonContiguousRange {
                token_index,
                expected_start,
                actual_start: token.range.start,
            });
        }

        let Some(bytes) = token.source_bytes(source) else {
            return Err(SerializeError::InvalidRange {
                token_index,
                range: token.range,
            });
        };

        output.extend_from_slice(bytes);
        expected_start = token.range.end;
    }

    if expected_start != source.len() {
        return Err(SerializeError::NonContiguousRange {
            token_index: tokens.len(),
            expected_start,
            actual_start: source.len(),
        });
    }

    Ok(output)
}

/// Return the input bytes unchanged.
///
/// This placeholder makes the round-trip contract explicit before the real
/// tokenizer lands.
#[must_use]
pub fn serialize_unmodified(input: &[u8]) -> Vec<u8> {
    input.to_vec()
}

#[cfg(test)]
mod tests {
    use super::{
        Delimiter, Keyword, NumberKind, SerializeError, StringKind, Token, TokenKind,
        TokenizeErrorKind, TriviaKind, serialize_tokens_unmodified, serialize_unmodified, tokenize,
    };
    use presslint_core::ByteRange;

    #[test]
    fn unmodified_serializer_is_byte_identical() {
        let input = b"0 0 0 rg\n10 20 m\n";
        assert_eq!(serialize_unmodified(input), input);
    }

    #[test]
    fn token_returns_exact_source_bytes() {
        let input = b"/DeviceRGB 1 0 obj";
        let token = Token::new(TokenKind::Name, ByteRange { start: 0, end: 10 });

        assert_eq!(token.kind, TokenKind::Name);
        assert_eq!(token.source_bytes(input), Some(&input[..10]));
    }

    #[test]
    fn token_rejects_invalid_ranges_without_panicking() {
        let input = b"q";
        let reversed = Token::new(TokenKind::Operator, ByteRange { start: 1, end: 0 });
        let out_of_bounds = Token::new(TokenKind::Operator, ByteRange { start: 0, end: 2 });

        assert_eq!(reversed.source_bytes(input), None);
        assert_eq!(out_of_bounds.source_bytes(input), None);
    }

    #[test]
    fn token_kind_covers_initial_pdf_lexical_classes() {
        let kinds = [
            TokenKind::Trivia(TriviaKind::Whitespace),
            TokenKind::Trivia(TriviaKind::Comment),
            TokenKind::Boolean,
            TokenKind::Null,
            TokenKind::Number(NumberKind::Integer),
            TokenKind::Number(NumberKind::Real),
            TokenKind::String(StringKind::Literal),
            TokenKind::String(StringKind::Hexadecimal),
            TokenKind::Name,
            TokenKind::Delimiter(Delimiter::ArrayOpen),
            TokenKind::Delimiter(Delimiter::ArrayClose),
            TokenKind::Delimiter(Delimiter::DictionaryOpen),
            TokenKind::Delimiter(Delimiter::DictionaryClose),
            TokenKind::Keyword(Keyword::ObjectBegin),
            TokenKind::Keyword(Keyword::ObjectEnd),
            TokenKind::Keyword(Keyword::StreamBegin),
            TokenKind::Keyword(Keyword::StreamEnd),
            TokenKind::Operator,
        ];

        assert_eq!(kinds.len(), 18);
    }

    #[test]
    fn tokenizer_preserves_ranges_for_common_content_tokens() -> Result<(), String> {
        let input = b"q\n/DeviceRGB cs [1 -2.5 .75] << /K (v\\)x) /H <4E6F> >> Do\n% note\n";
        let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
        let observed: Option<Vec<(TokenKind, &[u8])>> = tokens
            .iter()
            .map(|token| Some((token.kind, token.source_bytes(input)?)))
            .collect();

        assert_eq!(
            observed,
            Some(vec![
                (TokenKind::Operator, &b"q"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b"\n"[..]),
                (TokenKind::Name, &b"/DeviceRGB"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Operator, &b"cs"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Delimiter(Delimiter::ArrayOpen), &b"["[..]),
                (TokenKind::Number(NumberKind::Integer), &b"1"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Number(NumberKind::Real), &b"-2.5"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Number(NumberKind::Real), &b".75"[..]),
                (TokenKind::Delimiter(Delimiter::ArrayClose), &b"]"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Delimiter(Delimiter::DictionaryOpen), &b"<<"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Name, &b"/K"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::String(StringKind::Literal), &b"(v\\)x)"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Name, &b"/H"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::String(StringKind::Hexadecimal), &b"<4E6F>"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Delimiter(Delimiter::DictionaryClose), &b">>"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b" "[..]),
                (TokenKind::Operator, &b"Do"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b"\n"[..]),
                (TokenKind::Trivia(TriviaKind::Comment), &b"% note"[..]),
                (TokenKind::Trivia(TriviaKind::Whitespace), &b"\n"[..]),
            ])
        );
        Ok(())
    }

    #[test]
    fn tokenizer_recognizes_boolean_null_and_object_keywords() -> Result<(), String> {
        let input = b"true false null obj endobj stream endstream";
        let kinds: Vec<TokenKind> = tokenize(input)
            .map_err(|error| format!("{error:?}"))?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Trivia(_)))
            .map(|token| token.kind)
            .collect();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Boolean,
                TokenKind::Boolean,
                TokenKind::Null,
                TokenKind::Keyword(Keyword::ObjectBegin),
                TokenKind::Keyword(Keyword::ObjectEnd),
                TokenKind::Keyword(Keyword::StreamBegin),
                TokenKind::Keyword(Keyword::StreamEnd),
            ]
        );
        Ok(())
    }

    #[test]
    fn tokenizer_reports_unterminated_strings() -> Result<(), String> {
        let Err(literal) = tokenize(b"(unterminated") else {
            return Err("literal string unexpectedly tokenized".to_owned());
        };
        let Err(hex) = tokenize(b"<4E6F") else {
            return Err("hex string unexpectedly tokenized".to_owned());
        };

        assert_eq!(literal.kind, TokenizeErrorKind::UnterminatedLiteralString);
        assert_eq!(literal.range, ByteRange { start: 0, end: 13 });
        assert_eq!(hex.kind, TokenizeErrorKind::UnterminatedHexString);
        assert_eq!(hex.range, ByteRange { start: 0, end: 5 });
        Ok(())
    }

    #[test]
    fn token_stream_serializer_is_byte_identical() -> Result<(), String> {
        let input = b"q\n/DeviceRGB cs [1 -2.5 .75]\n";
        let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;

        assert_eq!(
            serialize_tokens_unmodified(input, &tokens).map_err(|error| format!("{error:?}"))?,
            input
        );
        Ok(())
    }

    #[test]
    fn token_stream_serializer_rejects_bad_ranges() {
        let source = b"abc";
        let invalid = [Token::new(
            TokenKind::Operator,
            ByteRange { start: 0, end: 4 },
        )];
        let gapped = [
            Token::new(TokenKind::Operator, ByteRange { start: 0, end: 1 }),
            Token::new(TokenKind::Operator, ByteRange { start: 2, end: 3 }),
        ];
        let overlapping = [
            Token::new(TokenKind::Operator, ByteRange { start: 0, end: 2 }),
            Token::new(TokenKind::Operator, ByteRange { start: 1, end: 3 }),
        ];

        assert_eq!(
            serialize_tokens_unmodified(source, &invalid),
            Err(SerializeError::InvalidRange {
                token_index: 0,
                range: ByteRange { start: 0, end: 4 },
            })
        );
        assert_eq!(
            serialize_tokens_unmodified(source, &gapped),
            Err(SerializeError::NonContiguousRange {
                token_index: 1,
                expected_start: 1,
                actual_start: 2,
            })
        );
        assert_eq!(
            serialize_tokens_unmodified(source, &overlapping),
            Err(SerializeError::NonContiguousRange {
                token_index: 1,
                expected_start: 2,
                actual_start: 1,
            })
        );
    }
}
