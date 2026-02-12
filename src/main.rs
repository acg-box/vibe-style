//! Rust style checker executable.

#![deny(clippy::all, missing_docs, unused_crate_dependencies)]

mod cli;
mod style_checker;

mod prelude {
	pub use color_eyre::{Result, eyre};
}

use clap::Parser;

use std::process::ExitCode;

use crate::cli::Cli;

fn main() -> ExitCode {
	if let Err(err) = color_eyre::install() {
		eprintln!("Failed to initialize error reporter: {err}.");
		return ExitCode::FAILURE;
	}

	match Cli::parse().run() {
		Ok(code) => code,
		Err(err) => {
			eprintln!("{err:?}");
			ExitCode::FAILURE
		},
	}
}
