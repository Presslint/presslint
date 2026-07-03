//! `presslint audit` execution.

use std::{fs, time::Instant};

use presslint::audit_color_usage;

use crate::{
    args::AuditArgs,
    error::CliError,
    report::{RunReport, write_timing},
};

/// Generous decoded-stream cap for current large-file smoke runs.
///
/// The CLI still reads the full PDF into memory because the library API is
/// `&[u8] -> owned report`. A command-line override is intentionally deferred.
const MAX_DECODED_STREAM_BYTES: usize = 512 * 1024 * 1024;

/// Execute an `audit` command.
pub fn run(args: &AuditArgs) -> Result<RunReport, CliError> {
    let total_start = args.timing.then(Instant::now);
    let phase_start = args.timing.then(Instant::now);
    let input = fs::read(&args.input)
        .map_err(|source| CliError::io("read input PDF", &args.input, source))?;
    let read_input = phase_start.map(|start| start.elapsed());

    let phase_start = args.timing.then(Instant::now);
    let audit = audit_color_usage(&input, MAX_DECODED_STREAM_BYTES)?;
    let audit_duration = phase_start.map(|start| start.elapsed());

    if let (Some(total_start), Some(read_input), Some(audit_duration)) =
        (total_start, read_input, audit_duration)
    {
        write_timing(
            &[("read_input", read_input), ("audit", audit_duration)],
            total_start.elapsed(),
        )?;
    }

    Ok(RunReport::audit(audit, MAX_DECODED_STREAM_BYTES))
}
