<h1 align="center"><img src="https://github.com/user-attachments/assets/175d7284-8b17-4ce4-bfd9-36790017ccca" alt="modelcmp" width="480"></h1>

<p align="center"><b>Compare models by price and benchmarks, choose one per task or get a recommendation.</b></p>

<p align="center">
  <a href="https://github.com/danctorres/modelcmp/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/danctorres/modelcmp/ci.yml?branch=main&event=push&label=ci" alt="CI"></a>
  <a href="https://github.com/danctorres/modelcmp/releases"><img src="https://img.shields.io/github/v/release/danctorres/modelcmp" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/danctorres/modelcmp" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust 1.88+">
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

By default it shows only the models you can already use; `a` in the TUI or `--all` adds the rest.

## Install

```sh
brew install danctorres/tap/modelcmp
# or
cargo install --git https://github.com/danctorres/modelcmp
```

Linux and macOS; binaries are on the [releases](https://github.com/danctorres/modelcmp/releases)
page. `cargo install` needs Rust 1.88 or later.

## Usage

```sh
modelcmp                                    # the TUI
modelcmp recommend                          # best model per price for each task
modelcmp list --task coding                 # the models worth paying for, cheapest first
modelcmp list --min coding=70 --sort price  # good enough, cheapest first
modelcmp compare sonnet-5 gpt-5             # side by side, with a verdict
modelcmp show sonnet-5                      # everything about one model
modelcmp fav coding sonnet-5                # your model for coding: recommend and agents use it
```

```
$ modelcmp recommend
coding  writing and fixing code  (modelcmp list --task coding)
  best per price:  Gemini 3.7 Flash [gemini37flash] $1.0 (65) · Claude Sonnet 5 [claudesonnet5] $2.8 (68) · Claude Opus 5 [claudeopus5] $7.0 (95)

agentic  multi-step tool use, long autonomous tasks  (modelcmp list --task agentic)
  best per price:  Gemini 3.7 Flash [gemini37flash] $1.0 (77) · Claude Fable 5.1 [claudefable51] $13 (93)
...
```

Picks run cheapest first. Each shows the name, key, $ per 1M tokens and the task score (a
0-100 percentile) in parentheses.

In the TUI, `R` recommends, `space` selects models, `C` compares them and `/` filters. `esc`
goes back, `q` quits and `?` lists every key. [KEYS.md](KEYS.md) covers keys and mouse,
[REFERENCE.md](REFERENCE.md) the columns, commands and JSON fields, and `modelcmp --help` the
flags.

## Your model for each task

Press `f` on a model in the TUI and tick its tasks with `space`, or run
`modelcmp fav <task> <model>`. `modelcmp fav` lists your choices and `modelcmp fav <task> --rm`
clears one. Your choice beats the computed pick: `recommend` marks it `★` and agents use it
first.

A favorite can also cover one tier of a task, so easy work goes to a cheaper model than hard
work: `modelcmp fav coding flash --tier low` makes `--tier low` return Flash while the other
tiers keep your coding favorite. In the TUI, `f` lists each task's tiers under it.

## For agents

```sh
npx skills add danctorres/modelcmp -g                        # teach your agent to ask modelcmp
opencode -m $(modelcmp list --task coding --tier mid --id)   # or ask it yourself
```

`recommend --json` gives your model for a task as its `favorite`, and per tier as
`tier_favorites`; `--tier` returns the tier's, else the task's. It
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
  `B` or `--source aa`: its Intelligence Index, Coding Index, Terminal-Bench, GPQA and HLE,
  and each model's speed (output tokens per second, time to first token).
  Needs a free API key: the TUI's first start asks which source to use, then for the key
  if you pick it (`B` changes it later), or set `ARTIFICIAL_ANALYSIS_API_KEY`. With Epoch AI,
  only its sitemap is read, to link models that have a page there (`o`).
- What you have: the models `opencode models` lists, the `claude`, `codex` and `gemini`
  binaries on `PATH` (each counts as its own provider), and providers whose API key is set
  in the environment. The Via column says which.

Downloaded on first run and cached for 24 hours under `~/.cache/modelcmp/`. Your selection,
exclusions, notes and per-task favorites live in `~/.config/modelcmp/user.json`.

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
