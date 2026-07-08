//! Structured run reports and deterministic rendering.

use std::{
    io::{self, Write},
    time::Duration,
};

use presslint::{ColorAuditStatus, ColorUsageAudit};
use presslint_write::{ConvertContentColorsOutput, ConvertPageSkip, ConvertedPage};
use serde::Serialize;

use crate::error::CliError;

const MEMORY_NOTE: &str =
    "whole-file mode: input and output/report are held in memory by the library API";

/// Requested report format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Human-readable report.
    Human,
    /// JSON report.
    Json,
}

impl ReportFormat {
    /// Build a report format from a `--json` flag.
    pub const fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }
}

/// Structured command report boundary.
#[derive(Debug)]
pub struct RunReport {
    payload: ReportPayload,
    warnings: Vec<String>,
}

#[derive(Debug)]
enum ReportPayload {
    Convert(ConvertContentColorsOutput),
    Audit {
        audit: Box<ColorUsageAudit>,
        max_decoded_stream_bytes: usize,
    },
}

impl RunReport {
    /// Build a report for a successful conversion.
    pub fn convert(output: ConvertContentColorsOutput) -> Self {
        let warnings = convert_warnings(&output);
        Self {
            payload: ReportPayload::Convert(output),
            warnings,
        }
    }

    /// Build a report for a successful audit.
    pub fn audit(audit: ColorUsageAudit, max_decoded_stream_bytes: usize) -> Self {
        let mut warnings = Vec::new();
        if audit.status == ColorAuditStatus::Incomplete || !audit.coverage_gaps.is_empty() {
            warnings.push(format!(
                "audit coverage incomplete: {} coverage gap(s)",
                audit.coverage_gaps.len()
            ));
        }
        Self {
            payload: ReportPayload::Audit {
                audit: Box::new(audit),
                max_decoded_stream_bytes,
            },
            warnings,
        }
    }

    /// Render this report to the command's required output stream.
    pub fn render(&self, format: ReportFormat) -> Result<(), CliError> {
        match format {
            ReportFormat::Human => self.render_human(),
            ReportFormat::Json => self.render_json(),
        }
    }

    /// Serialize this report as deterministic pretty JSON.
    pub fn to_json_string(&self) -> Result<String, CliError> {
        match &self.payload {
            ReportPayload::Convert(output) => {
                let report = JsonRunReport {
                    command: "convert",
                    memory_note: MEMORY_NOTE,
                    warnings: &self.warnings,
                    result: JsonPayload::Convert {
                        library_output: ConvertJsonOutput {
                            converted: &output.converted,
                            skipped: &output.skipped,
                        },
                    },
                };
                serde_json::to_string_pretty(&report).map_err(CliError::report_json)
            }
            ReportPayload::Audit {
                audit,
                max_decoded_stream_bytes,
            } => {
                let report = JsonRunReport {
                    command: "audit",
                    memory_note: MEMORY_NOTE,
                    warnings: &self.warnings,
                    result: JsonPayload::Audit {
                        max_decoded_stream_bytes: *max_decoded_stream_bytes,
                        library_output: audit,
                    },
                };
                serde_json::to_string_pretty(&report).map_err(CliError::report_json)
            }
        }
    }

    fn render_human(&self) -> Result<(), CliError> {
        let mut stderr = io::stderr().lock();
        match &self.payload {
            ReportPayload::Convert(output) => {
                render_convert_human(&mut stderr, output, &self.warnings)
            }
            ReportPayload::Audit {
                audit,
                max_decoded_stream_bytes,
            } => render_audit_human(
                &mut stderr,
                audit,
                *max_decoded_stream_bytes,
                &self.warnings,
            ),
        }
    }

    fn render_json(&self) -> Result<(), CliError> {
        let mut stdout = io::stdout().lock();
        let report = self.to_json_string()?;
        stdout
            .write_all(report.as_bytes())
            .map_err(|source| CliError::io_stream("write JSON report", source))?;
        stdout
            .write_all(b"\n")
            .map_err(|source| CliError::io_stream("write JSON report", source))
    }
}

/// Render the human conversion report to an arbitrary writer.
pub fn render_convert_human(
    out: &mut dyn Write,
    output: &ConvertContentColorsOutput,
    warnings: &[String],
) -> Result<(), CliError> {
    writeln!(out, "presslint convert").map_err(stderr_error)?;
    writeln!(out, "  {MEMORY_NOTE}").map_err(stderr_error)?;
    let totals = ConvertTotals::from_output(output);
    writeln!(out, "  pages analysed: {}", output.converted.len()).map_err(stderr_error)?;
    writeln!(out, "  pages skipped: {}", output.skipped.len()).map_err(stderr_error)?;
    writeln!(out, "  operators converted: {}", totals.converted).map_err(stderr_error)?;
    writeln!(out, "  black preserved: {}", totals.black_preserved).map_err(stderr_error)?;
    writeln!(out, "  no matching link: {}", totals.no_matching_link).map_err(stderr_error)?;
    writeln!(out, "  selector excluded: {}", totals.selector_excluded).map_err(stderr_error)?;
    for page in &output.converted {
        writeln!(
            out,
            "  page {}: converted={} black_preserved={} no_matching_link={} selector_excluded={}",
            page.page_index.0 + 1,
            page.operators_converted,
            page.black_preserved,
            page.operator_skips.no_matching_link,
            page.operator_skips.selector_excluded
        )
        .map_err(stderr_error)?;
    }
    for skip in &output.skipped {
        writeln!(
            out,
            "  skipped page {}: {:?}",
            skip.page_index.0 + 1,
            skip.reason
        )
        .map_err(stderr_error)?;
    }
    render_warnings(out, warnings)
}

/// Render the human audit report to an arbitrary writer.
pub fn render_audit_human(
    out: &mut dyn Write,
    audit: &ColorUsageAudit,
    max_decoded_stream_bytes: usize,
    warnings: &[String],
) -> Result<(), CliError> {
    writeln!(out, "presslint audit").map_err(stderr_error)?;
    writeln!(out, "  {MEMORY_NOTE}").map_err(stderr_error)?;
    writeln!(
        out,
        "  max decoded stream bytes: {max_decoded_stream_bytes}"
    )
    .map_err(stderr_error)?;
    writeln!(out, "  status: {:?}", audit.status).map_err(stderr_error)?;
    writeln!(out, "  pages: {}", audit.pages.len()).map_err(stderr_error)?;
    writeln!(out, "  rgb findings: {}", audit.rgb_findings.len()).map_err(stderr_error)?;
    writeln!(
        out,
        "  default color-space findings: {}",
        audit.default_color_space_findings.len()
    )
    .map_err(stderr_error)?;
    writeln!(
        out,
        "  icc-based findings: {}",
        audit.icc_based_findings.len()
    )
    .map_err(stderr_error)?;
    writeln!(out, "  spot names: {}", audit.spot_names.len()).map_err(stderr_error)?;
    writeln!(out, "  coverage gaps: {}", audit.coverage_gaps.len()).map_err(stderr_error)?;
    render_warnings(out, warnings)
}

fn render_warnings(out: &mut dyn Write, warnings: &[String]) -> Result<(), CliError> {
    for warning in warnings {
        writeln!(out, "  WARNING: {warning}").map_err(stderr_error)?;
    }
    Ok(())
}

/// Render a deterministic timing block from already-measured durations.
pub fn render_timing(
    phases: &[(&str, Duration)],
    total: Duration,
    out: &mut impl Write,
) -> Result<(), CliError> {
    write!(out, "timing:").map_err(stderr_error)?;
    for (label, duration) in phases {
        write!(out, " {label} {}", format_duration(*duration)).map_err(stderr_error)?;
    }
    writeln!(out, " total {}", format_duration(total)).map_err(stderr_error)
}

/// Render already-measured timing to the CLI diagnostics stream.
pub fn write_timing(phases: &[(&str, Duration)], total: Duration) -> Result<(), CliError> {
    let mut stderr = io::stderr().lock();
    render_timing(phases, total, &mut stderr)
}

fn format_duration(duration: Duration) -> String {
    if duration.as_millis() > 0 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}µs", duration.as_micros())
    }
}

const fn stderr_error(source: io::Error) -> CliError {
    CliError::io_stream("write report to stderr", source)
}

/// Build warnings from conversion coverage counters.
pub fn convert_warnings(output: &ConvertContentColorsOutput) -> Vec<String> {
    let totals = ConvertTotals::from_output(output);
    let mut warnings = Vec::new();
    if totals.converted == 0 {
        warnings.push("zero operators converted".to_owned());
    }
    if totals.coverage_gap_count() > 0 || !output.skipped.is_empty() {
        warnings.push(format!(
            "coverage gaps or skips observed: no_matching_link={} selector_excluded={} invalid_operands={} skipped_pages={}",
            totals.no_matching_link,
            totals.selector_excluded,
            totals.invalid_operands,
            output.skipped.len()
        ));
    }
    warnings
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ConvertTotals {
    converted: usize,
    black_preserved: usize,
    no_matching_link: usize,
    selector_excluded: usize,
    invalid_operands: usize,
}

impl ConvertTotals {
    fn from_output(output: &ConvertContentColorsOutput) -> Self {
        output
            .converted
            .iter()
            .fold(Self::default(), |mut totals, page| {
                totals.converted += page.operators_converted;
                totals.black_preserved += page.black_preserved;
                totals.no_matching_link += page.operator_skips.no_matching_link;
                totals.selector_excluded += page.operator_skips.selector_excluded;
                totals.invalid_operands += page.operator_skips.wrong_operand_count
                    + page.operator_skips.non_number_operand
                    + page.operator_skips.operand_out_of_range;
                totals
            })
    }

    const fn coverage_gap_count(&self) -> usize {
        self.no_matching_link + self.selector_excluded + self.invalid_operands
    }
}

#[derive(Serialize)]
struct JsonRunReport<'a, T> {
    command: &'static str,
    memory_note: &'static str,
    warnings: &'a [String],
    result: T,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonPayload<'a> {
    Convert {
        library_output: ConvertJsonOutput<'a>,
    },
    Audit {
        max_decoded_stream_bytes: usize,
        library_output: &'a ColorUsageAudit,
    },
}

#[derive(Serialize)]
struct ConvertJsonOutput<'a> {
    converted: &'a [ConvertedPage],
    skipped: &'a [ConvertPageSkip],
}

#[cfg(test)]
#[path = "tests/timing.rs"]
mod timing_tests;
