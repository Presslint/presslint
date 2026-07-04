//! CLI error taxonomy and user-facing messages.

use std::{fmt, io, path::Path, path::PathBuf};

use presslint::PdfInventoryError;
use presslint_write::ConvertContentColorsError;

const SUPPORTED_SELECTOR_SUBSET: &str = "supported selector subset: ColorSpace \
DeviceGray/DeviceRGB/DeviceCMYK, Page/PageMatch, ColorUsage fill/stroke, and \
ColorComponents or ComponentCompare over those device spaces with usage None/Fill/Stroke";

/// Hard CLI error. Every variant maps to process exit code 1.
#[derive(Debug)]
pub enum CliError {
    /// Invalid command-line usage or parsed argument value.
    Usage(String),
    /// File or stream I/O failure.
    Io {
        /// Action being attempted.
        action: &'static str,
        /// Optional path associated with the failure.
        path: Option<PathBuf>,
        /// Source I/O error.
        source: io::Error,
    },
    /// Selector JSON parsing failed.
    SelectorJson {
        /// Raw selector source description.
        source: String,
        /// Serde error with line/column.
        error: serde_json::Error,
    },
    /// Conversion library failure.
    Convert(ConvertContentColorsError),
    /// Audit library failure.
    Audit(PdfInventoryError),
    /// JSON report rendering failed.
    ReportJson(serde_json::Error),
}

impl CliError {
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub(crate) fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: Some(path.to_path_buf()),
            source,
        }
    }

    pub(crate) const fn io_stream(action: &'static str, source: io::Error) -> Self {
        Self::Io {
            action,
            path: None,
            source,
        }
    }

    pub(crate) const fn selector_json(source: String, error: serde_json::Error) -> Self {
        Self::SelectorJson { source, error }
    }

    pub(crate) const fn report_json(error: serde_json::Error) -> Self {
        Self::ReportJson(error)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Io {
                action,
                path: Some(path),
                source,
            } => write!(formatter, "{action} '{}': {source}", path.display()),
            Self::Io {
                action,
                path: None,
                source,
            } => write!(formatter, "{action}: {source}"),
            Self::SelectorJson { source, error } => {
                write!(formatter, "invalid selector JSON from {source}: {error}")
            }
            Self::Convert(ConvertContentColorsError::UnsupportedTargetSelector { unsupported }) => {
                write!(
                    formatter,
                    "unsupported target selector leaves: {unsupported:?}; {SUPPORTED_SELECTOR_SUBSET}"
                )
            }
            Self::Convert(error) => write!(formatter, "conversion failed: {error:?}"),
            Self::Audit(error) => write!(formatter, "audit failed: {error:?}"),
            Self::ReportJson(error) => write!(formatter, "render JSON report: {error}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ConvertContentColorsError> for CliError {
    fn from(error: ConvertContentColorsError) -> Self {
        Self::Convert(error)
    }
}

impl From<PdfInventoryError> for CliError {
    fn from(error: PdfInventoryError) -> Self {
        Self::Audit(error)
    }
}
