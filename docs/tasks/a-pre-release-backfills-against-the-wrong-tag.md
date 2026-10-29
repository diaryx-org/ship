---
title: A pre-release backfills against the wrong tag
description: The changelog backfill takes each missing section's previous tag from git's `v:refname` order, which puts `v1.0.0-rc.1` after `v1.0.0`, so the pre-release's section is empty and the release after it repeats commits the release before it already listed
status: open
created: 2026-10-29
updated: 2026-10-29
part_of: "[Tasks](tasks.md)"
---

# A pre-release backfills against the wrong tag

`missing_sections` in `src/changelog.rs` gives each tag without a section
the commits between it and the tag before it, and takes "the tag before" as
`all[i - 1]` from `version::release_tags`. That list is in
`git tag --sort=v:refname` order. Without `versionsort.suffix` set, git sorts
a pre-release *after* the release it is a candidate for:

```console
$ git tag --sort=v:refname
v0.9.0
v1.0.0
v1.0.0-rc.1
v1.0.1
```

SemVer puts `v1.0.0-rc.1` between `v0.9.0` and `v1.0.0`. `floor` and
`latest_tag` already order by `version::compare` for this reason. The
backfill is the one place that still trusts the list order. A per-crate line
(`<crate>-v1.0.0-rc.1`) goes through the same code.

## Repro

A single-crate repository whose `.config/ship.figl` names only `name`, `url`
and `publishes = false`, with a `docs/CHANGELOG.md` holding `# Changelog`,
`## Unreleased` and the two git-cliff markers. One commit per tag, in
SemVer order:

```console
$ git commit -m "feat: start"            && git tag v0.9.0
$ git commit -m "feat: candidate work"   && git tag v1.0.0-rc.1
$ git commit -m "fix: final polish"      && git tag v1.0.0
$ git commit -m "fix: a patch"           && git tag v1.0.1
$ ship changelog --write
```

writes (links trimmed):

```markdown
## v1.0.1 — 2026-10-29

### Fixed

- a patch

### Fixed

- final polish

## v1.0.0 — 2026-10-29

### Fixed

- final polish

### Added

- candidate work

## v1.0.0-rc.1 — 2026-10-29

_Nothing recorded._
```

`v1.0.0-rc.1` is rendered as `v1.0.0..v1.0.0-rc.1`, which is empty because
the candidate is an ancestor of the release. `v1.0.1` is rendered as
`v1.0.0-rc.1..v1.0.1`, which takes in `final polish` again. `v1.0.0` is
rendered against `v0.9.0`, so it lists `candidate work` as well.

## What it costs

Only backfilled sections are affected. A section `ship <spec>` cuts at
release time comes from the unreleased region, and `latest_tag` gives that
the right base. The backfill exists for a tag that arrives after its release
went out, though, and a pre-release is the tag most likely to be cut by hand
outside ship. When it goes wrong, `ship notes` publishes the section exactly
as written, so the GitHub release for the tag after a pre-release lists the
previous release's changes again.

## Done when

`missing_sections` takes each tag's predecessor in `version::compare` order,
whatever `versionsort.suffix` the machine has. The repro above gives
`candidate work` to `v1.0.0-rc.1`, `final polish` to `v1.0.0` and `a patch`
to `v1.0.1`. A test in `tests/fixtures.rs` pins this for the whole-repository
line and for one crate's line.
