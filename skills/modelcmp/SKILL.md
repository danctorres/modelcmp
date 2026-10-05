---
name: modelcmp
description: "Pick the model for a software task: the user's own task when one fits, else their favorite, else the cheapest that is good enough. Use when choosing a model for a subagent, a delegated task or a harness run (opencode, pi, omp, codex, claude, gemini, copilot), or when the user asks which model or LLM to use for a task, which is cheapest or best, or to compare models: it answers from the models the user has, with their prices and benchmarks."
---

`modelcmp` joins the models the user can call with their prices and benchmark scores. Ask it rather than the web or your memory of which model is best. If the command is not found, tell the user to install it (`brew install danctorres/tap/modelcmp`), choose no model and do not look for a copy.

What the user chose wins: their own task over a built-in one, their **favorite** over any ranking. Otherwise the goal is **good enough**: the cheapest model that can do the job, not the strongest.

## Choose

1. **Task.** Run `modelcmp recommend`. A task with a `your model:` line is the user's own: when the work fits its name or description, choose it even if a built-in task fits too (the user's `debugging` over `coding` for a bug), unless the line says `no data`. Otherwise pick the built-in task whose `use for:` fits. When several do, pick the one naming what makes the work hard (a screenshot: `vision`, a long unattended run: `agentic`). When none does (docs, summaries), pick `overall`.
2. **Tier.** `low` for routine work (a mechanical edit, a bug with a clear cause, boilerplate), `mid` for ordinary work (a feature, a change across files), `high` for the hardest (a subtle bug, a design, a long run where a mistake is costly).
3. **Model.** Ask for the one model, with `--via` naming the harness you will start (opencode, pi, omp, claude, codex, gemini, copilot):

   ```sh
   modelcmp list --task coding --tier mid --via opencode --id  # anthropic/claude-sonnet-5-5
   modelcmp list --task coding --tier mid --via claude --cmd   # claude --model claude-sonnet-5-5
   ```

   It prints the user's favorite for the tier, else for the task, else the cheapest good-enough model on that harness, and under it the model's `context` (tokens) and the user's `note`. Pass the id exactly as printed, prefix included (`-m opencode/ling-3.1-flash-free`, never `-m ling-3.1-flash-free`). `--cmd` opens an interactive session. For a run with a prompt, use the harness's run command (`opencode run -m <id> "<prompt>"`, `claude -p --model <id> "<prompt>"`). A Claude Code subagent takes a family: ask with `--via claude --id` and pass the one in the id (`claude-sonnet-5-5` is `sonnet`).
   - If you can start any harness, leave `--via` out of `--cmd`: it prints the one the user runs that model on.
   - `<harness> lacks <model>`: the favorite is not on your harness. Start the harness that `--cmd` without `--via` prints. If you cannot, add `--no-fav` to your first command for the tier's recommended model on yours. A custom task has none: go back to step 1 as if it did not fit.
   - `no models match`: go back to step 1 as if the task did not fit.
   - `no model has --via '<harness>'`: tell the user, and do not guess a model.
4. **Check** that line, with no other command:
   - A `context` too small for a large prompt (a whole repo, a long log) rules the model out, favorite or not. Ask again with `--no-fav --min ctx=600` for 600,000 tokens.
   - A `note` saying the model is bad at this kind of work rules it out. Ask again with `--no-fav`, and if that prints the same model, take the next tier up. At `high`, ask the same of `overall`, and tell the user if that is ruled out too. Any other note changes nothing.
5. **Step up** one tier only after the model fails the task. When `high` fails, tell the user.

Done when you hold one id and can say which task and tier it came from.

## Explain or compare

```sh
modelcmp compare <a> <b>            # side by side, opening with a verdict
modelcmp show <model>               # every benchmark, and the price at each provider
modelcmp list --sort coding -n 10   # the best at coding first
```

Name a model by the `[key]` that `recommend` prints (`claudesonnet55`). Read these as text: `--json` is five to ten times the size. The $ in `recommend` is one blended price per 1M tokens, and a missing price is unknown, not free. Compare scores only within one task. `modelcmp --help` has every flag.
