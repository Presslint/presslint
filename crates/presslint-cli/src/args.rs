//! Argument parsing helpers for the thin CLI.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process,
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use presslint_selectors::Selector;
use presslint_types::PageIndex;
use presslint_write::PageSelection;

use crate::error::CliError;

const SELECT_HELP: &str = "Selector JSON, or @file.json. Conversion supports only ColorSpace \
    DeviceGray/DeviceRGB/DeviceCMYK, Page/PageMatch, ColorUsage fill/stroke, and \
    ColorComponents over those device spaces.";
const MEMORY_HELP: &str = "Reads the whole input and holds the converted output in memory; peak \
    memory is roughly two to three times the PDF size.";
const MAX_EXPLICIT_PAGE_INDICES: usize = 1_000_000;

/// Parsed command line.
#[derive(Debug)]
pub struct Cli {
    /// Command to run.
    pub command: Command,
}

/// Top-level command.
#[derive(Debug)]
pub enum Command {
    /// Convert direct device content colours through `DeviceLink` ICC profiles.
    Convert(ConvertArgs),
    /// Audit observed document colour usage without modifying the PDF.
    Audit(AuditArgs),
}

/// `presslint convert` arguments.
#[derive(Debug)]
pub struct ConvertArgs {
    /// Input PDF.
    pub input: PathBuf,
    /// `DeviceLink` ICC profile, repeatable. Use id=path to set a stable report id.
    pub device_links: Vec<String>,
    /// Optional target selector JSON.
    pub select: Option<String>,
    /// One-based page spec: all, 1,3,5, 2-8, or mixed 1,3-5,9.
    pub pages: String,
    /// Preserve exact neutral black as destination CMYK K-only black.
    pub preserve_black: bool,
    /// Print the run report as JSON to stdout.
    pub json: bool,
    /// Print coarse per-phase wall-clock timing to stderr.
    pub timing: bool,
    /// Output PDF path, or - for stdout.
    pub output: String,
}

/// `presslint audit` arguments.
#[derive(Debug)]
pub struct AuditArgs {
    /// Input PDF.
    pub input: PathBuf,
    /// Print the run report as JSON to stdout.
    pub json: bool,
    /// Print coarse per-phase wall-clock timing to stderr.
    pub timing: bool,
}

impl Cli {
    pub fn parse() -> Self {
        match Self::try_parse_from(std::env::args_os()) {
            Ok(cli) => cli,
            Err(error) => {
                let exit_code = clap_error_exit_code(&error);
                let _ = error.print();
                process::exit(exit_code);
            }
        }
    }

    pub fn try_parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let matches = command_definition().try_get_matches_from(args)?;
        let Some((name, subcommand)) = matches.subcommand() else {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::MissingSubcommand,
                "missing command",
            ));
        };
        let command = match name {
            "convert" => Command::Convert(ConvertArgs::from_matches(subcommand)),
            "audit" => Command::Audit(AuditArgs::from_matches(subcommand)),
            _ => {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::InvalidSubcommand,
                    "unknown command",
                ));
            }
        };
        Ok(Self { command })
    }
}

pub fn clap_error_exit_code(error: &clap::Error) -> i32 {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => 0,
        _ => 1,
    }
}

impl ConvertArgs {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            input: PathBuf::from(required_string(matches, "input")),
            device_links: matches
                .get_many::<String>("device-link")
                .map(|values| values.cloned().collect())
                .unwrap_or_default(),
            select: matches.get_one::<String>("select").cloned(),
            pages: required_string(matches, "pages"),
            preserve_black: matches.get_flag("preserve-black"),
            json: matches.get_flag("json"),
            timing: matches.get_flag("timing"),
            output: required_string(matches, "output"),
        }
    }
}

impl AuditArgs {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            input: PathBuf::from(required_string(matches, "input")),
            json: matches.get_flag("json"),
            timing: matches.get_flag("timing"),
        }
    }
}

fn required_string(matches: &ArgMatches, name: &str) -> String {
    matches.get_one::<String>(name).cloned().unwrap_or_default()
}

fn command_definition() -> ClapCommand {
    ClapCommand::new("presslint")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Thin PDF prepress command-line driver.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(convert_command())
        .subcommand(audit_command())
}

fn convert_command() -> ClapCommand {
    ClapCommand::new("convert")
        .about("Convert direct device content colours through `DeviceLink` ICC profiles.")
        .after_help(MEMORY_HELP)
        .arg(Arg::new("input").value_name("IN.pdf").required(true))
        .arg(
            Arg::new("device-link")
                .long("device-link")
                .value_name("path")
                .required(true)
                .action(ArgAction::Append)
                .help(
                    "`DeviceLink` ICC profile, repeatable. Use id=path to set a stable report id.",
                ),
        )
        .arg(
            Arg::new("select")
                .long("select")
                .value_name("json|@file")
                .help(SELECT_HELP),
        )
        .arg(
            Arg::new("pages")
                .long("pages")
                .value_name("spec")
                .default_value("all")
                .help("One-based page spec: all, 1,3,5, 2-8, or mixed 1,3-5,9."),
        )
        .arg(
            Arg::new("preserve-black")
                .long("preserve-black")
                .action(ArgAction::SetTrue)
                .help("Preserve exact neutral black as destination CMYK K-only black."),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print the run report as JSON to stdout."),
        )
        .arg(
            Arg::new("timing")
                .long("timing")
                .action(ArgAction::SetTrue)
                .help("Print coarse read/compute/write wall-clock timing to stderr."),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .required(true)
                .value_name("OUT.pdf|-")
                .help("Output PDF path, or - for stdout."),
        )
}

fn audit_command() -> ClapCommand {
    ClapCommand::new("audit")
        .about("Audit observed document colour usage without modifying the PDF.")
        .after_help(MEMORY_HELP)
        .arg(Arg::new("input").value_name("IN.pdf").required(true))
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print the run report as JSON to stdout."),
        )
        .arg(
            Arg::new("timing")
                .long("timing")
                .action(ArgAction::SetTrue)
                .help("Print coarse read/compute wall-clock timing to stderr."),
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLinkArg {
    /// Opaque report id for this link.
    pub id: Option<String>,
    /// Filesystem path to the ICC profile bytes.
    pub path: PathBuf,
}

/// Parse a human one-based page selection into the write crate page selection.
pub fn parse_pages(spec: &str) -> Result<PageSelection, CliError> {
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return Ok(PageSelection::All);
    }
    if trimmed.is_empty() {
        return Err(CliError::usage("page spec must not be empty"));
    }

    let mut pages = BTreeSet::new();
    for segment in trimmed.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err(CliError::usage("page spec contains an empty segment"));
        }
        if let Some((start, end)) = segment.split_once('-') {
            let start = parse_one_based_page(start)?;
            let end = parse_one_based_page(end)?;
            if start > end {
                return Err(CliError::usage(format!(
                    "page range start {start} is greater than end {end}"
                )));
            }
            let count = end - start + 1;
            if count > MAX_EXPLICIT_PAGE_INDICES as u64 {
                return Err(CliError::usage(format!(
                    "page range {start}-{end} selects {count} pages; maximum explicit selection is {MAX_EXPLICIT_PAGE_INDICES}"
                )));
            }
            for page in start..=end {
                pages.insert(to_page_index(page)?);
            }
        } else {
            pages.insert(to_page_index(parse_one_based_page(segment)?)?);
        }
        if pages.len() > MAX_EXPLICIT_PAGE_INDICES {
            return Err(CliError::usage(format!(
                "page spec selects more than {MAX_EXPLICIT_PAGE_INDICES} pages"
            )));
        }
    }

    Ok(PageSelection::Indices(pages.into_iter().collect()))
}

/// Parse optional selector JSON from a raw argument or `@file`.
pub fn parse_selector_arg(input: Option<&str>) -> Result<Option<Selector>, CliError> {
    let Some(input) = input else {
        return Ok(None);
    };
    let (source, json) = if let Some(path) = input.strip_prefix('@') {
        if path.is_empty() {
            return Err(CliError::usage("--select @file requires a path"));
        }
        let path = Path::new(path);
        let json = fs::read_to_string(path)
            .map_err(|source| CliError::io("read selector", path, source))?;
        (format!("@{}", path.display()), json)
    } else {
        ("--select".to_owned(), input.to_owned())
    };
    if json.is_empty() {
        return Err(CliError::usage(format!("{source} is empty")));
    }
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|error| CliError::selector_json(source, error))
}

/// Parse one `--device-link` value.
pub fn parse_device_link_arg(input: &str) -> Result<DeviceLinkArg, CliError> {
    if let Some((id, path)) = input.split_once('=') {
        if id.is_empty() || path.is_empty() {
            return Err(CliError::usage(
                "--device-link id=path requires a non-empty id and path",
            ));
        }
        Ok(DeviceLinkArg {
            id: Some(id.to_owned()),
            path: PathBuf::from(path),
        })
    } else {
        let path = PathBuf::from(input);
        Ok(DeviceLinkArg {
            id: Some(derive_link_id(&path)),
            path,
        })
    }
}

fn parse_one_based_page(input: &str) -> Result<u64, CliError> {
    let value = input
        .trim()
        .parse::<u64>()
        .map_err(|_| CliError::usage(format!("invalid page number '{input}'")))?;
    if value == 0 {
        return Err(CliError::usage("page numbers are one-based; 0 is invalid"));
    }
    Ok(value)
}

fn to_page_index(one_based: u64) -> Result<PageIndex, CliError> {
    let zero_based = one_based - 1;
    let index = u32::try_from(zero_based)
        .map_err(|_| CliError::usage(format!("page number {one_based} is too large")))?;
    Ok(PageIndex(index))
}

fn derive_link_id(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "device-link".to_owned())
}

/// Return whether two paths point to the same filesystem location.
pub fn same_input_output(input: &Path, output: &Path) -> io::Result<bool> {
    if input == output {
        return Ok(true);
    }
    let input = input.canonicalize()?;
    match output.canonicalize() {
        Ok(output) => Ok(input == output),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = output.parent().filter(|path| !path.as_os_str().is_empty());
            let parent = parent.unwrap_or_else(|| Path::new(".")).canonicalize()?;
            let normalized = parent.join(output.file_name().unwrap_or_default());
            Ok(input == normalized)
        }
        Err(error) => Err(error),
    }
}
