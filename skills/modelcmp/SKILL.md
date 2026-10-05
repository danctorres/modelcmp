---
name: modelcmp
description: "Pick the model for a software task: the user's own task when one fits, else their favorite, else the cheapest that is good enough. Use when choosing a model for a subagent, a delegated task or a harness run (opencode, pi, omp, codex, claude, gemini, copilot), or when the user asks which model or LLM to use for a task, which is cheapest or best, or to compare models: it answers from the models the user has, with their prices and benchmarks."
---

`modelcmp` already joins the models the user can call with their prices and software benchmark scores. It is the source of truth for model choice: ask it rather than the web or your own memory of which model is best.

If the `modelcmp` command is not found, tell the user to install it (`brew install danctorres/tap/modelcmp`) and choose no model with this skill. Do not look for a copy of it.

What the user chose wins: a task they defined over a built-in one, their **favorite** over any ranking. Without one, the goal is **good enough**: the cheapest model that can do the job, not the strongest one available.

## Choose

1. **Task.** Run `modelcmp recommend`. A task with a `your model:` line is one the user defined: when the work fits its name or what it says of itself, choose it, even if a built-in task fits too (fixing a bug fits `coding`, but the user's `debugging` is the one to choose). One whose line says `no data` has no model you can use: go on as if it did not fit. With no custom task that fits, pick the built-in task whose `use for:` fits what the work must deliver. When several do, pick the one naming what makes the work hard (a screenshot to read: `vision`, a long unattended run: `agentic`), and `overall` when none does (docs, summaries, anything they do not name).
2. **Tier.** `low` for routine work (a mechanical edit, a bug with a clear cause, boilerplate), `mid` for ordinary work (a feature, a change across files), `high` for the hardest (a subtle bug, a design, a long run where a mistake is costly).
3. **Model.** Ask for the one model of that task and tier, with `--via` naming the harness you will start it on (opencode, pi, omp, claude, codex, gemini, copilot):

   ```sh
   modelcmp list --task coding --tier mid --via claude --id    # claude-sonnet-5-5
   modelcmp list --task coding --tier mid --via opencode --id  # anthropic/claude-sonnet-5-5
   modelcmp list --task coding --tier mid --via claude --cmd   # claude --model claude-sonnet-5-5
   ```

   It is the user's favorite for the tier, else for the task, else the cheapest good-enough model that harness has. Under the id it prints the model's `context` (tokens) and the user's `note`, for step 4. Pass the whole id exactly as printed, prefix included: each harness takes its own form (`opencode -m opencode/ling-3.1-flash-free`, never `-m ling-3.1-flash-free`). `--cmd` opens an interactive session. For a run with a prompt, put the id in that harness's own run command (`opencode run -m <id> "<prompt>"`, `claude -p --model <id> "<prompt>"`). A Claude Code subagent takes a family, not an id: ask with `--via claude` and pass the family named in the id (`claude-sonnet-5-5` is `sonnet`, `claude-opus-5-5` is `opus`).
   - Free to start any harness? Leave `--via` out of `--cmd`: it prints the harness the user runs that favorite on, else their default harness, else the first that has the model.
   - Exit 1 with `<harness> lacks <model>`: the user's favorite is not on your harness. Run `--cmd` without `--via` and start the harness it prints: the favorite on a harness that has it. If you cannot start that one either, add `--no-fav` to your first command: the recommended model for the tier on your harness. A custom task has none: go back to step 1 as if it did not fit.
   - Exit 1 with `no models match`: a custom task without a model for it. Go back to step 1 as if that task did not fit.
   - Exit 1 with `no model has --via '<harness>'`: the user can call no model on that harness. Tell the user and do not guess a model for it.
4. **Check** that line, with no other command:
   - For a large prompt (a whole repo, a long log), a `context` too small for it rules the model out, favorite or not. Ask again with `--no-fav --min ctx=<thousands of tokens>`: `--min ctx=600` for 600,000.
   - A `note` is the user's own experience with that model. One that says it is bad at this kind of work rules it out: ask again with `--no-fav`, and if that prints the same model, take the next tier up. At `high`, ask the same of `overall`, and tell the user if that is ruled out too. Any other note changes nothing.
5. **Step up** one tier only after the model fails the task. When `high` fails, tell the user.

Done when you hold one id and can say which task and tier it came from.

## Reading the numbers

- The $ in `recommend` is one blended price per 1M tokens (input, cache reads and output mixed), while `show` and `compare` give input and output apart. A missing price is unknown, not free.
- The score in brackets is on the scale of the benchmark source in use: Artificial Analysis when the user has its key and picked no other, else Epoch AI. Compare scores only within one task.

## Look closer

To explain a choice, or when the user asks to compare:

```sh
modelcmp compare <a> <b>            # side by side, opening with a verdict
modelcmp show <model>               # every benchmark, and the price at each provider
modelcmp list --sort coding -n 10   # the best at coding first
```

Read these as text. `--json` gives the same for a script, with every provider's price, at five to ten times the size.

Name a model by the `[key]` that `recommend` prints (`claudesonnet55`), which always matches exactly. A name matches by substring, and exit code 3 means it matched several: stderr lists the candidates with their keys, and a key always matches exactly. `modelcmp --help` has every flag.
