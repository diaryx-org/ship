//! Cutting a release: bump, regenerate, commit, tag — and push only when asked.
//!
//! That asymmetry is the whole safety model. Every step before the push is a
//! local commit that can be amended or thrown away, and the push is the one
//! that spends a version number for good. So the push is asked for each time,
//! and the default run prints the commands it did not run.

use std::cmp::Ordering;

use crate::changelog::{self, Cliff};
use crate::config::Config;
use crate::line::Line;
use crate::sh::Sh;
use crate::version::{self, Version};
use crate::{Result, bail, cargo, ci};

const CLIFF_HINT: &str = "nix profile install nixpkgs#git-cliff, or cargo install git-cliff";
const GH_HINT: &str = "brew install gh, then `gh auth login`";

/// Is this exact version already on crates.io? None where the registry could
/// not be asked — which is reported as such, not taken as a no. A yanked
/// version answers 200 too, correctly: a number can never be reused.
fn already_published(sh: &Sh, name: &str, version: &str) -> Option<bool> {
    let code = sh.try_capture(
        "curl",
        &[
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            "--user-agent",
            "ship (https://github.com/diaryx-org/ship)",
            &format!("https://crates.io/api/v1/crates/{name}/{version}"),
        ],
    );
    match code.trim() {
        "" | "000" => None,
        "200" => Some(true),
        _ => Some(false),
    }
}

/// Whether the repository has an origin, and whether it answers. Neither is a
/// reason to refuse a local commit and tag, but both are a reason to say which
/// checks did not happen.
fn origin_state(sh: &Sh) -> &'static str {
    if sh
        .try_capture("git", &["remote", "get-url", "origin"])
        .trim()
        .is_empty()
    {
        return "not configured";
    }
    if sh
        .try_capture("git", &["ls-remote", "--exit-code", "origin", "HEAD"])
        .trim()
        .is_empty()
    {
        return "unreachable";
    }
    "reachable"
}

/// Refuse a release that is already doomed: nothing in it where that is not
/// allowed, no gh to finish a push, a dirty tree, the wrong branch, a tag or a
/// crates.io version already spent, a branch behind its origin.
#[allow(clippy::too_many_arguments)]
fn preflight(
    sh: &Sh,
    config: &Config,
    line: &Line,
    cliff: &Cliff,
    name: &str,
    next: &Version,
    tag: &str,
    push: bool,
) -> Result<()> {
    sh.require("git-cliff", CLIFF_HINT)?;
    if !config.allow_empty_release && changelog::nothing_unreleased(sh, cliff, line)? {
        bail!(
            "nothing to release: no commits since the last tag\nhint: {tag} would name the same tree as the tag before it"
        );
    }
    // A push writes the GitHub release too, so its tool is checked before
    // anything is committed rather than after the tag has left.
    if push {
        sh.require("gh", GH_HINT)?;
        if !sh.ok("gh", &["auth", "status"]) {
            bail!("gh is installed but not logged in\nhint: gh auth login");
        }
    }
    if !sh
        .capture("git", &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        bail!(
            "the working tree is dirty — commit or stash first, so the release commit holds only the version bump and the changelog"
        );
    }
    let branch = sh.capture("git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.trim() != config.release_branch {
        bail!(
            "on branch `{}`, and {name} releases from `{}`",
            branch.trim(),
            config.release_branch
        );
    }
    if !sh
        .capture("git", &["tag", "--list", tag])?
        .trim()
        .is_empty()
    {
        bail!("tag {tag} already exists locally");
    }
    let origin = origin_state(sh);
    if origin == "reachable" {
        if !sh
            .capture("git", &["ls-remote", "--tags", "origin", tag])?
            .trim()
            .is_empty()
        {
            bail!("tag {tag} already exists on origin");
        }
    } else {
        println!("warning: origin is {origin}; {tag} was checked against the local tags only");
    }
    // A crate can go up from a laptop, untagged, so ask the registry rather
    // than the tag list — about this line's crates, at this line's version.
    let mut unasked = 0;
    let publishable = cargo::publishable(sh, config)?
        .into_iter()
        .filter(|c| line.whole || line.crates.contains(c));
    for crate_name in publishable {
        match already_published(sh, &crate_name, &next.to_string()) {
            None => unasked += 1,
            Some(true) => bail!(
                "{crate_name} {next} is already on crates.io\nhint: a published version number can never be reused; release {} instead",
                version::next_patch(next)
            ),
            Some(false) => {}
        }
    }
    if unasked > 0 {
        println!(
            "warning: could not reach crates.io; {unasked} crate(s) were not checked for {next}"
        );
    }
    if origin == "reachable" {
        sh.capture(
            "git",
            &["fetch", "--quiet", "origin", &config.release_branch],
        )?;
        let behind = sh.capture(
            "git",
            &[
                "rev-list",
                "--count",
                &format!("HEAD..origin/{}", config.release_branch),
            ],
        )?;
        if behind.trim() != "0" {
            bail!(
                "{0} is {1} commits behind origin/{0} — pull first",
                config.release_branch,
                behind.trim()
            );
        }
    } else {
        println!(
            "warning: releasing against the local {}; it was not compared with origin",
            config.release_branch
        );
    }
    Ok(())
}

/// Whatever this repository calls its CI: `verify` where it names a command,
/// the `job` list where it does not, and `cargo xtask ci` where there is
/// neither.
fn verify(sh: &Sh, config: &Config, skip: bool) -> Result<()> {
    if skip {
        println!("skipping verification (--no-verify)");
        return Ok(());
    }
    match &config.verify {
        None if !config.jobs.is_empty() => ci::run_all(sh, config, false),
        command => {
            sh.step("Verify");
            sh.shell(command.as_deref().unwrap_or("cargo xtask ci"))
        }
    }
}

/// Run a step that writes to the tree, and put the tree back if it fails.
/// Exact, because preflight proved the tree clean: nothing a `checkout -- .`
/// could throw away was not written by this release.
fn restoring(sh: &Sh, step: impl FnOnce() -> Result<()>) -> Result<()> {
    step().map_err(|e| match sh.call("git", &["checkout", "--", "."]) {
        Ok(()) => format!("{e}\n\nthe bump was undone; the tree is as it was"),
        Err(undo) => format!("{e}\n\nand undoing the bump failed too: {undo}"),
    })
}

pub fn release(
    sh: &Sh,
    config: &Config,
    line: &Line,
    spec: &str,
    push: bool,
    no_verify: bool,
) -> Result<()> {
    let (repository, url) = config.release_identity()?;
    let name = if line.whole { repository } else { &line.name };
    let current = version::read(sh, config, line)?;
    // The floor is the highest tag, not the manifest: the two come apart when a
    // bump is committed and left unreleased, which `as-is` then tags.
    let next = version::bump(&current, spec, Some(&version::floor(sh, line, &current)?))?;
    let tag = line.tag(&next);
    let moving = version::compare(&next, &current) != Ordering::Equal;

    let cliff = Cliff::new(sh, config)?;
    preflight(sh, config, line, &cliff, name, &next, &tag, push)?;

    // Verification comes before anything is written, except where it judges
    // the versions being written: twig's `zig build check` compares the zon
    // against the Rust manifests, so there the bump goes first.
    if !config.verify_after_bump {
        verify(sh, config, no_verify)?;
    }
    if moving {
        sh.step(&format!("{current} -> {next}"));
        restoring(sh, || version::write(sh, config, line, &next))?;
    } else {
        sh.step(&format!("{next}, as is"));
        println!("the manifests already hold this version; nothing to bump");
    }
    if config.verify_after_bump {
        restoring(sh, || verify(sh, config, no_verify))?;
    }
    restoring(sh, || changelog::cut(sh, config, line, &next))?;

    // Only the files a release moves, by name: whatever else is in the tree
    // stays out of the release commit.
    let mut staged = version::version_files(sh, config)?;
    staged.push(line.changelog.clone());
    let mut add = vec!["add"];
    add.extend(staged.iter().map(String::as_str));
    sh.call("git", &add)?;
    // `as-is` moves no version, and a commit claiming it did would read as a
    // bump nobody can find in the diff.
    // Per crate, the subject names it: the log is shared by every line.
    let what = if line.whole {
        next.to_string()
    } else {
        format!("{} {next}", line.name)
    };
    let subject = if moving {
        format!("chore: bump to {what}")
    } else {
        format!("chore: release {what}")
    };
    sh.call("git", &["commit", "-m", &subject])?;
    // Annotated: a release workflow reads `github.ref_name`, and `git
    // describe` wants an object to read.
    sh.call("git", &["tag", "-a", &tag, "-m", &tag])?;

    let branch = sh
        .capture("git", &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    // Which ref goes up first matters nowhere but where something builds the
    // tag and will not build one commit twice; see `push_tag_first`.
    let refs = if config.push_tag_first {
        [tag.clone(), branch]
    } else {
        [branch, tag.clone()]
    };
    let order_note = if config.push_tag_first {
        "\nThat order matters: the tag has to carry a commit nothing has seen yet.\n"
    } else {
        ""
    };

    if !push {
        println!(
            "\n{}\n\nNothing has left this machine. To release:\n\n    git push origin {}\n    git push origin {}\n    ship github {tag}\n{order_note}\n{}\n\nTo undo locally instead: git tag -d {tag} && git reset --hard HEAD~1\n",
            sh.green(&format!("{tag} is committed and tagged locally.")),
            refs[0],
            refs[1],
            config.push_warning()
        );
        return Ok(());
    }

    for r in &refs {
        sh.call("git", &["push", "origin", r])?;
    }
    println!(
        "\n{} Watch it at {url}/actions",
        sh.green(&format!("{tag} pushed."))
    );

    // The one step after the tag has left, so a failure here is something left
    // to finish rather than a release that went wrong.
    sh.step("GitHub release");
    if let Err(e) = github(sh, config, line, Some(&tag)) {
        println!(
            "\n{}\n\n{e}\n\nNothing is half-done — the tag and the commit are as they should be. To finish:\n\n    ship github {tag}\n",
            sh.red(&format!(
                "{tag} is pushed; the GitHub release was not written."
            ))
        );
    }
    Ok(())
}

/// The GitHub release for a tag: created, or its body written where
/// something else has already made it — a binary-attaching workflow can get
/// there first. Everything is read from the tag's tree, so this is as right
/// for a release cut months ago as for one cut a second ago.
pub fn github(sh: &Sh, config: &Config, line: &Line, tag: Option<&str>) -> Result<()> {
    let tag = match tag {
        Some(t) => t.to_string(),
        None => line.tag(&version::read(sh, config, line)?),
    };
    sh.require("gh", GH_HINT)?;
    if !sh.ok(
        "git",
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{tag}^{{commit}}"),
        ],
    ) {
        bail!("{tag} is not a tag in this repository\nhint: git fetch --tags origin");
    }
    // The tag must name the version it was cut from. Structural in a release
    // ship cut; not in one named by hand months later.
    if let Some(named) = version::read_at(sh, config, line, &tag)?
        && line.tag(&named) != tag
    {
        bail!(
            "{tag} does not name version {named}, which is what {} says at that tag",
            line.manifest
        );
    }
    let notes = changelog::notes_text(sh, config, line, Some(&tag))?;
    // A pre-release tag cuts a pre-release — a version to try, not one GitHub
    // should show as Latest. Stated either way, so an edit corrects a release
    // something else created without knowing.
    let prerelease = match line.version_of(&tag) {
        Some(v) => v.pre.is_some(),
        None => version::parse(tag.strip_prefix('v').unwrap_or(&tag))?
            .pre
            .is_some(),
    };
    let pre = format!("--prerelease={prerelease}");
    if sh.ok("gh", &["release", "view", &tag, "--json", "tagName"]) {
        sh.call_with_stdin(
            "gh",
            &["release", "edit", &tag, "--notes-file", "-", &pre],
            &notes,
        )?;
    } else {
        sh.call_with_stdin(
            "gh",
            &[
                "release",
                "create",
                &tag,
                "--title",
                &tag,
                "--notes-file",
                "-",
                "--verify-tag",
                &pre,
            ],
            &notes,
        )?;
    }
    let url = sh.capture(
        "gh",
        &["release", "view", &tag, "--json", "url", "--jq", ".url"],
    )?;
    println!("\n{} {}", sh.green(&format!("{tag} released.")), url.trim());
    Ok(())
}
