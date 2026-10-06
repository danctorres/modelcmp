# modelcmp reference

The columns, every command and the JSON fields. The [README](README.md) covers install and
the basics, [KEYS.md](KEYS.md) every TUI key, and `modelcmp --help` (or `modelcmp <command>
--help`) every flag.

## Columns

One row per model. Columns: Model, Dev, Released (year and month), Price ($/1M tokens, blended 3:1 input:output with 90% of the input read from the prompt cache, as in an agent session, `%` or `--cache` to change it; providers without a cache price pay full input; a provider that charges more past a context size is priced as at 100k tokens; a price after a `~` is the list price of other providers, yours listing none),
$in, $cache (cached input, $in when a provider has no discount), $out, Ctx, ECI (Epoch Capabilities Index), Coding, Agentic, Reason,
Value, Via, Notes. With Artificial Analysis as the source (`B`, `--source aa`), ECI is AAII,
its Intelligence Index, and each task column is a benchmark score (0-100): Coding the
mean of Terminal-Bench 4.0 and SciCode, Agentic Terminal-Bench 4.0, Reason HLE; two more columns show speed,
Tok/s (output tokens per second) and TTFT (seconds to the first answer token, after any thinking), medians
on the developer's own API, or across providers when it has none. A model's scores and speed there are those of one reasoning setting, the best index among those scored on the most benchmarks; a row that names a setting
("Grok 4.20 Non-Reasoning") has that setting's, or `-` when Artificial Analysis did not measure it, and one that cannot reason has its non-reasoning setting's. With either source a row has no scores when its offers are another release than the one scored, as a `-latest` id is. Epoch does not measure speed, so they are hidden with it. With Epoch AI, the default, task
columns are the model's capability on the task's benchmarks, in ECI points, fitted from its
ECI and its scores (a model with few scores stays near its ECI):

- Coding: DeepSWE, FrontierCode, FrontierSWE, WeirdML, MirrorCode
- Agentic: APEX-Agents, Remote Labor Index, OSWorld 2.0
- Reason: GPQA diamond, HLE, ARC-AGI-2, ARC-AGI, SimpleBench, Mystery Game Puzzles, Chess Puzzles, LMCA, DTBench

[Scores](#scores) says how each of these numbers is obtained.

`modelcmp recommend --json` gives the same lists as `benchmarks`; with Artificial Analysis, the one field the score is. In the TUI, move the column cursor and the
top border says what the column means. Green and red mark the best and worst value in a
column, and in a row of compare.

## Scores

Where the numbers in the score columns come from. Both sources are downloaded on a refresh, Artificial Analysis only with its key, so switching between them needs no second refresh. Each source's sitemap is read too, with or without a key, to link the models that have a page there (`o`), and OpenRouter's list of models for the Hugging Face repo it names for each.

Epoch AI (the default without an Artificial Analysis key, <https://epoch.ai/data/benchmark_data.zip>, no key):

- ECI is Epoch's Capabilities Index as Epoch publishes it. modelcmp does not compute it.
- A benchmark score is the best one Epoch lists for any version of the model, taken as a share
  of the way from guessing (Epoch's random baseline) to the benchmark's ceiling.
- Coding, Agentic and Reason are computed by modelcmp, in ECI points. Each starts at the
  model's ECI and moves toward what its scores on the task's benchmarks say, using the
  difficulty and slope Epoch publishes for each benchmark. One score moves it a little, many
  that agree move it more. A model without an ECI starts from a fit to all its scores, and
  has task scores only when it was run on at least four of the benchmarks listed above.
- A task score stays close to the ECI. On 2026-10-05 half of them were within 0.3 points of
  their model's ECI and nine in ten within 1 point. Epoch fits all its benchmarks with one
  capability per model, the ECI, and most models were run on a single coding or agentic
  benchmark. So a task column reorders models of similar ECI and does not set a model far
  from where its ECI puts it.
- Only benchmarks Epoch still runs on new models count toward a task.

Artificial Analysis (`--source aa`, its API with your key):

- AAII is its Intelligence Index as it publishes it.
- Agentic and Reason are one benchmark's score as published, 0-100. Coding is the mean of
  two, computed by modelcmp. Tok/s and TTFT are its measurements.
- A model's numbers are those of one of its reasoning settings, chosen as [Columns](#columns) says.
- It scores more models than Epoch AI does, most of all on Agentic, and its task scores can
  differ from the AAII, where Epoch's stay close to the ECI.

With either source:

- Value is computed by modelcmp: the model's coding percentile divided by its price, ranked
  0-100 among the models that have both. Free models rank above every priced one.
- A row has the scores of the model the source lists under its name, else under the id most
  of the row's offers are sold as, and none when those offers are another release.

## Commands

```sh
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level, cheapest first, down to 8 months behind your best
modelcmp list --task coding --tier mid         # just one: the cheapest within 3 months of your best
modelcmp list --task coding --tier mid --id    # only its provider/model, for opencode -m $(...)
modelcmp list --min coding=155 --sort price    # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp list --dev china                      # every model of a developer from there
modelcmp show sonnet-5                         # everything about one model
modelcmp compare sonnet-5 gpt-5 --json         # side by side, with a verdict
modelcmp open sonnet-5 --on epoch              # its web page: models.dev, epoch, aa, openrouter or hf; without --on, the first to have one
modelcmp get gemma-3-4b-it --via ollama       # download its GGUF copy from Hugging Face and run it here: ollama or llama-cli, the one you have without --via, installed first, if you agree, when you have neither, and the size of the download is said first
modelcmp models-dir ~/models                   # the folder of the .gguf files you downloaded, listed at the next --refresh: alone shows it, --rm clears it
modelcmp select sonnet-5                       # shortlist it, list --selected shows them: --rm to deselect
modelcmp note sonnet-5 "fast enough for refactors" # agents weigh it when choosing; without text shows it, --rm deletes it
modelcmp exclude llama-4-maverick              # have it, can't use it; --rm to include again
modelcmp fav coding sonnet-5                   # your favorite for a task: --tier picks it; alone lists them, --rm clears
modelcmp fav coding 3.7-flash --tier low           # your favorite for one tier: list --tier low picks it over the task's
modelcmp fav coding opus-5.5 --via claude       # and the harness you run it on: list --task coding --id prints the id claude takes
modelcmp list --task coding --tier mid --cmd    # the command that opens a harness on it: the favorite's harness, else your default one, else the first that has the model
modelcmp harness claude                        # your default harness: --cmd and --id go by it when it has the model. Alone shows it, --rm clears it (`H` in the TUI)
modelcmp fav debugging opus-5.5 --about "finding and fixing a bug"  # a task of your own: its model and what it is about; --rm clears the model
modelcmp fav debugging --about "bugs and flaky tests"               # rewrite what it is about; "" clears it
modelcmp fav debugging 3.7-flash --tier low    # its model for one tier: list --task debugging --tier low returns it
modelcmp list --task debugging --tier mid --id # that tier's provider/model, else the task's
modelcmp fav debugging --rename triage         # another name for a task of your own; its models stay
modelcmp recommend                             # best model per price for each task, what it measures, when to use it
modelcmp pick                                  # for an agent, one call: each task, what it is for, and the command that runs a prompt on the model of its low, mid and high tier
modelcmp pick --via opencode                   # the same among opencode's models, each by the id opencode takes
modelcmp pick --via claude --min ctx=600       # only models that hold a 600,000-token prompt, favorites too
modelcmp pick --not opus-5.5                   # without a model, as one your note rules out for the work
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
tokens, and a value of 10000 or more is read as tokens), `release` (a date: `--min release=2026-06` is June 2026 on, `--max release=2026-06` up to the end of June), `eci`, `coding`, `agentic`, `reasoning`, `value`, and with Artificial Analysis `tps` and `ttft`. `--task` and
`recommend` leave excluded models out; plain `list` shows them marked `✗`, and your selection `✓`. `--tier` picks one
model from the task's list: `low` the cheapest within 8 months of progress of your best model on the task,
`mid` the cheapest within 3, `high` your best. The list keeps the best model of
each price level (free, up to $0.5, $2, $5, $15, and above), and what a tier picks even when its level
has a better one. `high` is the best to the whole point: of two models a fraction of a point apart, the cheaper.
A month of progress is a twelfth of what the best
score on the task rose in the last year, among the models listed here that the source scored; a task scored for
less than a year, or whose best of a year ago scored nothing, has no such pace, and every tier picks the best.
`value` lists the models within 8 months of your best on coding. A model you `fav` for the tier, else for the task, beats the
tier's pick. Every favorite of the task that you have and did not exclude sits on the task's list, whatever `--dev`, `--via`, `--min`, `--max` or `--selected` narrow it to, marked `★` in `recommend`, whether or not it is on the frontier or has a score for
the task, which then shows as `-`. `--all`
includes models you have no access to. `--refresh` on any command re-downloads first, `--cache PERCENT` (default 90) sets how much input Price reads from the prompt cache, and `--source epoch|aa` which benchmarks to use for this run (default: the one picked with `B`).
`list` prints every match unless `-n` limits it, and then says how many it left out. `--id` prints the id opencode takes, or pi or omp when only they have the model, else the id at the provider you'd pay, or the tag `ollama run` takes, or the repo (`-hf`) or file (`-m`) `llama-cli` takes, for a model only your machine runs; with `--task`, a favorite given a harness (`fav --via`) prints the id that harness takes, while it has the model, else your default harness's (`modelcmp harness`) when it has it. `--via` naming a harness prints the id that one takes instead (`--via claude --id` gives `claude-opus-5-5`, `--via opencode` the id opencode takes, for a script that always starts opencode), and is an error when the harness lacks the model. `--cmd` prints the command that starts the harness on the model instead (`claude --model claude-opus-5-5`): the favorite's harness, else your default one, else the first that has the model in Via's order, among `--via`'s when given; a model no harness has is an error. `--tier` with `--no-fav` gives the tier's pick without your favorites, for when a favorite is on no harness you can start (`--tier mid --via claude --no-fav --id`). A task of your own has none. With `--tier`, `--id` or `--cmd`, no match is an error (exit code 1), so a harness is never started on an empty model. Without them `list` says `no models match` and exits with 0. With `--tier`, `--id` and `--cmd` also say the model's context and your note on stderr (`Claude Opus 5.5: context 1000000, note: slow`), which an agent checks and a `$(...)` does not read.

`pick` is `list --task --tier` for every task and tier at once, your own tasks last: under each
task and what it is for, a line per tier with the model, its context and your note, and tiers with
the same model share a line (`low/mid`). With `--via` the model is named by the id that harness
takes, and a favorite the harness lacks follows the tier's own pick, with the command that starts
it where it runs: `(★ favorite Ling 3.1 Flash is not on claude, start: opencode run --model
opencode/ling-3.1-flash-free "<prompt>")`. Without `--via` each line has the command that runs one
prompt on the model and exits, on the harness `--cmd` goes by (`opencode run --model
opencode/ling-3.1-flash-free "<prompt>"`, `pi --model ... -p "<prompt>"`), so an agent in one
harness reaches a model only another has, or the id when no harness has the model. The command
has the flag that lets claude, codex, gemini or copilot edit files without asking, as opencode, pi
and omp do, and no wider permission. `--min` and
`--not` leave a favorite out too, and a task of your own whose model is out says `no model`.
`--task` and `--tier` print only that task or tier, for an agent that chose it already.

Model names match by substring, among the models you have first; the shortest match wins
only when every other contains it (`opus-4.5` over its `-thinking` variant), else the name
is ambiguous: it exits with code 3 and lists the candidates. An unknown `--dev` or `--via` is an error rather
than an empty list (exit code 1), and an unknown task or harness, or a `models-dir` that is not a
folder, exits with code 2.

## What you have

A refresh asks each harness on your `PATH` which models it has, and the Via column names them:

- `opencode models`, `pi --list-models` and `omp models` list theirs.
- The `claude`, `codex` and `gemini` binaries each count as their own provider.
- `copilot` gives the models GitHub Copilot's CLI takes on your plan, asked of the CLI itself
  with `copilot --acp`, under its own login. Each refresh leaves an empty folder in
  `~/.copilot/session-state/`.
- ollama and llama.cpp give the models on your machine, as below.

Under WSL the harnesses are asked in a new session, as `x` launches them, so a key exported by
hand in your shell does not count. A harness that fails to list its models keeps the ones from
the last refresh, and the refresh says so.

## Models on your machine

The models ollama and llama.cpp run here show as free, with `ollama` or `llama-cli` in Via. `ollama`, or
llama.cpp's `llama-cli` (else its `llama` or `llama-server`), must be on `PATH`, and the model must be one of these:

| You have | What makes it show |
|---|---|
| a model pulled by ollama | `ollama list` shows it, with ollama running. The tag must say the size: `qwen3.5:4b`, not `:latest` |
| a model llama.cpp downloaded | `llama-cli --cache-list` shows it, as after `llama-cli -hf unsloth/Qwen3.5-4B-GGUF:Q4_K_M` |
| a `.gguf` file you downloaded | `modelcmp models-dir ~/models` names its folder, or `LLAMA_ARG_MODELS_DIR`, llama.cpp's own variable, does when it is set. Subfolders are not read |

Then run `modelcmp --refresh`, which opens the TUI, or `modelcmp list --refresh` in a script.

The scores and the context shown are the full weights', which a quantized copy with its own context
falls short of, so a model only your machine runs is recommended for a task only as your favorite.

A model is matched by its tag, repo or file name, without the quantization, so the name must be the
one the model has here (`modelcmp list --all` shows them): `gemma-3-4b-it-Q4_K_M.gguf` is Gemma 3 4B IT.
A name that says `it` is the instruction-tuned model, never the base one beside it.
A repo that repeats the developer in its name (`bartowski/Qwen_Qwen3.5-4B-GGUF`) matches none.

`modelcmp get <model>` finds the most downloaded GGUF copy of the model's Hugging Face repo and
starts `llama-cli -hf <repo>` or `ollama run hf.co/<repo>`, which download it. Both say the size
of the download first, as `2.5 GB`. With neither installed it asks before installing one:
llama.cpp with Homebrew, else ollama with its own script on Linux. A model with no repo on
Hugging Face has no such option, and it does not check that the model fits your machine. In
the TUI, `x` on a model and `download and run` does the same.

## Files

The data is downloaded on first run and cached for 24 hours under `~/.cache/modelcmp/` on Linux
(`~/Library/Caches/modelcmp/` on macOS). Your favorites, exclusions, notes and settings live in
`~/.config/modelcmp/user.json` (`~/Library/Application Support/modelcmp/` on macOS), and the `?`
help shows the path. Your selection is kept there too, until the TUI closes. Each refresh also
asks GitHub for the latest release, to say when a new version is out, and `UU` in the TUI
upgrades to it.

## Data age

The TUI's bottom border always shows the refresh state: `⟳ refreshing 7/9, waiting for opencode` (the downloads
and harnesses that answered, and the last one or two still awaited), `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h; then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`. It is indented at a terminal and one line in a pipe.

```sh
opencode -m $(modelcmp list --task coding --tier mid --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"; each has its frontier, the user's "favorite" and "tier_favorites", and their harnesses as "via" and "tier_via"
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite for the tier or task when set, else the tier's
```

Each JSON model carries `key`, `selected` (on your shortlist), `excluded`, `favorite_for` (the tasks it is the user's favorite
for, and `task:tier` for a tier's), `price` (with the provider's model `id`, which opencode takes after `provider/`, and `cache_read_per_mtok`, null when input is never discounted; `input_per_mtok` and `output_per_mtok` are null when no provider lists a price, which the tables show as `-`, not `free`; `listed` is true when yours lists none and they are the list prices of other providers, `~` in the tables), `context`,
`eci` (the overall index of `source`, `epoch` or `aa`), per-task `tasks` (each task's table column: ECI points with `epoch`, the benchmark's 0-100 score with `aa`, overall and vision the `eci`, value a 0-100 percentile), and with `aa` `tokens_per_second` and `ttft_seconds`. `show` and
`compare` add `benchmarks` (every score, as a fraction: 0.545 is 54.5%), `providers` (every provider's price) and `pages` (site to URL, for the sites that have a page for it; `url` is the first of them, null when none has). `recommend` prints each
frontier entry as `name [key] $price (value)`, the value being the task's table column: the
same as `tasks`. A favorite that is on the line only as
the favorite is marked `not recommended`, and its `recommend --json` entry has `"recommended": false`. An entry carries its `context` in tokens, `via` (the harnesses that have the model, as in `list --json`), and the user's `note` when the model has one.
A task of your own follows the built-in ones in `recommend --json` with the same fields and
`"custom": true`: its `about` is what the user wrote (`your own task` when nothing), its `when`
says it wins over a built-in task that fits too, its `favorite` and `tier_favorites` are the
models the user gave it, `via` and `tier_via` the harnesses they run on when the user chose one (null and `{}` otherwise, as for a built-in task), and its `frontier` lists them, cheapest first, with a null `score`.
