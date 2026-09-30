---
name: modelcmp
description: "Pick the model for a software task: the user's favorite, else the cheapest that is good enough. Use when choosing a model for a subagent, a delegated task or a harness run (opencode, codex, claude, gemini), or when the user asks which model to use."
---

`modelcmp` already joins the models the user can call with their prices and software benchmark scores. It is the source of truth for model choice: ask it rather than the web or your own memory of which model is best.

The user's **favorite** for a task always wins. Without one, the goal is **good enough**: the cheapest model that can do the job, not the strongest one available.

## Choose

1. Run `modelcmp recommend --json`. Pick the task whose `when` fits the work (`overall` when none does). For a large prompt (a whole repo, a long log), skip frontier entries whose `context` (tokens) is too small.
2. If the task has a favorite, use it: `tier_favorites` holds the user's pick per difficulty (`low` routine, `mid` ordinary, `high` the hardest work), and `favorite` covers every difficulty without one. The user chose them, and they override everything below.
3. Otherwise read the task's `frontier`: the Pareto frontier, cheapest first, each entry costing more and scoring higher. Start at the cheapest entry that fits the difficulty.
   An entry's `note`, when present, is the user's own experience with that model: let it rule out an entry or decide between close ones.
4. **Step up** one entry only after the current model fails the task.

Done when you hold one model `key` and can say which task and frontier entry it came from.

## One id for a harness

`--tier` applies the same rules and prints the provider/model string a harness takes:

```sh
opencode -m "$(modelcmp list --task coding --tier mid --id)"
```

`low` is the cheapest entry scoring 50+, `mid` the cheapest 75+, `high` the best. The tier's favorite, else the task's, wins over the tier's pick.

## Reading the numbers

- `price` is $ per 1M tokens. `null` means unknown, not free.
- `score` is a 0-100 percentile, except for `overall` and `vision`, where it is the overall index of the model's `source`: Epoch Capabilities Index (`epoch`) or Artificial Analysis Intelligence Index (`aa`). Scores from different sources do not compare.
- `"recommended": false` marks a favorite that sits on the frontier only because the user chose it. Still use it.
- Models the user excluded never appear in `recommend` or `--task`.

## Look closer

When two entries are close, compare them before choosing:

```sh
modelcmp show <model> --json          # every benchmark and every provider's price
modelcmp compare <a> <b> --json       # side by side
modelcmp list --min coding=70 --sort price --json   # everything good enough, cheapest first
```

A model name matches by substring. Exit code 2 means it matched several; stderr lists the candidates with their keys, and a key always matches exactly. `modelcmp --help` has every flag.
