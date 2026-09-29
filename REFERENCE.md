# modelcmp reference

The columns, every command and the JSON fields. The [README](README.md) covers install and
the basics, [KEYS.md](KEYS.md) every TUI key, and `modelcmp --help` (or `modelcmp <command>
--help`) every flag.

## Columns

One row per model. Columns: Model, Dev, Price ($/1M tokens, blended 3:1 input:output with 90% of the input read from the prompt cache, as in an agent session, `%` or `--cache` to change it; providers without a cache price pay full input),
$in, $cache (cached input, $in when a provider has no discount), $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason,
Code/$, Via, Note. Task columns are mean percentiles (0-100) across the task's
benchmarks, ranked against every model Epoch has evaluated:

- Coding: DeepSWE, FrontierCode, SWE-Bench verified, Terminal Bench, WeirdML, MirrorCode, GSO-Bench, Aider polyglot
- Agentic: APEX-Agents, Remote Labor Index, OSWorld 2.0, OSWorld, METR Time Horizons, The Agent Company, DeepResearch Bench, Terminal Bench
- Reason: GPQA diamond, HLE, ARC-AGI-2, ARC-AGI, SimpleBench, Mystery Game Puzzles, Chess Puzzles

`modelcmp recommend` prints the same lists. In the TUI, move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column.

## Commands

```sh
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level scoring 50+, cheapest first
modelcmp list --task coding --tier mid         # just one: the cheapest scoring 75+
modelcmp list --task coding --tier mid --id    # only its provider/model, for opencode -m $(...)
modelcmp list --min coding=70 --sort price     # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp show sonnet-5                         # everything about one model
modelcmp compare sonnet-5 gpt-5 --json         # side by side, with a verdict
modelcmp open sonnet-5 --on epoch              # its web page: models.dev, epoch, artificialanalysis or openrouter (default)
modelcmp select sonnet-5                       # shortlist it, list --selected shows them: --rm to deselect
modelcmp note sonnet-5 "fast enough for refactors" # agents weigh it when choosing; without text shows it, --rm deletes it
modelcmp exclude llama-4-maverick              # have it, can't use it; --rm to include again
modelcmp fav coding sonnet-5                   # your favorite for a task: --tier picks it; alone lists them, --rm clears
modelcmp recommend                             # best model per price for each task, what it measures, when to use it
modelcmp recommend --cache 0                   # priced as one-off prompts: no input read from the prompt cache
```

Tasks: `overall`, `coding`, `value`, `agentic`, `reasoning`, `vision`, `long-context`.
Columns for `--sort`, `--min` and `--max`: `price`, `in`, `out`, `ctx` (thousands of
tokens), `eci`, `coding`, `agentic`, `reasoning`, `value` (the TUI's Code/$). `--task` and
`recommend` leave excluded models out; plain `list` shows them marked `✗`, your selection `✓` and available models `●`. `--tier` picks one
model from the task's list: `low` the cheapest scoring 50+, `mid` the cheapest 75+, `high`
the best; the best when none reaches the floor. A model you `fav` for the task beats the
tier's pick and sits on the task's list marked `★` whether or not it is on the frontier or has a score for
the task, which then shows as `-`. Scores are percentiles among the models
Epoch benchmarked. `--all`
includes models you have no access to. `--refresh` on any command re-downloads first, and `--cache PERCENT` (default 90) sets how much input Price reads from the prompt cache.
`list` prints every match unless `-n` limits it, and then says how many it left out.

Model names match by substring, among the models you have first; the shortest match wins
only when every other contains it (`opus-4.5` over its `-thinking` variant), else the name
is ambiguous: it exits with code 2 and lists the candidates. An unknown `--dev` or `--via` is an error rather
than an empty list.

## Data age

The TUI's bottom border always shows the refresh state: `⟳ refreshing`, `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h; then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`.

```sh
opencode -m $(modelcmp list --task coding --tier mid --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"; each has its frontier and the user's "favorite" key
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite one when set, else the tier's
```

Each JSON model carries `key`, `selected` (on your shortlist), `excluded`, `favorite_for` (the tasks it is the user's favorite
for), `price` (with the provider's model `id`, the string a harness takes, and `cache_read_per_mtok`, null when input is never discounted; `input_per_mtok` and `output_per_mtok` are null when the provider lists no price, which the tables show as `-`, not `free`), `context`,
`eci`, per-task `tasks` percentiles. `show` and
`compare` add every benchmark score and every provider's price. `recommend` prints each
frontier entry as `name [key] $price (value)`, the value being the task's table column: the
ECI for overall, vision and long-context, the percentile for the others. A favorite that is on the line only as
the favorite is marked `not recommended`, and its `recommend --json` entry has `"recommended": false`. An entry carries the user's `note` when the model has one.
