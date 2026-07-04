//! Interim `ExtGState` (`gs`) presence scanner for the device-colour converter.
//!
//! The shipped device-colour converter is overprint/transparency BLIND: it
//! recognises only the direct device operators `g/G rg/RG k/K` and ignores `gs`
//! (the `ExtGState`-set operator, ISO 32000 §8.4.5). A stream that activates
//! overprint via `gs` and then paints with `k` gets its `k` rewritten through a
//! CMYK->CMYK `DeviceLink`; the link can change which channels are 0%, and under
//! OPM=1 that silently changes per-colorant knockout on press. That is a
//! match-or-skip violation (a silent wrong colour = a reprint).
//!
//! This module is the deliberately CONSERVATIVE, spine-independent INTERIM guard:
//! a pure `gs`-presence scanner. The converter uses it to poison an ENTIRE page
//! (ISO 32000 §7.8.2: a page's content streams are concatenated and share
//! graphics state, so `gs` set in one stream carries across the others) whenever
//! any decodable stream of that page contains `gs`. A later phase replaces this
//! coarse guard with a precise per-operator `ExtGState`-aware guard on the
//! `presslint-paint` spine.

use presslint_syntax::{assemble_operators, tokenize};

/// The `ExtGState` set operator (ISO 32000 §8.4.5): `/name gs`.
const GS_OPERATOR: &[u8] = b"gs";

/// Return `true` iff `decoded` content-stream bytes contain at least one `gs`
/// (`ExtGState` set) OPERATOR token.
///
/// Detection is operator-precise: the bytes are tokenized and assembled into
/// operator records, and only a record whose operator token bytes are exactly
/// `gs` counts — the bytes `gs` inside a string, name, or comment never match
/// (the same operator-assembly pattern the converter uses to classify colour
/// operators). A stream that fails to tokenize or assemble is treated as
/// containing no `gs`: the pipeline's per-stream round-trip gate already refuses
/// to convert such a stream, so it can never be a wrongly-converted stream.
#[must_use]
pub fn has_extgstate(decoded: &[u8]) -> bool {
    let Ok(tokens) = tokenize(decoded) else {
        return false;
    };
    let Ok(assembled) = assemble_operators(&tokens) else {
        return false;
    };
    assembled.records.iter().any(|record| {
        tokens[record.operator.token_index].source_bytes(decoded) == Some(GS_OPERATOR)
    })
}
