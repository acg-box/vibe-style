use std::{
	env, fs,
	ops::Deref,
	path::{Path, PathBuf},
	process,
	sync::atomic::{AtomicU64, Ordering},
	time::{SystemTime, UNIX_EPOCH},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
	root: PathBuf,
}
impl Fixture {
	pub fn new(label: &str) -> Self {
		let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read clock.").as_nanos();
		let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
		let root =
			env::temp_dir().join(format!("vstyle-{label}-{}-{stamp}-{sequence}", process::id()));

		fs::create_dir(&root).expect("Create fixture directory.");

		Self { root }
	}
}

impl Deref for Fixture {
	type Target = Path;

	fn deref(&self) -> &Self::Target {
		&self.root
	}
}

impl AsRef<Path> for Fixture {
	fn as_ref(&self) -> &Path {
		&self.root
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.root);
	}
}
