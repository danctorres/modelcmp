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
Favorites, marks, exclusions and notes live in `~/.config/modelcmp/user.json`.

## TUI

```sh
modelcmp
```

One row per model. Columns: Model, Dev, Price ($/1M tokens, blended 3:1 input:output),
$in, $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason,
Code/$, Via, Note. Task columns are mean percentiles (0-100) across the task's
benchmarks, ranked against every model Epoch has evaluated. Move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column.

| Key | Action |
|-----|--------|
| `j` `k` | move; a count repeats, as in `3j`; past the last row back to the first |
| `h` `l` | pick a column, scrolling the ones right of Dev; `‹` `›` mark columns off screen, `▲` `▼` on the left border rows above or below; in compare and recommend, pick a model for `o` `x` `y` `f` `e` `n`, with the same marks for models off screen |
| `0` `$` `w` `b` | first / last column; next / previous group: prices, benchmarks, Via; in compare and recommend, `0` `$` pick the first / last model |
| `s` | sort by the column; again reverses |
| `enter` | details: every benchmark, price per provider |
| `>` `<` | minimum / maximum for the column, e.g. `>` `70` `enter` on Coding |
| `d` | dropdown on the Dev, Price and Via headers (▾); `/` searches it, `space` toggles several |
| `(` `)` `^d` `^u` | half a page up / down; overlays and dropdowns mark lines off screen with the same `▲` `▼` |
| `gg` `G` `3gg` | top / bottom / row 3 |
| `m` | mark the model |
| `V` | select a range of rows: move to extend it, then `m` `e` `f` or `C` act on all of it; `esc` cancels |
| `e` `f` on a mark | act on every marked model, not just the one under the cursor |
| `M` | marked models only; with `F`, marked and favorites |
| `C` | compare marked models: cheapest, best coder, most coding per $ |
| `/` | filter by name, developer, Via or note, words in any order (`anthropic opus`), a typo forgiven when nothing matches (`opsu`); `/` again starts a new search, `esc` clears; in compare, filters the rows |
| `c` | clear filters, bounds, task and marks |
| `a` | all models, including ones you have no access to |
| `f` `F` | favorite / favorites only |
| `n` | note for the model |
| `e` | exclude the model: you have it but cannot use it. It stays in the table, struck through, but recommendations (`R` and `--task`) skip it |
| typing | `←` `→` `^a` `^e` move, `alt-b` `alt-f` `^←` `^→` by word; `^w` `alt-d` delete a word, `^u` `^k` to the start / end |
| `y` `Y` | copy the model id (`provider/model`) / the model name |
| `o` | open the model on openrouter.ai |
| `x` | open a harness on the model in a new terminal (Windows Terminal under WSL, else `$TERMINAL`); asks which when several have it |
| `r` | refresh data now |
| `R` | recommend: the best model per price for each task, what it measures and when to use it; `h` `l` pick a model on the task's line for `o` `x` `y` `f` `e` `n`; `enter` shows the task's models in the table, best first, each row cheaper and scoring lower |
| `?` | help |
| mouse | click a row to select it, again for details; ctrl click adds or removes it from the selection, shift click or a drag selects a range, a plain click drops the selection, right click marks it; a header sorts, its ▾ opens the dropdown, where clicks toggle entries until a click elsewhere; the wheel scrolls, sideways moves the column, or the model in compare and recommend |
| `qq` | quit; the first `q` asks. `esc` closes an overlay or the filter |

## CLI

```sh
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level scoring 50+, cheapest first
modelcmp list --task coding --tier mid         # just one: the cheapest scoring 75+
modelcmp list --min coding=70 --sort price     # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp show sonnet                           # everything about one model
modelcmp compare sonnet gpt-5 --json           # side by side, with a verdict
modelcmp open sonnet                           # its web page
modelcmp fav sonnet                            # --rm to remove
modelcmp note sonnet "fast enough for refactors"   # without text shows it, --rm deletes it
modelcmp exclude llama                         # have it, can't use it; --rm to include again
modelcmp recommend                             # best model per price for each task, what it measures, when to use it
```

Tasks: `overall`, `coding`, `value`, `agentic`, `reasoning`, `vision`, `long-context`.
Columns for `--sort`, `--min` and `--max`: `price`, `in`, `out`, `ctx` (thousands of
tokens), `eci`, `coding`, `agentic`, `reasoning`, `value` (the TUI's Code/$). `--task` and
`recommend` leave excluded models out; plain `list` shows them marked `✗`. `--tier` picks one
model from the task's list: `low` the cheapest scoring 50+, `mid` the cheapest 75+, `high`
the best; the best when none reaches the floor. Scores are percentiles among the models
Epoch benchmarked. `--all`
includes models you have no access to. `--refresh` on any command re-downloads first.
`list` prints every match unless `-n` limits it, and then says how many it left out.

Model names match by substring, among the models you have first; an ambiguous name exits
with code 2 and lists the candidates. An unknown `--dev` or `--via` is an error rather
than an empty list.

For agents:

```sh
modelcmp recommend --json                      # choose the task by its "when"; each has its frontier
modelcmp list --task coding --tier mid --json  # the one model to use
```

Each JSON model carries `key`, `excluded`, `price` (with the provider's model `id`, the
string a harness takes), `context`, `eci`, per-task `tasks` percentiles. `show` and
`compare` add every benchmark score and every provider's price. `recommend` prints each
frontier entry as `name [key] $price (percentile)`.

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
