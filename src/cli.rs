// crates.io
use clap::{
	Parser, Subcommand,
	builder::{
		Styles,
		styling::{AnsiColor, Effects},
	},
};

// std
use std::{path::PathBuf, process::ExitCode};

// self
use crate::{
	prelude::*,
	style_checker::{self, RunSummary},
};

/// Command-line interface for the Rust style checker.
#[derive(Debug, Parser)]
#[command(
	version = concat!(
		env!("CARGO_PKG_VERSION"),
		"-",
		env!("VERGEN_GIT_SHA"),
		"-",
		env!("VERGEN_CARGO_TARGET_TRIPLE"),
	),
	rename_all = "kebab",
	styles = styles(),
)]
pub(crate) struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
	/// Run style checks and report violations.
	Check {
		/// Optional Rust files. Defaults to git-tracked `*.rs`.
		files: Vec<PathBuf>,
	},
	/// Apply all safe automatic fixes, then re-check.
	Fix {
		/// Optional Rust files. Defaults to git-tracked `*.rs`.
		files: Vec<PathBuf>,
	},
	/// Print implemented rule IDs.
	Coverage,
}

impl Cli {
	pub(crate) fn run(&self) -> Result<ExitCode> {
		match &self.command {
			Command::Check { files } => {
				let summary = style_checker::run_check(files)?;
				print_summary(&summary, false);
				if summary.violation_count > 0 {
					eprintln!("\nFound {} style violation(s).", summary.violation_count);
					return Ok(ExitCode::FAILURE);
				}
			},
			Command::Fix { files } => {
				let summary = style_checker::run_fix(files)?;
				print_summary(&summary, true);
				if summary.violation_count > 0 {
					eprintln!(
						"\nFound {} remaining style violation(s) after fix.",
						summary.violation_count
					);
					return Ok(ExitCode::FAILURE);
				}
			},
			Command::Coverage => style_checker::print_coverage(),
		}

		Ok(ExitCode::SUCCESS)
	}
}

fn print_summary(summary: &RunSummary, fix_mode: bool) {
	for line in &summary.output_lines {
		println!("{line}");
	}

	if fix_mode {
		println!(
			"\nChecked {} file(s). Applied {} fix(es).",
			summary.file_count, summary.applied_fix_count
		);
	} else {
		println!("\nChecked {} file(s).", summary.file_count);
	}

	if summary.unfixable_count > 0 {
		println!("{} violation(s) require manual fixes.", summary.unfixable_count);
	}
}

fn styles() -> Styles {
	Styles::styled()
		.header(AnsiColor::Red.on_default() | Effects::BOLD)
		.usage(AnsiColor::Red.on_default() | Effects::BOLD)
		.literal(AnsiColor::Blue.on_default() | Effects::BOLD)
		.placeholder(AnsiColor::Green.on_default())
}

#[cfg(test)]
mod tests {
	// self
	use super::*;

	#[test]
	fn parses_check_subcommand() {
		let cli = Cli::parse_from(["app", "check"]);
		assert!(matches!(cli.command, Command::Check { .. }));
	}
}
