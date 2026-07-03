//! `presslint convert` execution.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use presslint_write::{
    BlackPreservationPolicy, ConvertContentColorsRequest, DeviceLinkInput,
    convert_content_colors_incremental,
};

use crate::{
    args::{
        ConvertArgs, parse_device_link_arg, parse_pages, parse_selector_arg, same_input_output,
    },
    error::CliError,
    report::RunReport,
};

/// Execute a `convert` command.
pub fn run(args: &ConvertArgs) -> Result<RunReport, CliError> {
    validate_output_routing(args)?;

    let input = fs::read(&args.input)
        .map_err(|source| CliError::io("read input PDF", &args.input, source))?;
    if args.output != "-" {
        let output = Path::new(&args.output);
        if same_input_output(&args.input, output)
            .map_err(|source| CliError::io("compare input and output paths", output, source))?
        {
            return Err(CliError::usage("input and output paths must be distinct"));
        }
    }

    let request = ConvertContentColorsRequest {
        pages: parse_pages(&args.pages)?,
        device_links: read_device_links(&args.device_links)?,
        black_preservation: if args.preserve_black {
            BlackPreservationPolicy::NeutralBlackToK
        } else {
            BlackPreservationPolicy::None
        },
        target: parse_selector_arg(args.select.as_deref())?,
    };

    let output = convert_content_colors_incremental(&input, &request)?;
    write_output(&args.output, &output.bytes)?;
    Ok(RunReport::convert(output))
}

/// Validate stdout/report routing before conversion starts.
pub fn validate_output_routing(args: &ConvertArgs) -> Result<(), CliError> {
    if args.json && args.output == "-" {
        return Err(CliError::usage(
            "--json cannot be combined with -o - because both JSON and PDF bytes would use stdout",
        ));
    }
    Ok(())
}

fn read_device_links(inputs: &[String]) -> Result<Vec<DeviceLinkInput>, CliError> {
    inputs
        .iter()
        .map(|input| {
            let parsed = parse_device_link_arg(input)?;
            let bytes = fs::read(&parsed.path)
                .map_err(|source| CliError::io("read DeviceLink", &parsed.path, source))?;
            Ok(DeviceLinkInput {
                id: parsed.id,
                bytes,
            })
        })
        .collect()
}

fn write_output(output: &str, bytes: &[u8]) -> Result<(), CliError> {
    if output == "-" {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(bytes)
            .map_err(|source| CliError::io_stream("write PDF to stdout", source))?;
        stdout
            .flush()
            .map_err(|source| CliError::io_stream("flush PDF stdout", source))?;
        return Ok(());
    }

    write_atomic(Path::new(output), bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| CliError::usage("output path must name a file"))?
        .to_string_lossy();
    let temp = temp_path(parent, &file_name)?;

    let write_result = (|| {
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|source| CliError::io("create temp output", &temp, source))?;
            file.write_all(bytes)
                .map_err(|source| CliError::io("write temp output", &temp, source))?;
            file.sync_all()
                .map_err(|source| CliError::io("sync temp output", &temp, source))?;
        }
        fs::rename(&temp, path).map_err(|source| CliError::io("rename temp output", path, source))
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn temp_path(parent: &Path, file_name: &str) -> Result<PathBuf, CliError> {
    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(
            ".{file_name}.presslint-tmp-{}-{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CliError::usage(
        "could not allocate a temporary output path",
    ))
}
