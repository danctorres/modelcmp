---
name: modelcmp
description: "Pick the model for a software task: the user's own task when one fits, else their favorite, else the cheapest that is good enough. Use before you name a model for a subagent, a delegated task or a harness run (opencode, pi, omp, codex, claude, gemini, copilot), instead of choosing one from memory, and when the user asks which model or LLM to use for a task, which is cheapest or best, or to compare models: it answers from the models the user has, with their prices and benchmarks."
---

`modelcmp` joins the models the user can call with their prices, benchmark scores and the user's own choices. Ask it rather than the web or your memory of which model is best. If the command is not found, tell the user to install it (`brew install danctorres/tap/modelcmp`), choose no model and do not look for a copy.

## Choose

```sh
modelcmp pick --via opencode   # the harness you will start: opencode, pi, omp, claude, codex, gemini, copilot
```

Add `--min ctx=600` when the prompt is large (a whole repo, a long log: 600,000 tokens here). It prints each task with what it is for, and under it a line per tier: the id to pass, the model's context and the user's `note` if any. The model is the user's favorite where they set one, else the cheapest that is good enough for the tier. Take one line:

1. **Task.** One marked `the user's own task` when the work fits it, even if a built-in task fits too (the user's `debugging` over `coding` for a bug). Otherwise the task whose description fits. When several do, the one naming what makes the work hard (a screenshot: `vision`, a long unattended run: `agentic`). When none does (docs, summaries), `overall`.
2. **Tier.** `low` for routine work (a mechanical edit, a bug with a clear cause, boilerplate), `mid` for ordinary work (a feature, a change across files), `high` for the hardest (a subtle bug, a design, a long run where a mistake is costly).
3. **Check.** A `note` saying the model is bad at this kind of work rules it out: run again with `--not <id>`. Any other note changes nothing. A line ending in `(★ favorite ..., start: <command>)` has the user's favorite on another harness: use that command when you can start that harness, else the id on the line. `no model`: take another task. `no model has --via '<harness>'`: tell the user, and do not guess a model.
4. **Use** the id exactly as printed, prefix included (`opencode run -m opencode/ling-3.1-flash-free "<prompt>"`, `claude -p --model <id> "<prompt>"`). A Claude Code subagent takes a family: ask with `--via claude` and pass the one in the id (`claude-sonnet-5-5` is `sonnet`). If you can start any harness, leave `--via` out: each line then has the command that opens the harness the user runs that model on.
5. **Step up** one tier only after the model fails the task. When `high` fails, tell the user.

## Explain or compare

```sh
modelcmp compare <a> <b>            # side by side, opening with a verdict
modelcmp show <model>               # every benchmark, and the price at each provider
modelcmp list --sort coding -n 10   # the best at coding first
```

Name a model by an id `pick` prints or part of its name (`sonnet-5.5`). Read these as text: `--json` is five to ten times the size. A price is one blended $ per 1M tokens, and a missing price is unknown, not free. Compare scores only within one task. `modelcmp --help` has every flag.
