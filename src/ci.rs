//! A repository's CI, as data: the `job` list in its config.
//!
//! The same list is read twice. `ship ci` runs the jobs in order on this
//! machine; on GitHub, the workflow asks `ship ci --matrix` for them and runs
//! one per runner, each as `ship ci <id>`. One reader, so there is nothing for
//! two to disagree about.
//!
//! A job is an id, a name and a command, and a `runner` table that is the
//! workflow's alone: components, a toolchain, targets, a runner image, whatever
//! that repository's workflow sets up before the command can start. `--matrix`
//! flattens it beside the job's own fields and ship otherwise ignores it,
//! because a run here is on this machine. So the shared contract stays three
//! fields however much a runner needs.
//!
//! Three more say where and when, and are ship's: `local = false` leaves a job
//! out of `ship ci` with no job named (one too slow for every local run, which
//! a runner can afford), `matrix = false` leaves it out of `--matrix` (one only
//! a developer's machine can run), and `after` names an earlier job it waits
//! for when the jobs run at once — `--parallel`, or `ci_parallel = true`; see
//! `parallel.rs`. A job whose `after` has passed in the same run, at once or in
//! order, gets `SHIP_CI_AFTER=<that id>`, so it can trust what that job left
//! behind; named alone, it does not, and should not.
//!
//! Anything that needs more than a command — packaging, a wasm build, a
//! simulator — is a `cargo xtask` task a job names. This is a list, not a CI
//! language.

use crate::config::{Config, Job};
use crate::sh::Sh;
use crate::{Result, bail};

/// The checks every job list passes, wherever it was read from.
pub fn check(jobs: &[Job], source: &str) -> Result<()> {
    let mut seen: Vec<&str> = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        let place = format!("{source}'s job {}", i + 1);
        for (field, value) in [("id", &job.id), ("name", &job.name), ("run", &job.run)] {
            if value.trim().is_empty() {
                bail!("{place} has no `{field}`");
            }
        }
        // The matrix is JSON and the check name a URL segment, so an id is a
        // word; `all` would read as running every job.
        let usable = job
            .id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && job
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && job.id != "all";
        if !usable {
            bail!(
                "{place}'s id `{}` is not a usable id\nhint: lowercase letters, digits and `-`, and not `all`",
                job.id
            );
        }
        let clash: Vec<&str> = ["id", "name", "run"]
            .into_iter()
            .filter(|k| job.runner.contains_key(*k))
            .collect();
        if !clash.is_empty() {
            bail!(
                "{place}'s runner sets {}, which would overwrite the job's own in the matrix",
                clash.join(", ")
            );
        }
        if seen.contains(&job.id.as_str()) {
            bail!("{source} names `{}` more than once", job.id);
        }
        if let Some(after) = &job.after {
            // Earlier, so that the list in order already honours it and no two
            // jobs can wait on each other.
            match jobs[..i].iter().find(|j| &j.id == after) {
                None => bail!(
                    "{place} (`{}`) runs after `{after}`, which is not a job listed before it\nhint: `after` names an earlier job",
                    job.id
                ),
                Some(earlier) if job.local && !earlier.local => bail!(
                    "{place} (`{}`) runs after `{after}`, which `ship ci` leaves out (`local = false`)",
                    job.id
                ),
                Some(_) => {}
            }
        }
        seen.push(&job.id);
    }
    Ok(())
}

fn jobs(config: &Config) -> Result<&[Job]> {
    if config.jobs.is_empty() {
        bail!(
            "{} names no CI jobs\nhint: a `job` list, one entry per job, each with id, name and run",
            config.source
        );
    }
    Ok(&config.jobs)
}

/// What a job learns about the run it is in: `SHIP_CI_AFTER`, where the job
/// it runs after has passed in it.
pub const AFTER_VAR: &str = "SHIP_CI_AFTER";

pub fn run_job(sh: &Sh, job: &Job, after_passed: bool) -> Result<()> {
    sh.step(&job.name);
    let envs: Vec<(&str, &str)> = match &job.after {
        Some(after) if after_passed => vec![(AFTER_VAR, after.as_str())],
        _ => Vec::new(),
    };
    sh.shell_env(&job.run, &envs)
        .map_err(|e| format!("{e}\nhint: ship ci {} runs it alone", job.id))
}

/// Every job but those that say `local = false`, stopping at the first
/// failure — a red job is worth reading before the next one buries it. In
/// order, or at once where `parallel` or the config asks for it.
pub fn run_all(sh: &Sh, config: &Config, parallel: bool) -> Result<()> {
    let (chosen, left_out): (Vec<&Job>, Vec<&Job>) = jobs(config)?.iter().partition(|j| j.local);
    if chosen.is_empty() {
        bail!(
            "every job in {} says `local = false`, so there is nothing to run here\nhint: ship ci <job> runs one by name",
            config.source
        );
    }
    for job in &left_out {
        println!(
            "{}",
            sh.dim(&format!(
                "leaving out {} (`local = false`); `ship ci {}` runs it",
                job.id, job.id
            ))
        );
    }
    if parallel || config.ci_parallel {
        crate::parallel::run(sh, &chosen)?;
    } else {
        // In order, and stopping at the first failure, so an earlier job in
        // the run has passed by the time a later one starts.
        for job in &chosen {
            let after_passed = job
                .after
                .as_deref()
                .is_some_and(|a| chosen.iter().any(|j| j.id == a));
            run_job(sh, job, after_passed)?;
        }
    }
    println!(
        "\n{}",
        sh.green(&format!("all {} jobs passed", chosen.len()))
    );
    Ok(())
}

pub fn run_one(sh: &Sh, config: &Config, id: &str) -> Result<()> {
    let jobs = jobs(config)?;
    let Some(job) = jobs.iter().find(|j| j.id == id) else {
        let ids: Vec<&str> = jobs.iter().map(|j| j.id.as_str()).collect();
        bail!(
            "no job `{id}` in {}\nhint: {}",
            config.source,
            ids.join(", ")
        );
    };
    run_job(sh, job, false)
}

/// The jobs, one per line, and nothing run.
pub fn list(config: &Config) -> Result<()> {
    let jobs = jobs(config)?;
    let id_w = jobs.iter().map(|j| j.id.len()).max().unwrap_or(0);
    let name_w = jobs
        .iter()
        .map(|j| j.name.chars().count())
        .max()
        .unwrap_or(0);
    for j in jobs {
        let mut lines = j.run.trim().lines();
        let first = lines.next().unwrap_or_default();
        let more = if lines.next().is_some() { " …" } else { "" };
        let mut notes = Vec::new();
        if !j.local {
            notes.push("only when named".to_string());
        }
        if !j.matrix {
            notes.push("not in the matrix".to_string());
        }
        if let Some(after) = &j.after {
            notes.push(format!("after {after}"));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join(", "))
        };
        println!("{:<id_w$}  {:<name_w$}  {first}{more}{notes}", j.id, j.name);
    }
    Ok(())
}

/// The workflow's matrix: each job's own fields with its runner table beside
/// them, as one line of JSON for `fromJSON`. A job that says `matrix = false`
/// is not in it; `local` and `after` are this machine's, and not either.
pub fn matrix(config: &Config) -> Result<String> {
    let entries: Vec<String> = jobs(config)?
        .iter()
        .filter(|j| j.matrix)
        .map(|j| {
            let mut fields = vec![
                format!("\"id\":{}", json_string(&j.id)),
                format!("\"name\":{}", json_string(&j.name)),
                format!("\"run\":{}", json_string(&j.run)),
            ];
            for (key, value) in &j.runner {
                fields.push(format!("{}:{}", json_string(key), json_value(value)));
            }
            format!("{{{}}}", fields.join(","))
        })
        .collect();
    Ok(format!("[{}]", entries.join(",")))
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_value(value: &fig::Value) -> String {
    use fig::Value;
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Uint(n) => n.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Float(_) => "null".into(),
        Value::Str(s) => json_string(s),
        Value::Seq(items) => format!(
            "[{}]",
            items.iter().map(json_value).collect::<Vec<_>>().join(",")
        ),
        Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Str(s) => s.clone(),
                        other => json_value(other),
                    };
                    format!("{}:{}", json_string(&key), json_value(v))
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => json_string(&format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn job(id: &str, name: &str, run: &str) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            run: run.into(),
            local: true,
            matrix: true,
            after: None,
            runner: BTreeMap::new(),
        }
    }

    fn after(mut j: Job, id: &str) -> Job {
        j.after = Some(id.into());
        j
    }

    /// `after` is an earlier job in the same run, so the list in order honours
    /// it and nothing can wait on itself.
    #[test]
    fn after_names_an_earlier_job() {
        let gen_ = job("gen", "Generate", "x");
        assert!(check(&[gen_.clone(), after(job("t", "T", "y"), "gen")], "f").is_ok());
        assert!(check(&[after(job("t", "T", "y"), "gen"), gen_.clone()], "f").is_err());
        assert!(check(&[after(job("t", "T", "y"), "t")], "f").is_err());
        let mut remote = gen_.clone();
        remote.local = false;
        assert!(check(&[remote.clone(), after(job("t", "T", "y"), "gen")], "f").is_err());
        let mut also_remote = after(job("t", "T", "y"), "gen");
        also_remote.local = false;
        assert!(check(&[remote, also_remote], "f").is_ok());
    }

    #[test]
    fn matrix_leaves_out_what_says_so() {
        let mut here = job("here", "Here", "x");
        here.matrix = false;
        let mut slow = job("slow", "Slow", "y");
        slow.local = false;
        slow.after = None;
        let config_jobs = vec![here, slow];
        check(&config_jobs, "f").unwrap();
        let config = crate::config::Config::for_jobs(config_jobs);
        assert_eq!(
            matrix(&config).unwrap(),
            r#"[{"id":"slow","name":"Slow","run":"y"}]"#
        );
    }

    #[test]
    fn checks() {
        assert!(check(&[job("fmt", "Format", "cargo fmt --check")], "f").is_ok());
        assert!(check(&[job("fmt", "Format", "")], "f").is_err());
        assert!(check(&[job("Fmt Check", "Format", "x")], "f").is_err());
        assert!(check(&[job("all", "All", "x")], "f").is_err());
        assert!(check(&[job("a", "A", "x"), job("a", "B", "y")], "f").is_err());
        let mut clash = job("a", "A", "x");
        clash
            .runner
            .insert("name".into(), fig::Value::Str("B".into()));
        assert!(check(&[clash], "f").is_err());
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_string("a \"b\"\n\\c"), r#""a \"b\"\n\\c""#);
    }
}
