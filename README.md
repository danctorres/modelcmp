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
Marks, exclusions, notes and per-task favorites live in `~/.config/modelcmp/user.json`.

## TUI

```sh
modelcmp
```

One row per model. Columns: Model, Dev, Price ($/1M tokens, blended 3:1 input:output with 90% of the input read from the prompt cache, as in an agent session; providers without a cache price pay full input),
$in, $cache (cached input, $in when a provider has no discount), $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason,
Code/$, Via, Note. Task columns are mean percentiles (0-100) across the task's
benchmarks, ranked against every model Epoch has evaluated:

- Coding: DeepSWE, FrontierCode, SWE-Bench verified, Terminal Bench, WeirdML, MirrorCode, GSO-Bench, Aider polyglot
- Agentic: APEX-Agents, Remote Labor Index, OSWorld 2.0, OSWorld, METR Time Horizons, The Agent Company, DeepResearch Bench, Terminal Bench
- Reason: GPQA diamond, HLE, ARC-AGI-2, ARC-AGI, SimpleBench, Mystery Game Puzzles, Chess Puzzles

`modelcmp recommend` prints the same lists. Move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column.

| Key | Action |
|-----|--------|
| `j` `k` | move; a count repeats, as in `3j`; past the last row back to the first |
| `h` `l` | pick a column, scrolling the ones right of Dev; `‹` `›` mark columns off screen, `▲` `▼` on the left border rows above or below; in compare and recommend, pick a model for `o` `x` `y` `f` `e` `n`, with the same marks for models off screen |
| `0` `_` `$` `w` `b` | first / last column; next / previous group: prices, benchmarks, Via; in compare and recommend, `0` `_` `$` pick the first / last model |
| `s` | sort by the column; again reverses |
| `enter` | details: every benchmark, price per provider |
| `>` `<` | minimum / maximum for the column, e.g. `>` `70` `enter` on Coding |
| `d` | dropdown on the Dev, Price and Via headers (▾); `/` searches it, as in every list, `m` toggles several, as it marks models |
| `(` `)` `^d` `^u` | half a page up / down; overlays and dropdowns mark lines off screen with the same `▲` `▼` |
| `gg` `G` `3gg` | top / bottom / row 3 |
| `m` | mark the model: its box `☐` becomes `✓` in light blue; the shortlist you are deciding between, kept until you unmark it. A click on the box toggles it |
| `U` | unmark every model |
| `V` | select a range of rows: move to extend it, then `m` `e` or `C` act on all of it; `esc` cancels |
| `e` on a mark | act on every marked model, not just the one under the cursor |
| `M` | marked models only; `M` again or `esc`: every model |
| `F` | favorite models only, the rows with a `★` (the picked task's favorite when a task is picked); `F` again or `esc`: every model |
| `E` | excluded models only, the rows with a red `✗`; `E` again or `esc`: every model |
| `C` | compare 2+ marked models: cheapest, best coder, most coding per $; `h` `l` pick a model, and the table's bar follows it; `C` again or `esc` closes it |
| `/` | filter by name, developer, Via or note, words in any order (`anthropic opus`), a typo forgiven when nothing matches (`opsu`); `/` again starts a new search, `esc` clears; in compare it filters the rows, in the help its lines, and in a list like the theme panel its entries; what matched is underlined in yellow everywhere |
| `c` | clear filters, bounds, task, `M`, `F` and `E`; marks stay |
| `a` | all models, including ones you have no access to |
| `n` | note for the model |
| `e` | exclude the model: you have it but cannot use it. It stays in the table, greyed out, but recommendations (`R` and `--task`) skip it. Every row has a `·`, a red `✗` when excluded, and a click on it toggles the exclusion of that row alone, even on a mark |
| `f` | favorite the model for tasks. Every row has a `☆`, filled bold `★` for a favorite: with a task picked, that task's favorite in the task's colour, the colour of its name in recommend; with none, a gold `★` for the favorite of any task. `f` or a click on the `☆` lists the tasks with `☐`/`✓`: `m` or a click ticks one, `enter` ticks the one under the bar and closes; the list starts on the picked task, or the task under the cursor in recommend. The status bar names the tasks the model under the cursor is the favorite for. The model joins the task's line even off the price frontier, and `--tier` picks it |
| typing | `←` `→` `^a` `^e` move, `alt-b` `alt-f` `^←` `^→` by word; `^w` `alt-d` delete a word, `^u` `^k` to the start / end |
| `y` `Y` | copy the model id (`provider/model`) / the model name |
| `o` | open the model on models.dev, epoch.ai or openrouter.ai; asks which |
| `x` | open a harness on the model in a new terminal (Windows Terminal under WSL, else `$TERMINAL`); asks which when several have it |
| `r` | refresh data now |
| `R` | recommend: the best model per price for each task, what it measures and when to use it; `h` `l` pick a model on the task's line for `o` `x` `y` `f` `e` `n`, and the table's bar follows it, so `esc` lands on it; `enter` shows the task's models in the table, best first, each row cheaper and scoring lower |
| `t` | theme panel: `j` `k` preview, `/` searches, `enter` saves, `esc` or `t` closes. `terminal` (its own colours, the default), then the dark `gruvbox`, `nord`, `catppuccin`, `dracula`, `tokyonight`, `kanagawa`, `monokai`, `rose-pine`, `github`, `solarized`, `synthwave` and `cyberpunk`, the light `github-light`, `gruvbox-light`, `paper` and `sepia`, and the retro `amber`, `phosphor`, `c64` and `gameboy`; saved with your marks. A theme sets the 16 terminal colours, the text colour and 5 to 14 colours for developers, each of which has to read on that theme's background and stay apart from the others. Every theme paints its own background, so a dark one stays dark on a light terminal; only `terminal` keeps a transparent background |
| `?` | help; `/` keeps the lines that match |
| mouse | click a row to select it, again for details; ctrl click adds or removes it from the selection, shift click or a drag selects a range, a plain click drops the selection, right click marks it, a click on its `☐` toggles the mark, on its `☆` picks its tasks, on its `✗` box excludes it; a click on the `#` header goes to the first row, on the `✓` header shows marked models only, on the `★` header favorites only, on the `✗` header excluded only; a header sorts, its ▾ opens the dropdown, where clicks toggle entries until a click elsewhere; a click on a key hint in the status bar presses that key; the wheel scrolls, sideways moves the column, or the model in compare and recommend |
| `qq` | quit; the first `q` asks. `esc` goes back: closes an overlay, drops the selection, clears the `/` filter, leaves `M`, then `F`, then `E`, then a task picked in recommend back to recommend |

## CLI

```sh
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level scoring 50+, cheapest first
modelcmp list --task coding --tier mid         # just one: the cheapest scoring 75+
modelcmp list --task coding --tier mid --id    # only its provider/model, for codex -m $(...)
modelcmp list --min coding=70 --sort price     # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp show sonnet                           # everything about one model
modelcmp compare sonnet gpt-5 --json           # side by side, with a verdict
modelcmp open sonnet --on epoch                # its web page: models.dev, epoch or openrouter (default)
modelcmp mark sonnet                           # shortlist it, list --marked shows them: --rm to unmark
modelcmp note sonnet "fast enough for refactors"   # without text shows it, --rm deletes it
modelcmp exclude llama                         # have it, can't use it; --rm to include again
modelcmp fav coding sonnet                     # your favorite for a task: --tier picks it; alone lists them, --rm clears
modelcmp recommend                             # best model per price for each task, what it measures, when to use it
```

Tasks: `overall`, `coding`, `value`, `agentic`, `reasoning`, `vision`, `long-context`.
Columns for `--sort`, `--min` and `--max`: `price`, `in`, `out`, `ctx` (thousands of
tokens), `eci`, `coding`, `agentic`, `reasoning`, `value` (the TUI's Code/$). `--task` and
`recommend` leave excluded models out; plain `list` shows them marked `✗`, your marks `✓` and available models `●`. `--tier` picks one
model from the task's list: `low` the cheapest scoring 50+, `mid` the cheapest 75+, `high`
the best; the best when none reaches the floor. A model you `fav` for the task beats the
tier's pick and sits on the task's list marked `★` whether or not it is on the frontier. Scores are percentiles among the models
Epoch benchmarked. `--all`
includes models you have no access to. `--refresh` on any command re-downloads first. The
TUI's bottom border always shows the refresh state: `⟳ refreshing`, `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h; then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.
`list` prints every match unless `-n` limits it, and then says how many it left out.

Model names match by substring, among the models you have first; an ambiguous name exits
with code 2 and lists the candidates. An unknown `--dev` or `--via` is an error rather
than an empty list.

For agents:

```sh
modelcmp recommend --json                      # choose the task by its "when"; each has its frontier and the user's "favorite" key
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite one when set, else the tier's
opencode -m $(modelcmp list --task coding --tier mid --id)   # --id prints just provider/model
```

Each JSON model carries `key`, `excluded`, `favorite_for` (the tasks it is the user's favorite
for), `price` (with the provider's model `id`, the string a harness takes, and `cache_read_per_mtok`, null when input is never discounted), `context`,
`eci`, per-task `tasks` percentiles. `show` and
`compare` add every benchmark score and every provider's price. `recommend` prints each
frontier entry as `name [key] $price (value)`, the value being the task's table column: the
ECI for overall, vision and long-context, the percentile for the others.

## Data

- [models.dev](https://models.dev): prices, context windows, capabilities.
- [Epoch AI Benchmarking Hub](https://epoch.ai/data/ai-benchmarking-hub) (CC-BY): ECI and
  per-benchmark scores.
- `opencode models` and the `claude`, `codex`, `gemini` binaries on `PATH`: what you have.

## Development

```sh
cargo fmt && cargo clippy -- -D warnings && cargo test
```

CI runs the same on every push. Pushing a `v*` tag builds a release binary and attaches it
to a GitHub release.

`app.rs` holds all state and key handling with no I/O, so every key is unit tested.
`tui.rs` owns the terminal and the refresh thread, `cli.rs` the subcommands, `data.rs`
the sources and cache, `fit.rs` the task scores.
