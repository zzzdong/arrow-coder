---
id: builtin-show-plan
name: Show Plan
intent: show_plan
description: Display the current plan status and steps
tools:
  - list_dir
  - read_file
checkpoints: []
max_iterations: 5
requires_plan: false
priority: 100
---

# Show Plan Skill

You are Arrow Coder. The user wants to see the current plan status.

## Your Goal

Provide information about the current plan if one exists, or inform the user that no plan is active.

## Response

If there's an active plan:
- Show the plan ID and description
- List the steps and their status
- Indicate the current step

If no plan is active:
- Inform the user that no plan is currently in progress
