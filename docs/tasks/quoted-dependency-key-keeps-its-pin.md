---
title: A quoted dependency key keeps its old pin
description: A bump moves an internal `{ path, version }` pin only when the line's key, as written, is a member's name, so a TOML-quoted key such as `"demo-core" = { … }` is never moved — silently on a minor bump, and on a major one `cargo update` fails after the manifest has been rewritten
status: done
created: 2026-10-28
updated: 2026-10-30
part_of: "[Tasks](tasks.md)"
---

# A quoted dependency key keeps its old pin

**Status.** Done, in `fix(version): a quoted dependency key is the member it
names`. `retarget_path_dep` compares a key by the name it spells, with one
pair of `"…"` or `'…'` quotes taken off, and
`a_quoted_key_is_the_name_it_spells` pins both quote styles.

`retarget_path_dep` in `src/version.rs` decides whether a `{ path, version }`
line is an internal pin by its key: the text before the line's first `=`,
trimmed, compared with the workspace's member names. TOML lets a key be
quoted, and cargo reads `"demo-core" = { … }` and `demo-core = { … }` as
the same dependency, but the comparison sees `"demo-core"`, quotes and all,
matches no member, and leaves the line alone. Both `write_whole` and the
per-crate `write_line` go through it, so every bump has the gap.

## Repro

A workspace whose `[workspace.dependencies]` quotes the key of a member it
pins:

```toml
[workspace]
members = ["crates/core", "crates/app"]
resolver = "2"

[workspace.package]
version = "1.0.0"
edition = "2021"

[workspace.dependencies]
"demo-core" = { path = "crates/core", version = "1.0.0" }
```

with `crates/core` named `demo-core`, `crates/app` depending on
`demo-core.workspace = true`, both on `version.workspace = true`, and a
`.config/ship.figl` holding only `name` and `url`. Committed, then:

```console
$ ship bump 1.1.0
1.0.0 -> 1.1.0
Cargo.toml -> 1.1.0 (and 0 internal dependencies)
$ grep demo-core Cargo.toml
"demo-core" = { path = "crates/core", version = "1.0.0" }
```

Written without the quotes, the same line moves to `1.1.0` and the count is
1. The unit-level form:

```rust
let members = ["demo-core".to_string()];
let line = r#""demo-core" = { path = "crates/core", version = "1.0.0" }"#;
assert!(retarget_path_dep(line, &Version::new(2, 0, 0), Some(&members)).is_some()); // fails: None
```

## What it costs

On a minor or patch bump nothing fails: `^1.0.0` still admits `1.1.0`, the
lockfile resolves, and the release publishes `demo-app` with a requirement
the bump was meant to raise. That stale requirement is exactly what moving
the pins is for. On a major bump, `ship bump 2.0.0` rewrites `Cargo.toml`
and then stops at `cargo update`, with `failed to select a version for the
requirement demo-core = "^1.0.0"`, leaving the tree half-moved.

## Done when

A key written as a TOML basic (`"…"`) or literal (`'…'`) string is compared
by the name it spells, a quoted internal pin moves with the version on
`ship bump` for a whole workspace and for one crate, and a test in
`only_members_move`'s neighbourhood pins both quote styles.
