use std::fs;
use std::path::Path;
use std::process::Command;
use toml_edit::DocumentMut;
use toml_edit::Item;

/// Release notes: one line per commit subject since `previous_tag`.
///
/// Merge commits are left out. Their subjects are GitHub's defaults, which name the
/// source repository and branch rather than describing a change, and the commits they
/// merge are already listed.
fn get_commit_history(
	repo: &Path,
	previous_tag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
	let output = Command::new("git")
		.current_dir(repo)
		.args([
			"log",
			"--no-merges",
			"--pretty=format:- %s",
			&format!("{}..HEAD", previous_tag),
		])
		.output()?;

	if !output.status.success() {
		return Err(format!(
			"Failed to read commit history since {}: {}",
			previous_tag,
			String::from_utf8_lossy(&output.stderr).trim()
		)
		.into());
	}

	Ok(String::from_utf8(output.stdout)?)
}

fn get_latest_tag() -> Result<String, Box<dyn std::error::Error>> {
	let output = Command::new("git")
		.args(["describe", "--tags", "--abbrev=0"])
		.output()?;

	Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	// Read current Cargo.toml
	let cargo_content = fs::read_to_string("Cargo.toml")?;
	let mut doc = cargo_content.parse::<DocumentMut>()?;

	// Get current version
	let current_version = doc["package"]["version"]
		.as_str()
		.expect("Could not find version in Cargo.toml");

	// Ask for new version
	println!("Current version is: {}", current_version);
	println!("Enter new version:");
	let mut new_version = String::new();
	std::io::stdin().read_line(&mut new_version)?;
	let new_version = new_version.trim();

	// Update Cargo.toml
	doc["package"]["version"] = Item::from(new_version);
	fs::write("Cargo.toml", doc.to_string())?;

	// Get the latest tag for commit history
	let previous_tag = get_latest_tag()?;
	let commit_history = get_commit_history(Path::new("."), &previous_tag)?;

	// Build in release mode to update Cargo.lock
	let build_status = Command::new("cargo")
		.args(["build", "--release"])
		.status()?;

	if !build_status.success() {
		return Err("Failed to build release version".into());
	}

	// The web example is a crate of its own with its own lockfile, which records this
	// crate's version. Bring it along, or the example's `--locked` build in CI fails as
	// soon as the version changes.
	let example_lock_status = Command::new("cargo")
		.args([
			"update",
			"--manifest-path",
			"examples/web/Cargo.toml",
			"--package",
			"transientdb",
		])
		.status()?;

	if !example_lock_status.success() {
		return Err("Failed to update the web example's lockfile".into());
	}

	// Git commands for version bump with combined commit
	let release_commands = [
		(
			"git add Cargo.toml Cargo.lock examples/web/Cargo.lock",
			"Failed to stage files",
		),
		(
			&format!("git commit -m \"Release version {}\"", new_version),
			"Failed to commit version bump",
		),
		("git push", "Failed to push commits"),
		(
			&format!("git tag -a v{} -m \"Version {}\"", new_version, new_version),
			"Failed to create tag",
		),
		("git push --tags", "Failed to push tags"),
	];

	// Execute release commands
	for (cmd, error_msg) in release_commands.iter() {
		let status = Command::new("sh").arg("-c").arg(cmd).status()?;

		if !status.success() {
			return Err(error_msg.to_string().into());
		}
	}

	// Publish to crates.io
	let publish_status = Command::new("cargo").arg("publish").status()?;

	if !publish_status.success() {
		return Err("Failed to publish to crates.io".into());
	}

	// Create GitHub release
	let create_release = Command::new("gh")
		.args([
			"release",
			"create",
			&format!("v{}", new_version),
			"--title",
			&format!("v{}", new_version),
			"--notes",
			&commit_history,
		])
		.status()?;

	if !create_release.success() {
		return Err("Failed to create GitHub release".into());
	}

	println!("Successfully released version {}", new_version);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::get_commit_history;
	use std::path::{Path, PathBuf};
	use std::process::Command;

	/// A throwaway git repository, removed when dropped.
	struct Repo(PathBuf);

	impl Repo {
		fn new(name: &str) -> Repo {
			let dir = std::env::temp_dir().join(format!(
				"transientdb-release-{}-{}",
				name,
				std::process::id()
			));
			let _ = std::fs::remove_dir_all(&dir);
			std::fs::create_dir_all(&dir).unwrap();
			let repo = Repo(dir);
			repo.git(&["init", "-q", "-b", "main"]);
			repo
		}

		fn path(&self) -> &Path {
			&self.0
		}

		/// Runs git with an isolated identity and no user or system config.
		fn git(&self, args: &[&str]) {
			let status = Command::new("git")
				.current_dir(&self.0)
				.env("GIT_CONFIG_GLOBAL", "/dev/null")
				.env("GIT_CONFIG_NOSYSTEM", "1")
				.args([
					"-c",
					"user.name=Test",
					"-c",
					"user.email=test@example.com",
					"-c",
					"commit.gpgsign=false",
					"-c",
					"tag.gpgsign=false",
				])
				.args(args)
				.status()
				.unwrap();
			assert!(status.success(), "git {:?} failed", args);
		}

		fn commit(&self, subject: &str) {
			self.git(&["commit", "-q", "--allow-empty", "-m", subject]);
		}
	}

	impl Drop for Repo {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	#[test]
	fn lists_commits_since_the_tag_newest_first() {
		let repo = Repo::new("linear");
		repo.commit("Before the tag");
		repo.git(&["tag", "-a", "v1.0.0", "-m", "Version 1.0.0"]);
		repo.commit("First change");
		repo.commit("Second change");

		let notes = get_commit_history(repo.path(), "v1.0.0").unwrap();

		assert_eq!(notes, "- Second change\n- First change");
	}

	#[test]
	fn leaves_out_merge_commits_but_keeps_what_they_merged() {
		let repo = Repo::new("merge");
		repo.commit("Before the tag");
		repo.git(&["tag", "-a", "v1.0.0", "-m", "Version 1.0.0"]);
		repo.git(&["checkout", "-q", "-b", "feature"]);
		repo.commit("Change on a branch");
		repo.git(&["checkout", "-q", "main"]);
		repo.commit("Change on main");
		repo.git(&[
			"merge",
			"-q",
			"--no-ff",
			"-m",
			"Merge pull request #1 from some-org/some-branch",
			"feature",
		]);

		let notes = get_commit_history(repo.path(), "v1.0.0").unwrap();

		assert!(!notes.contains("Merge pull request"), "notes: {notes}");
		assert!(notes.contains("- Change on a branch"), "notes: {notes}");
		assert!(notes.contains("- Change on main"), "notes: {notes}");
		assert_eq!(notes.lines().count(), 2, "notes: {notes}");
	}

	#[test]
	fn nothing_since_the_tag_gives_empty_notes() {
		let repo = Repo::new("empty");
		repo.commit("Only commit");
		repo.git(&["tag", "-a", "v1.0.0", "-m", "Version 1.0.0"]);

		let notes = get_commit_history(repo.path(), "v1.0.0").unwrap();

		assert_eq!(notes, "");
	}

	#[test]
	fn an_unknown_tag_is_an_error_not_empty_notes() {
		let repo = Repo::new("badtag");
		repo.commit("Only commit");

		let err = get_commit_history(repo.path(), "v9.9.9").unwrap_err();

		assert!(err.to_string().contains("v9.9.9"), "error: {err}");
	}
}
