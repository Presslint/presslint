//! Test aggregator for the `presslint-paint` crate.
//!
//! The focused unit tests live in the submodules below, split by subject so each
//! file stays well under the file-size gate with headroom for Phase 1 additions:
//!
//! - [`walker`]: `Rc`-interned graphics-state identity across ops and save/restore.
//! - [`paint_program`]: the replayable [`PaintProgram`](crate::PaintProgram) stream
//!   (replay + walk agreement + error fusing).
//! - [`provenance`]: [`DecodedRange`](crate::DecodedRange) serde-transparency.
//! - [`mutation_class`]: the [`MutationClass`](crate::MutationClass) routing predicates.
//! - [`call_machine`]: the call/return [`CallMachine`](crate::CallMachine) traversal,
//!   invocation identity, and the `InvocationPath` JSON round trip.
//! - [`extgstate_env`]: `gs` classification onto the snapshot — layered hits, the
//!   all-`Unresolved` miss, the empty-env legacy identity, and `q`/`Q` restore.
//! - [`mini_json`]: the dependency-free JSON serializer/parser shared by the
//!   serde-transparency and invocation-path locks.
//!
//! Genuinely shared fixtures live in this aggregator so no submodule duplicates
//! them: the [`assemble`] operator-record builder and the
//! [`name`]/[`page_program`]/[`form_program`] program builders (also reused by
//! the `flat_projection` colocated tests).

use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};
use presslint_types::{ContentScope, PdfName};

use crate::{ColorSpaceEnv, ExtGStateEnv, PaintSubProgram};

mod call_machine;
mod extgstate_env;
mod mini_json;
mod mutation_class;
mod paint_program;
mod provenance;
mod walker;

/// Tokenize + assemble a content stream into owned operator records for testing.
pub fn assemble(input: &[u8]) -> Result<Vec<OperatorRecord>, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    Ok(assembled.records)
}

pub fn name(value: &[u8]) -> PdfName {
    PdfName(value.to_vec())
}

pub fn page_program<'a>(
    source: &'a [u8],
    records: &'a [OperatorRecord],
    images: &'a [PdfName],
    forms: &'a [PdfName],
) -> PaintSubProgram<'a> {
    PaintSubProgram {
        source,
        records,
        color_space_env: ColorSpaceEnv::empty(),
        extgstate_env: ExtGStateEnv::empty(),
        image_xobject_names: images,
        form_xobject_names: forms,
        scope: ContentScope::Page,
    }
}

pub fn form_program<'a>(
    source: &'a [u8],
    records: &'a [OperatorRecord],
    images: &'a [PdfName],
    forms: &'a [PdfName],
    form_name: PdfName,
) -> PaintSubProgram<'a> {
    PaintSubProgram {
        source,
        records,
        color_space_env: ColorSpaceEnv::empty(),
        extgstate_env: ExtGStateEnv::empty(),
        image_xobject_names: images,
        form_xobject_names: forms,
        scope: ContentScope::FormXObject { name: form_name },
    }
}
