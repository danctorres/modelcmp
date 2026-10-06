---
name: modelcmp
description: "Before you name a model for a subagent, a delegated task or a harness run (opencode, pi, omp, codex, claude, gemini, copilot), run `modelcmp pick` and follow this skill instead of choosing from memory. Also use when the user asks which model to use for a task, which is cheapest or best, or to compare models: it answers from the models the user has, with their prices, benchmarks and favorites."
---

`modelcmp` joins the models the user can call with their prices, benchmark scores and the user's own choices. If the command is not found, tell the user to install it (`brew install danctorres/tap/modelcmp`), choose no model and do not look for a copy.

## Choose

```sh
modelcmp pick
```

Only when the prompt is large (a whole repo, a long log), add `--min ctx=600`, for 600,000 tokens. It prints each task with what it is for, and under it a line per tier, one for tiers with the same model: the command that runs the model, the model's context and the user's `note` if any. The command starts the harness the user has that model on, which is often not the one you are running in. Take one line:

1. **Task.** One marked `the user's own task` when the work fits it, even if a built-in task fits too (the user's `debugging` over `coding` for a bug). Otherwise the task whose description fits. When several do, the one naming what makes the work hard (a screenshot: `vision`, a long unattended run: `agentic`). When none does (docs, summaries), `overall`.
2. **Tier.** `low` for routine work (a mechanical edit, a bug with a clear cause, boilerplate), `mid` for ordinary work (a feature), `high` for the hardest (a subtle bug, a design, a long run where a mistake is costly).
3. **Check.** A `note` saying the model is bad at this kind of work rules it out: run again with `--not <id>`, the id after `--model`. Any other note changes nothing. `no model`: take another task.
4. **Run** the command in your shell as printed, with the subtask in place of `<prompt>`, written for a model that knows nothing of your conversation and quoted for the shell (single quotes when it has `$`, a backtick or `"`). Name a file (an image, a log) by its path in it, never paste it. The command runs the prompt on that model and exits, printing the model's answer alone, or the end of the harness's log when it fails. It lets the harness edit files and run commands in the folder you run it in, so run it where the work is. Your own subagent tool takes your harness's models alone, so never pass it another harness's id, and never swap the model for one of yours because it is easier to start. Only for a model your harness has too, use your subagent tool instead: in Claude Code, a line with a Claude model (`opencode run --model anthropic/claude-sonnet-5-5 ...`) is a subagent of that family (`sonnet`).
5. **Step up** to the next tier with another model only after the model fails the task. When `high` fails, tell the user.

Only when you cannot run another harness (no shell, or the user says to stay in yours), `modelcmp pick --via <harness>` with yours (opencode, pi, omp, claude, codex, gemini, copilot): each line then has the id that harness takes in place of the command (in Claude Code, pass its family). A line ending in `(★ favorite ..., start: <command>)` has the user's favorite on another harness, which you tell the user of.

## Explain or compare

```sh
modelcmp compare <a> <b>            # side by side, opening with a verdict
modelcmp show <model>               # every benchmark, and its price where the user has it
modelcmp list --sort coding -n 10   # the best at coding first
```

Name a model by an id `pick` prints or part of its name (`sonnet-5.5`). Read these as text, `--json` is several times the size. `Price` is one blended $ per 1M tokens, and `-` is unknown, not free. Compare scores only within one task. `modelcmp <command> -h` has its flags.
