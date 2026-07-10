use presslint_paint::DecodedRange;
use presslint_pdf::{IndirectObjectEditDisposition, IndirectRef};
use presslint_types::ByteRange;
use std::collections::BTreeMap;

use crate::page_content_sequence::{LocalSplice, OccurrenceInput, PageContentSequence};

fn reference(number: u32) -> IndirectRef {
    IndirectRef {
        object_number: number,
        generation: 0,
    }
}

fn sequence(parts: &[(IndirectRef, &[u8])]) -> Option<PageContentSequence> {
    let inputs: Vec<_> = parts
        .iter()
        .enumerate()
        .map(|(ordinal, (content_object, decoded))| OccurrenceInput {
            stream_ordinal: ordinal,
            content_object: *content_object,
            decoded,
            disposition: IndirectObjectEditDisposition::InPlaceMutation,
        })
        .collect();
    PageContentSequence::new(&inputs, 1024 * 1024)
}

#[test]
fn trivia_boundaries_and_empty_occurrences_are_accepted() {
    for parts in [
        vec![(reference(1), &b"q "[..]), (reference(2), &b" Q"[..])],
        vec![
            (reference(1), &b"% com"[..]),
            (reference(2), &b"ment\nq"[..]),
        ],
        vec![(reference(1), &b"% x\r"[..]), (reference(2), &b"\nq"[..])],
        vec![
            (reference(1), &b"q "[..]),
            (reference(2), &b""[..]),
            (reference(3), &b"Q"[..]),
        ],
    ] {
        assert!(sequence(&parts).is_some(), "parts: {parts:?}");
    }
}

#[test]
fn non_trivia_token_boundaries_are_rejected() {
    for parts in [
        (&b"1"[..], &b"2 0 m"[..]),
        (&b"/Na"[..], &b"me gs"[..]),
        (&b"r"[..], &b"g"[..]),
        (&b"(a"[..], &b"b) Tj"[..]),
        (&b"<a"[..], &b"b> Tj"[..]),
        (&b"<"[..], &b"< /A 1 >>"[..]),
    ] {
        assert!(sequence(&[(reference(1), parts.0), (reference(2), parts.1)]).is_none());
    }
}

#[test]
fn localization_requires_one_nonempty_occurrence() {
    let sequence = sequence(&[(reference(4), &b"1 0 "[..]), (reference(5), &b"0 rg"[..])])
        .expect("logical record is valid");
    assert!(
        sequence
            .localize(DecodedRange::new(ByteRange { start: 0, end: 8 }))
            .is_none()
    );
    let localized = sequence
        .localize(DecodedRange::new(ByteRange { start: 4, end: 8 }))
        .expect("second occurrence range");
    assert_eq!(localized.content_object, reference(5));
    assert_eq!(localized.local_range, ByteRange { start: 0, end: 4 });
}

#[test]
fn repeated_reference_plans_must_match_completely() {
    let sequence = sequence(&[
        (reference(4), &b"1 0 0 rg\n"[..]),
        (reference(4), &b"1 0 0 rg\n"[..]),
    ])
    .expect("sequence");
    let splice = LocalSplice {
        range: ByteRange { start: 0, end: 8 },
        replacement: b"0 1 1 0 k".to_vec(),
    };
    assert_eq!(
        sequence
            .reconcile(vec![vec![splice.clone()], vec![splice]])
            .expect("identical plans collapse")
            .len(),
        1
    );
    assert!(
        sequence
            .reconcile(vec![
                Vec::new(),
                vec![LocalSplice {
                    range: ByteRange { start: 0, end: 8 },
                    replacement: b"0 1 1 0 k".to_vec(),
                }]
            ])
            .is_none()
    );
}

#[test]
fn post_edit_validation_rechecks_physical_token_boundaries() {
    let sequence = sequence(&[(reference(4), &b"q "[..]), (reference(5), &b"Q"[..])])
        .expect("initial boundary is whitespace");
    let edited = BTreeMap::from([
        (reference(4), b"q".as_slice()),
        (reference(5), b"Q".as_slice()),
    ]);
    assert!(!sequence.validate_edited(&edited, 1024));
}

#[test]
fn logical_cap_counts_every_repeated_occurrence() {
    let bytes = b"q Q\n";
    let inputs = [
        OccurrenceInput {
            stream_ordinal: 0,
            content_object: reference(4),
            decoded: bytes,
            disposition: IndirectObjectEditDisposition::InPlaceMutation,
        },
        OccurrenceInput {
            stream_ordinal: 1,
            content_object: reference(4),
            decoded: bytes,
            disposition: IndirectObjectEditDisposition::InPlaceMutation,
        },
    ];
    assert!(PageContentSequence::new(&inputs, bytes.len() * 2 - 1).is_none());
}
