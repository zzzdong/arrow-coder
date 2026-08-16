Use the `todo` tool to manage a task list. This tool helps you track tasks and their progress.

## Commands

- **add**: Add a new todo item. Params: `content` (required), `priority` (optional: `high`|`medium`|`low`).
- **update**: Update a todo item. Params: `id` (required) plus any of `content`, `status`, `priority`.
- **remove**: Remove a todo item by `id`.
- **list**: View the current todo list. Optional `status` filter (`pending`|`in_progress`|`completed`).
- **clear**: Remove all todos (optionally filtered by `status`).

## Todo Structure

Each todo item has:
- `id`: A unique string identifier (returned by `add`; use it for `update`/`remove`).
- `content`: The task description.
- `status`: One of `pending` | `in_progress` | `completed`.
- `priority`: One of `high` | `medium` | `low`.

## When to Use This Tool

**Use proactively for:**
- Complex multi-step tasks (3+ distinct steps)
- Non-trivial tasks requiring careful planning
- Multiple tasks provided by the user (numbered or comma-separated)
- Tracking progress on ongoing work
- After receiving new instructions — immediately capture requirements
- When starting work — mark task as `in_progress` BEFORE beginning
- After completing work — mark as `completed` and add any follow-up tasks discovered

**Skip this tool for:**
- Single, straightforward tasks
- Trivial operations (< 3 simple steps)
- Purely conversational or informational requests
- Tasks that provide no organizational benefit

## Task Management Best Practices

1. **Status Management:**
   - Only ONE task should be `in_progress` at a time.
   - Mark tasks `in_progress` BEFORE starting work on them (call `update` with `status: "in_progress"`).
   - Mark tasks `completed` IMMEDIATELY after finishing (call `update` with `status: "completed"`).
   - If blocked or encountering errors, keep the task `in_progress` and note the blocker in a new task.

2. **Task Completion Rules:**
   - ONLY mark as `completed` when FULLY accomplished.
   - Never mark complete if tests are failing, implementation is partial, or errors are unresolved.
   - When blocked, create a new task describing what needs resolution.

3. **Task Organization:**
   - Create specific, actionable items.
   - Break complex tasks into manageable steps.
   - Use clear, descriptive task names.
   - Remove irrelevant tasks entirely (call `remove`) rather than leaving stale entries.

4. **Keep the list current:** Whenever you begin, finish, or pivot on a task, update its status with `update` so the visible list always reflects reality.
