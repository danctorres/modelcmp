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

Frontier models can cost ten times as much as smaller ones, and many tasks do not need them.
`modelcmp` puts prices and benchmarks side by side and, for each kind of task (coding, agentic
runs, reasoning, ...), recommends the best model at each price. It reads the harnesses you have
installed, shows which one has each model, and starts it on the model you pick.

- **You** get a TUI to filter, sort, compare and see recommended models for each task.
  You can set your favorite model for a task, exclude models you cannot use, write notes, start a
  harness or download and run a local model.
- **Your agent** gets a CLI to find the favorite or recommended model for a task,
  for each price tier. In a multi-agent setup, the CLI allows an orchestrator in one
  harness (e.g. `Opus 5.5` in `Claude Code`) to find and hand a subtask to a cheaper model in another harness (e.g.
  `DeepSeek V4 Pro` in `pi`), or even to local one (e.g. `Qwen3.8 Flash Next` in `llama.cpp`).

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

## Usage

```sh
modelcmp                                   # start the TUI
modelcmp recommend                         # best model per price for each task
modelcmp list --task coding                # best model per price for coding, cheapest first
modelcmp compare sonnet-5 gpt-5            # side by side, with a verdict
modelcmp show sonnet-5                     # everything about one model
modelcmp open sonnet-5 --on openrouter     # its page there, or on models.dev, Hugging Face, ...
modelcmp fav coding sonnet-5               # set your favorite for coding
modelcmp get gemma-3-4b-it                 # download a model and run it on your machine
```

In the TUI, `R` recommends, `space` selects models, `C` compares them, `f` sets your favorite
for a task and `/` filters. `enter` shows everything about a model, `o` opens its page on
a site like OpenRouter, `x` starts a harness on it and `t` changes the theme. `?` lists the
keys and `qq` quits.

![The model recommended for each task and tier](https://github.com/user-attachments/assets/781a6bce-e79b-4a00-af14-4cdd3c3767c5)

<p align="center"><i>The model recommended for each task and tier.</i></p>

Each task has four tiers, `free`, `low`, `mid` and `high`. `free` gets the best free model, `low`
the best up to $2 and `mid` up to $5 per 1M tokens, and `high` the best one. A favorite you set
beats that pick.

To get a local model, press `a` in the TUI to list every model, then `x` on one and pick its
`download and run` entry, or run `modelcmp get <model>`. Models on your machine show as free.

## For agents

```sh
npx skills add danctorres/modelcmp -g                        # install the skill for your agent
modelcmp pick                                                # what the skill runs: every task and tier, with the command to run
```

The [skill](skills/modelcmp/SKILL.md) makes your agent run `modelcmp pick` before it names a
model for a subtask.

![An agent using the modelcmp skill to pick a free model for the unit tests and the best one for a race condition](https://github.com/user-attachments/assets/d2c5349e-0cc6-475c-b978-9897ca203bdf)

<p align="center"><i>An agent uses the modelcmp skill to pick a free model for the unit tests and the best one for a race condition.</i></p>

## Data

- [models.dev](https://models.dev): prices, context windows and what each model can do
  (tools, reasoning, vision).
- [Epoch AI](https://epoch.ai/benchmarks) (CC-BY): benchmarks and what a coding task cost, and it
  needs no API key.
- [Artificial Analysis](https://artificialanalysis.ai): benchmarks and speed, and it needs a
  free API key.
- [Arena](https://arena.ai/leaderboard/agent) (CC-BY): how models rank in real agent sessions,
  the Arena column, and it needs no API key.

When the TUI is first started, it prompts the user for an Artificial Analysis API key.
If a key is provided, the recommended models are based on Artificial Analysis benchmarks, else Epoch AI is used.
Artificial Analysis provides more information (including performance metrics) and covers more models than Epoch AI.
Data is cached for 24 hours.

## More

- [KEYS.md](KEYS.md): every key and mouse action in the TUI.
- [REFERENCE.md](REFERENCE.md): detailed information about the TUI and CLI.
- `modelcmp --help`: available options.

## License

[MIT](LICENSE). Benchmark data from Epoch AI and Arena is licensed CC-BY.
