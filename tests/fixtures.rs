//! ship, run as a person runs it, on throwaway repositories: a workspace whose
//! crates release on their own, and one at a single version that has to behave
//! exactly as it did before there was a choice.
//!
//! Each needs git, cargo and git-cliff. Without git-cliff a test says so and
//! passes: the unit tests still run, and CI installs it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn have_git_cliff() -> bool {
    let found = Command::new("git-cliff").arg("--version").output().is_ok();
    if !found {
        eprintln!("git-cliff is not on PATH; skipping");
    }
    found
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "{name}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let f = Fixture { root };
        f.git(&["init", "--quiet", "--initial-branch=main"]);
        for (k, v) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.com"),
            ("commit.gpgSign", "false"),
            ("tag.gpgSign", "false"),
        ] {
            f.git(&["config", k, v]);
        }
        f
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap()
    }

    /// Commands see this repository's config alone: a hook or a signing
    /// key in the person's own would make a test about their machine.
    fn command(&self, prog: &str) -> Command {
        let mut c = Command::new(prog);
        c.current_dir(&self.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_COLOR", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
        c
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.command("git").args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn commit(&self, subject: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", subject]);
    }

    fn ship(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_ship"))
            .args(args)
            .output()
            .unwrap()
    }

    /// ship's stdout, where it succeeds.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.ship(args);
        assert!(
            out.status.success(),
            "ship {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// ship's stderr, where it fails.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.ship(args);
        assert!(
            !out.status.success(),
            "ship {args:?} succeeded:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn lockfile(&self) {
        let out = self
            .command("cargo")
            .args(["generate-lockfile", "--offline", "--quiet"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn tags(&self) -> Vec<String> {
        self.git(&["tag", "--list"])
            .lines()
            .map(str::to_string)
            .collect()
    }
}

const CHANGELOG: &str = "# Changelog\n\n## Unreleased\n\n<!-- git-cliff:begin — generated; edits here are overwritten -->\n\n_No commits since the last tag._\n\n<!-- git-cliff:end -->\n";

/// A version line under `[package]` reads `version = "…"` and nothing else.
fn package_version(manifest: &str) -> &str {
    let at = manifest.find("[package]").unwrap();
    let line = manifest[at..]
        .lines()
        .find(|l| l.starts_with("version = \""))
        .unwrap();
    line.split('"').nth(1).unwrap()
}

/// Two crates at their own versions, `beta` depending on `alpha` by a pin in
/// its own manifest, and the root carrying a pin to `beta` in
/// `[workspace.dependencies]`.
fn two_crates() -> Fixture {
    let f = Fixture::new("per-crate");
    f.write(".gitignore", "target/\n");
    f.write(
        ".config/ship.figl",
        "name = fixture\nurl = https://github.com/diaryx-org/fixture\nper_crate = true\npublishes = false\nverify = exit 0\n",
    );
    f.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"2\"\n\n[workspace.dependencies]\nbeta = { path = \"beta\", version = \"0.3.0\" }\n",
    );
    f.write(
        "alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    f.write("alpha/src/lib.rs", "pub fn one() {}\n");
    f.write("alpha/CHANGELOG.md", CHANGELOG);
    f.write(
        "beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.3.0\"\nedition = \"2021\"\n\n[dependencies]\nalpha = { path = \"../alpha\", version = \"0.1.0\" }\n",
    );
    f.write("beta/src/lib.rs", "pub fn two() {}\n");
    f.write("beta/CHANGELOG.md", CHANGELOG);
    f.lockfile();
    f.commit("feat(alpha): the first alpha");
    f.git(&["tag", "-a", "alpha-v0.1.0", "-m", "alpha-v0.1.0"]);

    f.write("beta/src/lib.rs", "pub fn two() {}\npub fn three() {}\n");
    f.commit("fix(beta): a beta fix");
    f.write("alpha/src/lib.rs", "pub fn one() {}\npub fn four() {}\n");
    f.commit("feat(alpha): a second alpha");
    f
}

#[test]
fn each_crate_releases_on_its_own() {
    if !have_git_cliff() {
        return;
    }
    let f = two_crates();

    assert_eq!(f.ok(&["version"]), "alpha 0.1.0\nbeta 0.3.0\n");
    assert_eq!(f.ok(&["version", "-p", "beta"]), "0.3.0\n");

    // With no crate named, which crates have something to release.
    let summary = f.ok(&[]);
    assert!(
        summary.contains("1 commit(s) since alpha-v0.1.0"),
        "{summary}"
    );
    assert!(summary.contains("never released"), "{summary}");
    assert!(
        summary.contains("Unreleased changes in alpha, beta"),
        "{summary}"
    );

    let proposal = f.ok(&["-p", "alpha"]);
    assert!(
        proposal.contains("alpha is at 0.1.0; alpha-v0.1.0 is the highest tag"),
        "{proposal}"
    );
    assert!(
        proposal.contains("1 commits since alpha-v0.1.0 — 1 feat"),
        "{proposal}"
    );
    assert!(proposal.contains("ship -p alpha patch"), "{proposal}");

    // A release needs a crate named, and one that exists.
    assert!(f.fails(&["patch"]).contains("name one with -p"));
    assert!(f.fails(&["-p", "gamma", "patch"]).contains("alpha, beta"));

    let printed = f.ok(&["changelog", "-p", "alpha"]);
    assert!(
        printed.contains("a second alpha") && !printed.contains("beta fix"),
        "{printed}"
    );

    // alpha-v0.1.0 has no section yet: the backfill gives it one, from the
    // first commit to its tag, and alpha's commits alone.
    f.ok(&["changelog", "--write", "-p", "alpha"]);
    let backfilled = f.read("alpha/CHANGELOG.md");
    assert!(
        backfilled.contains("## v0.1.0 — ") && backfilled.contains("the first alpha"),
        "{backfilled}"
    );
    f.commit("docs: alpha's changelog");

    f.ok(&["-p", "alpha", "minor", "--no-verify"]);

    assert_eq!(f.tags(), ["alpha-v0.1.0", "alpha-v0.2.0"]);
    assert_eq!(package_version(&f.read("alpha/Cargo.toml")), "0.2.0");
    // The pin to alpha moved, and nothing about beta did.
    let beta = f.read("beta/Cargo.toml");
    assert!(
        beta.contains("alpha = { path = \"../alpha\", version = \"0.2.0\" }"),
        "{beta}"
    );
    assert_eq!(package_version(&beta), "0.3.0");
    assert!(f.read("Cargo.toml").contains("version = \"0.3.0\""));
    assert_eq!(f.read("beta/CHANGELOG.md"), CHANGELOG);
    assert!(
        f.read("Cargo.lock")
            .contains("name = \"alpha\"\nversion = \"0.2.0\"")
    );

    let alpha = f.read("alpha/CHANGELOG.md");
    assert!(alpha.contains("## v0.2.0 — "), "{alpha}");
    assert!(
        alpha.contains("a second alpha") && !alpha.contains("beta fix"),
        "{alpha}"
    );
    assert!(alpha.find("## v0.2.0").unwrap() < alpha.find("## v0.1.0").unwrap());
    assert_eq!(
        f.git(&["log", "-1", "--format=%s"]).trim(),
        "chore: bump to alpha 0.2.0"
    );
    assert!(f.git(&["status", "--porcelain"]).trim().is_empty());

    // The tag says whose it is.
    let notes = f.ok(&["notes", "alpha-v0.2.0"]);
    assert!(notes.contains("a second alpha"), "{notes}");
    assert!(
        notes.contains("/blob/alpha-v0.2.0/alpha/CHANGELOG.md"),
        "{notes}"
    );

    // alpha is released and beta is not: the check names beta alone.
    let stale = f.fails(&["changelog", "--check"]);
    assert!(
        stale.contains("beta") && !stale.contains("alpha:"),
        "{stale}"
    );

    // beta, never tagged: everything that touched it, and a first tag.
    f.ok(&["changelog", "--write", "-p", "beta"]);
    f.commit("docs: beta's changelog");
    f.ok(&["-p", "beta", "patch", "--no-verify"]);
    assert!(f.tags().contains(&"beta-v0.3.1".to_string()));
    let beta = f.read("beta/CHANGELOG.md");
    assert!(
        beta.contains("## v0.3.1") && beta.contains("a beta fix"),
        "{beta}"
    );
    assert!(!beta.contains("a second alpha"), "{beta}");
    // The root's `[workspace.dependencies]` pin, which cargo does not list.
    assert!(
        f.read("Cargo.toml")
            .contains("beta = { path = \"beta\", version = \"0.3.1\" }")
    );
    assert_eq!(package_version(&f.read("alpha/Cargo.toml")), "0.2.0");
    assert_eq!(
        f.git(&["log", "-1", "--format=%s"]).trim(),
        "chore: bump to beta 0.3.1"
    );
    f.ok(&["changelog", "--check"]);

    // Nothing waits now.
    assert!(
        f.ok(&[])
            .contains("No crate has commits since its last release")
    );
}

/// The crates that inherit the workspace version are one line, with the
/// plain `v` tags and the repository's changelog, and every commit no other
/// line owns.
#[test]
fn the_workspace_version_is_a_line_of_its_own() {
    if !have_git_cliff() {
        return;
    }
    let f = Fixture::new("per-crate-workspace");
    f.write(".gitignore", "target/\n");
    f.write(
        ".config/ship.figl",
        "name = fixture\nurl = https://github.com/diaryx-org/fixture\nper_crate = true\npublishes = false\nverify = exit 0\n",
    );
    f.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"plugin\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"1.0.0\"\n\n[package]\nname = \"fixture\"\nversion.workspace = true\nedition = \"2021\"\n",
    );
    f.write("src/lib.rs", "pub fn core() {}\n");
    f.write("docs/CHANGELOG.md", CHANGELOG);
    f.write(
        "plugin/Cargo.toml",
        "[package]\nname = \"fixture-plugin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfixture = { path = \"..\", version = \"1.0.0\" }\n",
    );
    f.write("plugin/src/lib.rs", "pub fn plugin() {}\n");
    f.write("plugin/CHANGELOG.md", CHANGELOG);
    f.lockfile();
    f.commit("feat: the core");
    f.git(&["tag", "-a", "v1.0.0", "-m", "v1.0.0"]);
    f.write(
        "plugin/src/lib.rs",
        "pub fn plugin() {}\npub fn more() {}\n",
    );
    f.commit("feat(plugin): more plugin");
    f.write("src/lib.rs", "pub fn core() {}\npub fn deeper() {}\n");
    f.commit("feat: a deeper core");

    assert_eq!(f.ok(&["version"]), "fixture 1.0.0\nfixture-plugin 0.1.0\n");
    f.ok(&["-p", "fixture", "minor", "--no-verify"]);
    assert!(f.tags().contains(&"v1.1.0".to_string()));
    assert!(
        f.read("Cargo.toml")
            .contains("[workspace.package]\nversion = \"1.1.0\"")
    );
    let plugin = f.read("plugin/Cargo.toml");
    assert!(
        plugin.contains("fixture = { path = \"..\", version = \"1.1.0\" }"),
        "{plugin}"
    );
    assert_eq!(package_version(&plugin), "0.1.0");
    let core = f.read("docs/CHANGELOG.md");
    assert!(
        core.contains("## v1.1.0") && core.contains("a deeper core"),
        "{core}"
    );
    assert!(!core.contains("more plugin"), "{core}");
}

/// A repository at one version, as every repository was: one `v` tag, one
/// changelog, every pin, and `-p` refused rather than ignored.
#[test]
fn a_single_version_repository_is_as_it_was() {
    if !have_git_cliff() {
        return;
    }
    let f = Fixture::new("single-version");
    f.write(".gitignore", "target/\n");
    f.write(
        ".config/ship.figl",
        "name = fixture\nurl = https://github.com/diaryx-org/fixture\npublishes = false\nverify = exit 0\n",
    );
    f.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.4.0\"\nedition = \"2021\"\n",
    );
    f.write(
        "a/Cargo.toml",
        "[package]\nname = \"a\"\nversion.workspace = true\nedition.workspace = true\n",
    );
    f.write("a/src/lib.rs", "pub fn a() {}\n");
    f.write(
        "b/Cargo.toml",
        "[package]\nname = \"b\"\nversion.workspace = true\nedition.workspace = true\n\n[dependencies]\na = { path = \"../a\", version = \"0.4.0\" }\n",
    );
    f.write("b/src/lib.rs", "pub fn b() {}\n");
    f.write("docs/CHANGELOG.md", CHANGELOG);
    f.lockfile();
    f.commit("feat: a and b");
    f.git(&["tag", "-a", "v0.4.0", "-m", "v0.4.0"]);
    f.write("b/src/lib.rs", "pub fn b() {}\npub fn c() {}\n");
    f.commit("fix(b): a fix in b");
    f.write("a/src/lib.rs", "pub fn a() {}\npub fn d() {}\n");
    f.commit("feat(a): more a");

    assert_eq!(f.ok(&["version"]), "0.4.0\n");
    let proposal = f.ok(&[]);
    assert!(
        proposal.contains("fixture is at 0.4.0; v0.4.0 is the highest tag"),
        "{proposal}"
    );
    assert!(
        proposal.contains("2 commits since v0.4.0 — 1 feat, 1 fix"),
        "{proposal}"
    );
    assert!(proposal.contains("    ship patch | minor"), "{proposal}");
    assert!(f.fails(&["-p", "a", "patch"]).contains("one version"));

    f.ok(&["changelog", "--write"]);
    assert!(f.read("docs/CHANGELOG.md").contains("## v0.4.0 — "));
    f.commit("docs: the changelog");

    f.ok(&["patch", "--no-verify"]);
    assert_eq!(f.tags(), ["v0.4.0", "v0.4.1"]);
    assert!(f.read("Cargo.toml").contains("version = \"0.4.1\""));
    assert!(
        f.read("b/Cargo.toml")
            .contains("a = { path = \"../a\", version = \"0.4.1\" }")
    );
    let log = f.read("docs/CHANGELOG.md");
    assert!(log.contains("## v0.4.1 — "), "{log}");
    assert!(
        log.contains("a fix in b") && log.contains("more a"),
        "{log}"
    );
    assert_eq!(
        f.git(&["log", "-1", "--format=%s"]).trim(),
        "chore: bump to 0.4.1"
    );
    let notes = f.ok(&["notes"]);
    assert!(notes.contains("more a"), "{notes}");
    assert!(notes.contains("/blob/v0.4.1/docs/CHANGELOG.md"), "{notes}");
    f.ok(&["changelog", "--check"]);
}
