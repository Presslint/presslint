mod multi_stream;
#[path = "../../../presslint-pdf/src/tests/content_stream_extent/serde_harness.rs"]
mod serde_harness;

use miniz_oxide::deflate::compress_to_vec_zlib;
use serde::{Serialize, de::DeserializeOwned};

use serde_harness::{from_serde_value, serde_value};

use super::{classic_pdf, multi_stream_page_pdf, single_page_pdf, vector_content};
use crate::{
    ClassicPdfInventoryPageResult, ObjectKind, PdfInventory, PdfInventoryError, PdfInventoryPage,
    PdfInventoryPageResult, PdfInventoryRejection, PdfInventorySkip, build_classic_pdf_inventory,
    build_pdf_inventory,
};

fn xref_record(entry_type: u8, field2: usize, generation: u8) -> Result<[u8; 4], String> {
    let [hi, lo] = u16::try_from(field2)
        .map_err(|_| format!("test offset {field2} exceeds two-byte xref field"))?
        .to_be_bytes();
    Ok([entry_type, hi, lo, generation])
}

fn xref_stream_single_page_pdf(
    content_dict_suffix: &[u8],
    content_data: &[u8],
    indirect_length: bool,
    prev: Option<usize>,
) -> Result<Vec<u8>, String> {
    let mut objects = Vec::new();
    objects.push(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec());
    objects.push(b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n".to_vec());
    objects.push(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n".to_vec());

    let mut content = Vec::new();
    content.extend_from_slice(b"4 0 obj\n<< /Length ");
    if indirect_length {
        content.extend_from_slice(b"5 0 R");
    } else {
        content.extend_from_slice(content_data.len().to_string().as_bytes());
    }
    content.extend_from_slice(content_dict_suffix);
    content.extend_from_slice(b" >>\nstream\n");
    content.extend_from_slice(content_data);
    content.extend_from_slice(b"\nendstream\nendobj\n");
    objects.push(content);

    if indirect_length {
        objects.push(format!("5 0 obj\n{}\nendobj\n", content_data.len()).into_bytes());
    }

    let mut source = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for object in &objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_object_number = objects.len() + 1;
    let xref_offset = source.len();
    let size = xref_object_number + 1;
    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0)?);
    for offset in &offsets {
        records.extend_from_slice(&xref_record(1, *offset, 0)?);
    }
    records.extend_from_slice(&xref_record(1, xref_offset, 0)?);

    let xref_body = compress_to_vec_zlib(&records, 6);
    let prev_field = prev.map_or_else(String::new, |offset| format!(" /Prev {offset}"));
    source.extend_from_slice(
        format!(
            "{xref_object_number} 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R{prev_field} /Filter /FlateDecode /Length {} >>\nstream\n",
            xref_body.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&xref_body);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    Ok(source)
}

fn xref_stream_incremental_single_page_pdf() -> Result<Vec<u8>, String> {
    let objects = vec![
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n".to_vec(),
        b"4 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n".to_vec(),
    ];

    let mut source = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for object in &objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let old_xref_offset = source.len();
    let mut old_records = Vec::new();
    old_records.extend_from_slice(&xref_record(0, 0, 0)?);
    for offset in &offsets {
        old_records.extend_from_slice(&xref_record(1, *offset, 0)?);
    }
    old_records.extend_from_slice(&xref_record(1, old_xref_offset, 0)?);
    let old_xref_body = compress_to_vec_zlib(&old_records, 6);
    source.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 6 /W [ 1 2 1 ] /Index [ 0 6 ] /Root 1 0 R /Filter /FlateDecode /Length {} >>\nstream\n",
            old_xref_body.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&old_xref_body);
    source.extend_from_slice(b"\nendstream\nendobj\n");

    let updated_content = vector_content();
    let updated_content_offset = source.len();
    source.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", updated_content.len()).as_bytes(),
    );
    source.extend_from_slice(updated_content);
    source.extend_from_slice(b"\nendstream\nendobj\n");

    let new_xref_offset = source.len();
    let mut new_records = Vec::new();
    new_records.extend_from_slice(&xref_record(1, updated_content_offset, 0)?);
    new_records.extend_from_slice(&xref_record(1, new_xref_offset, 0)?);
    let new_xref_body = compress_to_vec_zlib(&new_records, 6);
    source.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /W [ 1 2 1 ] /Index [ 4 2 ] /Root 1 0 R /Prev {old_xref_offset} /Filter /FlateDecode /Length {} >>\nstream\n",
            new_xref_body.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&new_xref_body);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{new_xref_offset}\n%%EOF\n").as_bytes());
    Ok(source)
}

fn classic_incremental_single_page_pdf() -> Vec<u8> {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n";
    let old_content = b"4 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n";

    let catalog_offset = prefix.len();
    let pages_offset = catalog_offset + catalog.len();
    let page_offset = pages_offset + pages.len();
    let old_content_offset = page_offset + page.len();
    let old_xref_offset = old_content_offset + old_content.len();

    let mut source = prefix.to_vec();
    source.extend_from_slice(catalog);
    source.extend_from_slice(pages);
    source.extend_from_slice(page);
    source.extend_from_slice(old_content);

    source.extend_from_slice(b"xref\n0 5\n");
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in [
        catalog_offset,
        pages_offset,
        page_offset,
        old_content_offset,
    ] {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\n");

    let updated = vector_content();
    let new_content_offset = source.len();
    source.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", updated.len()).as_bytes(),
    );
    source.extend_from_slice(updated);
    source.extend_from_slice(b"\nendstream\nendobj\n");

    let new_xref_offset = source.len();
    source.extend_from_slice(b"xref\n4 1\n");
    source.extend_from_slice(format!("{new_content_offset:010} 00000 n \n").as_bytes());
    source.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R /Prev {old_xref_offset} >>\n").as_bytes(),
    );
    source.extend_from_slice(format!("startxref\n{new_xref_offset}\n%%EOF\n").as_bytes());
    source
}

fn round_trip<T>(value: &T) -> Result<(), String>
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_value(value).map_err(|error| error.to_string())?;
    let decoded: T = from_serde_value(encoded).map_err(|error| error.to_string())?;
    assert_eq!(&decoded, value);
    Ok(())
}

#[test]
fn inventories_flate_xref_stream_with_direct_content_length() -> Result<(), String> {
    let compressed = compress_to_vec_zlib(vector_content(), 6);
    let source = xref_stream_single_page_pdf(b" /Filter /FlateDecode", &compressed, false, None)?;

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.pages.len(), 1);
    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    assert_eq!(report.inventory.entries[0].id.page.0, 0);
    Ok(())
}

#[test]
fn inventories_flate_xref_stream_with_indirect_content_length() -> Result<(), String> {
    let compressed = compress_to_vec_zlib(vector_content(), 6);
    let source = xref_stream_single_page_pdf(b" /Filter /FlateDecode", &compressed, true, None)?;

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn inventories_flate_page_with_single_element_array_decode_parms() -> Result<(), String> {
    // `/DecodeParms [ null ]` is the single-filter array form of the direct
    // `null` value: it resolves to default Flate parameters, so the page content
    // decodes and inventories end to end.
    let compressed = compress_to_vec_zlib(vector_content(), 6);
    let source = single_page_pdf(b" /Filter /FlateDecode /DecodeParms [ null ]", &compressed);

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn inventories_flate_xref_stream_with_single_element_array_decode_parms() -> Result<(), String> {
    // The same single-element array form on the content stream of an xref-stream
    // PDF also decodes and inventories through the neutral bridge.
    let compressed = compress_to_vec_zlib(vector_content(), 6);
    let source = xref_stream_single_page_pdf(
        b" /Filter /FlateDecode /DecodeParms [ null ]",
        &compressed,
        false,
        None,
    )?;

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn neutral_bridge_matches_classic_bridge_for_classic_xref() -> Result<(), String> {
    let source = single_page_pdf(b"", vector_content());

    let neutral = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;
    let classic =
        build_classic_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(neutral.byte_len, classic.byte_len);
    assert_eq!(neutral.inventory, classic.inventory);
    assert_eq!(neutral.pages.len(), classic.pages.len());
    assert_eq!(neutral.pages[0].page_index, classic.pages[0].page_index);
    assert_eq!(
        neutral.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(
        classic.pages[0].result,
        ClassicPdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    Ok(())
}

#[test]
fn inventories_two_section_xref_stream_incremental_update() -> Result<(), String> {
    let source = xref_stream_incremental_single_page_pdf()?;

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.pages.len(), 1);
    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn inventories_two_section_classic_incremental_update() -> Result<(), String> {
    let source = classic_incremental_single_page_pdf();

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.pages.len(), 1);
    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn invalid_xref_stream_prev_is_top_level_document_access_rejection() -> Result<(), String> {
    let source = xref_stream_single_page_pdf(b"", vector_content(), false, Some(9999))?;

    let Err(error) = build_pdf_inventory(&source, 1024) else {
        return Err("invalid /Prev should reject".to_string());
    };

    assert_eq!(error.byte_len, source.len());
    let PdfInventoryRejection::DocumentAccess { error } = error.reason else {
        return Err("expected delegated document-access rejection".to_string());
    };
    assert!(matches!(
        error.reason,
        crate::pdf::DocumentAccessRejection::XrefStreamChain { .. }
    ));
    Ok(())
}

#[test]
fn neutral_bridge_inventories_raw_multi_stream_page() -> Result<(), PdfInventoryError> {
    let source = multi_stream_page_pdf();

    let report = build_pdf_inventory(&source, 1024)?;

    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn neutral_bridge_skips_unsupported_filter() -> Result<(), PdfInventoryError> {
    let source = single_page_pdf(b" /Filter /ASCIIHexDecode", vector_content());

    let report = build_pdf_inventory(&source, 1024)?;

    assert!(matches!(
        report.pages[0].result,
        PdfInventoryPageResult::Skipped {
            reason: PdfInventorySkip::UnsupportedFilter { .. }
        }
    ));
    assert!(report.inventory.is_empty());
    Ok(())
}

#[test]
fn neutral_report_retains_no_source_bytes() -> Result<(), String> {
    let source =
        xref_stream_single_page_pdf(b" /DoNotCopy (secret)", vector_content(), false, None)?;

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;
    let debug = format!("{report:?}");

    assert!(!debug.contains("DoNotCopy"));
    assert!(!debug.contains("secret"));
    Ok(())
}

#[test]
fn neutral_inventory_serde_round_trips_report_page_skip_and_rejection_shapes() -> Result<(), String>
{
    let source = single_page_pdf(b" /Filter /ASCIIHexDecode", vector_content());
    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;
    round_trip::<PdfInventory>(&report)?;

    let page: PdfInventoryPage = report.pages[0].clone();
    round_trip(&page)?;
    round_trip(&page.result)?;
    let PdfInventoryPageResult::Skipped { reason } = &page.result else {
        return Err("fixture should produce a skip".to_string());
    };
    round_trip(reason)?;

    let prev_source = xref_stream_single_page_pdf(b"", vector_content(), false, Some(9999))?;
    let Err(error) = build_pdf_inventory(&prev_source, 1024) else {
        return Err("present /Prev should reject".to_string());
    };
    round_trip(&error)?;
    round_trip(&error.reason)?;
    Ok(())
}

#[test]
fn neutral_public_surface_accepts_classic_fixture_helper() -> Result<(), PdfInventoryError> {
    let source = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = build_pdf_inventory(&source, 1024)?;

    assert_eq!(report.pages.len(), 1);
    Ok(())
}

#[test]
fn page_image_xobjects_expose_structural_metadata() -> Result<(), String> {
    use crate::pdf::{ImageColorSpaceMetadata, ImageIntegerMetadata};

    let source = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Length 13 >>\nstream\n/Im Do /Fm Do\nendstream\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 8 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceCMYK >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n<< /Type /XObject /Subtype /Form /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    // Only the page-scope image target is surfaced here; the form target is not.
    assert_eq!(report.pages[0].image_xobjects.len(), 1);
    let image = &report.pages[0].image_xobjects[0];
    assert_eq!(image.name.0, b"Im");
    let metadata = image
        .image_metadata
        .as_ref()
        .ok_or("image target should carry metadata")?;
    assert_eq!(metadata.width, ImageIntegerMetadata::Value { value: 8 });
    assert_eq!(metadata.height, ImageIntegerMetadata::Value { value: 4 });
    assert_eq!(
        metadata.bits_per_component,
        ImageIntegerMetadata::Value { value: 8 }
    );
    assert_eq!(metadata.color_space, ImageColorSpaceMetadata::DeviceCmyk);

    // Serde round-trips the enriched page shape.
    round_trip::<PdfInventoryPage>(&report.pages[0].clone())?;
    Ok(())
}

#[test]
fn real_pdf_do_operators_become_image_and_form_inventory_entries() -> Result<(), String> {
    let source = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Length 13 >>\nstream\n/Im Do /Fm Do\nendstream\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n<< /Type /XObject /Subtype /Form /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let neutral = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;
    let classic =
        build_classic_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(neutral.xobject_resource_error, None);
    assert!(neutral.pages[0].xobject_resource_skipped.is_empty());
    assert_eq!(
        neutral.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 2,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(
        neutral
            .inventory
            .entries
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>(),
        vec![ObjectKind::Image, ObjectKind::FormXObject]
    );
    assert_eq!(classic.inventory, neutral.inventory);
    assert_eq!(
        classic.pages[0].result,
        ClassicPdfInventoryPageResult::Inventoried {
            entry_count: 2,
            form_skipped: Vec::new()
        }
    );
    Ok(())
}
