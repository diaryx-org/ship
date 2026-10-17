//! `ship ci --parallel`: every job at once, each into a log of its own.
//!
//! In order, a job's output streams to the terminal, and that is the right
//! default — one job is what someone is watching. A repository whose jobs
//! spend most of their time somewhere other than one shared lock (a prover, a
//! simulator, test binaries, a second toolchain) finishes the whole list in
//! the time of its longest job instead, and that is what this is for.
//!
//! Each job is `bash -eo pipefail -c <run>` at the root, the same shell as in
//! order, with stdout and stderr into `<git dir>/ship/ci/<id>.log` — inside the
//! git directory so a run never dirties the tree a release is about to be cut
//! from. A job with `after` starts once that job has passed, with
//! `SHIP_CI_AFTER` set to say so. The first job to fail stops the rest, and
//! its log's tail is printed.
//!
//! Each job leads a process group of its own, so that stopping it stops the
//! cargo or rustc under it too rather than leaving them running against the
//! build directory. A process group is out of the terminal's reach, so Ctrl-C
//! reaches ship alone, and ship stops every group it started.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::Job;
use crate::sh::{self, Sh};
use crate::{Result, bail};

/// How much of a failed job's log is printed: enough for a compiler error or
/// a failed test's panic, and the whole log is a path away.
const TAIL: usize = 60;

struct Running<'a> {
    job: &'a Job,
    log: PathBuf,
    child: Child,
    started: Instant,
}

pub fn run(sh: &Sh, jobs: &[&Job]) -> Result<()> {
    let logs = log_dir(sh)?;
    // Last run's logs go, so a log here is always this run's.
    let _ = fs::remove_dir_all(&logs);
    fs::create_dir_all(&logs).map_err(|e| format!("could not create {}: {e}", logs.display()))?;

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    // Only ever set once per process, and this runs once.
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));

    sh.step(&format!("{} jobs at once", jobs.len()));
    println!(
        "{}",
        sh.dim(&format!(
            "each one's output is in {}/<id>.log",
            shown(sh, &logs)
        ))
    );
    let width = jobs.iter().map(|j| j.id.len()).max().unwrap_or(0);

    let in_run = |id: &str| jobs.iter().any(|j| j.id == id);
    let mut pending: Vec<&Job> = jobs.to_vec();
    let mut running: Vec<Running> = Vec::new();
    let mut passed: Vec<&str> = Vec::new();
    let mut failed = None;
    while failed.is_none() && !(pending.is_empty() && running.is_empty()) {
        if interrupted.load(Ordering::SeqCst) {
            stop_all(sh, running);
            bail!("interrupted; every job still running was stopped");
        }
        let (ready, waiting): (Vec<&Job>, Vec<&Job>) = pending.into_iter().partition(|j| {
            j.after
                .as_deref()
                .is_none_or(|a| passed.contains(&a) || !in_run(a))
        });
        pending = waiting;
        if ready.is_empty() && running.is_empty() && !pending.is_empty() {
            // `ci::check` keeps `after` to an earlier job in the same run, so
            // this is a bug rather than a config to correct.
            bail!("nothing left to run can start: every job still waiting is waiting on another");
        }
        for job in ready {
            let after_passed = job.after.as_deref().is_some_and(|a| passed.contains(&a));
            match start(sh, &logs, job, after_passed) {
                Ok(r) => running.push(r),
                Err(e) => {
                    stop_all(sh, running);
                    return Err(e);
                }
            }
        }

        let mut i = 0;
        while i < running.len() {
            let status = match running[i].child.try_wait() {
                Ok(None) => {
                    i += 1;
                    continue;
                }
                Ok(Some(status)) => status,
                Err(e) => {
                    let id = running[i].job.id.clone();
                    stop_all(sh, running);
                    bail!("could not wait for `{id}`: {e}");
                }
            };
            let done = running.swap_remove(i);
            let secs = done.started.elapsed().as_secs_f64();
            if status.success() {
                println!(
                    "  {}  {:<width$}  {secs:>6.1}s",
                    sh.green("passed"),
                    done.job.id
                );
                passed.push(&done.job.id);
            } else {
                println!(
                    "  {}  {:<width$}  {secs:>6.1}s  ({})",
                    sh.red("FAILED"),
                    done.job.id,
                    sh::exit(&status)
                );
                failed = Some(done);
                break;
            }
        }
        if failed.is_none() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let Some(done) = failed else { return Ok(()) };
    stop_all(sh, running);
    let text = fs::read_to_string(&done.log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    eprintln!(
        "\n{}",
        sh.bold(&format!("── the last of {} ──", shown(sh, &done.log)))
    );
    for line in &lines[lines.len().saturating_sub(TAIL)..] {
        eprintln!("{line}");
    }
    bail!(
        "`{}` failed; its whole log is {}\nhint: ship ci {} runs it alone, its output streaming",
        done.job.name,
        shown(sh, &done.log),
        done.job.id
    )
}

/// `<git dir>/ship/ci`, which git resolves for a worktree too.
fn log_dir(sh: &Sh) -> Result<PathBuf> {
    let path = sh.capture("git", &["rev-parse", "--git-path", "ship/ci"])?;
    Ok(sh.root.join(path.trim()))
}

fn start<'a>(sh: &Sh, logs: &Path, job: &'a Job, after_passed: bool) -> Result<Running<'a>> {
    let log = logs.join(format!("{}.log", job.id));
    let mut out =
        File::create(&log).map_err(|e| format!("could not create {}: {e}", log.display()))?;
    // The same transcript line the job would print in order, so the log says
    // what ran.
    writeln!(out, "$ {}", job.run.trim_end())
        .map_err(|e| format!("could not write {}: {e}", log.display()))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("could not write {}: {e}", log.display()))?;
    let mut cmd = Command::new("bash");
    cmd.args(["-eo", "pipefail", "-c", &job.run])
        .current_dir(&sh.root)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    if let (Some(after), true) = (&job.after, after_passed) {
        cmd.env(crate::ci::AFTER_VAR, after);
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not run bash for `{}`: {e}", job.id))?;
    Ok(Running {
        job,
        log,
        child,
        started: Instant::now(),
    })
}

fn stop_all(sh: &Sh, running: Vec<Running>) {
    for mut r in running {
        stop(&mut r.child);
        println!("  {}  {}", sh.dim("stopped"), r.job.id);
    }
}

/// Stop a job and everything under it: TERM to its group, a moment to go,
/// then KILL.
fn stop(child: &mut Child) {
    let group = format!("-{}", child.id());
    signal("TERM", &group);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            // The job's bash is gone; anything it left in the group is not
            // owed the same patience.
            signal("KILL", &group);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal("KILL", &group);
    let _ = child.kill();
    let _ = child.wait();
}

fn signal(name: &str, group: &str) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args([&format!("-{name}"), "--", group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(not(unix))]
    let _ = (name, group);
}

/// A path as the reader would type it: relative to the root where it is
/// under it.
fn shown(sh: &Sh, path: &Path) -> String {
    path.strip_prefix(&sh.root)
        .unwrap_or(path)
        .display()
        .to_string()
}
