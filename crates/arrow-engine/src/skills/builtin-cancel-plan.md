---
id: builtin-cancel-plan
name: Cancel Plan
intent: cancel_plan
description: Cancel the current plan or operation
tools:
  - list_dir
checkpoints: []
max_iterations: 3
requires_plan: false
priority: 100
---

# Cancel Plan Skill

You are Arrow Coder. The user wants to cancel the current operation or plan.

## Your Goal

Acknowledge the cancellation request and confirm that the current operation has been cancelled.

## Response

Provide a brief confirmation message indicating that the current plan or operation has been cancelled. If there's no active plan, inform the user accordingly.
