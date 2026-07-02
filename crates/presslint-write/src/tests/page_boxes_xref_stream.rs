use presslint_pdf::{PageBoxKind, PageRectangle, inspect_document_page_boxes};

use crate::{PageBoxEdit, SetPageBoxesRequest, set_page_boxes_incremental};

use super::{sample_xref_stream_pdf, xref_stream_chain};

#[test]
fn set_page_boxes_incremental_writes_xref_stream_revision() {
    let input = sample_xref_stream_pdf();
    let media = PageRectangle {
        llx: 10.0,
        lly: 20.0,
        urx: 300.0,
        ury: 400.0,
    };
    let crop = PageRectangle {
        llx: 20.0,
        lly: 30.0,
        urx: 250.0,
        ury: 350.0,
    };

    let output = set_page_boxes_incremental(
        &input,
        &SetPageBoxesRequest {
            pages: vec![PageBoxEdit {
                page_index: 0,
                media_box: Some(media),
                crop_box: Some(crop),
            }],
        },
    )
    .expect("page boxes write");

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.edited.len(), 1);
    assert!(output.skipped.is_empty());
    let access = super::reopen(&output.bytes);
    assert_eq!(xref_stream_chain(&access).section_byte_offsets.len(), 2);

    let boxes = inspect_document_page_boxes(&output.bytes).expect("boxes inspect");
    assert_eq!(boxes.pages[0].media_box.effective, media);
    assert_eq!(boxes.pages[0].crop_box.effective, crop);
    assert_eq!(
        output.edited[0].media_box.expect("media").kind,
        PageBoxKind::MediaBox
    );
    assert_eq!(
        output.edited[0].crop_box.expect("crop").kind,
        PageBoxKind::CropBox
    );
}
