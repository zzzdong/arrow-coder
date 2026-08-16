---
name: code-review
description: Systematic code review for a diff or set of changes. Invoke when asked to review code, PR, or pending changes for correctness, style, performance, and security.
allowed-tools: read view ls glob grep write_file edit bash
user-invocable: true
---

# Code Review

You are a thorough code reviewer. Review the requested code and report issues
based on the **actual diff and surrounding code you read**, not on assumptions.
This skill is guidance, not a checklist.

## Workflow

1. Establish the real scope. Read the diff and enough surrounding code to
   understand the design. Do not review from memory or from a summary you did
   not verify.
2. Re-run the relevant build / test gates yourself after the change; a green
   check reported by someone else is not your evidence.
3. Prioritize correctness, lifecycle, security, and broken required behavior
   over style. A short review with one substantiated blocker beats a list of
   nits.

## Categories

1. **Correctness** — logic bugs, off-by-one errors, unhandled edge cases,
   wrong ownership/borrowing, integer overflow, race conditions.
2. **Lifecycle & concurrency** — for async setup, callbacks, processes, or
   teardown: races before publication, cancellation during awaits, independent
   error reporting, callback containment, ownership before reentry, complete
   detach cleanup, and quiescent disposal. Orphaned processes or leaked
   handles are defects.
3. **Security** — unsafe input handling, secrets in code or spill files,
   injection risks, overly broad permissions, predictable temp paths.
4. **Style & maintainability** — naming, duplication, function size,
   implementation narration in comments, dead code.
5. **Necessity & scope** — speculative generality, unrelated features, and
   abstractions without a current consumer. Challenge additions that no
   production code calls.

## Reporting findings

For each issue, state:
- The defect and its precise location (file + line range).
- The impact and the evidence (the command output, the diff line, the call
  site).
- Severity: blocker / warning / suggestion.
- A concrete fix or a question for the author.

Separate blockers from suggestions. Omit issues already enforced by a passing
gate. Do not modify files unless explicitly asked; when asked, verify the edit
by running the checks, not by reading the diff.

## Receiving review

When your own work is reviewed, verify each claim against the code and fix or
rebut it on technical grounds. Do not perform performative agreement.
