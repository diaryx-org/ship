//! The version a workspace calls itself, and the places it moves.
//!
//! A version is the triple and the pre-release beside it, stored as written so
//! that what this reads round-trips to the same bytes. What it will not read it
//! will not rewrite: build metadata (`+sha`) is refused rather than dropped,
//! because dropping it would silently rename the release.

use std::cmp::Ordering;
use std::fmt;

use crate::cargo;
use crate::config::Config;
use crate::sh::Sh;
use crate::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub release: [u64; 3],
    pub pre: Option<String>,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let [a, b, c] = self.release;
        match &self.pre {
            None => write!(f, "{a}.{b}.{c}"),
            Some(pre) => write!(f, "{a}.{b}.{c}-{pre}"),
        }
    }
}

impl Version {
    pub fn new(a: u64, b: u64, c: u64) -> Self {
        Version {
            release: [a, b, c],
            pre: None,
        }
    }

    pub fn tag(&self) -> String {
        format!("v{self}")
    }
}

/// A pre-release, by SemVer's rule: dot-separated identifiers of
/// `[0-9A-Za-z-]`, none empty, numeric ones without leading zeros.
pub fn is_prerelease(text: &str) -> bool {
    !text.is_empty()
        && text.split('.').all(|id| {
            let shaped =
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            let padded =
                id.len() > 1 && id.starts_with('0') && id.bytes().all(|b| b.is_ascii_digit());
            shaped && !padded
        })
}

/// `1.2.3` or `1.2.3-rc.1`. Anything else is an error rather than a best guess.
pub fn parse(text: &str) -> Result<Version> {
    let text = text.trim();
    let unreadable = format!("`{text}` is not an x.y.z or x.y.z-pre version");
    if text.contains('+') {
        bail!(
            "{unreadable}\nhint: build metadata is not used here, and a version this cannot read is one it must not rewrite"
        );
    }
    let (triple, pre) = match text.find('-') {
        None => (text, None),
        Some(at) => (&text[..at], Some(&text[at + 1..])),
    };
    let parts: Vec<&str> = triple.split('.').collect();
    if parts.len() != 3 {
        bail!("{unreadable}");
    }
    let mut release = [0u64; 3];
    for (slot, part) in release.iter_mut().zip(&parts) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            bail!("{unreadable}");
        }
        *slot = part.parse().map_err(|_| unreadable.clone())?;
    }
    if let Some(pre) = pre
        && !is_prerelease(pre)
    {
        bail!(
            "`{pre}` is not a pre-release: dot-separated identifiers of `[0-9A-Za-z-]`, numeric ones without leading zeros\nhint: `rc.1`, not `rc.01`"
        );
    }
    Ok(Version {
        release,
        pre: pre.map(str::to_string),
    })
}

/// SemVer precedence between two pre-releases: identifier by identifier,
/// numeric ones numerically and below alphanumeric ones, and a prefix below
/// what extends it.
fn precedence(ours: &str, theirs: &str) -> Ordering {
    let a: Vec<&str> = ours.split('.').collect();
    let b: Vec<&str> = theirs.split('.').collect();
    for (x, y) in a.iter().zip(&b) {
        let xn = x.bytes().all(|c| c.is_ascii_digit());
        let yn = y.bytes().all(|c| c.is_ascii_digit());
        let ordering = match (xn, yn) {
            (true, true) => {
                let xi: u128 = x.parse().unwrap_or(0);
                let yi: u128 = y.parse().unwrap_or(0);
                xi.cmp(&yi)
            }
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => x.cmp(y),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    a.len().cmp(&b.len())
}

/// A version with a pre-release sorts below the same triple without one:
/// `1.0.0-rc.1` comes before the `1.0.0` it is a candidate for.
pub fn compare(a: &Version, b: &Version) -> Ordering {
    match a.release.cmp(&b.release) {
        Ordering::Equal => {}
        other => return other,
    }
    match (&a.pre, &b.pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => precedence(x, y),
    }
}

/// `patch`, `minor`, `major`, `as-is`, or a literal version to move to.
///
/// `floor` is what the result has to be ahead of — the highest tag rather than
/// the manifest, because the two come apart: a version raised in an earlier
/// commit and never released sits above the highest tag, and `as-is` tags it.
/// A release that goes backwards is a typo every time.
///
/// The three keywords refuse to run from a pre-release, because any answer
/// would be a guess: from `1.0.0-rc.1`, `patch` reads as `1.0.1` and means
/// `1.0.0` to the person finishing the release.
pub fn bump(current: &Version, spec: &str, floor: Option<&Version>) -> Result<Version> {
    let floor = floor.unwrap_or(current);
    let [a, b, c] = current.release;
    let next = match spec {
        "as-is" => current.clone(),
        "patch" | "minor" | "major" if current.pre.is_some() => {
            let triple = Version::new(a, b, c);
            bail!(
                "`{spec}` has no meaning from the pre-release {current}\nhint: name the version — {triple} finishes this pre-release, and another pre-release carries it on"
            );
        }
        "patch" => Version::new(a, b, c + 1),
        "minor" => Version::new(a, b + 1, 0),
        "major" => Version::new(a + 1, 0, 0),
        literal => parse(literal)?,
    };
    if compare(&next, floor) != Ordering::Greater {
        bail!(
            "{next} is not ahead of {floor}\nhint: releases only move forward — a tag that has been pushed is one other people have already fetched"
        );
    }
    Ok(next)
}

pub fn next_patch(version: &Version) -> Version {
    let [a, b, c] = version.release;
    Version::new(a, b, c + 1)
}

/// Every `v*` tag, oldest first, by git's own version ordering.
pub fn release_tags(sh: &Sh) -> Result<Vec<String>> {
    Ok(sh
        .capture("git", &["tag", "--sort=v:refname", "--list", "v[0-9]*"])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn tagged_versions(sh: &Sh) -> Result<Vec<(String, Version)>> {
    Ok(release_tags(sh)?
        .into_iter()
        .filter_map(|t| parse(&t[1..]).ok().map(|v| (t, v)))
        .collect())
}

/// The highest version any tag names, or `current` where nothing is tagged.
/// This, and not the manifest, is what a bump has to clear.
pub fn floor(sh: &Sh, current: &Version) -> Result<Version> {
    Ok(highest(tagged_versions(sh)?)
        .map(|(_, v)| v)
        .unwrap_or_else(|| current.clone()))
}

/// The tag naming the highest version — by SemVer, not by list order, so a
/// repository whose tags sort oddly still names the same one as `floor`.
pub fn latest_tag(sh: &Sh) -> Result<Option<String>> {
    Ok(highest(tagged_versions(sh)?).map(|(t, _)| t))
}

fn highest(tagged: Vec<(String, Version)>) -> Option<(String, Version)> {
    tagged.into_iter().reduce(|acc, it| {
        if compare(&it.1, &acc.1) == Ordering::Greater {
            it
        } else {
            acc
        }
    })
}

/// The version every member inherits.
pub fn read(sh: &Sh, config: &Config) -> Result<Version> {
    version_in(&sh.read(&config.manifest)?, config, &config.manifest)
}

/// The version a tag's own tree names, or none where the tag has no manifest.
/// The tree, not the working copy: a release cut months ago is read back from
/// the commit it was cut at.
pub fn read_at(sh: &Sh, config: &Config, tag: &str) -> Result<Option<Version>> {
    let manifest = sh.try_capture("git", &["show", &format!("{tag}:{}", config.manifest)]);
    if manifest.is_empty() {
        return Ok(None);
    }
    version_in(&manifest, config, &format!("{} at {tag}", config.manifest)).map(Some)
}

fn version_in(manifest: &str, config: &Config, place: &str) -> Result<Version> {
    let Some(line) = manifest
        .lines()
        .find(|l| l.starts_with(&config.version_prefix))
    else {
        bail!("no line starting `{}` in {place}", config.version_prefix);
    };
    if config.version_quoted {
        parse(line.split('"').nth(1).unwrap_or_default())
    } else {
        parse(line[config.version_prefix.len()..].trim())
    }
}

/// `name = { path = "…", version = "0.1.0" }` with the version moved, or none
/// if this line is not such an entry — or names a crate outside `members`.
///
/// `members` is what makes a path dependency internal. historica-git depends
/// on `historica = { version = "1.0", path = "../historica" }`, a different
/// repository; rewriting by line shape alone repointed it at historica-git's
/// own version. A dependency renamed by its key is left alone, since the key
/// is not the package name cargo reports.
pub fn retarget_path_dep(line: &str, next: &Version, members: Option<&[String]>) -> Option<String> {
    if !line.contains("path = \"") {
        return None;
    }
    let at = line.find("version = \"")?;
    if let Some(members) = members {
        let key = line.split('=').next().unwrap_or_default().trim();
        if !members.iter().any(|m| m == key) {
            return None;
        }
    }
    let start = at + "version = \"".len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(format!("{}{next}{}", &line[..start], &rest[end..]))
}

/// The version after the prefix replaced, and everything after its closing
/// quote kept — a build.zig.zon's trailing comma is part of its syntax.
/// Unquoted, the version runs to the end of the line.
pub fn retarget_version_line(
    line: &str,
    prefix: &str,
    next: &Version,
    unquoted: bool,
) -> Option<String> {
    if unquoted {
        return Some(format!("{prefix}{next}"));
    }
    let rest = &line[prefix.len()..];
    let end = rest.find('"')?;
    Some(format!("{prefix}{next}{}", &rest[end..]))
}

fn lines_out(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Move one manifest: the line the prefix finds, and — in a Cargo workspace —
/// every internal `{ path, version }` pin, which is what `cargo publish`
/// uploads as the requirement.
pub fn rewrite_manifest(
    sh: &Sh,
    path: &str,
    prefix: &str,
    next: &Version,
    members: Option<&[String]>,
    unquoted: bool,
) -> Result<()> {
    let manifest = sh.read(path)?;
    let mut out = Vec::new();
    let (mut found, mut internal) = (0, 0);
    for line in manifest.lines() {
        if line.starts_with(prefix) {
            let Some(retargeted) = retarget_version_line(line, prefix, next, unquoted) else {
                bail!("unterminated version string in {path}:\n  {line}");
            };
            out.push(retargeted);
            found += 1;
        } else if let Some(retargeted) =
            members.and_then(|m| retarget_path_dep(line, next, Some(m)))
        {
            out.push(retargeted);
            internal += 1;
        } else {
            out.push(line.to_string());
        }
    }
    if found != 1 {
        bail!("expected exactly one line starting `{prefix}` in {path}, found {found}");
    }
    sh.write(path, &lines_out(&out))?;
    println!("{path} -> {next} (and {internal} internal dependencies)");
    Ok(())
}

/// The internal pins in a member manifest, which has no version line of its
/// own. Written back only when a line moved.
pub fn retarget_path_deps(
    sh: &Sh,
    path: &str,
    next: &Version,
    members: &[String],
) -> Result<usize> {
    let manifest = sh.read(path)?;
    let mut out = Vec::new();
    let mut internal = 0;
    for line in manifest.lines() {
        match retarget_path_dep(line, next, Some(members)) {
            Some(retargeted) => {
                out.push(retargeted);
                internal += 1;
            }
            None => out.push(line.to_string()),
        }
    }
    if internal > 0 {
        sh.write(path, &lines_out(&out))?;
        let noun = if internal == 1 {
            "dependency"
        } else {
            "dependencies"
        };
        println!("{path} -> {next} in {internal} internal {noun}");
    }
    Ok(internal)
}

/// Every file a bump rewrites, for the release commit to stage by name.
pub fn version_files(sh: &Sh, config: &Config) -> Result<Vec<String>> {
    let mut files = vec![config.manifest.clone()];
    if config.cargo_workspace {
        files.push("Cargo.lock".into());
    }
    for mirror in &config.mirrors {
        files.push(mirror.clone());
        files.push(lockfile_beside(mirror));
    }
    for w in cargo::workspaces(sh, config)? {
        files.extend(w.manifests);
    }
    files.extend(config.extra_version_files.iter().cloned());
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(f.clone()));
    Ok(files)
}

fn lockfile_beside(manifest: &str) -> String {
    match std::path::Path::new(manifest)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
    {
        Some(dir) if !dir.is_empty() => format!("{dir}/Cargo.lock"),
        _ => "Cargo.lock".into(),
    }
}

fn dir_of(manifest: &str) -> Option<String> {
    std::path::Path::new(manifest)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|d| !d.is_empty())
}

/// Move the workspace to `next`, in every file that carries it.
pub fn write(sh: &Sh, config: &Config, next: &Version) -> Result<()> {
    let workspaces = cargo::workspaces(sh, config)?;
    let members_of = |manifest: &str| {
        workspaces
            .iter()
            .find(|w| w.manifest == manifest)
            .map(|w| w.members.clone())
    };

    let canonical = members_of(&config.manifest);
    rewrite_manifest(
        sh,
        &config.manifest,
        &config.version_prefix,
        next,
        canonical.as_deref(),
        !config.version_quoted,
    )?;
    for mirror in &config.mirrors {
        let members = members_of(mirror);
        rewrite_manifest(sh, mirror, "version = \"", next, members.as_deref(), false)?;
    }

    // Then every other manifest in each workspace, for the internal pins
    // declared outside the root's `[workspace.dependencies]`.
    let mut rewritten = vec![config.manifest.clone()];
    rewritten.extend(config.mirrors.iter().cloned());
    for w in &workspaces {
        for path in w.manifests.iter().filter(|p| !rewritten.contains(p)) {
            retarget_path_deps(sh, path, next, &w.members)?;
        }
    }

    // Whatever the version reaches that no manifest parser would find, before
    // the lockfiles, so a `post_bump` that moves a manifest is reflected in
    // the lock rather than left one cargo invocation from dirtying the tree.
    if let Some(command) = &config.post_bump {
        sh.shell(command)?;
    }

    // The lockfile records the members' own versions; `--workspace` touches
    // nothing else, because a release is not the moment to pick up a new
    // upstream dependency.
    if config.cargo_workspace {
        sh.call("cargo", &["update", "--workspace", "--quiet"])?;
    }
    for mirror in &config.mirrors {
        sh.call_in(
            dir_of(mirror).as_deref(),
            "cargo",
            &["update", "--workspace", "--quiet"],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: u64, b: u64, c: u64) -> Version {
        Version::new(a, b, c)
    }
    fn vp(a: u64, b: u64, c: u64, pre: &str) -> Version {
        Version {
            release: [a, b, c],
            pre: Some(pre.into()),
        }
    }

    #[test]
    fn parses_and_round_trips() {
        assert_eq!(parse("0.1.0").unwrap(), v(0, 1, 0));
        assert_eq!(parse("1.0.0-rc.1").unwrap(), vp(1, 0, 0, "rc.1"));
        assert_eq!(v(0, 9, 2).to_string(), "0.9.2");
        assert_eq!(vp(1, 0, 0, "rc.1").to_string(), "1.0.0-rc.1");
        assert_eq!(
            parse("1.0.0-alpha.0.beta").unwrap().to_string(),
            "1.0.0-alpha.0.beta"
        );
        for bad in [
            "0.1",
            "1.0.0+sha.abc",
            "1.0.0-rc.01",
            "1.0.0-",
            "1.0.0-rc..1",
            "1..0",
            "a.b.c",
        ] {
            assert!(parse(bad).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn orders_by_semver() {
        use Ordering::*;
        assert_eq!(compare(&vp(1, 0, 0, "rc.1"), &v(1, 0, 0)), Less);
        assert_eq!(compare(&v(1, 0, 0), &vp(1, 0, 0, "rc.1")), Greater);
        assert_eq!(compare(&v(1, 0, 0), &v(1, 0, 0)), Equal);
        assert_eq!(compare(&v(1, 0, 1), &v(1, 0, 0)), Greater);
        assert_eq!(compare(&vp(1, 0, 0, "rc.2"), &vp(1, 0, 0, "rc.10")), Less);
        assert_eq!(compare(&vp(1, 0, 0, "alpha"), &vp(1, 0, 0, "rc")), Less);
        assert_eq!(compare(&vp(1, 0, 0, "rc.1"), &vp(1, 0, 0, "rc.1.1")), Less);
        assert_eq!(compare(&vp(1, 0, 0, "1"), &vp(1, 0, 0, "alpha")), Less);
    }

    #[test]
    fn bumps_forward_only() {
        let s = |cur: &Version, spec, floor: Option<&Version>| {
            bump(cur, spec, floor).map(|v| v.to_string())
        };
        assert_eq!(s(&v(0, 1, 0), "patch", None).unwrap(), "0.1.1");
        assert_eq!(s(&v(0, 1, 0), "minor", None).unwrap(), "0.2.0");
        assert_eq!(s(&v(0, 1, 0), "major", None).unwrap(), "1.0.0");
        assert_eq!(s(&v(0, 1, 0), "0.9.3", None).unwrap(), "0.9.3");
        assert_eq!(s(&v(0, 9, 3), "1.0.0-rc.1", None).unwrap(), "1.0.0-rc.1");
        assert!(s(&v(0, 1, 0), "0.0.9", None).is_err());
        assert!(s(&v(0, 1, 0), "0.1.0", None).is_err());
        for kw in ["patch", "minor", "major"] {
            assert!(s(&vp(1, 0, 0, "rc.1"), kw, None).is_err());
        }
        assert_eq!(
            s(&vp(1, 0, 0, "rc.1"), "1.0.0", Some(&v(0, 9, 0))).unwrap(),
            "1.0.0"
        );
        assert_eq!(
            s(&vp(1, 0, 0, "rc.1"), "1.0.0-rc.2", Some(&v(0, 9, 0))).unwrap(),
            "1.0.0-rc.2"
        );
        assert_eq!(s(&v(1, 0, 0), "as-is", Some(&v(0, 2, 0))).unwrap(), "1.0.0");
        assert!(s(&v(1, 0, 0), "as-is", Some(&v(1, 0, 0))).is_err());
        assert!(s(&v(1, 0, 0), "as-is", None).is_err());
    }

    #[test]
    fn retargets_lines() {
        assert_eq!(
            retarget_path_dep(
                r#"flower-core = { path = "crates/flower-core", version = "0.1.0" }"#,
                &v(0, 2, 0),
                None
            )
            .unwrap(),
            r#"flower-core = { path = "crates/flower-core", version = "0.2.0" }"#
        );
        assert_eq!(
            retarget_path_dep(
                r#"flower-ratatui = { path = "crates/flower-ratatui" }"#,
                &v(0, 2, 0),
                None
            ),
            None
        );
        assert_eq!(
            retarget_version_line(
                r#"    .version = "3.2.1","#,
                r#"    .version = ""#,
                &v(3, 3, 0),
                false
            )
            .unwrap(),
            r#"    .version = "3.3.0","#
        );
        assert_eq!(
            retarget_version_line(r#"version = "0.9.2""#, r#"version = ""#, &v(0, 9, 3), false)
                .unwrap(),
            r#"version = "0.9.3""#
        );
        assert_eq!(
            retarget_version_line(r#"version = "0.9.2"#, r#"version = ""#, &v(0, 9, 3), false),
            None
        );
        assert_eq!(
            retarget_version_line("version: v2.2.1", "version: v", &v(2, 3, 0), true).unwrap(),
            "version: v2.3.0"
        );
        assert_eq!(
            retarget_version_line("version: v2.2.1", "version: v", &v(2, 3, 0), false),
            None
        );
        assert_eq!(
            retarget_path_dep(r#"fig = { version = "3" }"#, &v(0, 2, 0), None),
            None
        );
        let features = r#"historica = { version = "1.0.0", path = "..", default-features = false, features = ["#;
        assert_eq!(
            retarget_path_dep(features, &vp(1, 0, 0, "rc.1"), None).unwrap(),
            r#"historica = { version = "1.0.0-rc.1", path = "..", default-features = false, features = ["#
        );
        assert_eq!(
            retarget_path_dep(features, &v(1, 1, 0), None).unwrap(),
            r#"historica = { version = "1.1.0", path = "..", default-features = false, features = ["#
        );
    }

    /// `members` tells an internal pin from a dependency on a repository that
    /// happens to sit beside this one.
    #[test]
    fn only_members_move() {
        let sibling = r#"historica = { version = "1.0", path = "../historica" }"#;
        let members = ["historica-git".to_string(), "xtask".to_string()];
        assert_eq!(
            retarget_path_dep(sibling, &v(0, 2, 0), Some(&members)),
            None
        );
        let leaf = ["leaf-core".to_string(), "leaf-ffi".to_string()];
        assert_eq!(
            retarget_path_dep(
                r#"leaf-core = { path = "../leaf-core", version = "0.1.2" }"#,
                &v(0, 2, 0),
                Some(&leaf)
            )
            .unwrap(),
            r#"leaf-core = { path = "../leaf-core", version = "0.2.0" }"#
        );
        assert_eq!(
            retarget_path_dep(sibling, &v(0, 2, 0), None).unwrap(),
            r#"historica = { version = "0.2.0", path = "../historica" }"#
        );
        assert_eq!(
            retarget_path_dep(r#"version = "0.1.0""#, &v(0, 2, 0), None),
            None
        );
    }
}
