//! `presslint audit` execution.

use std::fs;

use presslint::audit_color_usage;

use crate::{args::AuditArgs, error::CliError, report::RunReport};

/// Generous decoded-stream cap for current large-file smoke runs.
///
/// The CLI still reads the full PDF into memory because the library API is
/// `&[u8] -> owned report`. A command-line override is intentionally deferred.
const MAX_DECODED_STREAM_BYTES: usize = 512 * 1024 * 1024;

/// Execute an `audit` command.
pub fn run(args: &AuditArgs) -> Result<RunReport, CliError> {
    let input = fs::read(&args.input)
        .map_err(|source| CliError::io("read input PDF", &args.input, source))?;
    let audit = audit_color_usage(&input, MAX_DECODED_STREAM_BYTES)?;
    Ok(RunReport::audit(audit, MAX_DECODED_STREAM_BYTES))
}
