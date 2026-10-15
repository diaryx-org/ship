//! What a release would be, before anything is written.
//!
//! Which of patch, minor and major a release is, is a judgement about the
//! promise a repository makes to everything downstream. It is not a fact about
//! the diff, and nothing here tries to derive one: `ship` with no version
//! gathers what the commits say and stops. The number is named by a person,
//! and naming it is the whole of the consent.
//!
//! The evidence is what the commits already carry. Conventional Commits' `!`
//! and its `BREAKING CHANGE:` footer say the promise broke. A
//! `Behavioural-change:` trailer — written whenever a caller who upgrades
//! without editing their own code would see a difference — is the
//! minor-versus-patch question, answered at the time by whoever knew.

use std::cmp::Ordering;

use crate::Result;
use crate::config::Config;
use crate::sh::Sh;
use crate::version::{self, Version};

/// Unit and record separator: the only bytes that delimit, because a commit
/// message cannot contain them.
const US: char = '\u{1f}';
const RS: char = '\u{1e}';

#[derive(Debug, PartialEq)]
pub struct SubjectFacts {
    pub kind: Option<String>,
    pub breaking: bool,
}

/// The type a Conventional Commits subject declares, and whether it declares
/// a break. No type is a subject that is not conventional at all — a merge,
/// or one written by hand — counted as "other" rather than refused, since this
/// reads history from before the rule too.
pub fn subject_facts(subject: &str) -> SubjectFacts {
    let none = SubjectFacts {
        kind: None,
        breaking: false,
    };
    let bytes = subject.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 {
        return none;
    }
    let kind = subject[..i].to_ascii_lowercase();
    if bytes.get(i) == Some(&b'(') {
        match subject[i..].find(')') {
            Some(close) => i += close + 1,
            None => return none,
        }
    }
    let breaking = bytes.get(i) == Some(&b'!');
    if breaking {
        i += 1;
    }
    if bytes.get(i) != Some(&b':') {
        return none;
    }
    SubjectFacts {
        kind: Some(kind),
        breaking,
    }
}

/// Whether a body carries the footer that says what the `!` does, in either
/// of the spec's spellings.
pub fn body_breaks(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim();
        line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:")
    })
}

/// Whether a line starts a trailer of its own: `Key: value`.
fn starts_trailer(line: &str) -> bool {
    let bytes = line.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    bytes.get(i) == Some(&b':') && bytes.get(i + 1).is_some_and(u8::is_ascii_whitespace)
}

/// Whether a line carries on the trailer above it. Git's rule is an indent,
/// and these trailers are written as wrapped paragraphs with none, so a value
/// runs until a blank line, a line starting a trailer of its own, or the end.
fn continues(line: &str) -> bool {
    !line.trim().is_empty() && !starts_trailer(line)
}

/// The value of a `Behavioural-change:` line, in any casing and in the
/// American spelling too: a trailer missed because it was typed
/// `Behavioral-change` is evidence lost from the decision it was written for.
fn behavioural_value(line: &str) -> Option<&str> {
    for key in ["behavioural-change:", "behavioral-change:"] {
        // `get`, not indexing: a line can carry a multibyte character exactly
        // where the key would end.
        if line
            .get(..key.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(key))
        {
            return Some(line[key.len()..].trim_start());
        }
    }
    None
}

/// The `Behavioural-change:` values a body carries, each folded back into one
/// string.
pub fn behavioural_trailers(body: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut open = false;
    for line in body.lines() {
        if let Some(value) = behavioural_value(line) {
            values.push(value.trim().to_string());
            open = true;
            continue;
        }
        if open
            && continues(line)
            && let Some(last) = values.last_mut()
        {
            *last = format!("{last} {}", line.trim());
            continue;
        }
        open = false;
    }
    values.retain(|v| !v.is_empty());
    values
}

/// A hanging indent, so a trailer that is a paragraph reads as one entry. A
/// word longer than the width is not broken — a path split across two lines
/// is worse than a line that runs over.
pub fn hanging(text: &str, width: usize, indent: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if current.is_empty() {
            current = word.to_string();
        } else if current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.clone()
            } else {
                format!("{indent}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub kind: Option<String>,
    pub breaking: bool,
    pub behavioural: Vec<String>,
}

/// Every commit since the tag, or in the whole history where there is none.
pub fn commits_since(sh: &Sh, tag: Option<&str>) -> Vec<Commit> {
    let range = match tag {
        None => "HEAD".to_string(),
        Some(t) => format!("{t}..HEAD"),
    };
    let format = format!("--format=%h{US}%s{US}%b{RS}");
    sh.try_capture("git", &["log", &format, &range])
        .split(RS)
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let mut parts = entry.split(US);
            let sha = parts.next().unwrap_or_default().to_string();
            let subject = parts.next().unwrap_or_default().to_string();
            let body = parts.next().unwrap_or_default();
            let facts = subject_facts(&subject);
            Commit {
                sha,
                breaking: facts.breaking || body_breaks(body),
                behavioural: behavioural_trailers(body),
                kind: facts.kind,
                subject,
            }
        })
        .collect()
}

#[derive(Debug, PartialEq)]
pub struct Candidate {
    pub spec: &'static str,
    pub version: Option<String>,
    pub why: Option<String>,
}

/// What each spec would move to, or why it cannot. Every option is shown,
/// including the closed ones — an option being closed is part of the answer.
pub fn candidates(current: &Version, floor: &Version, tag: Option<&str>) -> Vec<Candidate> {
    ["patch", "minor", "major", "as-is"]
        .into_iter()
        .map(|spec| match version::bump(current, spec, Some(floor)) {
            Ok(next) => Candidate {
                spec,
                version: Some(next.to_string()),
                why: None,
            },
            Err(e) => {
                let why = if spec != "as-is" {
                    e.lines().next().unwrap_or_default().to_string()
                } else if let Some(tag) = tag {
                    format!("the manifest holds {floor}, which {tag} already names")
                } else {
                    "nothing is tagged, so there is no committed bump for it to release".to_string()
                };
                Candidate {
                    spec,
                    version: None,
                    why: Some(why),
                }
            }
        })
        .collect()
}

/// How wide to wrap. With no terminal to ask — piped, or read by an agent —
/// eighty.
fn columns() -> usize {
    match terminal_size::terminal_size() {
        Some((terminal_size::Width(w), _)) if w >= 50 => usize::from(w).min(100),
        _ => 80,
    }
}

/// The proposal, printed. Writes nothing, touches no remote, and needs no
/// git-cliff: a proposal that had to install a tool to be read would be one
/// people skip.
pub fn report(sh: &Sh, config: &Config) -> Result<()> {
    let (name, _) = config.release_identity()?;
    let current = version::read(sh, config)?;
    let floor = version::floor(sh, &current)?;
    let tag = version::latest_tag(sh)?;
    let commits = commits_since(sh, tag.as_deref());

    let head = match &tag {
        None => format!("{name} is at {current}, and has no release tag yet"),
        Some(t) if version::compare(&current, &floor) == Ordering::Greater => {
            format!(
                "{name} is at {current} in the manifest, above {t} — a bump committed and not released"
            )
        }
        Some(t) => format!("{name} is at {current}; {t} is the highest tag"),
    };
    println!("\n{}\n", sh.bold(&head));

    for c in candidates(&current, &floor, tag.as_deref()) {
        let spec = format!("{:<8}", c.spec);
        match c.version {
            Some(v) => println!("  {spec}{v}"),
            None => println!(
                "{}",
                sh.dim(&format!("  {spec}—  {}", c.why.unwrap_or_default()))
            ),
        }
    }

    let count = commits.len();
    let since = match &tag {
        None => "in the repository".to_string(),
        Some(t) => format!("since {t}"),
    };
    if count == 0 {
        println!("\nNo commits {since}.");
        if !config.allow_empty_release {
            println!("{name} refuses an empty release, so there is nothing to cut.");
        }
    } else {
        let feats = commits
            .iter()
            .filter(|c| c.kind.as_deref() == Some("feat"))
            .count();
        let fixes = commits
            .iter()
            .filter(|c| c.kind.as_deref() == Some("fix"))
            .count();
        println!(
            "\n{count} commits {since} — {feats} feat, {fixes} fix, {} other",
            count - feats - fixes
        );

        let breaks: Vec<&Commit> = commits.iter().filter(|c| c.breaking).collect();
        let behavioural: Vec<&Commit> = commits
            .iter()
            .filter(|c| !c.behavioural.is_empty())
            .collect();
        if !breaks.is_empty() {
            println!(
                "\n{}",
                sh.red(&format!("{} say they break the promise:", breaks.len()))
            );
            for c in &breaks {
                println!("  {}  {}", c.sha, c.subject);
            }
        }
        if !behavioural.is_empty() {
            let recorded: usize = behavioural.iter().map(|c| c.behavioural.len()).sum();
            println!("\n{recorded} behavioural change(s) recorded:");
            // Each is a paragraph written for someone deciding whether to
            // upgrade, which is this reader — so printed whole, never cut.
            let width = columns();
            for c in &behavioural {
                for value in &c.behavioural {
                    println!("  {}  {}", c.sha, hanging(value, width - 11, "           "));
                }
            }
        }
        // Said out loud, because an absence is the weakest evidence here and
        // the easiest to read as a green light.
        if breaks.is_empty() && behavioural.is_empty() {
            println!(
                "\nNothing is marked breaking and no `Behavioural-change:` trailer was recorded —\nwhich is what the commits say, and not a proof that nothing moved."
            );
        }
    }

    println!(
        "\nNothing has been written. The version is yours to name:\n\n    ship patch | minor | major | <x.y.z[-pre]> | as-is\n\n`ship changelog` prints the unreleased region in full."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(kind: Option<&str>, breaking: bool) -> SubjectFacts {
        SubjectFacts {
            kind: kind.map(str::to_string),
            breaking,
        }
    }

    #[test]
    fn subjects() {
        assert_eq!(
            subject_facts("feat(check)!: a thing"),
            facts(Some("feat"), true)
        );
        assert_eq!(subject_facts("fix: a thing"), facts(Some("fix"), false));
        assert_eq!(
            subject_facts("feat(a/b-c): a thing"),
            facts(Some("feat"), false)
        );
        assert_eq!(subject_facts("Merge branch 'main'"), facts(None, false));
        assert_eq!(
            subject_facts("just some words: with a colon"),
            facts(None, false)
        );
    }

    #[test]
    fn breaking_footers() {
        assert!(body_breaks("BREAKING CHANGE: it moved"));
        assert!(body_breaks("a paragraph\n\nBREAKING-CHANGE: it moved"));
        assert!(!body_breaks(
            "a paragraph mentioning a BREAKING CHANGE: inline"
        ));
    }

    #[test]
    fn trailers_fold_and_end() {
        let folded = behavioural_trailers(
            "Some body.\n\nBehavioural-change: a span that used to cover the whole node\nnow covers its head only, so a caller measuring one\nsees it shrink.\n",
        );
        assert_eq!(
            folded,
            [
                "a span that used to cover the whole node now covers its head only, so a caller measuring one sees it shrink."
            ]
        );
        let two = behavioural_trailers(
            "Behavioural-change: the first one\ncarries on.\nBehavioural-change: the second.\n\nCloses: docs/tasks/a.md\n",
        );
        assert_eq!(two, ["the first one carries on.", "the second."]);
        assert_eq!(
            behavioural_trailers("behavioral-change: counted"),
            ["counted"]
        );
        assert_eq!(
            behavioural_trailers("BEHAVIOURAL-CHANGE: counted"),
            ["counted"]
        );
        assert!(behavioural_trailers("Not-behavioural-change: ignored").is_empty());
        assert!(behavioural_trailers("Behavioural-change:").is_empty());
        // A multibyte character where the key would end is not a trailer, and
        // not a panic either.
        assert!(behavioural_trailers("See the spec, § 4.2 and on").is_empty());
    }

    #[test]
    fn every_option_at_once() {
        let v = |a, b, c| Version::new(a, b, c);
        let open = candidates(&v(0, 4, 2), &v(0, 4, 2), Some("v0.4.2"));
        let specs: Vec<&str> = open.iter().map(|c| c.spec).collect();
        assert_eq!(specs, ["patch", "minor", "major", "as-is"]);
        let versions: Vec<Option<&str>> = open.iter().map(|c| c.version.as_deref()).collect();
        assert_eq!(
            versions,
            [Some("0.4.3"), Some("0.5.0"), Some("1.0.0"), None]
        );
        assert_eq!(
            open[3].why.as_deref(),
            Some("the manifest holds 0.4.2, which v0.4.2 already names")
        );
        assert_eq!(
            candidates(&v(0, 5, 0), &v(0, 4, 2), Some("v0.4.2"))[3]
                .version
                .as_deref(),
            Some("0.5.0")
        );
        assert_eq!(
            candidates(&v(0, 1, 0), &v(0, 1, 0), None)[3].why.as_deref(),
            Some("nothing is tagged, so there is no committed bump for it to release")
        );
        let rc = Version {
            release: [1, 0, 0],
            pre: Some("rc.1".into()),
        };
        let pre = candidates(&rc, &v(0, 9, 0), Some("v0.9.0"));
        let versions: Vec<Option<&str>> = pre.iter().map(|c| c.version.as_deref()).collect();
        assert_eq!(versions, [None, None, None, Some("1.0.0-rc.1")]);
        assert!(
            pre[0]
                .why
                .as_deref()
                .unwrap()
                .contains("has no meaning from the pre-release")
        );
    }

    #[test]
    fn hanging_indent() {
        assert_eq!(
            hanging("one two three four five", 9, "  "),
            "one two\n  three\n  four five"
        );
        assert_eq!(hanging("", 20, "  "), "");
        assert_eq!(
            hanging("a supercalifragilistic word", 6, "  "),
            "a\n  supercalifragilistic\n  word"
        );
    }
}
