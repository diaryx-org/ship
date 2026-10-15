//! A repository's CI, as data: the `job` list in its config.
//!
//! The same list is read twice. `ship ci` runs the jobs in order on this
//! machine; on GitHub, the workflow asks `ship ci --matrix` for them and runs
//! one per runner, each as `ship ci <id>`. One reader, so there is nothing for
//! two to disagree about.
//!
//! A job is an id, a name and a command — the whole of what ship reads — and
//! a `runner` table that is the workflow's alone: components, a toolchain,
//! targets, a runner image, whatever that repository's workflow sets up before
//! the command can start. `--matrix` flattens it beside the job's own fields
//! and ship otherwise ignores it, because a run here is on this machine. So
//! the shared contract stays three fields however much a runner needs.
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

pub fn run_job(sh: &Sh, job: &Job) -> Result<()> {
    sh.step(&job.name);
    sh.shell(&job.run)
        .map_err(|e| format!("{e}\nhint: ship ci {} runs it alone", job.id))
}

/// Every job, in order, stopping at the first failure — a red job is worth
/// reading before the next one buries it.
pub fn run_all(sh: &Sh, config: &Config) -> Result<()> {
    let jobs = jobs(config)?;
    for job in jobs {
        run_job(sh, job)?;
    }
    println!("\n{}", sh.green(&format!("all {} jobs passed", jobs.len())));
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
    run_job(sh, job)
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
        println!("{:<id_w$}  {:<name_w$}  {first}{more}", j.id, j.name);
    }
    Ok(())
}

/// The workflow's matrix: each job's own fields with its runner table beside
/// them, as one line of JSON for `fromJSON`.
pub fn matrix(config: &Config) -> Result<String> {
    let entries: Vec<String> = jobs(config)?
        .iter()
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
            runner: BTreeMap::new(),
        }
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
