//! What one repository needs `ship` to know about it.
//!
//! Read from `.config/ship.figl` at the repository root: figl, fig's authoring
//! dialect, because these repositories comment everything and a config is read
//! by people more often than it is written. One file holds both halves — the
//! release settings at the top level, and the CI jobs as a `job` list.
//!
//! A repository from before ship keeps `.config/release.toml` and
//! `.config/ci.toml`, and ship reads those when there is no `ship.figl`, so a
//! repository moves over when it is ready rather than all at once. `fig get
//! .config/release.toml -o figl` converts one, comments and all.
//!
//! Every setting is something the copies of this program that came before it
//! disagreed on and nothing else did. The defaults are what a Cargo workspace
//! in this org looks like, so most repositories state only their name and URL.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::sh::Sh;
use crate::{Result, bail};

pub const SHIP: &str = ".config/ship.figl";
const RELEASE_TOML: &str = ".config/release.toml";
const CI_TOML: &str = ".config/ci.toml";

/// Everything a file may say, each optional. `deny_unknown_fields` because an
/// unknown key is almost always a known one misspelt or in the wrong place,
/// and the default that then stands is a release cut the wrong way, reported
/// as nothing at all.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Stated {
    name: Option<String>,
    url: Option<String>,
    manifest: Option<String>,
    version_prefix: Option<String>,
    version_quoted: Option<bool>,
    cargo_workspace: Option<bool>,
    mirrors: Option<Vec<String>>,
    post_bump: Option<String>,
    extra_version_files: Option<Vec<String>>,
    changelog: Option<String>,
    section_prefix: Option<String>,
    section_date: Option<bool>,
    changelog_since: Option<String>,
    cliff_config: Option<String>,
    release_branch: Option<String>,
    verify: Option<String>,
    verify_after_bump: Option<bool>,
    publishes: Option<bool>,
    push_tag_first: Option<bool>,
    allow_empty_release: Option<bool>,
    tag_starts: Option<Vec<TagStart>>,
    ci_parallel: Option<bool>,
    job: Option<Vec<Job>>,
}

/// One thing pushing the tag sets off, for the warning printed before the
/// push. `workflow` for a file in `.github/workflows/`, `system` for something
/// that watches the repository from outside it, and `does` for what it does.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TagStart {
    pub workflow: Option<String>,
    pub system: Option<String>,
    pub does: Option<String>,
}

/// One CI job. `id`, `name` and `run` are what it is; `local`, `matrix` and
/// `after` are where and when ship runs it; `runner` is the workflow's alone,
/// passed through to the matrix — see `ci.rs`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub run: String,
    /// Whether `ship ci` with no job named runs it. False for a job too slow
    /// for every local run, which a runner can afford.
    #[serde(default = "yes")]
    pub local: bool,
    /// Whether `--matrix` lists it. False for a job only a developer's
    /// machine can run, which no runner has.
    #[serde(default = "yes")]
    pub matrix: bool,
    /// An earlier job this one starts after, when the jobs run at once. In
    /// order, the list already says so.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub runner: BTreeMap<String, fig::Value>,
}

fn yes() -> bool {
    true
}

/// The settings, every default filled in.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where it was read from, for messages that name it.
    pub source: String,
    pub name: Option<String>,
    pub url: Option<String>,
    /// The file holding the one version line, and the prefix that finds it:
    /// `version = "` for a Cargo workspace, `    .version = "` for a
    /// build.zig.zon, `version: v` for a Diaryx document's frontmatter.
    pub manifest: String,
    pub version_prefix: String,
    /// False where the version runs to the end of the line rather than
    /// sitting in quotes.
    pub version_quoted: bool,
    /// Whether the canonical manifest is a Cargo workspace, whose lockfile
    /// and internal `{ path, version }` pins follow the version.
    pub cargo_workspace: bool,
    /// Cargo workspaces that copy the version rather than decide it.
    pub mirrors: Vec<String>,
    /// Run once every manifest has moved, for the files no manifest parser
    /// would find.
    pub post_bump: Option<String>,
    /// What `post_bump` writes, so the release commit stages it.
    pub extra_version_files: Vec<String>,
    pub changelog: String,
    /// How a released section heads itself: `## v0.9.2 — 2026-08-26` by
    /// default, `## 3.2.1` where `section_prefix` is empty and `section_date`
    /// false. Reading accepts both.
    pub section_prefix: String,
    pub section_date: bool,
    /// The earliest tag the changelog claims to cover.
    pub changelog_since: Option<String>,
    /// git-cliff's config, relative to the root; none means the shared one
    /// ship carries.
    pub cliff_config: Option<String>,
    pub release_branch: String,
    /// What to run before cutting. None means the `job` list where there is
    /// one, and `cargo xtask ci` where there is not.
    pub verify: Option<String>,
    pub verify_after_bump: bool,
    /// Whether anything goes to crates.io, and so whether preflight asks it.
    pub publishes: bool,
    pub push_tag_first: bool,
    pub allow_empty_release: bool,
    pub tag_starts: Vec<TagStart>,
    /// Whether `ship ci` runs its jobs at once, each into a log of its own,
    /// rather than in order with their output streaming.
    pub ci_parallel: bool,
    pub jobs: Vec<Job>,
}

impl Config {
    /// The repository's config: `ship.figl` where it has one, else the two
    /// TOML files from before it, else nothing but defaults.
    pub fn load(sh: &Sh) -> Result<Config> {
        if sh.exists(SHIP) {
            let stated: Stated = fig_figl::from_str(&sh.read(SHIP)?)
                .map_err(|e| unreadable(SHIP, &e.to_string()))?;
            return Config::from_stated(stated, SHIP);
        }
        let mut stated = Stated::default();
        let mut sources = Vec::new();
        if sh.exists(RELEASE_TOML) {
            stated = fig_toml::from_str(&sh.read(RELEASE_TOML)?)
                .map_err(|e| unreadable(RELEASE_TOML, &e.to_string()))?;
            if stated.job.is_some() {
                bail!("{RELEASE_TOML} sets `job`, which belongs in {CI_TOML}");
            }
            sources.push(RELEASE_TOML);
        }
        if sh.exists(CI_TOML) {
            let ci: Stated = fig_toml::from_str(&sh.read(CI_TOML)?)
                .map_err(|e| unreadable(CI_TOML, &e.to_string()))?;
            stated.job = ci.job;
            sources.push(CI_TOML);
        }
        let source = if sources.is_empty() {
            SHIP.to_string()
        } else {
            sources.join(" and ")
        };
        Config::from_stated(stated, &source)
    }

    fn from_stated(s: Stated, source: &str) -> Result<Config> {
        let tag_starts = s.tag_starts.unwrap_or_default();
        for entry in &tag_starts {
            // Both halves, because the warning before the push is a sentence:
            // an entry naming a workflow and saying nothing about it prints a
            // filename at someone about to spend a version number.
            if entry.workflow.is_some() == entry.system.is_some() || entry.does.is_none() {
                bail!(
                    "{source}'s `tag_starts` entry needs `does` and exactly one of `workflow` or `system`\nhint: `workflow` is a file in .github/workflows/; `system` is something that watches this repository from outside it, like Xcode Cloud"
                );
            }
        }
        let jobs = s.job.unwrap_or_default();
        crate::ci::check(&jobs, source)?;
        Ok(Config {
            source: source.to_string(),
            name: s.name,
            url: s.url,
            manifest: s.manifest.unwrap_or_else(|| "Cargo.toml".into()),
            version_prefix: s.version_prefix.unwrap_or_else(|| "version = \"".into()),
            version_quoted: s.version_quoted.unwrap_or(true),
            cargo_workspace: s.cargo_workspace.unwrap_or(true),
            mirrors: s.mirrors.unwrap_or_default(),
            post_bump: s.post_bump,
            extra_version_files: s.extra_version_files.unwrap_or_default(),
            changelog: s.changelog.unwrap_or_else(|| "docs/CHANGELOG.md".into()),
            section_prefix: s.section_prefix.unwrap_or_else(|| "v".into()),
            section_date: s.section_date.unwrap_or(true),
            changelog_since: s.changelog_since,
            cliff_config: s.cliff_config,
            release_branch: s.release_branch.unwrap_or_else(|| "main".into()),
            verify: s.verify,
            verify_after_bump: s.verify_after_bump.unwrap_or(false),
            publishes: s.publishes.unwrap_or(true),
            push_tag_first: s.push_tag_first.unwrap_or(false),
            allow_empty_release: s.allow_empty_release.unwrap_or(true),
            tag_starts,
            ci_parallel: s.ci_parallel.unwrap_or(false),
            jobs,
        })
    }

    /// Every default, and these jobs.
    #[cfg(test)]
    pub fn for_jobs(jobs: Vec<Job>) -> Config {
        let mut c = Config::from_stated(Stated::default(), SHIP).unwrap();
        c.jobs = jobs;
        c
    }

    /// The name and URL, which a release cannot do without and CI can.
    pub fn release_identity(&self) -> Result<(&str, &str)> {
        match (&self.name, &self.url) {
            (Some(name), Some(url)) => Ok((name, url)),
            (None, _) => bail!(
                "{} is missing `name`\nhint: a release needs the repository's name and url",
                self.source
            ),
            (_, None) => bail!(
                "{} is missing `url`\nhint: a release needs the repository's name and url",
                self.source
            ),
        }
    }

    /// The sentence that says what the push costs.
    pub fn push_warning(&self) -> String {
        if self.tag_starts.is_empty() {
            return "Nothing is configured to run on the tag.".into();
        }
        let parts: Vec<String> = self
            .tag_starts
            .iter()
            .map(|w| {
                let does = w.does.as_deref().unwrap_or_default();
                // A workflow is named as the file it is, in backticks, because
                // that is what you would go and read. A system is named as
                // itself: `Xcode Cloud` in backticks would read as a filename.
                match (&w.workflow, &w.system) {
                    (Some(file), _) => format!("`{file}` {does}"),
                    (_, Some(system)) => format!("{system} {does}"),
                    _ => does.to_string(),
                }
            })
            .collect();
        format!(
            "The tag is what publishes: {}. None of it can be undone.",
            parts.join(", and ")
        )
    }
}

fn unreadable(path: &str, why: &str) -> String {
    format!(
        "could not read {path}: {why}\nhint: an unknown field is usually a known one misspelt, or written below a list it was not meant to join"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(figl: &str) -> Result<Config> {
        let stated: Stated = fig_figl::from_str(figl).map_err(|e| e.to_string())?;
        Config::from_stated(stated, SHIP)
    }

    #[test]
    fn everything_unstated_is_a_cargo_workspace_in_this_org() {
        let c = parse("name = alpha\nurl = https://github.com/diaryx-org/alpha\n").unwrap();
        assert!(c.version_quoted && c.publishes && c.allow_empty_release);
        assert!(!c.push_tag_first && !c.verify_after_bump && !c.ci_parallel);
        assert_eq!(c.post_bump, None);
        assert_eq!(c.verify, None);
        assert_eq!(c.manifest, "Cargo.toml");
        assert_eq!(c.version_prefix, "version = \"");
        assert!(c.extra_version_files.is_empty() && c.jobs.is_empty());
    }

    #[test]
    fn ci_alone_needs_no_name() {
        let c = parse("job\n> *\n> > id = fmt\n> > name = Format\n> > run = cargo fmt --check\n")
            .unwrap();
        assert_eq!(c.jobs.len(), 1);
        assert!(c.jobs[0].local && c.jobs[0].matrix && c.jobs[0].after.is_none());
        assert!(c.release_identity().is_err());
    }

    #[test]
    fn where_and_when_a_job_runs() {
        let c = parse(
            "ci_parallel = true\njob\n> *\n> > id = gen\n> > name = Generate\n> > run = x\n> > matrix = false\n> *\n> > id = msrv\n> > name = MSRV\n> > run = y\n> > local = false\n> *\n> > id = test\n> > name = Test\n> > run = z\n> > after = gen\n",
        )
        .unwrap();
        assert!(c.ci_parallel);
        assert!(c.jobs[0].local && !c.jobs[0].matrix);
        assert!(!c.jobs[1].local && c.jobs[1].matrix);
        assert_eq!(c.jobs[2].after.as_deref(), Some("gen"));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(parse("name = a\nurl = b\nverfy = cargo test\n").is_err());
    }

    /// An entry has to say both what starts and what it then does.
    #[test]
    fn a_tag_start_says_what_and_does() {
        let head = "name = a\nurl = b\ntag_starts\n> *\n";
        assert!(
            parse(&format!(
                "{head}> > workflow = publish.yml\n> > system = Xcode Cloud\n> > does = both\n"
            ))
            .is_err()
        );
        assert!(parse(&format!("{head}> > does = something, started by nothing\n")).is_err());
        assert!(parse(&format!("{head}> > workflow = publish.yml\n")).is_err());
        assert!(parse(&format!("{head}> > workflow = publish.yml\n> > does = uploads the crates\n> > push_tag_first = true\n")).is_err());
        assert!(
            parse(&format!(
                "{head}> > workflow = publish.yml\n> > does = uploads the crates\n"
            ))
            .is_ok()
        );
    }

    /// The warning before the push is a sentence, and what starts is not
    /// always a file in this repository.
    #[test]
    fn the_push_warning() {
        let mut c = parse("name = a\nurl = b\n").unwrap();
        assert_eq!(c.push_warning(), "Nothing is configured to run on the tag.");
        c.tag_starts = vec![
            TagStart {
                workflow: Some("publish.yml".into()),
                system: None,
                does: Some("uploads every publishable crate to crates.io".into()),
            },
            TagStart {
                workflow: Some("homebrew.yml".into()),
                system: None,
                does: Some("writes the tap".into()),
            },
        ];
        assert_eq!(
            c.push_warning(),
            "The tag is what publishes: `publish.yml` uploads every publishable crate to crates.io, and `homebrew.yml` writes the tap. None of it can be undone."
        );
        c.tag_starts = vec![TagStart {
            workflow: None,
            system: Some("Xcode Cloud".into()),
            does: Some("uploads the build to TestFlight external".into()),
        }];
        assert_eq!(
            c.push_warning(),
            "The tag is what publishes: Xcode Cloud uploads the build to TestFlight external. None of it can be undone."
        );
    }

    /// The TOML from before figl reads the same way.
    #[test]
    fn release_toml_reads_like_ship_figl() {
        let stated: Stated = fig_toml::from_str(
            "name = \"alpha\"\nurl = \"u\"\nmirrors = [\"Cargo.toml\"]\n\n[[tag_starts]]\nworkflow = \"publish.yml\"\ndoes = \"uploads the crates\"\n",
        )
        .unwrap();
        let c = Config::from_stated(stated, RELEASE_TOML).unwrap();
        assert_eq!(c.mirrors, ["Cargo.toml"]);
        assert_eq!(c.tag_starts.len(), 1);
    }
}
