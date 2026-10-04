---
name: modelcmp
description: "Pick the model for a software task: the user's own task when one fits, else their favorite, else the cheapest that is good enough. Use when choosing a model for a subagent, a delegated task or a harness run (opencode, pi, omp, codex, claude, gemini, copilot), or when the user asks which model to use."
---

`modelcmp` already joins the models the user can call with their prices and software benchmark scores. It is the source of truth for model choice: ask it rather than the web or your own memory of which model is best.

A task the user defined (`"custom": true`) wins over a built-in task that fits the same work. The user's **favorite** for a task always wins. Without one, the goal is **good enough**: the cheapest model that can do the job, not the strongest one available.

## Choose

1. Run `modelcmp recommend --json`. A task with `"custom": true` is one the user defined, with a `name`, an `about` in their words and the models they gave it. When the work fits its `name` or `about`, use its model and stop here (the `tier_favorites` entry for the difficulty, else its `favorite`, else the closest tier it has), even if a built-in task fits too: fixing a bug fits `coding`, but the user's `debugging` is the one to choose. A custom task with an empty `frontier` has no model you can use: go on as if it did not fit. Only when no custom task fits, pick the task whose `when` fits the work (`overall` when none does). For a large prompt (a whole repo, a long log), skip frontier entries whose `context` (tokens) is too small.
2. If the task has a favorite, use it: `tier_favorites` holds the user's pick per difficulty (`low` routine, `mid` ordinary, `high` the hardest work), and `favorite` covers every difficulty without one. The user chose them, and they override everything below. When the favorite you use has a harness (`via` for the task's, `tier_via` for a tier's), the user runs that model on that harness: start it there.
3. Otherwise read the task's `frontier`: the best model per price level and what each tier picks, cheapest first, each recommended entry costing more and scoring higher. Start at the cheapest entry that fits the difficulty.
   An entry's `note`, when present, is the user's own experience with that model: let it rule out an entry or decide between close ones.
4. **Step up** one entry only after the current model fails the task.

Done when you hold one model `key` and can say which task and frontier entry it came from.

## One id for a harness

`--tier` applies the same rules; `--id` prints the provider/model string opencode takes, or pi or omp when only they have the model (for claude, codex, gemini and copilot run `modelcmp show <key> --json` and use the `id` of the `providers` entry whose `via` names the harness). A favorite with a harness of its own (`via`, `tier_via` in `recommend --json`) prints the id that harness takes instead, so pass it to that harness; add `--via opencode` when you can only start opencode:

```sh
opencode -m "$(modelcmp list --task coding --tier mid --id)"
```

`--cmd` prints the command that opens the harness on the model instead (`claude --model claude-opus-5-5`): the harness the user chose for the favorite, else the first that has the model. It is the harness, `--model` and the id that harness takes, and it opens an interactive session; for a run with a prompt, take the harness and the id from it and add them to that harness's own run command.

```sh
modelcmp list --task coding --tier mid --cmd
```

`low` is the cheapest entry within 8 months of progress of the best one, `mid` the cheapest within 3, `high` the best; a month of progress is a twelfth of what the best score on the task rose in the last year. The tier's favorite, else the task's, wins over the tier's pick. A custom task takes `--task` and `--tier` too: the tier's model, else the task's. With neither it exits 1 and prints no id: take the model from `recommend --json` instead.

## Reading the numbers

- `price` is $ per 1M tokens. `null` means unknown, not free. `"listed": true` (in `price`, or on a `recommend --json` entry; `~` in the tables) means the user's provider lists no price and it is the list price of other providers: an estimate.
- `score` is on the scale of the source in use (`source` in `list --json`). With `epoch` it is in Epoch Capabilities Index points, the task's capability for coding, agentic and reasoning. With `aa` it is the Artificial Analysis Intelligence Index for `overall` and `vision`, and one benchmark's 0-100 score for the others (Coding Index, Terminal-Bench 4.0, HLE). `value` is a 0-100 percentile with either. Scores from different sources do not compare.
- `"recommended": false` marks a favorite that sits on the frontier only because the user chose it. Still use it.
- Models the user excluded never appear in `recommend` or `--task`.

## Look closer

When two entries are close, compare them before choosing:

```sh
modelcmp show <model> --json          # every benchmark and every provider's price
modelcmp compare <a> <b> --json       # side by side
modelcmp list --min coding=155 --sort price --json  # everything good enough, cheapest first
```

A model name matches by substring. Exit code 3 means it matched several; stderr lists the candidates with their keys, and a key always matches exactly. `modelcmp --help` has every flag.
