//! Rust style checker executable.

#![deny(clippy::all, missing_docs, unused_crate_dependencies)]

mod cli;
mod style;

mod prelude {
	pub use color_eyre::{Result, eyre};
}

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

fn normalize_args(mut args: Vec<String>) -> Vec<String> {
	if args.get(1).is_some_and(|arg| arg == "vstyle") {
		args.remove(1);
	}

	args
}

fn normalized_cli_args() -> Vec<String> {
	normalize_args(std::env::args().collect::<Vec<_>>())
}

fn main() -> ExitCode {
	if let Err(err) = color_eyre::install() {
		eprintln!("Failed to initialize error reporter: {err}.");

		return ExitCode::FAILURE;
	}

	match Cli::parse_from(normalized_cli_args()).run() {
		Ok(code) => code,
		Err(err) => {
			eprintln!("{err:?}");

			ExitCode::FAILURE
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cargo_subcommand_arg_shape_is_normalized() {
		let args = vec!["cargo-vstyle".to_owned(), "vstyle".to_owned(), "curate".to_owned()];
		let normalized = normalize_args(args);

		assert_eq!(normalized, vec!["cargo-vstyle", "curate"]);
	}
}
