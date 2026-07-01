use presslint_pdf::{ClassicXrefObjectLocation, resolve_classic_xref_chain_object};

use crate::write_incremental_revision;

use super::{
    PAGE_BODY, PAGES_BODY, classic_chain, dirty, final_startxref_target, last_trailer_size,
    page_leaf_numbers, reopen, sample_pdf, sample_pdf_with_prev_chain,
    sample_pdf_with_trailer_tail,
};

#[test]
fn output_prefix_is_verbatim_input() {
    let input = sample_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    assert!(output.len() > input.len());
    assert_eq!(&output[..input.len()], input.as_slice());
}

#[test]
fn appended_xref_offset_points_at_object_header() {
    let input = sample_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    let access = reopen(&output);
    let chain = classic_chain(&access);
    let ClassicXrefObjectLocation::InUse { byte_offset, .. } =
        resolve_classic_xref_chain_object(chain, 3)
    else {
        panic!("object 3 should resolve in-use");
    };

    // Newest-wins resolves object 3 to the appended header, not the original.
    assert!(byte_offset >= input.len());
    assert!(output[byte_offset..].starts_with(b"3 0 obj"));
}

#[test]
fn new_startxref_points_at_appended_table() {
    let input = sample_pdf();
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    let table_offset = final_startxref_target(&output);
    assert!(table_offset > input.len());
    assert!(output[table_offset..].starts_with(b"xref"));
}

#[test]
fn appended_prev_equals_previous_startxref_target() {
    let input = sample_pdf();
    let previous_target = final_startxref_target(&input);

    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    // The appended trailer chains back to exactly the previous startxref target.
    let access = reopen(&output);
    let chain = classic_chain(&access);
    assert_eq!(
        chain.section_byte_offsets.last().copied(),
        Some(previous_target)
    );

    let expected = format!("/Prev {previous_target}");
    assert!(
        output
            .windows(expected.len())
            .any(|window| window == expected.as_bytes())
    );
}

#[test]
fn size_spans_the_whole_prev_chain() {
    let input = sample_pdf_with_prev_chain();
    // The newest input section deliberately understates /Size as 4.
    assert_eq!(last_trailer_size(&input), 4);

    // Rewrite the highest object (5), which only exists in the older section.
    let output = write_incremental_revision(&input, &[dirty(5, 0, b"<< /Note (rewritten) >>")])
        .expect("append");

    // Highest object number across the whole chain is 5, so /Size must be >= 6,
    // never the newest-section-only value of 4 (the PDFBOX-5945 pitfall).
    let access = reopen(&output);
    let chain = classic_chain(&access);
    let highest = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .expect("chain entries");
    assert_eq!(highest, 5);
    assert!(last_trailer_size(&output) > usize::try_from(highest).unwrap());
    assert_eq!(last_trailer_size(&output), 6);
}

#[test]
fn reopens_and_preserves_page_leaves() {
    let input = sample_pdf();
    let before = reopen(&input);

    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");
    let after = reopen(&output);

    assert_eq!(page_leaf_numbers(&before), vec![3]);
    assert_eq!(page_leaf_numbers(&after), page_leaf_numbers(&before));
    assert_eq!(after.root_reference.object_number, 1);
}

#[test]
fn preserves_existing_id() {
    let input = sample_pdf_with_trailer_tail(" /ID [<0011> <2233>]");
    let output = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("append");

    // The original /ID array bytes are copied verbatim into the appended trailer.
    let id = b"/ID [<0011> <2233>]";
    let occurrences = output
        .windows(id.len())
        .filter(|window| *window == id)
        .count();
    assert_eq!(occurrences, 2);
    reopen(&output);
}

#[test]
fn second_append_lengthens_the_prev_chain() {
    let input = sample_pdf();
    let first = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).expect("first");
    let second = write_incremental_revision(&first, &[dirty(2, 0, PAGES_BODY)]).expect("second");

    // The second output keeps the first output as a verbatim prefix.
    assert_eq!(&second[..first.len()], first.as_slice());

    let access = reopen(&second);
    let chain = classic_chain(&access);
    // Three classic sections now: base + two appended revisions.
    assert_eq!(chain.section_byte_offsets.len(), 3);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}

#[test]
fn deterministic_regardless_of_dirty_order() {
    let input = sample_pdf();
    let forward =
        write_incremental_revision(&input, &[dirty(2, 0, PAGES_BODY), dirty(3, 0, PAGE_BODY)])
            .expect("forward");
    let reversed =
        write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY), dirty(2, 0, PAGES_BODY)])
            .expect("reversed");

    assert_eq!(forward, reversed);
}

#[test]
fn empty_dirty_set_appends_empty_revision() {
    let input = sample_pdf();
    let output = write_incremental_revision(&input, &[]).expect("append");

    assert_eq!(&output[..input.len()], input.as_slice());
    // Still a valid, reopenable document with an appended empty revision.
    let access = reopen(&output);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}
