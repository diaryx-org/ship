//! ship: cut a repository's release and run its CI, from one config file.
//!
//! Every command works on one repository — whichever the working directory is
//! inside — and reads its `.config/ship.figl`.

mod cargo;
mod changelog;
mod ci;
mod config;
mod cut;
mod line;
mod parallel;
mod propose;
mod sh;
mod version;

use std::process::ExitCode;

use config::Config;
use sh::Sh;

/// Anything that goes wrong is a message for whoever is reading the terminal:
/// a first line that says what, and a `hint:` line, where there is one, that
/// says what to do.
pub type Result<T> = std::result::Result<T, String>;

const ONELINE: &str = "ship: cut a repository's release and run its CI from .config/ship.figl: propose a version, bump, regenerate the changelog, commit, tag, push, write the GitHub release; `ship ci` runs the CI jobs. More: `ship help`.";

const USAGE: &str = "ship — cut a repository's release, and run its CI.

usage: ship [-p <crate>] [<spec>]     ship <command> --help for its flags and notes

  inside one repository:

    ship                 what each version would be, and what the commits say — cuts nothing
    ship <spec>          bump, changelog, commit, tag — and push only with --push (--no-verify)
    version              what the repository calls itself
    bump [<spec>]        move to patch | minor | major | x.y.z[-pre] | as-is
    changelog            regenerate the unreleased region (--write, --check)
    notes [tag]          that release's changelog section, for the GitHub release body
    github [tag]         write that section to the GitHub release
    ci [<job>]           run the config's CI jobs, or one of them (--parallel, --list, --matrix)

    help [--oneline]     this, or one line of it

  per crate, where the config says `per_crate = true`:

    -p <crate>           which crate a command acts on; `ship` alone lists the
                         crates with unreleased changes, and `version` and
                         `changelog` alone act on every crate

  worth knowing:

    - `ship` with no version only proposes. The version is the release
      owner's call, not the tool's. Named, it stops at a local tag unless --push.
    - A CI job is an id, a name and a command; its `runner` table is the
      workflow's, and not read here. Anything more than a command is a
      `cargo xtask` task a job runs.

Configuration is .config/ship.figl at the repository root; a repository from
before it keeps .config/release.toml and .config/ci.toml, which ship reads when
there is no ship.figl.";

fn help_for(command: &str) -> Option<&'static str> {
    Some(match command {
        "release" => {
            "ship [-p <crate>] [<spec>] [--push] [--no-verify] [--propose]

Bump, regenerate the changelog, commit, tag — and push only with --push.

With no spec, or with --propose, it is the proposal: every version the specs
would move to, what the commits since the last tag say about the promise, and
nothing written. The version is named by whoever owns the release.

  <spec>        patch | minor | major | X.Y.Z[-pre] | as-is
  -p <crate>    per crate, the crate to release; without one, the crates
                with commits since their last release
  --propose     print the proposal even with a spec, and cut nothing
  --push        push the commit and tag, and write the GitHub release
  --no-verify   skip the repository's CI before the cut"
        }
        "version" => {
            "ship version [-p <crate>]\n\nWhat the repository calls itself: the version in its manifest. Per crate, each\ncrate's, or the one named."
        }
        "bump" => {
            "ship bump [-p <crate>] [<spec>] [--propose]\n\nMove the version in every manifest that carries it, and nothing else. With no\nspec, the proposal."
        }
        "changelog" => {
            "ship changelog [-p <crate>] [--write | --check]\n\nThe unreleased region, rendered by git-cliff: printed, spliced into the\nchangelog (--write), or checked current (--check). --write and --check also\ngive any tag still without a section one. Per crate, every crate's changelog,\nor the one named."
        }
        "notes" => {
            "ship notes [-p <crate>] [<tag>]\n\nThat release's changelog section, as the GitHub release body. Default: the\nversion in the manifest. Per crate, the tag says whose it is."
        }
        "github" => {
            "ship github [-p <crate>] [<tag>]\n\nWrite that section to the GitHub release, creating it or editing one something\nelse made. `ship <spec> --push` does this as its last step; this is for a tag\npushed by hand, or a push whose gh call did not land."
        }
        "ci" => {
            "ship ci [<job>] [--parallel | --list | --matrix]

Run the config's CI jobs in order, stopping at the first failure — or the one
named. Each runs under `bash -eo pipefail -c` at the repository root, the shell
GitHub gives a `run:` step, so a job means the same thing here and there. A job
that says `local = false` runs only when named.

  --parallel    every job at once, each into <git dir>/ship/ci/<id>.log; a job
                with `after` waits for that one. The first to fail stops the
                rest. `ci_parallel = true` in the config makes it the default
  --list        the jobs, and run nothing
  --matrix      the jobs as one line of JSON for a workflow's matrix, each
                job's `runner` table flattened beside its id, name and run;
                a job that says `matrix = false` is left out"
        }
        _ => return None,
    })
}

struct Args {
    positional: Vec<String>,
    flags: Vec<String>,
}

fn split(args: &[String]) -> Args {
    let (flags, positional) = args.iter().cloned().partition(|a| a.starts_with('-'));
    Args { positional, flags }
}

impl Args {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
    /// Refuse a flag the command does not take, and more positionals than it
    /// takes — a typo should not run something.
    fn only(&self, command: &str, flags: &[&str], positionals: usize) -> Result<()> {
        let spelled = if command == "release" {
            "ship".to_string()
        } else {
            format!("ship {command}")
        };
        if let Some(f) = self.flags.iter().find(|f| !flags.contains(&f.as_str())) {
            bail!("`{spelled}` does not take {f}\nhint: {spelled} --help");
        }
        if self.positional.len() > positionals {
            bail!("`{spelled}` takes at most {positionals} argument(s)\nhint: {spelled} --help");
        }
        Ok(())
    }
    fn first(&self) -> Option<&str> {
        self.positional.first().map(String::as_str)
    }
}

/// `-p <crate>`, `--package <crate>` or `--package=<crate>`, taken out of the
/// arguments before they are split, so its value is not read as a spec.
fn take_package(args: &[String]) -> Result<(Option<String>, Vec<String>)> {
    let mut selector = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = match arg.as_str() {
            "-p" | "--package" => match it.next() {
                Some(v) if !v.starts_with('-') => v.clone(),
                _ => bail!("{arg} needs a crate\nhint: ship -p <crate>"),
            },
            a if a.starts_with("--package=") => a["--package=".len()..].to_string(),
            _ => {
                rest.push(arg.clone());
                continue;
            }
        };
        if selector.replace(value).is_some() {
            bail!("one crate at a time: -p is given twice");
        }
    }
    Ok((selector, rest))
}

/// The line a `notes` or `github` acts on: the one named, else the one the
/// tag belongs to, else — at one version — the repository.
fn line_for_tag(
    sh: &Sh,
    config: &Config,
    selector: Option<&str>,
    tag: Option<&str>,
) -> Result<line::Line> {
    if let (None, Some(tag), true) = (selector, tag, config.per_crate) {
        let all = line::lines(sh, config)?;
        return match line::for_tag(&all, tag) {
            Some(found) => Ok(found),
            None => bail!(
                "{tag} is not a tag of any crate here\nhint: a crate's tags are <crate>-v<x.y.z>, and the workspace's v<x.y.z>"
            ),
        };
    }
    line::select(sh, config, selector)
}

fn run(args: Vec<String>) -> Result<()> {
    let command = args.first().map(String::as_str).unwrap_or("");
    let is_command = matches!(
        command,
        "version" | "bump" | "changelog" | "notes" | "github" | "ci" | "help"
    );

    if matches!(command, "-h" | "--help" | "help") {
        let rest = split(&args[1..]);
        if rest.has("--oneline") {
            println!("{ONELINE}");
        } else if let Some(text) = rest.first().and_then(help_for) {
            println!("{text}");
        } else {
            println!("{USAGE}");
        }
        return Ok(());
    }
    if command == "--version" {
        println!("ship {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let (name, rest) = if is_command {
        (command, &args[1..])
    } else {
        ("release", &args[..])
    };
    let (selector, rest) = take_package(rest)?;
    let rest = split(&rest);
    let selector = selector.as_deref();
    if rest.has("--help") || rest.has("-h") {
        println!("{}", help_for(name).unwrap_or(USAGE));
        return Ok(());
    }

    let sh = Sh::new()?;
    let config = Config::load(&sh)?;
    // Per crate, with no crate named, a proposal is the list of crates.
    let propose = |sh: &Sh, config: &Config| {
        if config.per_crate && selector.is_none() {
            propose::summary(sh, config, &line::lines(sh, config)?)
        } else {
            propose::report(sh, config, &line::select(sh, config, selector)?)
        }
    };
    if selector.is_some() && name == "ci" {
        bail!("`ship ci` runs the repository's jobs, and takes no crate");
    }
    match name {
        "release" => {
            rest.only(name, &["--propose", "--push", "--no-verify"], 1)?;
            match rest.first() {
                Some(spec) if !rest.has("--propose") => cut::release(
                    &sh,
                    &config,
                    &line::select(&sh, &config, selector)?,
                    spec,
                    rest.has("--push"),
                    rest.has("--no-verify"),
                ),
                _ => {
                    // A proposal pushes nothing, so `--push` here cannot be
                    // honoured — refused rather than dropped, since the
                    // alternative is reading a proposal as a release.
                    if rest.has("--push") {
                        bail!(
                            "--push needs a version named: a proposal writes nothing\nhint: ship <spec> --push"
                        );
                    }
                    propose(&sh, &config)
                }
            }
        }
        "version" => {
            rest.only(name, &[], 0)?;
            if config.per_crate && selector.is_none() {
                for l in line::lines(&sh, &config)? {
                    println!("{} {}", l.name, version::read(&sh, &config, &l)?);
                }
            } else {
                let l = line::select(&sh, &config, selector)?;
                println!("{}", version::read(&sh, &config, &l)?);
            }
            Ok(())
        }
        "bump" => {
            rest.only(name, &["--propose"], 1)?;
            match rest.first() {
                Some(spec) if !rest.has("--propose") => {
                    let l = line::select(&sh, &config, selector)?;
                    let current = version::read(&sh, &config, &l)?;
                    let floor = version::floor(&sh, &l, &current)?;
                    let next = version::bump(&current, spec, Some(&floor))?;
                    if version::compare(&next, &current) == std::cmp::Ordering::Equal {
                        println!("{current}, as is — nothing to bump");
                        return Ok(());
                    }
                    println!("{current} -> {next}");
                    version::write(&sh, &config, &l, &next)
                }
                _ => propose(&sh, &config),
            }
        }
        "changelog" => {
            rest.only(name, &["--write", "--check"], 0)?;
            let mode = match (rest.has("--write"), rest.has("--check")) {
                (true, true) => bail!("--write and --check are opposites; pass one"),
                (true, false) => changelog::Mode::Write,
                (false, true) => changelog::Mode::Check,
                (false, false) => changelog::Mode::Print,
            };
            if config.per_crate && selector.is_none() {
                // Every crate, and every failure said, before the first fails
                // the command: a check in CI should name all that are stale.
                let mut failed = Vec::new();
                for l in line::lines(&sh, &config)? {
                    if let changelog::Mode::Print = mode {
                        println!("{}", sh.bold(&format!("{} — {}", l.name, l.changelog)));
                    }
                    if let Err(e) = changelog::check_or_write(&sh, &config, &l, &mode) {
                        eprintln!("error: {}: {e}", l.name);
                        failed.push(l.name);
                    }
                }
                if !failed.is_empty() {
                    bail!("the changelog of {} did not pass", failed.join(", "));
                }
                Ok(())
            } else {
                let l = line::select(&sh, &config, selector)?;
                changelog::check_or_write(&sh, &config, &l, &mode)
            }
        }
        "notes" => {
            rest.only(name, &[], 1)?;
            let l = line_for_tag(&sh, &config, selector, rest.first())?;
            println!("{}", changelog::notes_text(&sh, &config, &l, rest.first())?);
            Ok(())
        }
        "github" => {
            rest.only(name, &[], 1)?;
            let l = line_for_tag(&sh, &config, selector, rest.first())?;
            cut::github(&sh, &config, &l, rest.first())
        }
        "ci" => {
            rest.only(name, &["--parallel", "--list", "--matrix"], 1)?;
            if rest.has("--list") {
                ci::list(&config)
            } else if rest.has("--matrix") {
                println!("{}", ci::matrix(&config)?);
                Ok(())
            } else if let Some(id) = rest.first() {
                ci::run_one(&sh, &config, id)
            } else {
                ci::run_all(&sh, &config, rest.has("--parallel"))
            }
        }
        _ => unreachable!("every command is matched above"),
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let mut lines = message.lines();
            eprintln!("error: {}", lines.next().unwrap_or_default());
            for line in lines {
                eprintln!("{line}");
            }
            ExitCode::FAILURE
        }
    }
}
