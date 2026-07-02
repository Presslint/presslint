use presslint_pdf::{
    DocumentAccessBackend, ObjectLookup, ObjectLookupLocation, locate_xref_object,
};

use crate::{WriteError, write_incremental_revision};

use super::{
    PAGE_BODY, PAGES_BODY, dirty, final_startxref_target, page_leaf_numbers, reopen, sample_pdf,
    sample_xref_stream_pdf, xref_record, xref_stream_chain,
};

#[test]
fn output_prefix_is_verbatim_xref_stream_input() {
    let input = sample_xref_stream_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    assert!(output.len() > input.len());
    assert_eq!(&output[..input.len()], input.as_slice());
}

#[test]
fn appended_xref_stream_reopens_and_self_references() {
    let input = sample_xref_stream_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");
    let xref_offset = final_startxref_target(&output);

    assert!(output[xref_offset..].starts_with(b"6 0 obj\n<< /Type /XRef"));
    let access = reopen(&output);
    assert!(matches!(
        access.backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    let chain = xref_stream_chain(&access);
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamChain(chain), 6),
        ObjectLookupLocation::XrefStreamUncompressed {
            object_number: 6,
            generation: 0,
            byte_offset: xref_offset,
        }
    );
    assert_eq!(chain.section_byte_offsets[0], xref_offset);
}

#[test]
fn dirty_object_resolves_newest_wins_and_prev_is_preserved() {
    let input = sample_xref_stream_pdf();
    let previous_startxref = final_startxref_target(&input);
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");
    let xref_offset = final_startxref_target(&output);
    let access = reopen(&output);
    let chain = xref_stream_chain(&access);

    let ObjectLookupLocation::XrefStreamUncompressed { byte_offset, .. } =
        locate_xref_object(ObjectLookup::XrefStreamChain(chain), 3)
    else {
        panic!("object 3 should resolve uncompressed");
    };
    assert!(byte_offset >= input.len());
    assert!(output[byte_offset..].starts_with(b"3 0 obj"));
    assert_eq!(
        chain.section_byte_offsets,
        vec![xref_offset, previous_startxref]
    );
    assert!(output[xref_offset..].starts_with(
        format!("6 0 obj\n<< /Type /XRef /Size 7 /Index [3 1 6 1] /W [1 2 1] /Root 1 0 R /Prev {previous_startxref} /Length 8").as_bytes()
    ));
}

#[test]
fn size_spans_whole_chain_and_new_xref_object() {
    let input = xref_stream_pdf_with_understated_size_and_object_9();
    let output = write_incremental_revision(&input, &[dirty(9, 0, b"<< /Note (rewritten) >>")])
        .expect("append");
    let xref_offset = final_startxref_target(&output);

    assert!(
        output[xref_offset..]
            .starts_with(b"10 0 obj\n<< /Type /XRef /Size 11 /Index [9 2] /W [1 2 1]")
    );
    let access = reopen(&output);
    let chain = xref_stream_chain(&access);
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamChain(chain), 10),
        ObjectLookupLocation::XrefStreamUncompressed {
            object_number: 10,
            generation: 0,
            byte_offset: xref_offset,
        }
    );
}

#[test]
fn preserves_id_and_info_in_xref_stream_dictionary() {
    let input = sample_xref_stream_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");
    let appended = &output[input.len()..];

    assert!(
        appended
            .windows(b"/ID [<0011> <2233>]".len())
            .any(|w| w == b"/ID [<0011> <2233>]")
    );
    assert!(
        appended
            .windows(b"/Info 4 0 R".len())
            .any(|w| w == b"/Info 4 0 R")
    );
    reopen(&output);
}

#[test]
fn second_append_lengthens_xref_stream_prev_chain() {
    let input = sample_xref_stream_pdf();
    let first = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("first");
    let second = write_incremental_revision(&first, &[dirty(2, 0, PAGES_BODY)]).expect("second");

    assert_eq!(&second[..first.len()], first.as_slice());
    let access = reopen(&second);
    let chain = xref_stream_chain(&access);
    assert_eq!(chain.section_byte_offsets.len(), 3);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}

#[test]
fn classic_dispatch_matches_classic_backend_bytes() {
    let input = sample_pdf();
    let previous_startxref = final_startxref_target(&input);
    let via_dispatch =
        write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("dispatch");
    let via_classic = crate::writer::write_classic_incremental_revision(
        &input,
        &[dirty(3, 0, PAGE_BODY)],
        previous_startxref,
    )
    .expect("classic");

    assert_eq!(via_dispatch, via_classic);
}

#[test]
fn malformed_xref_stream_chain_surfaces_structured_error() {
    let mut input = sample_xref_stream_pdf();
    let pos = input
        .windows(b"/Length 24".len())
        .position(|w| w == b"/Length 24")
        .expect("length");
    input[pos + b"/Length ".len()] = b'0';

    let error = write_incremental_revision(&input, &[]).unwrap_err();
    assert!(matches!(error, WriteError::XrefStreamChain { .. }));
}

fn xref_stream_pdf_with_understated_size_and_object_9() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let catalog_offset = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    let page_offset = buf.len();
    buf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
    let note_offset = buf.len();
    buf.extend_from_slice(b"9 0 obj\n<< /Note (highest) >>\nendobj\n");
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));
    body.extend_from_slice(&xref_record(1, note_offset, 0));

    buf.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 5 /Index [0 4 5 1 9 1] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}
