//! Running things at the repository root, and printing what ran.
//!
//! Every command that changes something is echoed before it runs, so a log
//! reads as a transcript anyone can paste back. Commands that only ask a
//! question — a branch name, a tag list — are not: a log of them is noise
//! between the commands that did something.
//!
//! The root comes from git rather than from where `ship` was invoked, so every
//! read, write and command lands at the top of the checkout whichever
//! directory inside it you are standing in.

use std::env;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Result;

/// A failure for whoever is reading the terminal. The first line is the
/// error; a `hint:` line, if there is one, is what to do about it.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err(format!($($arg)*)) };
}

/// The repository root, and how to print.
pub struct Sh {
    pub root: PathBuf,
    color: bool,
}

impl Sh {
    /// The checkout the working directory is inside.
    ///
    /// Not being in one is something a person hits by standing in the wrong
    /// directory rather than by anything being wrong, and git's own wording
    /// does not say that.
    pub fn new() -> Result<Self> {
        let out = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("could not run git: {e}\nhint: ship needs git on PATH"))?;
        if !out.status.success() {
            let here = env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            bail!(
                "{here} is not inside a git repository\nhint: ship works on one repository — run it from inside the checkout"
            );
        }
        let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        Ok(Sh {
            root,
            color: color(),
        })
    }

    fn at(&self, cwd: Option<&str>) -> PathBuf {
        match cwd {
            None => self.root.clone(),
            Some(dir) => self.root.join(dir),
        }
    }

    /// Run a command at the root, echoed first. Its output is kept out of the
    /// way unless it fails, and then what it said on stderr is the error.
    pub fn call(&self, prog: &str, args: &[&str]) -> Result<()> {
        self.call_in(None, prog, args)
    }

    /// The same, in a directory below the root — for the one thing that
    /// genuinely has a working directory, a Cargo workspace nested inside a
    /// repository whose canonical manifest is elsewhere. Echoed with it,
    /// because a reader pasting the transcript back needs to know where.
    pub fn call_in(&self, cwd: Option<&str>, prog: &str, args: &[&str]) -> Result<()> {
        let prefix = cwd.map(|d| format!("(cd {d}) ")).unwrap_or_default();
        println!(
            "{}",
            self.dim(&format!("$ {prefix}{prog} {}", args.join(" ")))
        );
        let out = Command::new(prog)
            .args(args)
            .current_dir(self.at(cwd))
            .output()
            .map_err(|e| format!("could not run `{prog}`: {e}"))?;
        if !out.status.success() {
            bail!(
                "`{prog} {}` failed ({})\n{}",
                args.join(" "),
                exit(&out.status),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// A command's stdout, for an answer a task acts on. Not echoed.
    pub fn capture(&self, prog: &str, args: &[&str]) -> Result<String> {
        self.capture_in(None, prog, args)
    }

    pub fn capture_in(&self, cwd: Option<&str>, prog: &str, args: &[&str]) -> Result<String> {
        let out = Command::new(prog)
            .args(args)
            .current_dir(self.at(cwd))
            .output()
            .map_err(|e| format!("could not run `{prog}`: {e}"))?;
        if !out.status.success() {
            bail!(
                "`{prog} {}` failed ({})\n{}",
                args.join(" "),
                exit(&out.status),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Like `capture`, but empty where the command failed — for the questions
    /// whose failure is itself an answer.
    pub fn try_capture(&self, prog: &str, args: &[&str]) -> String {
        self.try_capture_env(prog, args, &[])
    }

    pub fn try_capture_env(&self, prog: &str, args: &[&str], envs: &[(&str, &str)]) -> String {
        Command::new(prog)
            .args(args)
            .envs(envs.iter().copied())
            .current_dir(&self.root)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    /// Whether a command succeeds — for questions answered by an exit code.
    /// `gh auth status` says what it says on stderr, so its stdout is no
    /// answer.
    pub fn ok(&self, prog: &str, args: &[&str]) -> bool {
        Command::new(prog)
            .args(args)
            .current_dir(&self.root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A command with `input` on its stdin, echoed with a note in place of the
    /// input — a release body of several kilobytes has no place in the log.
    pub fn call_with_stdin(&self, prog: &str, args: &[&str], input: &str) -> Result<String> {
        println!(
            "{}",
            self.dim(&format!(
                "$ {prog} {}   (with the notes on stdin)",
                args.join(" ")
            ))
        );
        let mut child = Command::new(prog)
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run `{prog}`: {e}"))?;
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(input.as_bytes())
            .map_err(|e| format!("could not write to `{prog}`: {e}"))?;
        let out = child
            .wait_with_output()
            .map_err(|e| format!("`{prog}` did not finish: {e}"))?;
        if !out.status.success() {
            bail!(
                "`{prog} {}` failed ({})\n{}",
                args.join(" "),
                exit(&out.status),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run a command string the repository named for itself — `verify`,
    /// `post_bump`, a CI job — under `bash -eo pipefail -c` at the root. That
    /// is GitHub's own default shell for a `run:` step, so a command means the
    /// same thing here and on a runner. Its output streams: a test suite's
    /// progress is the thing someone is watching, and its failure is the thing
    /// they need to read.
    pub fn shell(&self, command: &str) -> Result<()> {
        self.shell_env(command, &[])
    }

    /// The same, with variables set for it.
    pub fn shell_env(&self, command: &str, envs: &[(&str, &str)]) -> Result<()> {
        println!("{}", self.dim(&format!("$ {}", command.trim_end())));
        let status = Command::new("bash")
            .args(["-eo", "pipefail", "-c", command])
            .envs(envs.iter().copied())
            .current_dir(&self.root)
            .status()
            .map_err(|e| format!("could not run bash: {e}"))?;
        if !status.success() {
            bail!("`{}` failed ({})", first_line(command), exit(&status));
        }
        Ok(())
    }

    /// Fail early, with the install line, when a tool the task needs is
    /// missing — rather than halfway through a release, the version already
    /// bumped.
    pub fn require(&self, prog: &str, hint: &str) -> Result<()> {
        if which(prog).is_none() {
            bail!("`{prog}` not found on PATH\nhint: {hint}");
        }
        Ok(())
    }

    pub fn read(&self, path: &str) -> Result<String> {
        fs::read_to_string(self.root.join(path)).map_err(|e| format!("could not read {path}: {e}"))
    }

    pub fn write(&self, path: &str, contents: &str) -> Result<()> {
        fs::write(self.root.join(path), contents)
            .map_err(|e| format!("could not write {path}: {e}"))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }

    /// A heading in the transcript, between the phases of a run.
    pub fn step(&self, title: &str) {
        println!("\n{}", self.paint("1", &format!("━━ {title} ━━")));
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("90", text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// Colour on a terminal, off in a pipe or a log file, and off whenever
/// `NO_COLOR` says so. `CLICOLOR_FORCE` turns it back on for a CI log that
/// renders it.
fn color() -> bool {
    if env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    std::io::stdout().is_terminal()
}

pub fn exit(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".to_string(),
    }
}

fn first_line(command: &str) -> String {
    let mut lines = command.trim().lines();
    let first = lines.next().unwrap_or_default();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

/// The program `prog` names on PATH, as a shell would find it.
pub fn which(prog: &str) -> Option<PathBuf> {
    if prog.contains('/') {
        return Path::new(prog).is_file().then(|| PathBuf::from(prog));
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(prog))
        .find(|candidate| candidate.is_file())
}
