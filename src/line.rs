//! A release line: what one release is cut for.
//!
//! A repository at one version has one line, the whole of it: `v` tags, one
//! changelog, every commit. That is every repository unless its config says
//! `per_crate = true`, and then each crate of the workspace is a line of its
//! own, with a version, tags, a changelog and a proposal of its own:
//!
//! - A crate with its own `[package] version` is a line named after it, tagged
//!   `<crate>-v<x.y.z>`, with `CHANGELOG.md` in its directory, and the commits
//!   that touch that directory.
//! - The crates that inherit `[workspace.package] version` move together, as
//!   they always have, and are one line named after the repository: `v` tags,
//!   the configured `changelog`, and every commit that touches something no
//!   other line owns. So a repository that absorbs its plugins keeps the tags
//!   and changelog it already had.
//! - Where nothing inherits, the crate named as the repository is that line.
//!
//! A commit that touches two lines is in both changelogs.

use std::collections::HashSet;

use crate::config::Config;
use crate::sh::Sh;
use crate::version::{self, Version};
use crate::{Result, bail, cargo};

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// What `-p` takes, and what messages call it.
    pub name: String,
    /// What a tag is before its version: `v`, or `<crate>-v`.
    pub prefix: String,
    /// The file holding the version, and the table it sits under. No table is
    /// the whole repository, where the first line `version_prefix` finds is
    /// the version, as it always was.
    pub manifest: String,
    pub table: Option<&'static str>,
    pub changelog: String,
    /// The directories whose commits are this line's, and those inside them
    /// that belong to another line. No include is every directory.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    /// The crates at this version, whose `{ path, version }` pins move with it.
    pub crates: Vec<String>,
    /// The whole repository at one version: everything as it was before there
    /// were lines.
    pub whole: bool,
}

pub const WORKSPACE_TABLE: &str = "[workspace.package]";
pub const PACKAGE_TABLE: &str = "[package]";

impl Line {
    /// The repository at one version.
    pub fn whole(config: &Config) -> Line {
        Line {
            name: config.name.clone().unwrap_or_default(),
            prefix: "v".into(),
            manifest: config.manifest.clone(),
            table: None,
            changelog: config.changelog.clone(),
            include: Vec::new(),
            exclude: Vec::new(),
            crates: Vec::new(),
            whole: true,
        }
    }

    pub fn tag(&self, version: &Version) -> String {
        format!("{}{version}", self.prefix)
    }

    /// The version a tag of this line names, or none where it is not one.
    pub fn version_of(&self, tag: &str) -> Option<Version> {
        version::parse(tag.strip_prefix(&self.prefix)?).ok()
    }

    /// A tag as this line's changelog heads it: `v0.2.0`, whatever the
    /// prefix, because the changelog is the crate's own.
    pub fn short(&self, tag: &str) -> Option<String> {
        self.version_of(tag).map(|v| v.tag())
    }

    /// The glob `git tag --list` takes for this line's tags.
    pub fn tag_glob(&self) -> String {
        format!("{}[0-9]*", self.prefix)
    }

    /// The pathspec `git log` takes for this line's commits, after `--`; empty
    /// for every commit. A commit is this line's when it touches any file
    /// that is, so one that touches two lines is in both.
    pub fn pathspec(&self) -> Vec<String> {
        if self.include.is_empty() && self.exclude.is_empty() {
            return Vec::new();
        }
        let mut spec = if self.include.is_empty() {
            vec![".".to_string()]
        } else {
            self.include.clone()
        };
        spec.extend(self.exclude.iter().map(|d| format!(":(exclude){d}")));
        spec
    }

    /// The commits in `range` that are this line's: those `git log <range>
    /// -- <pathspec>` shows, which is what the proposal counts and what the
    /// changelog renders.
    pub fn commits(&self, sh: &Sh, range: &str) -> HashSet<String> {
        let spec = self.pathspec();
        let mut args = vec!["log", "--format=%H", range];
        if !spec.is_empty() {
            args.push("--");
            args.extend(spec.iter().map(String::as_str));
        }
        sh.try_capture("git", &args)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The commits this line's history leaves out entirely: those reachable
    /// from HEAD only through a side of a merge that git's walk over the
    /// line's paths prunes.
    ///
    /// A repository brought in by `git subtree add` is the case that needs
    /// it. Its whole history comes along, every commit of it touching paths
    /// at its old root — `src/lib.rs`, `Cargo.toml` — which no directory
    /// excludes, and its own `v*` tags come with it. The merge changes nothing
    /// outside the subtree, so git's walk follows the other parent and never
    /// reaches that history; a tag on it is not this line's.
    pub fn pruned(&self, sh: &Sh) -> HashSet<String> {
        let spec = self.pathspec();
        if spec.is_empty() {
            return HashSet::new();
        }
        let mut args = vec!["rev-list", "--sparse", "HEAD", "--"];
        args.extend(spec.iter().map(String::as_str));
        let walked: HashSet<String> = sh
            .try_capture("git", &args)
            .lines()
            .map(str::to_string)
            .collect();
        sh.try_capture("git", &["rev-list", "HEAD"])
            .lines()
            .filter(|c| !walked.contains(*c))
            .map(str::to_string)
            .collect()
    }

    /// Whether `-p <selector>` names this line: its own name, or any crate in it.
    pub fn answers_to(&self, selector: &str) -> bool {
        self.name == selector || self.crates.iter().any(|c| c == selector)
    }
}

/// How a member's manifest states its version.
#[derive(Debug, PartialEq)]
pub enum Stated {
    Own,
    Inherited,
    Neither,
}

/// Whether a line opens a table, and which: `[package]`, with any trailing
/// comment dropped.
fn table_of(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.starts_with('[') {
        return None;
    }
    let end = t.find(']')?;
    // `[[bin]]` closes with two.
    let end = if t[end..].starts_with("]]") {
        end + 2
    } else {
        end + 1
    };
    Some(&t[..end])
}

/// The index of the `version = "…"` line directly under `table`.
pub fn version_line(text: &str, table: &str) -> Option<usize> {
    let mut inside = false;
    for (i, line) in text.lines().enumerate() {
        if let Some(t) = table_of(line) {
            inside = t == table;
        } else if inside && line.starts_with("version = \"") {
            return Some(i);
        }
    }
    None
}

pub fn stated(manifest: &str) -> Stated {
    if version_line(manifest, PACKAGE_TABLE).is_some() {
        return Stated::Own;
    }
    let mut inside = false;
    for line in manifest.lines() {
        if let Some(t) = table_of(line) {
            inside = t == PACKAGE_TABLE;
        } else if inside {
            let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if squeezed.starts_with("version.workspace=true")
                || squeezed.starts_with("version={workspace=true")
            {
                return Stated::Inherited;
            }
        }
    }
    Stated::Neither
}

/// One member, as `lines` needs to see it.
pub struct Crate {
    pub name: String,
    pub manifest: String,
    /// Its directory relative to the root, `` for the root itself.
    pub dir: String,
    pub stated: Stated,
    pub publishable: bool,
}

fn dir_of(manifest: &str) -> String {
    std::path::Path::new(manifest)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every line in the repository: the whole of it, or one per crate.
pub fn lines(sh: &Sh, config: &Config) -> Result<Vec<Line>> {
    if !config.per_crate {
        return Ok(vec![Line::whole(config)]);
    }
    let mut crates = Vec::new();
    for m in cargo::members(sh)? {
        crates.push(Crate {
            stated: stated(&sh.read(&m.manifest)?),
            dir: dir_of(&m.manifest),
            name: m.name,
            manifest: m.manifest,
            publishable: m.publishable,
        });
    }
    per_crate(config, &crates)
}

/// The lines per crate, from the members — apart from `lines` so a test can
/// hand it members without a workspace.
pub fn per_crate(config: &Config, crates: &[Crate]) -> Result<Vec<Line>> {
    let name = config.name.clone().unwrap_or_default();
    // Released, by default, is every crate that can be: one `publish = false`
    // is an xtask or a test harness, and a list of lines that says it has
    // unreleased changes is noise. `crates` names them where that is wrong.
    let chosen = |c: &Crate| match &config.crates {
        Some(list) => list.contains(&c.name),
        None => c.publishable,
    };
    if let Some(list) = &config.crates
        && let Some(unknown) = list.iter().find(|n| !crates.iter().any(|c| &c.name == *n))
    {
        bail!(
            "{} names `{unknown}` in `crates`, which is not a member of the workspace",
            config.source
        );
    }

    // The crates that move with `[workspace.package] version`, as one line.
    let inheriting: Vec<&Crate> = crates
        .iter()
        .filter(|c| c.stated == Stated::Inherited)
        .collect();
    let workspace = inheriting.iter().any(|c| chosen(c));
    let mut groups: Vec<(Line, Vec<String>)> = Vec::new();
    if workspace {
        groups.push((
            Line {
                name: name.clone(),
                prefix: "v".into(),
                manifest: config.manifest.clone(),
                table: Some(WORKSPACE_TABLE),
                changelog: config.changelog.clone(),
                include: Vec::new(),
                exclude: Vec::new(),
                crates: inheriting.iter().map(|c| c.name.clone()).collect(),
                whole: false,
            },
            inheriting.iter().map(|c| c.dir.clone()).collect(),
        ));
    }
    for c in crates
        .iter()
        .filter(|c| c.stated == Stated::Own && chosen(c))
    {
        // The repository's own name keeps the plain tags, where the
        // workspace version has not claimed them.
        let plain = !workspace && c.name == name;
        let (prefix, changelog) = if plain {
            ("v".to_string(), config.changelog.clone())
        } else if c.dir.is_empty() {
            (format!("{}-v", c.name), "CHANGELOG.md".to_string())
        } else {
            (format!("{}-v", c.name), format!("{}/CHANGELOG.md", c.dir))
        };
        groups.push((
            Line {
                name: c.name.clone(),
                prefix,
                manifest: c.manifest.clone(),
                table: Some(PACKAGE_TABLE),
                changelog,
                include: Vec::new(),
                exclude: Vec::new(),
                crates: vec![c.name.clone()],
                whole: false,
            },
            vec![c.dir.clone()],
        ));
    }
    if let Some(list) = &config.crates
        && let Some(missing) = list.iter().find(|n| {
            crates
                .iter()
                .any(|c| &c.name == *n && c.stated == Stated::Neither)
        })
    {
        bail!(
            "{missing} states no version of its own and inherits none, so it has nothing to release"
        );
    }
    if groups.is_empty() {
        bail!(
            "{} sets `per_crate`, and no crate in the workspace has a version to release\nhint: a crate is released when it states `version` or `version.workspace = true` and does not say `publish = false`; `crates` names them otherwise",
            config.source
        );
    }

    // The paths each line owns: its directories, less those inside them
    // that another line owns. The root is every directory, so it is no
    // include at all.
    let dirs: Vec<Vec<String>> = groups.iter().map(|(_, d)| d.clone()).collect();
    for (i, (line, own)) in groups.iter_mut().enumerate() {
        if !own.iter().any(String::is_empty) {
            line.include = own.clone();
        }
        for (j, theirs) in dirs.iter().enumerate() {
            if i == j {
                continue;
            }
            for dir in theirs.iter().filter(|d| !d.is_empty()) {
                let inside = own
                    .iter()
                    .any(|o| o.is_empty() || dir.starts_with(&format!("{o}/")));
                if inside && !line.exclude.contains(dir) {
                    line.exclude.push(dir.clone());
                }
            }
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for (line, _) in &groups {
        if seen.contains(&line.changelog.as_str()) {
            bail!(
                "two release lines would write {}\nhint: a crate at the root shares the repository's changelog; name the lines with `crates`",
                line.changelog
            );
        }
        seen.push(&line.changelog);
    }
    Ok(groups.into_iter().map(|(l, _)| l).collect())
}

/// The one line a command acts on. At one version, that is the repository,
/// and naming a crate is refused rather than ignored; per crate, it is the
/// one named, and naming none is refused with the names to choose from.
pub fn select(sh: &Sh, config: &Config, selector: Option<&str>) -> Result<Line> {
    let all = lines(sh, config)?;
    pick(config, &all, selector)
}

pub fn pick(config: &Config, all: &[Line], selector: Option<&str>) -> Result<Line> {
    if !config.per_crate {
        if let Some(s) = selector {
            bail!(
                "-p {s}: {} releases the whole repository at one version\nhint: `per_crate = true` in {} releases each crate on its own",
                config.name.as_deref().unwrap_or("this repository"),
                config.source
            );
        }
        return Ok(all[0].clone());
    }
    let names: Vec<&str> = all.iter().map(|l| l.name.as_str()).collect();
    let Some(selector) = selector else {
        bail!(
            "this repository releases each crate on its own; name one with -p\nhint: {}",
            names.join(", ")
        );
    };
    match all.iter().find(|l| l.answers_to(selector)) {
        Some(line) => Ok(line.clone()),
        None => bail!(
            "no crate here releases as `{selector}`\nhint: {}",
            names.join(", ")
        ),
    }
}

/// The line a tag belongs to: the longest prefix that leaves a version.
pub fn for_tag(all: &[Line], tag: &str) -> Option<Line> {
    all.iter()
        .filter(|l| l.version_of(tag).is_some())
        .max_by_key(|l| l.prefix.len())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(figl: &str) -> Config {
        Config::parse_for_test(figl).unwrap()
    }

    fn krate(name: &str, dir: &str, stated: Stated, publishable: bool) -> Crate {
        let manifest = if dir.is_empty() {
            "Cargo.toml".to_string()
        } else {
            format!("{dir}/Cargo.toml")
        };
        Crate {
            name: name.into(),
            manifest,
            dir: dir.into(),
            stated,
            publishable,
        }
    }

    /// historica, once its plugins are members: the library and the CLI at
    /// the workspace version, each plugin at its own, and xtask at none.
    fn historica() -> Vec<Crate> {
        vec![
            krate("historica", "", Stated::Inherited, true),
            krate("historica-cli", "cli", Stated::Inherited, true),
            krate("xtask", "xtask", Stated::Own, false),
            krate("historica-git", "historica-git", Stated::Own, true),
            krate("historica-remark", "plugins/remark", Stated::Own, true),
        ]
    }

    #[test]
    fn one_line_per_crate_and_one_for_the_workspace() {
        let c = config("name = historica\nurl = u\nper_crate = true\n");
        let lines = per_crate(&c, &historica()).unwrap();
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["historica", "historica-git", "historica-remark"]);

        let ws = &lines[0];
        assert_eq!(ws.prefix, "v");
        assert_eq!(ws.table, Some(WORKSPACE_TABLE));
        assert_eq!(ws.changelog, "docs/CHANGELOG.md");
        assert_eq!(ws.crates, ["historica", "historica-cli"]);
        assert!(ws.include.is_empty());
        assert_eq!(ws.exclude, ["historica-git", "plugins/remark"]);
        assert!(ws.answers_to("historica-cli"));

        let git = &lines[1];
        assert_eq!(git.prefix, "historica-git-v");
        assert_eq!(git.manifest, "historica-git/Cargo.toml");
        assert_eq!(git.table, Some(PACKAGE_TABLE));
        assert_eq!(git.changelog, "historica-git/CHANGELOG.md");
        assert_eq!(git.include, ["historica-git"]);
        assert!(git.exclude.is_empty());
        assert_eq!(git.tag(&Version::new(0, 3, 0)), "historica-git-v0.3.0");
        assert_eq!(git.short("historica-git-v0.3.0").as_deref(), Some("v0.3.0"));
        assert_eq!(git.short("v0.3.0"), None);
        assert_eq!(
            ws.pathspec(),
            [".", ":(exclude)historica-git", ":(exclude)plugins/remark"]
        );
        assert_eq!(git.pathspec(), ["historica-git"]);

        assert_eq!(
            for_tag(&lines, "historica-git-v0.3.0").unwrap().name,
            "historica-git"
        );
        assert_eq!(for_tag(&lines, "v1.0.0").unwrap().name, "historica");
        assert_eq!(for_tag(&lines, "historica-v1.0.0"), None);
    }

    #[test]
    fn selecting_a_line() {
        let c = config("name = historica\nurl = u\nper_crate = true\n");
        let lines = per_crate(&c, &historica()).unwrap();
        assert_eq!(
            pick(&c, &lines, Some("historica-git")).unwrap().name,
            "historica-git"
        );
        assert_eq!(
            pick(&c, &lines, Some("historica-cli")).unwrap().name,
            "historica"
        );
        let none = pick(&c, &lines, None).unwrap_err();
        assert!(
            none.contains("historica, historica-git, historica-remark"),
            "{none}"
        );
        assert!(pick(&c, &lines, Some("xtask")).is_err());

        let single = config("name = alpha\nurl = u\n");
        let whole = [Line::whole(&single)];
        assert!(pick(&single, &whole, None).unwrap().whole);
        assert!(pick(&single, &whole, Some("alpha")).is_err());
        assert!(whole[0].pathspec().is_empty());
    }

    /// With nothing inheriting, the crate named as the repository keeps the
    /// plain tags and the configured changelog.
    #[test]
    fn the_repository_crate_keeps_plain_tags() {
        let c = config("name = alpha\nurl = u\nper_crate = true\n");
        let lines = per_crate(
            &c,
            &[
                krate("alpha", "", Stated::Own, true),
                krate("alpha-beta", "beta", Stated::Own, true),
            ],
        )
        .unwrap();
        assert_eq!(lines[0].prefix, "v");
        assert_eq!(lines[0].changelog, "docs/CHANGELOG.md");
        assert!(lines[0].include.is_empty());
        assert_eq!(lines[0].exclude, ["beta"]);
        assert_eq!(lines[1].prefix, "alpha-beta-v");
    }

    #[test]
    fn crates_names_the_lines() {
        let c = config(
            "name = historica\nurl = u\nper_crate = true\ncrates = [historica-git, xtask]\n",
        );
        let names: Vec<String> = per_crate(&c, &historica())
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(names, ["xtask", "historica-git"]);
        let c = config("name = historica\nurl = u\nper_crate = true\ncrates = [nope]\n");
        assert!(per_crate(&c, &historica()).is_err());
    }

    #[test]
    fn how_a_manifest_states_its_version() {
        assert_eq!(
            stated("[package]\nname = \"a\"\nversion = \"0.1.0\"\n"),
            Stated::Own
        );
        assert_eq!(
            stated("[package]\nname = \"a\"\nversion.workspace = true\n"),
            Stated::Inherited
        );
        assert_eq!(
            stated("[package]\nname = \"a\"\nversion = { workspace = true }\n"),
            Stated::Inherited
        );
        assert_eq!(
            stated("[package]\nname = \"a\"\n\n[dependencies]\nversion = \"1\"\n"),
            Stated::Neither
        );
        let root = "[workspace]\nmembers = [\"a\"]\n\n[workspace.package]\nversion = \"1.0.0\"\n\n[package]\nname = \"r\"\nversion = \"0.2.0\" # its own\n";
        assert_eq!(version_line(root, WORKSPACE_TABLE), Some(4));
        assert_eq!(version_line(root, PACKAGE_TABLE), Some(8));
        assert_eq!(
            version_line("[[bin]]\nversion = \"1\"\n", PACKAGE_TABLE),
            None
        );
    }
}
