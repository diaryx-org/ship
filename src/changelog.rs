//! The changelog: one generated region, and the released sections below it.
//!
//! The markers are the contract every repository shares, and the reason a
//! handwritten release intro survives regeneration — only the bytes between
//! them are ever rewritten.

use std::cmp::Ordering;
use std::path::PathBuf;

use crate::config::Config;
use crate::line::Line;
use crate::sh::Sh;
use crate::version::{self, Version};
use crate::{Result, bail};

pub const BEGIN: &str = "<!-- git-cliff:begin — generated; edits here are overwritten -->";
pub const END: &str = "<!-- git-cliff:end -->";
/// What the region says when there is nothing unreleased — the normal state
/// right after a release.
pub const EMPTY_REGION: &str = "_No commits since the last tag._";
const NOTHING_RECORDED: &str = "_Nothing recorded._";
const CLIFF_HINT: &str = "nix profile install nixpkgs#git-cliff, or cargo install git-cliff";

/// The shared git-cliff config: one style for every repository that does not
/// name its own, carried inside ship rather than copied into each.
const SHARED_CLIFF: &str = include_str!("../cliff.toml");

/// git-cliff's config, as a path it can read. The shared one is written to a
/// temporary file for the life of the command and removed after.
pub struct Cliff {
    path: PathBuf,
    temporary: bool,
}

impl Cliff {
    pub fn new(sh: &Sh, config: &Config) -> Result<Cliff> {
        if let Some(own) = &config.cliff_config {
            return Ok(Cliff {
                path: sh.root.join(own),
                temporary: false,
            });
        }
        let path = std::env::temp_dir().join(format!("ship-cliff-{}.toml", std::process::id()));
        std::fs::write(&path, SHARED_CLIFF).map_err(|e| {
            format!(
                "could not write the shared git-cliff config to {}: {e}",
                path.display()
            )
        })?;
        Ok(Cliff {
            path,
            temporary: true,
        })
    }

    fn arg(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for Cliff {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// `## v0.2.0 — 2026-08-18` and `## 3.2.1` alike → `v0.2.0` / `v3.2.1`, and
/// any other heading → none. Repositories disagree on how a heading spells
/// its version, so reading accepts both and always answers with the git tag.
pub fn section_tag(line: &str) -> Option<String> {
    let rest = line.strip_prefix("## ")?;
    let first = rest.split(' ').next().unwrap_or_default();
    let bare = first.strip_prefix('v').unwrap_or(first);
    version::parse(bare).ok()?;
    Some(format!("v{bare}"))
}

/// The heading a new section gets, in this repository's convention.
pub fn section_heading(config: &Config, tag: &str, date: &str) -> String {
    let shown = if config.section_prefix == "v" {
        tag
    } else {
        &tag[1..]
    };
    if config.section_date {
        format!("## {shown} — {date}")
    } else {
        format!("## {shown}")
    }
}

fn tag_order(tag: &str) -> Version {
    version::parse(&tag[1..]).expect("a section tag is a version")
}

fn section_rank(section: &str) -> Version {
    let first = section.lines().next().unwrap_or_default();
    tag_order(&section_tag(first).expect("a backfilled section starts with its heading"))
}

pub fn region(body: &str) -> String {
    [BEGIN, "", body, "", END].join("\n")
}

/// git-cliff's arguments: the config, the line's tags and paths, and these.
fn cliff_args(cliff: &Cliff, line: &Line, pattern: Option<&str>, rest: &[&str]) -> Vec<String> {
    let mut args = vec!["--config".to_string(), cliff.arg()];
    args.extend(line.cliff_args(pattern));
    args.extend(rest.iter().map(|a| a.to_string()));
    args
}

fn run_cliff(sh: &Sh, args: &[String], envs: &[(&str, &str)]) -> String {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    sh.try_capture_env("git-cliff", &args, envs)
}

/// The unreleased commits, rendered by git-cliff. git-cliff exits non-zero
/// when there is nothing unreleased, which right after a tag is normal.
pub fn generated(sh: &Sh, cliff: &Cliff, line: &Line) -> Result<String> {
    sh.require("git-cliff", CLIFF_HINT)?;
    let body = run_cliff(
        sh,
        &cliff_args(cliff, line, None, &["--unreleased", "--strip", "all"]),
        &[],
    );
    let body = body.trim();
    Ok(if body.is_empty() {
        EMPTY_REGION.to_string()
    } else {
        body.to_string()
    })
}

pub fn nothing_unreleased(sh: &Sh, cliff: &Cliff, line: &Line) -> Result<bool> {
    Ok(generated(sh, cliff, line)? == EMPTY_REGION)
}

/// The commits a tag covers. The first tag a changelog covers has no previous
/// tag, and `<root>..<tag>` would drop the root commit; for that one, ask
/// git-cliff for the tag's own section, with the tag pattern pinned to it.
fn tagged(sh: &Sh, cliff: &Cliff, line: &Line, previous: Option<&str>, tag: &str) -> String {
    let body = match previous {
        // The whole repository pins it through the environment, as it always
        // has; a line passes its pattern as a flag, which the pin replaces.
        None if line.whole => {
            let pinned = format!("^{}$", tag.replace('.', "\\."));
            run_cliff(
                sh,
                &cliff_args(cliff, line, None, &["--strip", "all", "--latest"]),
                &[("GIT_CLIFF__GIT__TAG_PATTERN", &pinned)],
            )
        }
        None => {
            let pinned = format!("^{}$", crate::line::escape(tag));
            run_cliff(
                sh,
                &cliff_args(cliff, line, Some(&pinned), &["--strip", "all", "--latest"]),
                &[],
            )
        }
        Some(previous) => run_cliff(
            sh,
            &cliff_args(
                cliff,
                line,
                None,
                &["--strip", "all", &format!("{previous}..{tag}")],
            ),
            &[],
        ),
    };
    let body = body.trim();
    if body.is_empty() {
        NOTHING_RECORDED.to_string()
    } else {
        body.to_string()
    }
}

/// Tags with no section of their own, rendered and dated. A tag can arrive
/// after the release it stood for went out, and without this its commits
/// would leave the file: no longer unreleased, and in no section either.
fn missing_sections(
    sh: &Sh,
    config: &Config,
    line: &Line,
    cliff: &Cliff,
    text: &str,
) -> Result<Vec<String>> {
    let existing: Vec<String> = text.lines().filter_map(section_tag).collect();
    let all: Vec<String> = version::release_tags(sh, line)?
        .into_iter()
        .filter(|t| line.version_of(t).is_some())
        .collect();
    // A changelog is allowed a starting point, and the backfill stops there
    // rather than inventing history nobody wrote a changelog for. It is a `v`
    // tag, so it is the starting point of the line with the plain tags.
    let since = match &config.changelog_since {
        Some(tag) if line.prefix == "v" => {
            Some(version::parse(tag.strip_prefix('v').unwrap_or(tag))?)
        }
        _ => None,
    };
    let mut sections = Vec::new();
    for (i, tag) in all.iter().enumerate() {
        let (Some(short), Some(named)) = (line.short(tag), line.version_of(tag)) else {
            continue;
        };
        if existing.contains(&short) {
            continue;
        }
        if let Some(since) = &since
            && version::compare(&named, since) == Ordering::Less
        {
            continue;
        }
        let previous = if i == 0 {
            None
        } else {
            Some(all[i - 1].as_str())
        };
        let body = tagged(sh, cliff, line, previous, tag);
        let date = sh.capture("git", &["log", "-1", "--format=%cs", tag])?;
        sections.push(format!(
            "{}\n\n{body}\n",
            section_heading(config, &short, date.trim())
        ));
    }
    Ok(sections)
}

/// Fold backfilled sections in newest-first, each above the first section it
/// outranks rather than appended to the end.
pub fn insert_sections(text: &str, sections: &[String]) -> String {
    if sections.is_empty() {
        return text.to_string();
    }
    let mut pending: Vec<&String> = sections.iter().collect();
    pending.sort_by(|a, b| version::compare(&section_rank(b), &section_rank(a)));
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(here) = section_tag(line) {
            let rank = tag_order(&here);
            let (newer, older): (Vec<&String>, Vec<&String>) = pending
                .into_iter()
                .partition(|s| version::compare(&section_rank(s), &rank) == Ordering::Greater);
            for s in newer {
                out.extend(s.lines().map(str::to_string));
                out.push(String::new());
            }
            pending = older;
        }
        out.push(line.to_string());
    }
    for s in pending {
        out.extend(s.lines().map(str::to_string));
        out.push(String::new());
    }
    out.join("\n") + "\n"
}

/// Replace the marked region, and optionally drop a fresh released section in
/// right below it. Everything else is left byte-for-byte alone.
pub fn rewrite_region(
    path: &str,
    text: &str,
    body: &str,
    released: Option<&str>,
) -> Result<String> {
    let all: Vec<&str> = text.lines().collect();
    for marker in [BEGIN, END] {
        if !all.contains(&marker) {
            bail!("marker not found in {path}:\n  {marker}");
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in all {
        if line == BEGIN {
            out.extend(region(body).lines().map(str::to_string));
            skipping = true;
        } else if line == END {
            skipping = false;
            // Released sections are separated by two blank lines, and the
            // source line after END supplies only one of them.
            if let Some(released) = released {
                out.push(String::new());
                out.extend(released.lines().map(str::to_string));
                out.push(String::new());
            }
        } else if !skipping {
            out.push(line.to_string());
        }
    }
    Ok(out.join("\n") + "\n")
}

/// The lines under a release's heading, up to the next `##` — so a
/// handwritten intro comes along with the generated `###` groups.
pub fn section_body(changelog: &str, tag: &str) -> Option<String> {
    let all: Vec<&str> = changelog.lines().collect();
    let at = all
        .iter()
        .position(|l| section_tag(l).as_deref() == Some(tag))?;
    let body: Vec<&str> = all[at + 1..]
        .iter()
        .copied()
        .take_while(|l| !l.starts_with("## "))
        .collect();
    Some(body.join("\n").trim().to_string())
}

pub enum Mode {
    Print,
    Check,
    Write,
}

/// `ship changelog [--write|--check]` — print, splice, or verify.
pub fn check_or_write(sh: &Sh, config: &Config, line: &Line, mode: &Mode) -> Result<()> {
    let cliff = Cliff::new(sh, config)?;
    let path = &line.changelog;
    let body = generated(sh, &cliff, line)?;
    if let Mode::Print = mode {
        println!("{}", region(&body));
        return Ok(());
    }
    let current = sh.read(path).map_err(|e| {
        if line.whole {
            e
        } else {
            format!(
                "{e}\nhint: a crate's changelog starts as `# Changelog`, `## Unreleased`, and the two git-cliff markers on lines of their own:\n  {BEGIN}\n  {END}"
            )
        }
    })?;
    // Two jobs, because a tag can arrive after the commits it names: refresh
    // the region, and give any tag still without a section one.
    let missing = missing_sections(sh, config, line, &cliff, &current)?;
    let named: Vec<String> = missing
        .iter()
        .filter_map(|s| section_tag(s.lines().next()?))
        .collect();
    let spliced = insert_sections(&rewrite_region(path, &current, &body, None)?, &missing);
    if let Mode::Check = mode {
        if !named.is_empty() {
            bail!(
                "{path} has no section for {}\nhint: run `ship changelog --write`",
                named.join(", ")
            );
        }
        if spliced != current {
            bail!("{path}'s generated region is stale\nhint: run `ship changelog --write`");
        }
        println!("{path}'s generated region is up to date");
        return Ok(());
    }
    sh.write(path, &spliced)?;
    for tag in &named {
        println!("{path} -> new section `## {tag}`");
    }
    println!("wrote {path}");
    Ok(())
}

/// Turn the unreleased region into a released section, and reset the region.
/// Between the bump and the commit, so the release commit carries both.
pub fn cut(sh: &Sh, config: &Config, line: &Line, next: &Version) -> Result<()> {
    let cliff = Cliff::new(sh, config)?;
    let path = &line.changelog;
    let body = generated(sh, &cliff, line)?;
    let date = sh.capture("date", &["+%Y-%m-%d"])?;
    let heading = section_heading(config, &next.tag(), date.trim());
    let released = format!("{heading}\n\n{body}\n");
    let text = sh.read(path)?;
    sh.write(
        path,
        &rewrite_region(path, &text, EMPTY_REGION, Some(&released))?,
    )?;
    println!("{path} -> new section `{heading}`");
    Ok(())
}

/// One release's section, without its heading — the body of a GitHub release.
/// Read from the changelog the tag's own tree carries, so the notes on the
/// release page are the ones the repository shipped.
pub fn notes_text(sh: &Sh, config: &Config, line: &Line, tag: Option<&str>) -> Result<String> {
    let (_, url) = config.release_identity()?;
    let tag = match tag {
        Some(t) => t.to_string(),
        None => line.tag(&version::read(sh, config, line)?),
    };
    // What the line's own changelog heads the section with: the tag itself
    // for the whole repository, `v<x.y.z>` in a crate's.
    let Some(heading) = line.short(&tag) else {
        bail!("{tag} is not a tag of {}", line.name);
    };
    let path = &line.changelog;
    // The tag's tree first, then the working one: a section can be written
    // after its tag — that is what the backfill is for — and a tag can predate
    // the changelog entirely.
    let at_tag = sh.try_capture("git", &["show", &format!("{tag}:{path}")]);
    let mut candidates = Vec::new();
    if !at_tag.is_empty() {
        candidates.push(at_tag);
    }
    candidates.push(sh.read(path)?);
    let Some(body) = candidates
        .iter()
        .find_map(|text| section_body(text, &heading))
    else {
        bail!(
            "{path} has no section for {tag}\nhint: `ship changelog --write`, commit, and re-run — a tag whose changelog section is missing is a tag cut without one"
        );
    };
    Ok(format!(
        "{body}\n\n---\n\nEvery change in this release and the ones before it: [{path}]({url}/blob/{tag}/{path})."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_tags() {
        assert_eq!(
            section_tag("## v0.2.0 — 2026-08-18").as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(section_tag("## v0.2.0").as_deref(), Some("v0.2.0"));
        assert_eq!(section_tag("## 3.2.1").as_deref(), Some("v3.2.1"));
        for not in [
            "## Unreleased",
            "## verify the build",
            "### Added",
            "- **tui** — ## v1.0.0 in a bullet",
        ] {
            assert_eq!(section_tag(not), None, "{not}");
        }
    }

    fn sample() -> String {
        format!(
            "# Changelog\n\nprose\n\n## Unreleased\n\n{BEGIN}\n\nold\n\n{END}\n\n## v0.1.0 — 2026-08-07\n\nkept\n"
        )
    }

    #[test]
    fn the_splice_touches_the_region_only() {
        let refreshed = rewrite_region("docs/CHANGELOG.md", &sample(), "new", None).unwrap();
        assert!(refreshed.contains("\nnew\n") && !refreshed.contains("\nold\n"));
        assert!(refreshed.contains("# Changelog\n\nprose"));
        assert!(refreshed.ends_with("## v0.1.0 — 2026-08-07\n\nkept\n"));

        let cut = rewrite_region(
            "docs/CHANGELOG.md",
            &sample(),
            EMPTY_REGION,
            Some("## v0.2.0 — 2026-08-18\n\nnew\n"),
        )
        .unwrap();
        assert!(
            cut.find("## v0.2.0").unwrap() > cut.find(END).unwrap(),
            "released sections go below the region"
        );
        assert!(
            cut.find("## v0.2.0").unwrap() < cut.find("## v0.1.0").unwrap(),
            "newest release first"
        );
        assert!(cut.contains(EMPTY_REGION));
    }

    /// Exact bytes: the spacing around a freshly cut section is part of the
    /// format, and a `contains` assertion cannot see a lost blank line.
    #[test]
    fn a_cut_section_is_spaced_exactly() {
        let text = format!("## Unreleased\n\n{BEGIN}\n\nold\n\n{END}\n\n## v0.1.0 — a\n\nkept\n");
        assert_eq!(
            rewrite_region("c.md", &text, EMPTY_REGION, Some("## v0.2.0 — b\n\nnew\n")).unwrap(),
            format!(
                "## Unreleased\n\n{BEGIN}\n\n{EMPTY_REGION}\n\n{END}\n\n## v0.2.0 — b\n\nnew\n\n\n## v0.1.0 — a\n\nkept\n"
            )
        );
        assert!(rewrite_region("docs/CHANGELOG.md", "# Changelog\n", "new", None).is_err());
    }

    #[test]
    fn backfilled_sections_land_in_version_order() {
        let merged = insert_sections(
            "## v0.6.0 — c\n\nsix\n\n## v0.4.0 — a\n\nfour\n",
            &[
                "## v0.5.0 — b\n\nfive\n".into(),
                "## v0.3.0 — z\n\nthree\n".into(),
            ],
        );
        let tags: Vec<String> = merged.lines().filter_map(section_tag).collect();
        assert_eq!(tags, ["v0.6.0", "v0.5.0", "v0.4.0", "v0.3.0"]);
        for kept in ["six", "four"] {
            assert_eq!(merged.matches(kept).count(), 1);
        }
        assert_eq!(
            insert_sections("## v0.6.0 — c\n\nsix\n", &[]),
            "## v0.6.0 — c\n\nsix\n"
        );
        let fresh = insert_sections(
            "# Changelog\n\n## Unreleased\n",
            &[
                "## v0.1.0 — a\n\none\n".into(),
                "## v0.1.1 — b\n\ntwo\n".into(),
            ],
        );
        let tags: Vec<String> = fresh.lines().filter_map(section_tag).collect();
        assert_eq!(tags, ["v0.1.1", "v0.1.0"]);
    }

    #[test]
    fn a_section_body_ends_at_the_next_release() {
        let changelog = "# Changelog\n\n## Unreleased\n\nnothing\n\n## v0.2.0 — 2026-08-17\n\nAn intro, handwritten.\n\n### Added\n\n- a thing\n\n## v0.1.0 — 2026-08-06\n\n### Added\n\n- an older thing\n";
        let body = section_body(changelog, "v0.2.0").unwrap();
        assert!(
            body.starts_with("An intro, handwritten.")
                && body.contains("### Added")
                && !body.contains("older")
        );
        assert_eq!(
            section_body(changelog, "v0.1.0").unwrap(),
            "### Added\n\n- an older thing"
        );
        assert_eq!(section_body(changelog, "v9.9.9"), None);
    }
}
