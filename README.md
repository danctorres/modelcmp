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

`modelcmp` puts model benchmarks and prices side by side and recommends the best model at each
price for each task.

- **You** get a TUI to compare models, set your favorites and see the recommended ones for each task.
- **Your agent** gets a CLI to find your favorite or the recommended model for each task and price
  tier. An orchestrator in one harness (e.g. `Opus 5.5` in `Claude Code`) can then hand a subtask to
  a cheaper model in another (e.g. `GLM-5.3` in `pi`), or to a local one (e.g. `Qwen3.8 27B` in `ollama`).

![Filtering to the Opus models, sorting them by price and opening a model's details](https://github.com/user-attachments/assets/ec554a4a-0f82-4a77-91fc-ac2d1e5c155b)

<p align="center"><i>Filter to the Opus models, sort by price and open a model's details.</i></p>

![Listing the best models for coding, comparing two and printing the command that starts one](https://github.com/user-attachments/assets/31cfd9f0-b286-4fac-ac17-f61caca59631)

<p align="center"><i>List the best models for coding, compare two, print the command that starts one.</i></p>

It works with:

- **Harnesses:** `opencode`, `pi`, `omp`, `Claude Code`, `GitHub Copilot CLI`, `Codex`
- **Harnesses not tested yet:** `Gemini CLI`
- **Local runners:** `ollama`, `llama.cpp`

## Install

```sh
brew install danctorres/tap/modelcmp
# or
cargo install --locked --git https://github.com/danctorres/modelcmp
```

Linux and macOS are supported.
The binaries are provided on the [releases](https://github.com/danctorres/modelcmp/releases) page.

Building from source requires Rust 1.89 or later:

```sh
git clone https://github.com/danctorres/modelcmp
cd modelcmp
cargo install --locked --path .   # or cargo build --release, for target/release/modelcmp
```

The TUI says when a newer version is out, and `UU` upgrades to it.

To let your agent use it, install the skill too:

```sh
npx skills add danctorres/modelcmp -g
```

## Usage

### TUI

```sh
modelcmp   # start the TUI
```

Navigate with vim keys, the arrow keys or the mouse. `R` recommends, `/` filters, `space` selects
models and `C` compares them. `enter` shows everything about a model, `f` makes it your favorite
for a task and `x` starts it in a harness, or downloads and runs a local one. `?` lists every key,
and `qq` quits.

![The model recommended for each task and tier](https://github.com/user-attachments/assets/781a6bce-e79b-4a00-af14-4cdd3c3767c5)

<p align="center"><i>The model recommended for each task and tier.</i></p>

In recommend, the tier header changes a tier's price cap. To add a task of your own, pick
`+ new task` there or in the grid `f` opens on a model.

To get a local model, press `a` to list every model, then `x` on one and pick its
`download and run` entry. Via shows how big each download is.
Models on your machine show as free.

### CLI

```sh
modelcmp recommend                         # best model per price for each task
modelcmp list --task coding                # best model per price for coding, cheapest first
modelcmp compare sonnet-5 gpt-5            # side by side, with a verdict
modelcmp show sonnet-5                     # everything about one model
modelcmp fav coding sonnet-5 --tier mid    # set your favorite for coding's mid tier
modelcmp get gemma-3-4b-it                 # download a model and run it on your machine
```

A task of your own needs a description in `--about`, and agents choose a task by it:

```sh
modelcmp fav debugging opus-5.5 --tier high --about "finding and fixing a bug"   # a new task, with its model
modelcmp fav debugging gemini-3.7-flash --tier low                               # a cheaper model for its low tier
```

### How recommendations work

Recommendations come from each task's Pareto frontier of price and score. A model with no
benchmark score for a task is never recommended for it, but you can still make it your favorite.

Each task has four tiers, `free`, `low`, `mid` and `high`. `free` gets the best free model, `low`
the best up to $2 per 1M tokens, `mid` the best up to $5 and `high` the best at any price. A
favorite you set for a tier beats its pick when it costs no more than the cap. Your own tasks show
up last in recommend.

## For agents

The [skill](skills/modelcmp/SKILL.md), set up in [Install](#install), lets an orchestrator agent choose
a model for each subtask and hand the subtask to it.

![An agent using the modelcmp skill to pick a free model for the unit tests and the best one for a race condition](https://github.com/user-attachments/assets/d2c5349e-0cc6-475c-b978-9897ca203bdf)

<p align="center"><i>An agent uses the modelcmp skill to pick a free model for the unit tests and the best one for a race condition.</i></p>

## Data

- [models.dev](https://models.dev): prices, context windows and what each model can do
  (tools, reasoning, vision).
- [Epoch AI](https://epoch.ai/benchmarks) (CC-BY): benchmarks, and it needs no API key.
- [Artificial Analysis](https://artificialanalysis.ai): benchmarks and speed, and it needs a
  free API key.
- [Arena](https://arena.ai/leaderboard/agent) (CC-BY): how models do in real agent sessions,
  chat and web development (the Arena column), and it needs no API key.

The first time you start the TUI, it asks for an Artificial Analysis API key.
With a key, recommendations use Artificial Analysis benchmarks, otherwise Epoch AI's.
Artificial Analysis covers more models than Epoch AI and also measures speed.
Data is cached for 24 hours.

## More

- [KEYS.md](KEYS.md): every key and mouse action in the TUI.
- [REFERENCE.md](REFERENCE.md): detailed information about the TUI and CLI.
- `modelcmp --help`: available options.

## License

[MIT](LICENSE). Benchmark data from Epoch AI and Arena is licensed CC-BY.
