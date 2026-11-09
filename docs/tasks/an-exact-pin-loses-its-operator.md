---
title: An exact pin loses its operator
description: A bump moves an internal `{ path, version }` pin by replacing everything between the version string's quotes, so `version = "=1.0.0"` becomes `version = "1.1.0"` — the exact pin turns into a caret requirement, and the crate publishes with a looser requirement than its manifest said
status: open
created: 2026-11-09
updated: 2026-11-09
part_of: "[Tasks](tasks.md)"
---

# An exact pin loses its operator

`retarget_path_dep` in `src/version.rs` moves an internal pin by finding
`version = "`, then writing the new version over everything up to the
closing quote. That text is a cargo version *requirement*, not a version,
and it can start with an operator: `=`, `~`, `^`, `>=`, `<`. The rewrite
drops it. `write_whole` and the per-crate `write_line` both go through this
function, so every bump has the gap.

The usual case is a crate and its proc-macro crate released in lockstep,
where the main crate pins `demo-macros = { path = …, version = "=1.0.0" }`
so that it never builds against another release of its macros.

## Repro

A workspace with two members, `demo-core` and `demo-macros`, both on
`version.workspace = true`, `demo-core` depending on
`demo-macros.workspace = true`, and a `.config/ship.figl` holding only
`name` and `url`:

```toml
[workspace]
members = ["crates/core", "crates/macros"]
resolver = "2"

[workspace.package]
version = "1.0.0"
edition = "2021"

[workspace.dependencies]
demo-macros = { path = "crates/macros", version = "=1.0.0" }
```

Committed, then:

```console
$ ship bump 1.1.0
1.0.0 -> 1.1.0
Cargo.toml -> 1.1.0 (and 1 internal dependencies)
$ grep demo-macros Cargo.toml
demo-macros = { path = "crates/macros", version = "1.1.0" }
```

The pin should read `version = "=1.1.0"`. `~1.0.0` comes out as `1.1.0`
the same way. The unit-level form:

```rust
let line = r#"demo-macros = { path = "../macros", version = "=1.0.0" }"#;
let out = retarget_path_dep(line, &Version::new(1, 1, 0), None).unwrap();
assert_eq!(out, r#"demo-macros = { path = "../macros", version = "=1.1.0" }"#); // fails: "1.1.0"
```

## What it costs

Nothing fails. The lockfile resolves, `cargo metadata` is happy, and the
release publishes `demo-core` with `demo-macros = "^1.1.0"` where its
author wrote an exact pin. A later `demo-macros` 1.2.0 then satisfies
`demo-core` 1.1.0's requirement, which is what the `=` existed to stop.
The diff is one character in a release commit, easy to miss in review.

No org repository has such a pin today (checked across every checkout's
`origin/HEAD` on 2026-11-09), so this is a gap in ship as a general tool
rather than a fault in a release already cut.

## Done when

`retarget_path_dep` keeps a requirement's leading operator (`=`, `~`, `^`,
`>=`, `>`, `<=`, `<`) and writes the new version after it. A requirement it
cannot move faithfully, such as a range with a comma (`>=1.0, <2.0`) or a
wildcard (`1.*`), makes the bump stop with an error before it writes
anything, not rewrite it. A test beside `retargets_lines` pins `=` and `~`.
