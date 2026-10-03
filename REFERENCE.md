# modelcmp reference

The columns, every command and the JSON fields. The [README](README.md) covers install and
the basics, [KEYS.md](KEYS.md) every TUI key, and `modelcmp --help` (or `modelcmp <command>
--help`) every flag.

## Columns

One row per model. Columns: Model, Dev, Released (year and month), Price ($/1M tokens, blended 3:1 input:output with 90% of the input read from the prompt cache, as in an agent session, `%` or `--cache` to change it; providers without a cache price pay full input),
$in, $cache (cached input, $in when a provider has no discount), $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason,
Code/$, Via, Notes. With Artificial Analysis as the source (`B`, `--source aa`), ECI is AAII,
its Intelligence Index, and each task column is one benchmark's score (0-100): Coding the
Coding Index, Agentic Terminal-Bench 4.0, Reason HLE; two more columns show speed,
Tok/s (output tokens per second) and TTFT (seconds to the first token), medians across
providers. A model's scores there are its best reasoning setting's; a row that names a setting
("Grok 4.20 Non-Reasoning") has that setting's, or `-` when Artificial Analysis did not measure it. Epoch does not measure speed, so they are hidden with it. With Epoch AI, the default, task
columns are the model's capability on the task's benchmarks, in ECI points, fitted from its
ECI and its scores (a model with few scores stays near its ECI):

- Coding: DeepSWE, FrontierCode, FrontierSWE, SWE-Bench verified, Terminal Bench, WeirdML, MirrorCode, GSO-Bench
- Agentic: APEX-Agents, Remote Labor Index, OSWorld 2.0, DeepResearch Bench, Terminal Bench
- Reason: GPQA diamond, HLE, ARC-AGI-2, ARC-AGI, SimpleBench, Mystery Game Puzzles, Chess Puzzles, LMCA, DTBench

The capability starts at the model's ECI and moves toward what its scores on those benchmarks
say, using Epoch's difficulty and slope for each benchmark, with guessing counted as 0. One score
moves it a little, many that agree move it more. A model without an ECI starts from a fit to
all its scores.

`modelcmp recommend --json` gives the same lists as `benchmarks`. In the TUI, move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column, and in a row of compare.

## Commands

```sh
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level in the top half, cheapest first
modelcmp list --task coding --tier mid         # just one: the cheapest in the top quarter
modelcmp list --task coding --tier mid --id    # only its provider/model, for opencode -m $(...)
modelcmp list --min coding=155 --sort price    # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp show sonnet-5                         # everything about one model
modelcmp compare sonnet-5 gpt-5 --json         # side by side, with a verdict
modelcmp open sonnet-5 --on epoch              # its web page: models.dev, epoch, aa or openrouter; without --on, the first to have one
modelcmp select sonnet-5                       # shortlist it, list --selected shows them: --rm to deselect
modelcmp note sonnet-5 "fast enough for refactors" # agents weigh it when choosing; without text shows it, --rm deletes it
modelcmp exclude llama-4-maverick              # have it, can't use it; --rm to include again
modelcmp fav coding sonnet-5                   # your favorite for a task: --tier picks it; alone lists them, --rm clears
modelcmp fav coding 3.7-flash --tier low           # your favorite for one tier: list --tier low picks it over the task's
modelcmp fav debugging opus-5.5 --about "finding and fixing a bug"  # a task of your own: its model and what it is about; --rm clears the model
modelcmp fav debugging --about "bugs and flaky tests"               # rewrite what it is about; "" clears it
modelcmp fav debugging 3.7-flash --tier low    # its model for one tier: list --task debugging --tier low returns it
modelcmp list --task debugging --id            # that model's provider/model
modelcmp fav debugging --rename triage         # another name for a task of your own; its models stay
modelcmp recommend                             # best model per price for each task, what it measures, when to use it
modelcmp recommend --cache 0                   # priced as one-off prompts: no input read from the prompt cache
```

Tasks: `overall`, `coding`, `value`, `agentic`, `reasoning`, `vision`. For a large prompt, bound the context instead: `--min ctx=200`.
Any other name given to `fav` is a task of your own (`debugging`, `"Tool Dispatch"` is stored as
`tool-dispatch`). No benchmark ranks it: it has the models you give it, one for the task and one
per tier if you like. `--task` lists them, cheapest first, and with `--tier` returns that tier's,
else the task's, leaving out one you excluded or lost access to. `recommend` lists it after the
built-in tasks, with what you wrote it is about and each model's tier, `(low)`. It is the task to
pick for work that fits it, even when a built-in task fits too: for a bug, your `debugging` over
`coding`. It is gone when its last model is cleared, and `--rename` gives it another name, one no
task has yet.
Columns for `--sort`, `--min` and `--max`: `price`, `in`, `cache`, `out`, `ctx` (thousands of
tokens), `release` (a date: `--min release=2026-06` is June 2026 on, `--max release=2026-06` up to the end of June), `eci`, `coding`, `agentic`, `reasoning`, `value` (the TUI's Code/$), and with Artificial Analysis `tps` and `ttft`. `--task` and
`recommend` leave excluded models out; plain `list` shows them marked `✗`, and your selection `✓`. `--tier` picks one
model from the task's list: `low` the cheapest in the top half of the models the source
evaluated, `mid` the cheapest in the top quarter, `high` the best; the best when none reaches the floor. A model you `fav` for the tier, else for the task, beats the
tier's pick. Every favorite of the task sits on the task's list, marked `★` in `recommend`, whether or not it is on the frontier or has a score for
the task, which then shows as `-`. `--all`
includes models you have no access to. `--refresh` on any command re-downloads first, `--cache PERCENT` (default 90) sets how much input Price reads from the prompt cache, and `--source epoch|aa` which benchmarks to use for this run (default: the one picked with `B`).
`list` prints every match unless `-n` limits it, and then says how many it left out. `--id` prints the id opencode takes, or pi when only pi has the model, else the id at the provider you'd pay. With no match it is an error (exit code 1), so a harness is never started on an empty model.

Model names match by substring, among the models you have first; the shortest match wins
only when every other contains it (`opus-4.5` over its `-thinking` variant), else the name
is ambiguous: it exits with code 3 and lists the candidates. An unknown `--dev` or `--via` is an error rather
than an empty list.

## Data age

The TUI's bottom border always shows the refresh state: `⟳ refreshing 7/9, waiting for opencode` (the downloads
and harnesses that answered, and the last one or two still awaited), `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h; then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`.

```sh
opencode -m $(modelcmp list --task coding --tier mid --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"; each has its frontier, the user's "favorite" and "tier_favorites"
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite for the tier or task when set, else the tier's
```

Each JSON model carries `key`, `selected` (on your shortlist), `excluded`, `favorite_for` (the tasks it is the user's favorite
for, and `task:tier` for a tier's), `price` (with the provider's model `id`, which opencode takes after `provider/`, and `cache_read_per_mtok`, null when input is never discounted; `input_per_mtok` and `output_per_mtok` are null when the provider lists no price, which the tables show as `-`, not `free`), `context`,
`eci` (the overall index of `source`, `epoch` or `aa`), per-task `tasks` (each task's table column: ECI points with `epoch`, the benchmark's 0-100 score with `aa`, overall and vision the `eci`, value a 0-100 percentile), and with `aa` `tokens_per_second` and `ttft_seconds`. `show` and
`compare` add `benchmarks` (every score, as a fraction: 0.545 is 54.5%), `providers` (every provider's price) and `pages` (site to URL, for the sites that have a page for it; `url` is the first of them, null when none has). `recommend` prints each
frontier entry as `name [key] $price (value)`, the value being the task's table column: the
same as `tasks`. A favorite that is on the line only as
the favorite is marked `not recommended`, and its `recommend --json` entry has `"recommended": false`. An entry carries its `context` in tokens, and the user's `note` when the model has one.
A task of your own follows the built-in ones in `recommend --json` with the same fields and
`"custom": true`: its `about` is what the user wrote (`your own task` when nothing), its `when`
says it wins over a built-in task that fits too, its `favorite` and `tier_favorites` are the
models the user gave it, and its `frontier` lists them, cheapest first, with a null `score`.
