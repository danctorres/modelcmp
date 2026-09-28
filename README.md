# modelcmp

Pick the cheapest LLM that is good enough for the job.

Frontier models cost ten times or more what smaller ones do, and many software tasks do not
need them. modelcmp puts prices and software-engineering benchmarks side by side for the
models you already have, and for any other model when you want to see what you are missing.
For each kind of task (coding, agentic runs, reasoning, ...) it recommends the best model at
each price, and you can set the model you want used for a task. An agent gets that answer
from one command, `modelcmp recommend --json`, instead of fetching prices and benchmarks and
comparing models in its own context, and can then hand a subtask to a cheaper model instead
of spending the strongest one's tokens on it.

![modelcmp TUI](https://github.com/user-attachments/assets/0b84321d-ac18-4267-8490-702b87efd0a9)

modelcmp joins prices from [models.dev](https://models.dev) with benchmarks from
[Epoch AI](https://epoch.ai/benchmarks) and by default shows only the models you can
already use: the ones your harnesses list (`opencode models`; `claude`, `codex` and
`gemini` count as their own provider) plus providers whose API key is set in the
environment. The VIA column says which; `a` in the TUI or `--all` adds every other model.

## Install

```sh
brew install danctorres/tap/modelcmp
# or
cargo install --git https://github.com/danctorres/modelcmp
```

## Use

```sh
modelcmp                                    # the TUI
modelcmp recommend                          # best model per price for each task
modelcmp list --task coding                 # the models worth paying for, cheapest first
modelcmp list --min coding=70 --sort price  # good enough, cheapest first
modelcmp compare sonnet-5 gpt-5             # side by side, with a verdict
modelcmp show sonnet-5                      # everything about one model
modelcmp fav coding sonnet-5                # your model for coding: recommend and agents use it
```

In the TUI, `R` recommends, `f` sets a model as your pick for tasks, `m` marks models and
`C` compares them, `/` filters and `?` lists every key. Only `q` quits, and it asks first;
`esc` goes back one step.

[KEYS.md](KEYS.md) has every key and the mouse, [REFERENCE.md](REFERENCE.md) the columns,
every command and the JSON fields, and `modelcmp --help` every flag.

## For agents

```sh
npx skills add danctorres/modelcmp -g                        # teach your agent to ask modelcmp
opencode -m $(modelcmp list --task coding --tier mid --id)   # or ask it yourself
```

The model you set for a task with `fav` or `f` wins: `recommend --json` gives it as the task's
`favorite`, and `--tier` returns it.

The [skill](skills/modelcmp/SKILL.md) works in Claude Code, opencode, Codex and the other
agents the [skills CLI](https://skills.sh) knows, and `npx skills update` updates it. An agent
without skills can take its body in `AGENTS.md`.

## Data

- [models.dev](https://models.dev): prices, context windows, capabilities.
- [Epoch AI Benchmarking Hub](https://epoch.ai/benchmarks) (CC-BY): ECI and
  per-benchmark scores.
- `opencode models` and the `claude`, `codex`, `gemini` binaries on `PATH`: what you have.

Downloaded on first run and cached for 24 hours under `~/.cache/modelcmp/`. Marks,
exclusions, notes and per-task favorites live in `~/.config/modelcmp/user.json`.
