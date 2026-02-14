Use `Plan` for complex, multi-step tasks where tracking progress matters. Avoid it for simple or read-only work.

- `create`: define phases with steps. `depends_on` references steps in earlier phases only. Step IDs are sequential across phases. Replaces any existing plan.
- `advance`: mark step done (`step_id` + `outcome`).
- `skip`/`fail`: skip or fail a step (`step_id` + `reason`).
- `edit`: add/remove/reorder steps or phases after creation.
- `status`: view progress.

Creating a plan does not pause execution. Begin step 1 immediately after creation. Advance steps as they complete without waiting for user prompts between steps.
