---
name: pre-commit-checks
description: Run the smallest local checks that cover an outgoing or in-progress change before committing, pushing, or claiming work is done. Invoke when asked to verify, gate, or "run the checks".
allowed-tools: read view ls glob grep bash
user-invocable: true
---

# Pre-Commit Checks

This skill is the concrete gate the `code-agent` discipline invokes before
claiming a task is done (its section "Gate before you claim done"). Use it to
gather local evidence that a change is sound before you commit, push, or report
success. The goal is the **narrowest set of checks that would fail for the
change's regression** — not reflexively running the
whole suite.

## Inspect the outgoing change

1. Confirm the working tree and branch.

   ```sh
   git status --short --branch
   ```

2. See the complete scope of what will be committed: staged, unstaged, and
   untracked paths. Reassess which behavior the combined scope can affect
   before choosing checks.

## Select relevant evidence

There is no universal local baseline. Every behavior change needs the narrowest
available test or purpose-built check that would fail for its regression; add
broader checks only for surfaces the diff actually reaches.

- **Source behavior (a crate / module):** run the owning test or build command
  (e.g. `cargo check`, `cargo test -p <crate>`) for the affected package. Leave
  repository-wide coverage to CI unless the change is genuinely cross-cutting.
- **A pure function or parser:** run the focused unit test file that owns it.
- **Documentation, comments, or generated catalogs:** run `cargo doc` or the
  project's doc/lint step when one exists.
- **Public exports, build configuration, or bin/worker entries:** run the build
  and the relevant hygiene checks.

Do not manually repeat a passing check merely because a commit or push
follows. Do not lower thresholds or widen ignores to make a check pass.

## Focus coverage on the affected source

Test selection and coverage selection are separate. A filter chooses which
tests run; coverage proves the affected source. When unit coverage is relevant,
name both the owning tests and the source files those tests must prove:

```sh
cargo test -p <crate> --lib <module>::tests
```

Use an exact source path when the behavior is truly confined to one module. If
the owning tests are unclear, discover a candidate set from the change, then
inspect the selected tests before treating the run as evidence.

## Handle failures

- If a relevant check fails, stop and fix or explain the blocker. Do not commit
  and hope CI differs.
- If a failure looks environment-specific, prove it: record the exact command,
  the failing test, and the platform-specific mismatch; confirm the
  non-platform evidence; prefer fixing cross-platform nondeterminism when the
  check is required.
- Never print secrets or bake them into output, spill files, or temp paths.

## Report

State which checks you ran and their real exit status. Report pending or
unverified states as pending — do not claim success on a check you did not
actually confirm exited 0.
