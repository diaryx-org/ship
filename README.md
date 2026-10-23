# ship

Cut a repository's release, and run its CI, from one config file.

```text
ship                    what each version would be, and what the commits say — cuts nothing
ship <spec>             bump, changelog, commit, tag — and push only with --push
ship version            what the repository calls itself
ship bump [<spec>]      move the version in every manifest that carries it
ship changelog          the unreleased region (--write splices it, --check verifies it)
ship notes [tag]        a release's changelog section, as the GitHub release body
ship github [tag]       write that section to the GitHub release
ship ci [<job>]         run the CI jobs, or one of them (--parallel, --list, --matrix)
ship -p <crate> …       per crate: the crate a command acts on
```

`<spec>` is `patch`, `minor`, `major`, an explicit `x.y.z[-pre]`, or `as-is`
for a version already committed and never tagged.

## A person names the version

`ship` on its own proposes and writes nothing: every version each spec would
move to, the commits since the last tag counted by Conventional Commits type,
the ones that say they break the promise (`!`, `BREAKING CHANGE:`), and every
`Behavioural-change:` trailer, printed whole. Which of patch, minor and major a
release is, is a judgement about what the repository promises downstream; the
tool gathers the evidence and stops.

Named, `ship <spec>` checks everything that could say no before it writes
anything — a dirty tree, the wrong branch, a tag or a crates.io version already
spent, a branch behind its origin — then runs the repository's CI, moves the
version, cuts the changelog section, commits, and tags. It stops there. Nothing
leaves the machine without `--push`, because the push is the one step that
spends a version number for good. `--push` pushes the branch and the tag and
writes the GitHub release from the section the tag carries.

## The config

`.config/ship.figl`, in [figl](https://github.com/diaryx-org/fig), fig's
authoring dialect. Everything has a default for a Cargo workspace, so most
repositories say only who they are:

```fig
name = fig-lua
url = https://github.com/diaryx-org/fig-lua
```

| setting | default | |
|---|---|---|
| `manifest` | `Cargo.toml` | the file holding the one version line |
| `version_prefix` | `version = "` | what finds it: `    .version = "` for a build.zig.zon |
| `version_quoted` | `true` | false where the version runs to the end of the line |
| `cargo_workspace` | `true` | the lockfile and internal `{ path, version }` pins follow the version |
| `mirrors` | `[]` | Cargo workspaces that copy the version rather than decide it |
| `post_bump` | none | a command for the files no manifest parser would find |
| `extra_version_files` | `[]` | what `post_bump` writes, so the release commit stages it |
| `changelog` | `docs/CHANGELOG.md` | |
| `section_prefix`, `section_date` | `v`, `true` | `## v1.2.0 — 2026-10-15`, or `## 1.2.0` |
| `changelog_since` | none | the earliest tag the changelog claims to cover |
| `cliff_config` | ship's own | a git-cliff config of the repository's own |
| `release_branch` | `main` | |
| `verify` | the `job` list | what to run before cutting, as `ship ci` runs it; `cargo xtask ci` where there are no jobs |
| `verify_after_bump` | `false` | true where `verify` checks the versions being written |
| `publishes` | `true` | whether preflight asks crates.io if the version is spent |
| `push_tag_first` | `false` | for a system that builds the tag and will not build one commit twice |
| `allow_empty_release` | `true` | |
| `tag_starts` | `[]` | what pushing the tag sets off, for the warning before the push |
| `ci_parallel` | `false` | `ship ci` runs every job at once, as `--parallel` |
| `per_crate` | `false` | each crate of the workspace releases on its own; see below |
| `crates` | every crate that publishes | per crate, the crates that release |

A repository from before ship keeps `.config/release.toml` and
`.config/ci.toml`, and ship reads those where there is no `ship.figl`.
`fig get .config/release.toml -o figl` converts one, comments and all.

## Per crate

A workspace whose crates move at different speeds says `per_crate = true`, and
then each crate is a release line of its own:

| | a crate with its own `[package] version` | the crates with `version.workspace = true` |
|---|---|---|
| version | its `[package] version` | `[workspace.package] version` |
| tags | `<crate>-v1.2.0` | `v1.2.0` |
| changelog | `CHANGELOG.md` in its directory | `changelog` |
| commits | those touching its directory | those touching anything no other line owns |
| `-p` | its name | the repository's `name`, or any of the crates |

So a repository that takes its plugins in as members keeps the tags and the
changelog it had, and each plugin gets its own. Where nothing inherits the
workspace version, the crate named as the repository has the plain tags. A
commit that touches two lines is in both changelogs; a changelog's headings
say `## v1.2.0` whatever the tag is, since the file is the crate's own.

Every command takes `-p <crate>`. `ship` alone lists every crate with its
version, its last tag and how many commits it has had since, and stops; `ship
-p <crate>` is that crate's proposal and `ship -p <crate> <spec>` its release.
`ship version` and `ship changelog` alone act on every crate, so `ship changelog
--check` in CI checks them all. `notes` and `github` find the crate from the
tag.

Releasing one crate moves its version, the `{ path, version }` pins that name
it in every manifest (`[workspace.dependencies]` included), and the lockfile;
a pin to any other crate stays where it is. Preflight asks crates.io about
that crate alone, at its own version. The commit says whose release it is:
`chore: bump to <crate> 1.2.0`. `post_bump` runs with the crate's line in
`SHIP_CRATE`. A crate that says `publish = false` is not released unless
`crates` names it, and `mirrors` cannot be combined with `per_crate`.

## The changelog

`docs/CHANGELOG.md` has one generated region, between two markers, rendered by
[git-cliff](https://git-cliff.org) from the commits since the last tag. Only the
bytes between the markers are ever rewritten, so anything written by hand —
above the region, or in a released section below it — survives. Cutting a
release turns the region into a section headed with the version and the date,
and resets it. A tag with no section of its own gets one the next time the
region is written.

## CI

The `job` list is the repository's CI:

```fig
job
> *
> > id = clippy
> > name = Clippy
> > run = cargo clippy --workspace --all-targets -- -D warnings
> > runner
> > > components = clippy
```

`ship ci` runs every job in order and stops at the first failure; `ship ci
clippy` runs one. Each runs under `bash -eo pipefail -c` at the repository
root, which is the shell GitHub gives a `run:` step, so a job means the same
thing on your machine and on a runner.

Three fields say where and when a job runs, and these are ship's:

| field | default | |
|---|---|---|
| `local` | `true` | false leaves it out of `ship ci` unless it is named: a job too slow for every run here, which a runner can afford |
| `matrix` | `true` | false leaves it out of `--matrix`: a job only a developer's machine can run |
| `after` | none | an earlier job this one waits for when the jobs run at once |

`ship ci --parallel`, or `ci_parallel = true`, starts every job at once, each
under its own process group with its output in `<git dir>/ship/ci/<id>.log`,
and a job with `after` once that job has passed. A job whose `after` has
passed in the same run, at once or in order, finds its id in `SHIP_CI_AFTER`,
so it can trust what that job left behind; named alone, it finds nothing. The first to fail stops the
rest and prints the end of its log. It is for a list whose jobs spend their
time anywhere but one shared lock — a prover, a simulator, test binaries — so
the whole list takes as long as its longest job. Where there is no `verify`,
the release runs the job list the same way.

A workflow asks `ship ci --matrix` for the jobs, as one line of JSON, and runs
each on a runner of its own as `ship ci <id>`, named `CI / <name>`. A job's
`runner` table is the workflow's alone: `--matrix` flattens it beside the job's
`id`, `name` and `run`, and ship otherwise ignores it. So a runner can need
components, a toolchain, targets or a macOS image without ship knowing what any
of them are. Anything that needs more than one command is a `cargo xtask` task
the job runs. See `.github/workflows/ci.yml` here for the shape.

## Installing

Every release carries binaries for Linux (static, musl) and macOS, x86_64 and
aarch64, under names without a version, so the latest is always one download:

```sh
mkdir -p ~/.local/bin
curl -fsSL https://github.com/diaryx-org/ship/releases/latest/download/ship-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin ship
```

The others are `ship-aarch64-unknown-linux-musl`, `ship-x86_64-apple-darwin`
and `ship-aarch64-apple-darwin`, each with a `.sha256` beside it. Or from
source:

```sh
cargo install --locked --git https://github.com/diaryx-org/ship ship
```

ship shells out to `git`, `cargo`, `git-cliff`, `gh` (for `--push` and `ship
github`) and `curl` (to ask crates.io), and checks for each before it is needed.

## License

MIT or Apache-2.0, at your option.
