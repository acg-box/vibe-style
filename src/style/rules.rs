use crate::style::{
	bindings, file, generics, impls, imports, module, quality,
	shared::{Edit, FileContext, Violation},
	spacing, test_modules, types,
};

pub(super) fn collect_violations(
	ctx: &FileContext,
	with_fixes: bool,
) -> (Vec<Violation>, Vec<Edit>) {
	collect_violations_with_import_shortening(ctx, with_fixes, true)
}

pub(super) fn collect_violations_with_import_shortening(
	ctx: &FileContext,
	with_fixes: bool,
	with_import_shortening: bool,
) -> (Vec<Violation>, Vec<Edit>) {
	collect_filtered_violations(ctx, with_fixes, with_import_shortening, true)
}

pub(super) fn collect_filtered_violations(
	ctx: &FileContext,
	with_fixes: bool,
	with_import_shortening: bool,
	with_binding_rewrites: bool,
) -> (Vec<Violation>, Vec<Edit>) {
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	file::check_mod_rs(ctx, &mut violations);

	if with_binding_rewrites {
		file::check_error_rs_no_use(ctx, &mut violations, &mut edits, with_fixes);
		bindings::check_let_mut_reorder(ctx, &mut violations, &mut edits, with_fixes);
		test_modules::check_test_module_super_glob(ctx, &mut edits, with_fixes);
		imports::check_import_rules(
			ctx,
			&mut violations,
			&mut edits,
			with_fixes,
			with_import_shortening,
		);
	}

	generics::check_unnecessary_turbofish(ctx, &mut violations, &mut edits, with_fixes);
	generics::check_turbofish_canonicalization(ctx, &mut violations, &mut edits, with_fixes);
	types::check_type_alias_renames(ctx, &mut violations);
	module::check_module_order(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_adjacency(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_rules(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_inline_trait_bounds(ctx, &mut violations);
	quality::check_logging_quality(ctx, &mut violations);
	quality::check_expect_unwrap(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_numeric_literals(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_function_length(ctx, &mut violations);
	spacing::check_vertical_spacing(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_test_rules(ctx, &mut violations);

	(violations, edits)
}
