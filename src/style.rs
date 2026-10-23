mod bindings;
mod engine;
mod file;
mod fixes;
mod generics;
mod impls;
mod imports;
mod module;
mod quality;
mod rules;
mod semantic;
mod shared;
mod spacing;
mod swift;
mod syntax;
mod test_modules;
mod types;
mod workspace;

pub(crate) use self::{
	engine::{run_check, run_fix},
	shared::{CargoOptions, RunSummary, StyleLanguage},
};

use crate::style::shared::STYLE_RULE_IDS;

pub(crate) fn print_coverage() {
	for rule in STYLE_RULE_IDS {
		println!("{rule}\timplemented");
	}
}
