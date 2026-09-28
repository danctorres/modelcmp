---
name: modelcmp
description: Pick which LLM to run a software task on, weighing benchmarks against price among the models the user can use. Use when choosing a model for a subagent, a delegated task or a harness run (opencode, codex, claude, gemini), or when the user asks which model to use for something.
---

# modelcmp

`modelcmp` knows the models the user can call (their harnesses and API keys), their prices
and their scores on software benchmarks. Ask it instead of defaulting to the strongest model:
a routine task should go to the cheapest model that is good enough.

Do not search the web for prices or benchmarks, or compare models yourself: modelcmp has
already joined and ranked them. `recommend --json` is about a thousand tokens, and
`list --task <t> --tier <tier> --id` returns a single id.

## Choose

1. Run `modelcmp recommend --json`. It lists tasks: `overall`, `coding`, `value`, `agentic`,
   `reasoning`, `vision`, `long-context`. Pick the one whose `when` fits the work.
2. If that task has a `favorite`, use it: the user chose it.
3. Otherwise pick from its `frontier`, ordered cheapest first, each entry costing more and
   scoring higher (`price` is $ per 1M tokens, `score` a 0-100 percentile). Take the
   cheapest entry that fits the difficulty and step up only after it fails.

For one id to pass to a harness, let modelcmp apply the same rules:

```sh
modelcmp list --task coding --tier mid --id    # provider/model; the favorite when set
opencode -m "$(modelcmp list --task coding --tier mid --id)"
```

Tiers: `low` is the cheapest scoring 50+, `mid` the cheapest 75+, `high` the best.

## Look closer

- `modelcmp show <model> --json`: every benchmark and every provider's price.
- `modelcmp compare <a> <b> --json`: side by side.
- `modelcmp list --min coding=70 --sort price --json`: good enough, cheapest first.

A model name matches by substring. Exit code 2 means the name is ambiguous, and stderr lists
the candidates with their keys, which always match exactly. A `null` price means unknown, not
free. Models the user excluded never appear in `--task` or `recommend`.
