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
kind of task (coding, agentic runs, reasoning, ...), recommends the best model at each price.

- **You** get a TUI to compare models by price and benchmarks, filter and sort them, and see
  which one it recommends for each task. You can shortlist models while you decide, set your
  favorite for a task, exclude the ones you cannot use and write notes on them. From there you
  can also start a harness on a model, or download a local one and run it.
- **Your agent** gets a CLI that gives it your model for the task if you set one. Otherwise it
  gets a model for each of three tiers, cheapest to best, and decides whether the subtask needs
  the top one or a cheaper one will do.

![Filtering to the Opus models, sorting them by price and opening one](https://github.com/user-attachments/assets/ec554a4a-0f82-4a77-91fc-ac2d1e5c155b)

It works with:

- **Harnesses:** opencode, pi, omp, Claude Code, Codex, Gemini CLI, GitHub Copilot CLI
- **Local runners:** ollama, llama.cpp

By default it lists only the models you can already use through them. To see every model,
press `a` in the TUI or pass `--all` on the command line.

## Install

```sh
brew install danctorres/tap/modelcmp
# or
cargo install --git https://github.com/danctorres/modelcmp
```

Linux and macOS, with binaries on the
[releases](https://github.com/danctorres/modelcmp/releases) page. To build from source, with
Rust 1.89 or later:

```sh
git clone https://github.com/danctorres/modelcmp
cd modelcmp
cargo install --path .   # or cargo build --release, for target/release/modelcmp
```

When a newer version is out the TUI says so, and `UU` there upgrades to it.

## Usage

```sh
modelcmp                                   # the TUI
modelcmp recommend                         # best model per price for each task
modelcmp list --task coding                # the models worth paying for, cheapest first
modelcmp compare sonnet-5 gpt-5            # side by side, with a verdict
modelcmp show sonnet-5                     # everything about one model
modelcmp open sonnet-5 --on openrouter     # its page there, or on models.dev, Hugging Face, ...
modelcmp fav coding sonnet-5               # your model for coding
modelcmp get gemma-3-4b-it                 # download a model and run it on your machine
```

In the TUI, `R` recommends, `space` selects models, `C` compares them, `f` sets your favorite
for a task and `/` filters. `enter` shows everything about a model, `o` opens its page on
a site like OpenRouter, `x` starts a harness on it and `t` changes the theme. `?` lists the
keys and `qq` quits.

![The model recommended for each task and tier](https://github.com/user-attachments/assets/bb30bc6d-8b18-4315-a7e2-2d51c9d16c2f)

Each task has three tiers, `low`, `mid` and `high`, from routine work to the hardest. `low` and
`mid` get the cheapest model that scores enough, `high` the best. A favorite you set beats that
pick.

To get a local model, press `a` in the TUI to list every model, then `x` on one and pick its
`download and run` entry, or run `modelcmp get <model>`. Models on your machine show as free.

## For agents

```sh
npx skills add danctorres/modelcmp -g                        # teach your agent to ask modelcmp
modelcmp pick                                                # what the skill runs: every task and tier, with the command to run
opencode -m $(modelcmp list --task coding --tier mid --id)   # or ask it yourself
```

The [skill](skills/modelcmp/SKILL.md) makes your agent run `modelcmp pick` before it names a
model for a subtask. It works in Claude Code, opencode, Codex and any other agent the
[skills CLI](https://skills.sh) supports, and `npx skills update` updates it. An agent without
skills can take its body in `AGENTS.md`.

![An agent asking modelcmp which model opencode should run for a rename](https://github.com/user-attachments/assets/9d4645f8-121f-4eb9-8ec5-eb01e1e94c09)

For scripts, `list`, `show`, `compare` and `recommend` take `--json`, and `list --id` or
`--cmd` prints only the model's id or the command that starts it.

## Data

- [models.dev](https://models.dev): prices, context windows and what each model can do
  (tools, reasoning, vision).
- [Epoch AI](https://epoch.ai/benchmarks) (CC-BY): benchmarks, with no key.
- [Artificial Analysis](https://artificialanalysis.ai): benchmarks and speed, with a free API
  key.

The TUI asks which benchmark source to use on its first start, and `B` changes it later.
Data is cached for 24 hours.

Benchmarks are proxies: a score says how a model did on that test, not how it will do in your
harness.

## More

- [KEYS.md](KEYS.md): every key and mouse action in the TUI.
- [REFERENCE.md](REFERENCE.md): the columns, how each score is obtained, every command,
  favorites and tasks of your own, local models, and the JSON for scripts.
- `modelcmp --help`: every flag.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run what CI runs:

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org).

## License

[MIT](LICENSE). Benchmark data from Epoch AI is licensed CC-BY.
