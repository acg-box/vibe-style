use std::path::Path;

use crate::style::{
	engine::{self, MAX_FIX_PASSES},
	fixes, module, rules,
	shared::{self, Edit},
};

#[test]
fn impl_fix_does_not_break_foreign_usage_paths() {
	let original = r#"
impl Usage {
	pub fn from_rig(local: Usage, usage: rig::completion::Usage) -> Usage {
		let _ = usage;
		local
	}
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("impl.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied > 0);
	assert!(rewritten.contains("local: Self"));
	assert!(rewritten.contains("usage: rig::completion::Usage"));
	assert!(rewritten.contains("-> Self"));
	assert!(!rewritten.contains(":: Self"));
}

#[test]
fn impl_fix_skips_trait_impl_signatures() {
	let original = r#"
struct UserData;
mod grpc {
	pub struct UserData;
}
impl From<UserData> for grpc::UserData {
	fn from(user: UserData) -> grpc::UserData {
		let _ = user;
		grpc::UserData
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("impl_trait_signature_guard.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-001"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-001"));
}

#[test]
fn impl_fix_does_not_rewrite_generic_target_variants_to_self() {
	let original = r#"
struct Inference<T> {
	output: T,
}
impl<T> Inference<T> {
	fn map_output<U>(self, f: impl FnOnce(T) -> U) -> Inference<U> {
		Inference { output: f(self.output) }
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("impl_generic_self_guard.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-001"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-001"));
}

#[test]
fn impl003_does_not_flag_std_trait_order_for_unqualified_display_import() {
	let original = r#"
use std::fmt::{Display, Formatter};

#[derive(Debug)]
enum DispatchError {
	QueueFull,
}
impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}
impl std::error::Error for DispatchError {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("impl003_std_display_import_order.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-003"));
}

#[test]
fn impl003_fix_reorders_impl_blocks_by_origin_group() {
	let original = r#"
struct Sample;

impl ext_crate::ExternalTrait for Sample {}

impl std::fmt::Display for Sample {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "sample")
	}
}

impl Sample {
	fn new() -> Self {
		Self
	}
}

impl crate::WorkspaceTrait for Sample {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("impl003_reorder_by_origin_group.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-003" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-003"));

	let impl003_edits =
		edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPL-003").collect::<Vec<_>>();
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, impl003_edits).expect("apply edits");

	assert!(applied > 0);

	let inherent_idx = rewritten.find("impl Sample {").expect("inherent impl");
	let std_trait_idx = rewritten
		.find("impl std::fmt::Display for Sample")
		.or_else(|| rewritten.find("impl Display for Sample"))
		.expect("std trait impl");
	let third_party_idx = rewritten
		.find("impl ext_crate::ExternalTrait for Sample")
		.or_else(|| rewritten.find("impl ExternalTrait for Sample"))
		.expect("third-party trait impl");
	let workspace_idx = rewritten
		.find("impl crate::WorkspaceTrait for Sample")
		.or_else(|| rewritten.find("impl WorkspaceTrait for Sample"))
		.expect("workspace trait impl");

	assert!(inherent_idx < std_trait_idx);
	assert!(std_trait_idx < third_party_idx);
	assert!(third_party_idx < workspace_idx);
}

#[test]
fn mod005_relocation_preserves_one_impl_when_another_edit_overlaps() {
	let original =
		"struct Sample;\n\nenum Other { Item }\n\nimpl Sample {\n    fn value() -> u32 { 1 }\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("move_overlap.rs"), original.to_owned())
			.expect("Read context.")
			.expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let start = original.find("impl Sample").expect("Find impl.");
	let mut edits =
		edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-MOD-005").collect::<Vec<_>>();

	edits.push(Edit {
		start,
		end: start + 1,
		replacement: "i".to_owned(),
		rule: "RUST-STYLE-IMPORT-001",
	});

	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Apply overlapping edits.");

	assert_eq!(rewritten.matches("impl Sample").count(), 1, "{rewritten}");
}

#[test]
fn mod005_fix_moves_impl_block_adjacent_to_type() {
	let original = r#"
struct Sample;

enum Other {
	Item,
}

impl Sample {
	fn new() -> Self {
		Self
	}
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("mod005_move_impl.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);

	let struct_idx = rewritten.find("struct Sample;").expect("struct");
	let impl_idx = rewritten.find("impl Sample").expect("impl");
	let enum_idx = rewritten.find("enum Other").expect("enum");

	assert!(struct_idx < impl_idx);
	assert!(impl_idx < enum_idx);
}

#[test]
fn mod005_fix_moves_impl_block_after_type_when_impl_precedes_type() {
	let original = r#"
impl Sample {
	fn new() -> Self {
		Self
	}
}

struct Sample;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod005_impl_before_type.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("struct Sample;\nimpl Sample {"));
}

#[test]
fn mod005_fix_does_not_accumulate_blank_lines_around_relocated_impls() {
	let original = r#"
use std::sync::Arc;

struct RoundGate {
	in_progress: bool,
}

struct RoundGateGuard {
	gate: Arc<RoundGate>,
}

impl RoundGate {
	fn new() -> Self {
		Self { in_progress: false }
	}
}

impl RoundGateGuard {
	fn new(gate: Arc<RoundGate>) -> Self {
		Self { gate }
	}
}

impl Drop for RoundGateGuard {
	fn drop(&mut self) {}
}

/// Bootstrap and run all crawlers.
pub async fn run() {}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..MAX_FIX_PASSES {
		let Some(ctx) = shared::read_file_context_from_text(
			Path::new("mod005_blank_lines.rs"),
			rewritten.clone(),
		)
		.expect("context") else {
			break;
		};
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		if applied == 0 {
			break;
		}
	}

	assert!(rewritten.contains("struct RoundGate {\n\tin_progress: bool,\n}\nimpl RoundGate {"));
	assert!(
		rewritten
			.contains("struct RoundGateGuard {\n\tgate: Arc<RoundGate>,\n}\nimpl RoundGateGuard {")
	);
	assert!(rewritten.contains("}\n\n/// Bootstrap and run all crawlers."));
	assert!(!rewritten.contains("}\n\n\n/// Bootstrap and run all crawlers."));
}

#[test]
fn mod005_fix_removes_blank_lines_between_type_and_impl() {
	let original = r#"
struct RuntimeEvent {
	id: usize,
}

impl RuntimeEvent {
	fn id(&self) -> usize {
		self.id
	}
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("mod005.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("}\nimpl RuntimeEvent {"));
}

#[test]
fn mod002_fix_reorders_pub_items_before_non_pub_items() {
	let original = r#"
fn internal() -> usize {
	1
}

pub fn external() -> usize {
	2
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("mod002.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("pub fn external() -> usize"));
	assert!(
		rewritten.find("pub fn external").unwrap_or_default()
			< rewritten.find("fn internal").unwrap_or_default()
	);
}

#[test]
fn mod003_fix_reorders_non_async_before_async_with_same_visibility() {
	let original = r#"
pub async fn pull() -> usize {
	1
}

pub fn plan() -> usize {
	2
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("mod003.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-003" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten.find("pub fn plan").unwrap_or_default()
			< rewritten.find("pub async fn pull").unwrap_or_default()
	);
}

#[test]
fn mod001_fix_reorders_cross_category_top_level_items() {
	let original = r#"
fn execute() -> usize {
	1
}

const LIMIT: usize = 3;
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("mod001_cross_kind.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten.find("const LIMIT").unwrap_or_default()
			< rewritten.find("fn execute").unwrap_or_default()
	);
}

#[test]
fn mod001_fix_reorders_use_before_macro_calls() {
	let original = r#"
macro_rules! define_placeholder {
	() => {};
}

define_placeholder! {}

pub use crate::api::Code;

define_placeholder! {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod001_macro_call_reorder.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-001"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	let use_pos = rewritten.find("pub use crate::api::Code;").expect("has use");
	let first_macro_call_pos = rewritten.find("define_placeholder! {}").expect("has macro call");

	assert!(use_pos < first_macro_call_pos);
}

#[test]
fn mod001_fix_reorders_use_before_macro_calls_inside_inline_module() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		def_api_codes! {
			ERR_A = -1,
			ERR_B = -2,
		}
	}

	def_api_codes! {
		OUTSIDE_A = -3,
	}

	#[cfg(feature = "pubfi")] pub use self::pubfi::*;

	#[cfg(feature = "pubfi")]
	def_api_codes! {
		OK = 0,
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("mod001_inline_module_macro_use.rs"), original, true)
			.expect("apply fix passes");
	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
	let use_pos = compact.find("pubuseself::pubfi::{ERR_A,ERR_B};").expect("has rewritten pub use");
	let first_macro_pos = compact.find("OUTSIDE_A=-3").expect("has first macro call");
	let second_macro_pos = compact.find("OK=0").expect("has second macro call");

	assert!(use_pos < first_macro_pos);
	assert!(first_macro_pos < second_macro_pos);
	assert!(rewritten.contains("}\n\n\t#[cfg(feature = \"pubfi\")] pub use self::pubfi::{"));
	assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};\n\n\tdef_api_codes! {"));
}

#[test]
fn mod001_fix_hoists_macro_rules_before_prior_nested_macro_call() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	pub mod pubfi {
		def_api_codes! {
			ERR_A = -1;
		}
	}

	pub use self::pubfi::{ERR_A};

	macro_rules! def_api_codes {
		($($name:ident = $code:expr;)*) => {
			$(pub const $name: i16 = $code;)*
		};
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("mod001_hoist_macro_rules.rs"), original, true)
			.expect("apply fix passes");
	let macro_pos = rewritten.find("macro_rules! def_api_codes").expect("has macro rules");
	let mod_pos = rewritten.find("pub mod pubfi").expect("has pubfi module");

	assert!(macro_pos < mod_pos, "{rewritten}");
}

#[test]
fn mod001_mod002_fix_reorders_use_blocks_split_by_mod_items() {
	let original = r#"use std::sync::LazyLock;

pub mod backoff;
#[cfg(feature = "crypto")] pub mod crypto;
pub mod retry;
#[cfg(feature = "serde")] pub mod serde_helpers;

#[cfg(feature = "crypto")] pub use crate::crypto::blake2b256;
pub use crate::{
	backoff::Backoff,
	retry::{RetryError, RetryPolicy},
};
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod001_mod002_split_use_blocks.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(
		violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable),
		"{violations:#?}"
	);
	assert!(
		violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable),
		"{violations:#?}"
	);
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-001"), "{edits:#?}");

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"pub mod backoff;\n#[cfg(feature = \"crypto\")] pub mod crypto;\npub mod retry;\n#[cfg(feature = \"serde\")] pub mod serde_helpers;"
		));
}

#[test]
fn mod_group_spacing_fix_keeps_pub_mod_group_compact() {
	let original = r#"
pub mod backoff;

#[cfg(feature = "crypto")] pub mod crypto;

pub mod retry;

#[cfg(feature = "serde")] pub mod serde_helpers;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod_group_compact_spacing.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within module declaration groups."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"pub mod backoff;\n#[cfg(feature = \"crypto\")] pub mod crypto;\npub mod retry;\n#[cfg(feature = \"serde\")] pub mod serde_helpers;"
		));
}

#[test]
fn mod002_fix_inserts_blank_line_between_pub_and_pub_crate_mod_batches() {
	let original = r#"
pub mod backoff;
pub(crate) mod crypto;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_visibility_batch_spacing_mod.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-MOD-002"
			&& v.message
				== "Insert exactly one blank line between visibility batches within the same item kind."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("pub mod backoff;\n\npub(crate) mod crypto;"));
}

#[test]
fn mod002_keeps_existing_blank_line_between_pub_and_pub_crate_mod_batches() {
	let original = r#"
pub mod backoff;

pub(crate) mod crypto;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_visibility_batch_spacing_mod_keep.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within module declaration groups."
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn mod002_fix_reorders_pub_items_across_interleaved_segments() {
	let original = r#"
fn internal_a() -> usize {
	1
}

const LIMIT: usize = 3;

pub fn external() -> usize {
	2
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_interleaved.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten.find("pub fn external").unwrap_or_default()
			< rewritten.find("fn internal_a").unwrap_or_default()
	);
}

#[test]
fn mod001_fix_keeps_adjacent_top_level_const_group_compact() {
	let original = r#"
const CONTENT_CHAR_LIMIT: usize = 500;

fn helper() -> usize {
	1
}

const INSIGHTS_PER_FEED_LIMIT: usize = 3;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod001_const_compact.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"const CONTENT_CHAR_LIMIT: usize = 500;\nconst INSIGHTS_PER_FEED_LIMIT: usize = 3;\n\nfn helper() -> usize {"
		));
}

#[test]
fn mod001_fix_keeps_blank_line_between_const_and_static_groups() {
	let original = r#"
static RE_HEADING: usize = 1;
const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;
const SANITIZE_SHRINK_LIMIT: f32 = 0.6;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod001_const_static_grouping.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;\nconst SANITIZE_SHRINK_LIMIT: f32 = 0.6;\n\nstatic RE_HEADING: usize = 1;"
		));
}

#[test]
fn mod002_fix_separates_pub_and_non_pub_const_groups() {
	let original = r#"
const INTERNAL_LIMIT: usize = 3;

pub const PUBLIC_LIMIT: usize = 5;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_const_compact.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten
			.contains("pub const PUBLIC_LIMIT: usize = 5;\n\nconst INTERNAL_LIMIT: usize = 3;")
	);
}

#[test]
fn mod002_fix_inserts_blank_line_between_pub_and_pub_crate_const_batches_without_reorder() {
	let original = r#"
pub const PUBLIC_LIMIT: usize = 5;
pub(crate) const CRATE_LIMIT: usize = 3;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_visibility_batch_spacing_const.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-MOD-002"
			&& v.message
				== "Insert exactly one blank line between visibility batches within the same item kind."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
		"pub const PUBLIC_LIMIT: usize = 5;\n\npub(crate) const CRATE_LIMIT: usize = 3;"
	));
}

#[test]
fn mod002_keeps_existing_blank_line_between_pub_and_pub_crate_const_batches() {
	let original = r#"
pub const PUBLIC_LIMIT: usize = 5;

pub(crate) const CRATE_LIMIT: usize = 3;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod002_visibility_batch_spacing_const_keep.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within constant declaration groups."
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn mod004_reports_outer_doc_comment_on_inline_module() {
	let text = r#"
/// API helpers.
mod api {
	fn run() {}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod004_outer_doc_inline_module.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-004"
				&& v.message
					== "Do not write module docs as outer comments on `mod`; place module docs inside the module with `//!`."
				&& !v.fixable
		}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
}

#[test]
fn mod004_reports_outer_doc_comment_on_external_module_declaration() {
	let text = r#"
/// API helpers.
mod api;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod004_outer_doc_external_module.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004" && !v.fixable));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
}

#[test]
fn mod004_allows_inner_doc_comment_inside_module_body() {
	let text = r#"
mod api {
	//! API helpers.
	fn run() {}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod004_inner_doc_module.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
}

#[test]
fn mod004_allows_doc_control_attribute_on_module() {
	let text = r#"
#[doc(hidden)]
mod api {
	fn run() {}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("mod004_doc_hidden_module.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
}

#[test]
fn module_sort_keeps_attached_impls_with_types() {
	let body = r"#[derive(Clone)]
struct Private;
impl Private { fn value() {} }
impl Default for Private { fn default() -> Self { Self } }

/// Public fixture type.
pub struct Public;
impl Public { pub fn value() {} }

pub enum Choice { One }
impl Choice { pub fn value() -> Self { Self::One } }
";

	for original in [body.to_owned(), format!("mod nested {{\n{body}}}\n")] {
		let path = Path::new("attached_impls.rs");
		let ctx = shared::read_file_context_from_text(path, original.clone())
			.expect("Parse input.")
			.expect("Input context.");
		let mut violations = Vec::new();
		let mut edits = Vec::new();

		module::check_module_order(&ctx, &mut violations, &mut edits, true);

		let mut sorted = original.clone();

		fixes::apply_edits(&mut sorted, edits).expect("Sort module items.");

		assert!(sorted.contains("pub struct Public;\nimpl Public"), "{sorted}");

		let (rewritten, _, _, _) =
			engine::apply_fix_passes(path, &original, true).expect("Apply fixes.");
		let ctx = shared::read_file_context_from_text(path, rewritten.clone())
			.expect("Parse fixture.")
			.expect("Fixture context.");
		let (violations, _) = rules::collect_violations(&ctx, false);

		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-MOD-002" | "RUST-STYLE-MOD-005")),
			"{rewritten}\n{violations:?}"
		);
		assert!(rewritten.contains("#[derive(Clone)]\nstruct Private;"), "{rewritten}");
		assert!(rewritten.contains("/// Public fixture type.\npub struct Public;"), "{rewritten}");

		let (repeated, _, _, _) =
			engine::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);
	}
}

#[test]
fn module_layout_preserves_import_group_separators() {
	let original = "struct Thing;\n\n#[cfg(test)]\nmod tests {\n    use std::fmt::{Debug, Display};\n    use std::path::Path;\n\n    use crate::Thing;\n\n    fn accepts<T: Debug + Display>(_: &Path, _: &Thing) {}\n}\n";
	let path = Path::new("src/lib.rs");
	let ctx = shared::read_file_context_from_text(path, original.into())
		.expect("Parse.")
		.expect("Context.");
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	module::check_module_order(&ctx, &mut violations, &mut edits, true);

	assert!(edits.is_empty(), "{edits:?}");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(path, original, true).expect("Apply fixes.");
	let (repeated, count, _, _) =
		engine::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

	assert_eq!(rewritten, repeated);
	assert_eq!(count, 0, "Import grouping and module layout must not oscillate.");
}
