# Drive app control through an observation-driven agent loop

**Status:** Accepted

Controlling an app beyond opening it requires accessibility refs that exist only after the target window is observed, so a complete action plan written before execution cannot address arbitrary apps. For those tasks DeskAct runs an agent loop instead: the primary model sees a bounded snapshot of the single verified target window, proposes a small number of tool calls from a fixed DeskAct tool set, and Rust validates, classifies, authorizes, and executes each one through `agent-desktop` before returning the next observation. The model never invokes `agent-desktop` or receives its raw CLI; each tool call stays a proposal under Rust policy. Opening an app keeps the up-front action-plan fast path.

## Considered Options

- **Complete plan up front, replan only on surprises** (previous PRD default): one approval for the whole plan and fewer model calls, but it cannot target refs the model has not yet seen, which limits it to scripted apps.
- **Observation-driven loop** (chosen): works on any app that exposes usable controls, at the cost of more model calls and approval that happens per step instead of per plan.

## Consequences

- Loop tasks require an endpoint that supports Chat Completions tool calling. Endpoints without it can still open apps through the JSON action plan. This replaces the PRD's "tool calling is optional" stance for these tasks.
- A user request starts the loop immediately with no task-level approval. Only steps that Rust classifies as consequential, or cannot classify, pause for action-specific approval.
- Every task has a budget (model turns, executed tool calls, wall-clock time, snapshot size). When the budget runs out, the task stops safely and reports a partial or unverified outcome.
- The model's tool prompt is DeskAct-owned, adapted from the `agent-desktop` skill text for the pinned CLI version, and updated only together with that pin.
- Notepad is the reliability gate; other apps are allowed but best-effort.
