use presslint_types::ByteRange;

/// Rebuild a stream object's body with the one direct `/Length` value replaced.
///
/// The result is `<< preserved-dict-with-/Length-replaced >>\nstream\n<data>\n
/// endstream`. Exactly the one direct `/Length` value span is rewritten to the
/// new data length; every other dictionary byte (including `/Filter` and
/// `/DecodeParms`) is preserved verbatim. LF is used for the synthesized
/// `stream`/`endstream` separators; the dictionary is not normalized.
pub fn build_stream_object_body(
    input: &[u8],
    dictionary_open_byte_offset: usize,
    after_dictionary_close_byte_offset: usize,
    length_value_range: ByteRange,
    new_stream_data: &[u8],
) -> Vec<u8> {
    let dictionary = &input[dictionary_open_byte_offset..after_dictionary_close_byte_offset];
    let relative_start = length_value_range.start - dictionary_open_byte_offset;
    let relative_end = length_value_range.end - dictionary_open_byte_offset;
    let new_length = new_stream_data.len().to_string();

    let mut body = Vec::with_capacity(
        dictionary.len()
            + new_length.len()
            + new_stream_data.len()
            + b"\nstream\n\nendstream".len(),
    );
    body.extend_from_slice(&dictionary[..relative_start]);
    body.extend_from_slice(new_length.as_bytes());
    body.extend_from_slice(&dictionary[relative_end..]);
    body.extend_from_slice(b"\nstream\n");
    body.extend_from_slice(new_stream_data);
    body.extend_from_slice(b"\nendstream");
    body
}
