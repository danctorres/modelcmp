<h1 align="center"><img src="https://github.com/user-attachments/assets/175d7284-8b17-4ce4-bfd9-36790017ccca" alt="modelcmp" width="480"></h1>

<p align="center"><b>Compare models, pick favorites, get recommendations.</b></p>

<p align="center">
  <a href="https://github.com/danctorres/modelcmp/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/danctorres/modelcmp/ci.yml?branch=main&event=push&label=ci" alt="CI"></a>
  <a href="https://github.com/danctorres/modelcmp/releases"><img src="https://img.shields.io/github/v/release/danctorres/modelcmp" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/danctorres/modelcmp" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.89%2B-orange" alt="Rust 1.89+">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#for-agents">For agents</a> ·
  <a href="KEYS.md">Keys</a> ·
  <a href="REFERENCE.md">Reference</a>
</p>

Frontier models cost ten times or more what smaller ones do, and many software tasks do not
need them. modelcmp puts prices and software-engineering benchmarks side by side and, for each
kind of task (coding, agentic runs, reasoning, ...), names the best model at each price.

- **You** get a TUI to compare models, keep a shortlist and set the model you want for each task.
- **Your agent** gets your model for the task from `modelcmp recommend --json` if you set one.
  Otherwise it gets the best model at each price and decides whether the subtask needs the top
  one or a cheaper one will do.

![modelcmp TUI](https://github.com/user-attachments/assets/6ffde581-f8f1-4b06-b35d-a79c3ecb6277)

By default it shows only the models you can already use, plus in the TUI the ones you selected; `a` in the TUI or `--all` adds the rest.

## Install

```sh
brew install danctorres/tap/modelcmp
# or
cargo install --git https://github.com/danctorres/modelcmp
```

Linux and macOS; binaries are on the [releases](https://github.com/danctorres/modelcmp/releases)
page. `cargo install` needs Rust 1.89 or later.

When a newer version is out the TUI says so, and `U` there upgrades to it.

## Usage

```sh
modelcmp                                    # the TUI
modelcmp recommend                          # best model per price for each task
modelcmp list --task coding                 # the models worth paying for, cheapest first
modelcmp list --min coding=155 --sort price # good enough, cheapest first
modelcmp compare sonnet-5 gpt-5             # side by side, with a verdict
modelcmp show sonnet-5                      # everything about one model
modelcmp fav coding sonnet-5                # your model for coding: recommend and agents use it
```

```
$ modelcmp recommend
best per price: the top model at each price level, cheapest first, as name [key] $/1M tokens (score on the task), plus ★ your favorite, marked not recommended when it is not one

...
coding  writing and fixing code  (modelcmp list --task coding)
  use for:         fixing a bug, adding a feature to an existing repo, refactors
  best per price:  Gemini 3.7 Flash [gemini37flash] $1.0 (158) · Claude Sonnet 5.5 [claudesonnet55] $2.8 (165) · Claude Opus 5.5 [claudeopus55] $5.4 (167)

...
```

Picks run cheapest first. Each shows the name, key, $ per 1M tokens and the task score (in
ECI points with Epoch AI) in parentheses.

In the TUI, `R` recommends, `space` selects models, `C` compares them and `/` filters. `esc`
goes back, `q` quits and `?` lists the keys. [KEYS.md](KEYS.md) covers keys and mouse,
[REFERENCE.md](REFERENCE.md) the columns, commands and JSON fields, and `modelcmp --help` the
flags.

## Your model for each task

Press `f` on a model in the TUI and tick its tasks with `space`, or run
`modelcmp fav <task> <model>`. `modelcmp fav` lists your choices and `modelcmp fav <task> --rm`
clears one. Your choice beats the computed pick: `recommend` marks it `★` and agents use it
first.

A favorite can also cover one tier of a task, so easy work goes to a cheaper model than hard
work: `modelcmp fav coding 3.7-flash --tier low` makes `--tier low` return Gemini 3.7 Flash while the other
tiers keep your coding favorite. In the TUI, `f` has a box per tier on each task's row.

A favorite can name the harness you run it on: `modelcmp fav coding opus-5.5 --via claude`, or
`v` on a ticked box of `f`'s grid. `--id` then prints the id that harness takes, `--cmd` the whole command
(`claude --model claude-opus-5-5`), and `recommend --json` says which harness it is. `modelcmp harness claude` sets one
for every model without a harness of its own: `--id` and `--cmd` go by it when it has the model.

A task can be one you name yourself, with the model you give it and what it is about:
`modelcmp fav debugging opus-5.5 --about "finding and fixing a bug"`, or `+ new task` at the end
of `f`'s grid. No benchmark ranks it, so `recommend` shows it with that model alone and
`modelcmp list --task debugging --id` returns it. When your task and a built-in one both fit the
work, as debugging and coding do for a bug, agents pick yours. It takes a model per tier too
(`modelcmp fav debugging 3.7-flash --tier low`), a cheap one for easy work and a strong one for
hard.

## For agents

```sh
npx skills add danctorres/modelcmp -g                        # teach your agent to ask modelcmp
opencode -m $(modelcmp list --task coding --tier mid --id)   # or ask it yourself
```

`recommend --json` gives your model for a task as its `favorite`, and per tier as
`tier_favorites`, with their harnesses as `via` and `tier_via` when you chose one; `--tier`
returns the tier's, else the task's. Your own tasks are there too,
marked `"custom": true`, and win over a built-in task that fits the same work. It
also gives each frontier model's `note` (written with `n` or `modelcmp note`), which the skill
uses to rule out a model or choose between close ones.

The [skill](skills/modelcmp/SKILL.md) works in Claude Code, opencode, Codex and any other agent
the [skills CLI](https://skills.sh) supports; `npx skills update` updates it. An agent without
skills can take its body in `AGENTS.md`.

## Data

- [models.dev](https://models.dev): prices, context windows, capabilities.
- [Epoch AI Benchmarking Hub](https://epoch.ai/benchmarks) (CC-BY): ECI and
  per-benchmark scores.
- [Artificial Analysis](https://artificialanalysis.ai), instead of Epoch AI when picked with
  `B` or `--source aa`, or when none was picked and its key is there: its Intelligence Index, Terminal-Bench 4.0, SciCode and HLE,
  and each model's speed (output tokens per second, time to first answer token).
  Needs a free API key: the TUI's first start asks which source to use, then for the key
  if you pick it (`B` changes it later), or set `ARTIFICIAL_ANALYSIS_API_KEY`. With Epoch AI,
  only its sitemap is read, to link models that have a page there (`o`), as Epoch AI's is.
- What you have: the models `opencode models`, `pi --list-models` and `omp models` list, the `claude`, `codex`
  and `gemini` binaries on `PATH` (each counts as its own provider), the models GitHub Copilot's
  CLI takes on your plan when `copilot` is on `PATH` (asked of the CLI itself with `copilot --acp`,
  under its own login; each refresh leaves an empty folder in `~/.copilot/session-state/`), and providers whose API key is set in the environment. The Via column says which. A
  harness that fails to list its models
  keeps the ones from the last refresh, and the refresh says so.

Downloaded on first run and cached for 24 hours under `~/.cache/modelcmp/` on Linux
(`~/Library/Caches/modelcmp/` on macOS). Your selection, exclusions, notes and per-task
favorites live in `~/.config/modelcmp/user.json` (`~/Library/Application Support/modelcmp/`
on macOS); the `?` help shows the path. Each refresh also asks GitHub for the latest release,
to say when a new version is out.

Benchmarks are proxies: a score says how a model did on that test, not how it will do in
your harness.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run what CI runs:

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org).

## License

[MIT](LICENSE). Benchmark data from Epoch AI is licensed CC-BY.
