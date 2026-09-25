# modelcmp

Pick the cheapest LLM that is good enough for the job.

modelcmp joins prices from [models.dev](https://models.dev) with benchmarks from
[Epoch AI](https://epoch.ai/data/ai-benchmarking-hub) and shows only the models you can
already use: the ones your harnesses list (`opencode models`; `claude`, `codex` and
`gemini` count as their own provider) plus providers whose API key is set in the
environment. The VIA column says which.

Run it without arguments for the interactive TUI. Subcommands print text, or JSON with
`--json`, so scripts and agents can ask the same questions.

## Install

```sh
cargo install --path .
```

Data is downloaded on first run and cached for 24 hours under `~/.cache/modelcmp/`.
Favorites and notes live in `~/.config/modelcmp/user.json`.

## TUI

```sh
modelcmp
```

One row per model. Columns: Model, Dev, Price ($/1M tokens, blended 3:1 input:output),
$in, $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason, Math,
Code/$, Via, Best for, Note. Task columns are mean percentiles (0-100) across the task's
benchmarks, ranked against every model Epoch has evaluated. Move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column.

| Key | Action |
|-----|--------|
| `j` `k` | move; a count repeats, as in `3j` |
| `h` `l` | pick a column |
| `s` | sort by the column; again reverses |
| `enter` | details: every benchmark, price per provider |
| `>` `<` | minimum / maximum for the column, e.g. `>` `70` `enter` on Coding |
| `d` | dropdown on the Dev, Price and Via headers (▾); `/` searches it, `space` toggles several |
| `(` `)` `^d` `^u` | half a page up / down |
| `gg` `G` `3gg` | top / bottom / row 3 |
| `m` | mark the model |
| `M` | marked models only; with `F`, marked and favorites |
| `C` | compare marked models: cheapest, best coder, most coding per $ |
| `p` | price frontier on a benchmark column: cheapest first, each row costs more and scores higher |
| `/` | filter by name; `esc` clears |
| `c` | clear filters, bounds, frontier, task ranking and marks |
| `a` | all models, including ones you have no access to |
| `f` `F` | favorite / favorites only |
| `n` | note for the model |
| typing | `←` `→` `^a` `^e` move, `alt-b` `alt-f` by word; `^w` `alt-d` delete a word, `^u` `^k` to the start / end |
| `y` `Y` | copy the model id (`provider/model`) / the model name |
| `o` | open the model on openrouter.ai |
| `x` | launch a harness with the model; quit it to come back |
| `r` | refresh data now |
| `t` | tasks: what each one measures, when to use it, the best model per price; `enter` ranks the table by it |
| `?` | help |
| `qq` | quit; the first `q` asks. `esc` closes an overlay or the filter |

## CLI

```sh
modelcmp list                          # models you have access to
modelcmp list --task coding -n 10      # ranked by task fit
modelcmp list --task coding --frontier # best model per price level, cheapest first
modelcmp list --max-price 2 --via opencode --dev anthropic --dev openai
modelcmp recommend agentic             # top 5 for a task
modelcmp show sonnet                   # everything about one model
modelcmp compare sonnet gpt-5 --json   # side by side, with a verdict
modelcmp open sonnet                   # its web page
modelcmp fav add sonnet
modelcmp note sonnet "fast enough for refactors"
modelcmp tasks                         # what each task measures, when to use it
modelcmp refresh                       # re-download data
```

Tasks: `coding`, `agentic`, `reasoning`, `math`, `knowledge`, `vision`, `long-context`,
`value`, `overall`. `--all` includes models you have no access to. `--refresh` on any
command re-downloads first. Model names match by substring; an ambiguous name exits with
code 2 and lists the candidates.

For agents:

```sh
modelcmp list --task coding --frontier --json | jq '.[0].key'
```

Each JSON model carries `key`, `price`, `context`, `eci`, per-task `tasks` percentiles and
`best_for`. `show` and `compare` add every benchmark score and every provider's price.

## Data

- [models.dev](https://models.dev): prices, context windows, capabilities.
- [Epoch AI Benchmarking Hub](https://epoch.ai/data/ai-benchmarking-hub) (CC-BY): ECI and
  per-benchmark scores.
- `opencode models` and the `claude`, `codex`, `gemini` binaries on `PATH`: what you have.

## Development

```sh
cargo fmt && cargo clippy -- -D warnings && cargo test
```

`app.rs` holds all state and key handling with no I/O, so every key is unit tested.
`tui.rs` owns the terminal and the refresh thread, `cli.rs` the subcommands, `data.rs`
the sources and cache, `fit.rs` the task scores.
