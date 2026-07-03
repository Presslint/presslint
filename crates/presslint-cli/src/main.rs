//! Thin command-line driver for `presslint`.

mod args;
mod audit;
mod convert;
mod error;
mod report;

use std::process::ExitCode;

use crate::{
    args::{Cli, Command},
    error::CliError,
    report::ReportFormat,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("presslint: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert(args) => {
            let report = convert::run(&args)?;
            let format = ReportFormat::from_json_flag(args.json);
            report.render(format)
        }
        Command::Audit(args) => {
            let report = audit::run(&args)?;
            let format = ReportFormat::from_json_flag(args.json);
            report.render(format)
        }
    }
}

#[cfg(test)]
mod tests;
