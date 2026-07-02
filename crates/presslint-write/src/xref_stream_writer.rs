use presslint_pdf::{
    IndirectRef, MAX_XREF_STREAM_SECTION_DECODED_BYTES, ObjectLookup, XrefStreamChain,
    XrefStreamTrailerInspection, build_xref_stream_chain, inspect_xref_stream_trailer,
    resolve_xref_object_offset,
};

use crate::writer::{
    ActiveTrailerScan, DirtyObjectBytes, WriteError, classify_resolution_error, order_dirty_objects,
};

const ENCRYPT_KEY: &[u8] = b"/Encrypt";
const ID_KEY: &[u8] = b"/ID";
const INFO_KEY: &[u8] = b"/Info";

struct XrefStreamEntry {
    object_number: u32,
    generation: u16,
    byte_offset: usize,
}

pub fn write_xref_stream_incremental_revision(
    input: &[u8],
    dirty_objects: &[DirtyObjectBytes],
    startxref_byte_offset: usize,
) -> Result<Vec<u8>, WriteError> {
    let active = inspect_xref_stream_trailer(input, startxref_byte_offset)
        .map_err(|error| WriteError::ActiveXrefStream { error })?;
    let active_scan = scan_active_xref_stream(input, &active)?;
    let chain = build_xref_stream_chain(
        input,
        startxref_byte_offset,
        MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    )
    .map_err(|error| WriteError::XrefStreamChain {
        error: Box::new(error),
    })?;

    let ordered = order_dirty_objects(dirty_objects)?;
    validate_dirty_objects(input, &chain, &ordered)?;

    let xref_object_number = next_xref_object_number(&chain)?;
    let mut writer = XrefStreamRevisionWriter::new(input, dirty_objects);
    writer.ensure_leading_eol();

    let mut entries = Vec::with_capacity(ordered.len() + 1);
    for dirty in &ordered {
        let byte_offset = writer.append_object(dirty.reference, &dirty.body_bytes);
        entries.push(XrefStreamEntry {
            object_number: dirty.reference.object_number,
            generation: dirty.reference.generation,
            byte_offset,
        });
    }

    let xref_byte_offset = writer.len();
    entries.push(XrefStreamEntry {
        object_number: xref_object_number,
        generation: 0,
        byte_offset: xref_byte_offset,
    });
    entries.sort_by_key(|entry| entry.object_number);

    let size = xref_effective_size(&chain, xref_object_number)?;
    writer.push_xref_stream(&XrefStreamAppend {
        xref_object_number,
        size,
        root: chain.root_reference,
        prev_byte_offset: startxref_byte_offset,
        id_bytes: active_scan.id_bytes(input),
        info_bytes: active_scan.info_bytes(input),
        entries: &entries,
    });
    writer.push_startxref(xref_byte_offset);

    Ok(writer.finish())
}

fn scan_active_xref_stream(
    input: &[u8],
    active: &XrefStreamTrailerInspection,
) -> Result<ActiveTrailerScan, WriteError> {
    let mut scan = ActiveTrailerScan {
        id_value_range: None,
        info_value_range: None,
    };
    let mut has_encrypt = false;
    for entry in &active.xref_stream_dictionary.object_dictionary.entries {
        let key = input.get(entry.key_range.start..entry.key_range.end);
        if key == Some(ENCRYPT_KEY) {
            has_encrypt = true;
        } else if key == Some(ID_KEY) && scan.id_value_range.is_none() {
            scan.id_value_range = Some((entry.value_range.start, entry.value_range.end));
        } else if key == Some(INFO_KEY) && scan.info_value_range.is_none() {
            scan.info_value_range = Some((entry.value_range.start, entry.value_range.end));
        }
    }
    if has_encrypt {
        return Err(WriteError::EncryptedInput);
    }
    Ok(scan)
}

fn next_xref_object_number(chain: &XrefStreamChain) -> Result<u32, WriteError> {
    let highest = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .unwrap_or(0);
    let next = highest
        .checked_add(1)
        .ok_or(WriteError::XrefStreamObjectNumberTooLarge {
            object_number: highest,
        })?;
    u32::try_from(next).map_err(|_| WriteError::XrefStreamObjectNumberTooLarge {
        object_number: next,
    })
}

fn xref_effective_size(
    chain: &XrefStreamChain,
    new_object_number: u32,
) -> Result<usize, WriteError> {
    let highest = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .unwrap_or(0)
        .max(usize::try_from(new_object_number).map_err(|_| {
            WriteError::XrefStreamObjectNumberTooLarge {
                object_number: usize::MAX,
            }
        })?);
    Ok(chain.effective_size.max(highest + 1))
}

fn validate_dirty_objects(
    input: &[u8],
    chain: &XrefStreamChain,
    ordered: &[&DirtyObjectBytes],
) -> Result<(), WriteError> {
    for dirty in ordered {
        let reference = dirty.reference;
        if let Err(error) =
            resolve_xref_object_offset(input, ObjectLookup::XrefStreamChain(chain), reference)
        {
            return Err(classify_resolution_error(reference, error));
        }
    }
    Ok(())
}

struct XrefStreamRevisionWriter {
    out: Vec<u8>,
}

struct XrefStreamAppend<'a> {
    xref_object_number: u32,
    size: usize,
    root: IndirectRef,
    prev_byte_offset: usize,
    id_bytes: Option<&'a [u8]>,
    info_bytes: Option<&'a [u8]>,
    entries: &'a [XrefStreamEntry],
}

impl XrefStreamRevisionWriter {
    fn new(input: &[u8], dirty_objects: &[DirtyObjectBytes]) -> Self {
        let body_bytes: usize = dirty_objects
            .iter()
            .map(|dirty| dirty.body_bytes.len())
            .sum();
        let headroom = body_bytes + 96 * dirty_objects.len() + 384;
        let mut out = Vec::with_capacity(input.len() + headroom);
        out.extend_from_slice(input);
        Self { out }
    }

    const fn len(&self) -> usize {
        self.out.len()
    }

    fn ensure_leading_eol(&mut self) {
        if !self
            .out
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.out.push(b'\n');
        }
    }

    fn append_object(&mut self, reference: IndirectRef, body: &[u8]) -> usize {
        let byte_offset = self.out.len();
        self.out.extend_from_slice(
            format!("{} {} obj\n", reference.object_number, reference.generation).as_bytes(),
        );
        self.out.extend_from_slice(body);
        if !self
            .out
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.out.push(b'\n');
        }
        self.out.extend_from_slice(b"endobj\n");
        byte_offset
    }

    fn push_xref_stream(&mut self, append: &XrefStreamAppend<'_>) {
        let widths = xref_widths(append.entries);
        let index = index_runs(append.entries);
        let body = pack_entries(append.entries, widths);

        self.out
            .extend_from_slice(format!("{} 0 obj\n", append.xref_object_number).as_bytes());
        self.out.extend_from_slice(b"<< /Type /XRef /Size ");
        self.out
            .extend_from_slice(append.size.to_string().as_bytes());
        self.out.extend_from_slice(b" /Index [");
        for (position, (first, count)) in index.iter().enumerate() {
            if position > 0 {
                self.out.push(b' ');
            }
            self.out
                .extend_from_slice(format!("{first} {count}").as_bytes());
        }
        self.out.extend_from_slice(b"] /W [");
        self.out
            .extend_from_slice(format!("{} {} {}", widths[0], widths[1], widths[2]).as_bytes());
        self.out.extend_from_slice(b"] /Root ");
        self.out.extend_from_slice(
            format!("{} {} R", append.root.object_number, append.root.generation).as_bytes(),
        );
        self.out.extend_from_slice(b" /Prev ");
        self.out
            .extend_from_slice(append.prev_byte_offset.to_string().as_bytes());
        self.out.extend_from_slice(b" /Length ");
        self.out
            .extend_from_slice(body.len().to_string().as_bytes());
        if let Some(id) = append.id_bytes {
            self.out.extend_from_slice(b" /ID ");
            self.out.extend_from_slice(id);
        }
        if let Some(info) = append.info_bytes {
            self.out.extend_from_slice(b" /Info ");
            self.out.extend_from_slice(info);
        }
        self.out.extend_from_slice(b" >>\nstream\n");
        self.out.extend_from_slice(&body);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn push_startxref(&mut self, xref_byte_offset: usize) {
        self.out.extend_from_slice(b"startxref\n");
        self.out
            .extend_from_slice(xref_byte_offset.to_string().as_bytes());
        self.out.extend_from_slice(b"\n%%EOF\n");
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

fn xref_widths(entries: &[XrefStreamEntry]) -> [usize; 3] {
    let max_offset = entries
        .iter()
        .map(|entry| entry.byte_offset)
        .max()
        .unwrap_or(0);
    let max_generation = entries
        .iter()
        .map(|entry| usize::from(entry.generation))
        .max()
        .unwrap_or(0);
    [1, minimal_width(max_offset), minimal_width(max_generation)]
}

fn minimal_width(value: usize) -> usize {
    for (index, byte) in value.to_be_bytes().iter().enumerate() {
        if *byte != 0 {
            return value.to_be_bytes().len() - index;
        }
    }
    1
}

fn index_runs(entries: &[XrefStreamEntry]) -> Vec<(u32, usize)> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let first = entries[index].object_number;
        let start = index;
        while index + 1 < entries.len()
            && entries[index + 1].object_number == entries[index].object_number + 1
        {
            index += 1;
        }
        index += 1;
        runs.push((first, index - start));
    }
    runs
}

fn pack_entries(entries: &[XrefStreamEntry], widths: [usize; 3]) -> Vec<u8> {
    let record_width = widths.iter().sum::<usize>();
    let mut out = Vec::with_capacity(record_width * entries.len());
    for entry in entries {
        push_be(&mut out, 1, widths[0]);
        push_be(&mut out, entry.byte_offset, widths[1]);
        push_be(&mut out, usize::from(entry.generation), widths[2]);
    }
    out
}

fn push_be(out: &mut Vec<u8>, value: usize, width: usize) {
    let bytes = value.to_be_bytes();
    out.extend_from_slice(&bytes[bytes.len() - width..]);
}

#[cfg(test)]
mod tests {
    use super::{XrefStreamEntry, index_runs, minimal_width, pack_entries, xref_widths};

    #[test]
    fn minimal_width_is_at_least_one() {
        assert_eq!(minimal_width(0), 1);
        assert_eq!(minimal_width(255), 1);
        assert_eq!(minimal_width(256), 2);
    }

    #[test]
    fn index_runs_are_ascending_subsections() {
        let entries = vec![
            entry(2, 0, 10),
            entry(3, 0, 20),
            entry(7, 0, 30),
            entry(8, 0, 40),
        ];
        assert_eq!(index_runs(&entries), vec![(2, 2), (7, 2)]);
    }

    #[test]
    fn entries_pack_big_endian_with_computed_widths() {
        let entries = vec![entry(3, 7, 0x0102)];
        let widths = xref_widths(&entries);
        assert_eq!(widths, [1, 2, 1]);
        assert_eq!(pack_entries(&entries, widths), vec![1, 0x01, 0x02, 7]);
    }

    fn entry(object_number: u32, generation: u16, byte_offset: usize) -> XrefStreamEntry {
        XrefStreamEntry {
            object_number,
            generation,
            byte_offset,
        }
    }
}
