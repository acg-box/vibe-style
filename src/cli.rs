use std::process::ExitCode;

use clap::{
	Args, Parser, Subcommand,
	builder::{
		Styles,
		styling::{AnsiColor, Effects},
	},
};
use color_eyre::Result;

use crate::style::{self, CargoOptions, RunSummary};

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
impl Cli {
	pub(crate) fn run(&self) -> Result<ExitCode> {
		match &self.command {
			Command::Curate { strict, cargo } => {
				let summary = style::run_check(&cargo.as_options())?;

				print_summary(&summary, false);

				if summary.violation_count > 0 {
					if *strict {
						eprintln!(
							"\nFound {} style violation(s) in strict mode.",
							summary.violation_count
						);
					} else {
						eprintln!("\nFound {} style violation(s).", summary.violation_count);
					}

					return Ok(ExitCode::FAILURE);
				}
			},
			Command::Tune { strict, cargo } => {
				let summary = style::run_fix(&cargo.as_options())?;

				print_summary(&summary, true);

				if summary.violation_count > 0 {
					eprintln!(
						"\nFound {} remaining style violation(s) after fix.",
						summary.violation_count
					);

					if *strict {
						return Ok(ExitCode::FAILURE);
					}
				}
			},
			Command::Coverage => style::print_coverage(),
		}

		Ok(ExitCode::SUCCESS)
	}
}

#[derive(Debug, Subcommand)]
enum Command {
	/// Curate style checks and report violations.
	Curate {
		/// Keep strict failure behavior explicit.
		#[arg(long)]
		strict: bool,

		#[command(flatten)]
		cargo: CargoCliOptions,
	},
	/// Tune style issues with safe automatic fixes, then re-check.
	Tune {
		/// Return a non-zero exit code when violations remain after fixes.
		#[arg(long)]
		strict: bool,

		#[command(flatten)]
		cargo: CargoCliOptions,
	},
	/// Print implemented rule IDs.
	Coverage,
}

#[derive(Debug, Clone, Args)]
struct CargoCliOptions {
	/// Check all packages in the workspace.
	#[arg(long)]
	workspace: bool,
	/// Check only the specified package(s), like cargo/clippy -p.
	#[arg(short = 'p', long = "package")]
	packages: Vec<String>,
	/// Space- or comma-separated feature list.
	#[arg(long, value_delimiter = ',')]
	features: Vec<String>,
	/// Activate all available features.
	#[arg(long = "all-features")]
	all_features: bool,
	/// Do not activate the `default` feature.
	#[arg(long = "no-default-features")]
	no_default_features: bool,
}
impl CargoCliOptions {
	fn as_options(&self) -> CargoOptions {
		CargoOptions {
			workspace: self.workspace,
			packages: self.packages.clone(),
			features: self.features.clone(),
			all_features: self.all_features,
			no_default_features: self.no_default_features,
		}
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
	use super::{Cli, Command};
	use clap::Parser;

	#[test]
	fn parses_curate_subcommand() {
		let cli = Cli::parse_from(["app", "curate"]);

		assert!(matches!(cli.command, Command::Curate { strict: false, .. }));
	}

	#[test]
	fn parses_curate_strict_subcommand() {
		let cli = Cli::parse_from(["app", "curate", "--strict"]);

		assert!(matches!(cli.command, Command::Curate { strict: true, .. }));
	}

	#[test]
	fn parses_tune_subcommand() {
		let cli = Cli::parse_from(["app", "tune"]);

		assert!(matches!(cli.command, Command::Tune { strict: false, .. }));
	}

	#[test]
	fn parses_tune_strict_subcommand() {
		let cli = Cli::parse_from(["app", "tune", "--strict"]);

		assert!(matches!(cli.command, Command::Tune { strict: true, .. }));
	}

	#[test]
	fn parses_tune_with_cargo_target_options() {
		let cli = Cli::parse_from([
			"app",
			"tune",
			"--workspace",
			"-p",
			"api",
			"--features",
			"serde,tracing",
			"--all-features",
			"--no-default-features",
		]);
		let Command::Tune { cargo, .. } = cli.command else {
			panic!("Expected tune command.");
		};

		assert!(cargo.workspace);
		assert_eq!(cargo.packages, vec!["api"]);
		assert_eq!(cargo.features, vec!["serde", "tracing"]);
		assert!(cargo.all_features);
		assert!(cargo.no_default_features);
	}

	#[test]
	fn rejects_curate_positional_paths() {
		let parsed = Cli::try_parse_from(["app", "curate", "src/main.rs"]);

		assert!(parsed.is_err());
	}

	#[test]
	fn rejects_tune_positional_paths() {
		let parsed = Cli::try_parse_from(["app", "tune", "src/main.rs"]);

		assert!(parsed.is_err());
	}
}
