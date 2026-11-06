---
title: A crates.io error reads as unpublished
description: "`already_published` answers `Some(false)` for every HTTP status but 200, so a 429 or a 503 from crates.io passes preflight as a version nobody has spent, with none of the warning an unreachable registry gets"
status: open
created: 2026-11-06
updated: 2026-11-06
part_of: "[Tasks](tasks.md)"
---

# A crates.io error reads as unpublished

`already_published` in `src/cut.rs` is how preflight asks whether a crate's
version is spent. Its comment promises three answers: "None where the registry
could not be asked — which is reported as such, not taken as a no." The match
under it keeps that promise only for a connection that fails:

```rust
match code.trim() {
    "" | "000" => None,
    "200" => Some(true),
    _ => Some(false),
}
```

Only a 404 says the version is not there. A 429, a 403 or a 5xx is the registry
not answering the question, and the `_` arm takes each of them as a no. preflight
then says nothing, because `Some(false)` is the arm with nothing to report, and
the release goes on to commit and tag a version that may already be spent. The
check exists for the case where the tags cannot tell: "A crate can go up from a
laptop, untagged". A version found spent only at `cargo publish` is found after
the tag has been pushed.

## Repro

In a repository whose crate publishes, with a clean tree on `main`, `git-cliff`
on PATH, and a `curl` ahead of the real one that answers as crates.io would:

```sh
mkdir shim
printf '#!/bin/sh\nprintf 503\n' > shim/curl && chmod +x shim/curl
PATH=$PWD/shim:$PATH ship patch --no-verify
```

Measured at 83b531d, with the shim printing each status in turn:

| status | what `ship patch` does |
|---|---|
| 200 | refuses: `demo-crate 0.1.1 is already on crates.io` |
| 000 | warns `could not reach crates.io; 1 crate(s) were not checked for 0.1.1`, and tags |
| 404 | tags, with no warning, as it should |
| 429 | tags, with no warning |
| 503 | tags, with no warning |

## The change

`"404" => Some(false)`, and everything else that is not a 200 is `None`, which
preflight already counts and warns about. The warning's words, "could not
reach", would then also cover a registry that was reached and refused.

## Done when

A status other than 200 or 404 is reported by preflight's warning and never
passes silently, and a test holds the five rows above.
