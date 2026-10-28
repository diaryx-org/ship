---
title: Tasks
description: Deferred work in ship — one file each, every one with a done state
created: 2026-10-28
updated: 2026-10-28
contents:
  - "[A quoted dependency key keeps its old pin](quoted-dependency-key-keeps-its-pin.md)"
---

# Tasks

Work ship has committed to and has not done yet, one document each. A bug is
a task with a repro; anything else is a task with a done state written down,
so that finishing it is a fact rather than an opinion.

`contents` above lists every task, open and closed: the index is the spine,
and what is open is a view of it (`org tasks`). Closing a task is an edit, not
a delete: its `status` becomes `done` or `dropped`, it names the commit or
release that resolved it, and it keeps its place above while the file stays
where it is, findable by grep.

`status` takes `open`, `in-progress`, `done`, or `dropped`, and nothing else,
so that a tool can read it across every repository in the org.
