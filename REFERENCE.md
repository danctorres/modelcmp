# modelcmp reference

The columns, every command and the JSON fields. The [README](README.md) covers install and
the basics, [KEYS.md](KEYS.md) every TUI key, and `modelcmp --help` (or `modelcmp <command>
--help`) every flag.

## Columns

One row per model. In the TUI, move the column cursor and the top border says what the column
means. Green and red mark the best and worst value in a column, and in a row of compare.

| Column | What it is |
|---|---|
| Model, Dev | the model and its developer |
| Released | year and month |
| Price | $ per 1M tokens, blended as [below](#price) |
| $in | $ per 1M input tokens |
| $cache | $ per 1M cached input tokens, `$in` when a provider has no discount |
| $out | $ per 1M output tokens |
| Ctx | context window, in tokens |
| ECI | Epoch Capabilities Index. With Artificial Analysis it is AAII, its Intelligence Index |
| Coding, Agentic, Reason | the [task scores](#task-columns) |
| Value | coding per dollar, ranked 0-100 |
| $task | $ one coding task cost, [measured](#task). Epoch AI only |
| Tok/s | output tokens per second. Artificial Analysis only |
| TTFT | seconds to the first answer token, after any thinking. Artificial Analysis only |
| Via | the harnesses that have the model |
| Notes | your note on it |

### Price

- It blends input and output 3:1, with 90% of the input read from the prompt cache, as in an
  agent session. `%` in the TUI or `--cache` changes the share.
- A provider without a cache price pays full input.
- A provider that charges more past a context size is priced as at 100k tokens.
- A price after a `~` is the list price of other providers, yours listing none.

### $task

- Price is per token, and models differ in how many tokens and steps the same job takes them.
  `$task` is what one task of DeepSWE cost the model in dollars, as the benchmark measured it,
  every model on the same harness.
- It is the cost of the run the model's DeepSWE score is of, its best reasoning setting. A lower
  setting costs less and scores less.
- Few models have it, 26 of them on 2026-10-06, and the rest show `-`. No tier goes by it.
- `--sort cost` and `--max cost=5` take it.

### Task columns

With Epoch AI, the default, a task column is the model's capability on the task's benchmarks,
in ECI points, fitted from its ECI and its scores. A model with few scores stays near its ECI.

- Coding: DeepSWE, FrontierCode, FrontierSWE, WeirdML, MirrorCode
- Agentic: APEX-Agents, Remote Labor Index, OSWorld 2.0
- Reason: GPQA diamond, HLE, ARC-AGI-2, ARC-AGI, SimpleBench, Mystery Game Puzzles, Chess Puzzles, LMCA, DTBench

With Artificial Analysis (`B`, `--source aa`), a task column is a benchmark score, 0-100.

- Coding: the mean of Terminal-Bench 4.0 and SciCode
- Agentic: Terminal-Bench 4.0
- Reason: HLE

Its scores and speed follow these rules:

- Tok/s and TTFT are medians on the developer's own API, or across providers when it has none.
- A model's scores and speed are those of one reasoning setting, the best index among those
  scored on the most benchmarks.
- A row that names a setting ("Grok 4.20 Non-Reasoning") has that setting's, or `-` when
  Artificial Analysis did not measure it.
- A row that cannot reason has its non-reasoning setting's.

With either source a row has no scores when its offers are another release than the one
scored, as a `-latest` id is.

`modelcmp recommend --json` gives the same lists as `benchmarks`. With Artificial Analysis it
is the one field the score is.

[Scores](#scores) says how each of these numbers is obtained.

## Scores

Where the numbers in the score columns come from.

What a refresh downloads:

- Both sources, Artificial Analysis only with its key, so switching between them needs no
  second refresh.
- Each source's sitemap, with or without a key, to link the models that have a page there (`o`).
- OpenRouter's list of models, for the Hugging Face repo it names for each.
- For an open model OpenRouter names no repo for, the repo Hugging Face has under an id a
  provider gives it. Hugging Face is asked after the early data, 400 ids a refresh, and an id
  with no repo again a month later.

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
- `$task` is the mean cost of a task that Epoch lists for the model's best DeepSWE run, as
  published. modelcmp does not compute it.

Artificial Analysis (`--source aa`, its API with your key):

- AAII is its Intelligence Index as it publishes it.
- Agentic and Reason are one benchmark's score as published, 0-100. Coding is the mean of
  two, computed by modelcmp. Tok/s and TTFT are its measurements.
- A model's numbers are those of one of its reasoning settings, chosen as
  [Task columns](#task-columns) says.
- It scores more models than Epoch AI does, most of all on Agentic, and its task scores can
  differ from the AAII, where Epoch's stay close to the ECI.
- Its API lists no cost of a task, so `$task` is hidden with it.

With either source:

- Value is computed by modelcmp: the model's coding percentile divided by its price, ranked
  0-100 among the models that have both. Free models rank above every priced one.
- A row has the scores of the model the source lists under its name, else under the id most
  of the row's offers are sold as, and none when those offers are another release.

## Commands

```sh
# list
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level, cheapest first
modelcmp list --task coding --tier mid         # just one, the tier's pick
modelcmp list --task coding --tier mid --id    # only its provider/model, for opencode -m $(...)
modelcmp list --task coding --tier mid --cmd   # the command that opens a harness on it
modelcmp list --min coding=155 --sort price    # good enough, cheapest first
modelcmp list --sort cost --max cost=5         # by what a coding task cost, $5 at most
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp list --dev china                      # every model of a developer from there

# one model
modelcmp show sonnet-5                         # everything about it, --all adds the price at providers you have no harness for
modelcmp compare sonnet-5 gpt-5 --json         # side by side, with a verdict
modelcmp open sonnet-5 --on epoch              # its web page: models.dev, epoch, aa, openrouter or hf
modelcmp get gemma-3-4b-it --via ollama        # download its GGUF copy and run it here
modelcmp models-dir ~/models                   # the folder of the .gguf files you downloaded

# your choices
modelcmp select sonnet-5                       # shortlist it, list --selected shows them
modelcmp note sonnet-5 "fast enough for refactors"
modelcmp exclude llama-4-maverick              # have it, can't use it
modelcmp harness claude                        # your default harness (`H` in the TUI)
modelcmp fav coding sonnet-5                   # your favorite for a task
modelcmp fav coding 3.7-flash --tier low       # your favorite for one tier
modelcmp fav coding opus-5.5 --via claude      # and the harness you run it on

# tasks of your own
modelcmp fav debugging opus-5.5 --about "finding and fixing a bug"
modelcmp fav debugging --about "bugs and flaky tests"   # rewrite what it is about, "" clears it
modelcmp fav debugging 3.7-flash --tier low    # its model for one tier
modelcmp fav debugging --rename triage         # another name, its models stay

# recommendations
modelcmp recommend                             # best model per price for each task, what it measures and when to use it
modelcmp recommend --cache 0                   # priced as one-off prompts
modelcmp pick                                  # for an agent: every task and tier, with the command to run
modelcmp pick --via opencode                   # the same among opencode's models, by the id opencode takes
modelcmp pick --via claude --min ctx=600       # only models that hold a 600,000-token prompt
modelcmp pick --not opus-5.5                   # without a model, as one your note rules out
```

`--rm` undoes `select`, `note`, `exclude`, `harness`, `models-dir` and `fav`. Alone, `harness`,
`models-dir` and `fav` show what is set, and `note <model>` without text shows the note. Agents
weigh a note when choosing.

- `open` without `--on` opens the first site to have a page for the model.
- `get` downloads the copy from Hugging Face and runs it with ollama or `llama-cli`, the one you have without `--via`. With
  neither it installs one first, if you agree. It says the size of the download first.
- `models-dir` is read at the next `--refresh`.

### Common flags

| Flag | What it does |
|---|---|
| `--refresh` | re-downloads the data first |
| `--cache PERCENT` | how much of the input Price reads from the prompt cache, 90 by default |
| `--source epoch\|aa` | the benchmarks for this run. The default is the one picked with `B` |
| `--all` | includes the models you have no access to |
| `--json` | on `list`, `show`, `compare` and `recommend`, as [JSON](#json) says |

### Tasks

The built-in tasks are `overall`, `coding`, `value`, `agentic`, `reasoning` and `vision`. For a
large prompt, bound the context instead: `--min ctx=200`.

Any other name given to `fav` is a task of your own (`debugging`, and `"Tool Dispatch"` is
stored as `tool-dispatch`).

- No benchmark ranks it. It has the models you give it, one for the task and one per tier if
  you like.
- `--task` lists them, cheapest first. With `--tier` it returns that tier's, else the task's,
  leaving out one you excluded or lost access to.
- `recommend` lists it after the built-in tasks, with what you wrote it is about and each
  model's tier, `(low)`.
- It is the task to pick for work that fits it, even when a built-in task fits too: for a bug,
  your `debugging` over `coding`.
- It is gone when its last model is cleared.
- `--rename` gives it another name, one no task has yet.

### Sorting and bounds

`--sort`, `--min` and `--max` take these columns:

| Column | Notes |
|---|---|
| `price`, `in`, `cache`, `out` | $ per 1M tokens |
| `ctx` | thousands of tokens, and a value of 10000 or more is read as tokens |
| `release` | a date: `--min release=2026-06` is June 2026 on, `--max release=2026-06` up to the end of June |
| `eci`, `coding`, `agentic`, `reasoning`, `value` | the scores |
| `cost` | `$task`, with Epoch AI |
| `tps`, `ttft` | with Artificial Analysis |

`list` prints every match unless `-n` limits it, and then says how many it left out. It shows
excluded models marked `✗` and your selection `✓`. `--task` and `recommend` leave excluded
models out.

### Tiers

`--task` keeps the best model of each price level (free, up to $0.5, $2, $5, $15, and above),
and what a tier picks even when its level has a better one. It goes down to 8 months behind
your best. `--tier` picks one model from that list:

| Tier | Its pick |
|---|---|
| `low` | the cheapest within 8 months of progress of your best model on the task |
| `mid` | the cheapest within 3 months |
| `high` | your best. It is the best to the whole point: of two models a fraction of a point apart, the cheaper |

- A month of progress is a twelfth of what the best score on the task rose in the last year,
  among the models listed here that the source scored.
- A task scored for less than a year, or whose best of a year ago scored nothing, has no such
  pace, and every tier picks the best.
- `value` lists the models within 8 months of your best on coding.

### Favorites

- A model you `fav` for the tier, else for the task, beats the tier's pick.
- Every favorite of the task that you have and did not exclude sits on the task's list,
  whatever `--dev`, `--via`, `--min`, `--max` or `--selected` narrow it to.
- `recommend` marks it `★`, whether or not it is on the frontier or has a score for the task,
  which then shows as `-`.
- `--tier` with `--no-fav` gives the tier's pick without your favorites, for when a favorite is
  on no harness you can start (`--tier mid --via claude --no-fav --id`). A task of your own
  has none.

### `--id` and `--cmd`

`--id` prints only the model's id, the first of these that applies:

1. With `--via` naming a harness, the id that harness takes (`--via claude --id` gives
   `claude-opus-5-5`, `--via opencode` the id opencode takes, for a script that always starts
   opencode). It is an error when the harness lacks the model.
2. With `--task`, the id the favorite's harness takes (`fav --via`), while it has the model.
3. With `--task`, the id your default harness takes (`modelcmp harness`), when it has the model.
4. The id opencode takes, or pi or omp when only they have the model.
5. The id at the provider you'd pay.
6. For a model only your machine runs, the tag `ollama run` takes, or the repo (`-hf`) or file
   (`-m`) `llama-cli` takes.

`--cmd` prints the command that starts the harness on the model instead (`claude --model
claude-opus-5-5`). The harness is the favorite's, else your default one, else the first that
has the model in Via's order, among `--via`'s when given. A model no harness has is an error.

- With `--tier`, `--id` or `--cmd`, no match is an error (exit code 1), so a harness is never
  started on an empty model. Without them `list` says `no models match` and exits with 0.
- With `--tier`, `--id` and `--cmd` also say the model's context and your note on stderr
  (`Claude Opus 5.5: context 1000000, note: slow`), which an agent checks and a `$(...)` does
  not read.

### `pick`

`pick` is `list --task --tier` for every task and tier at once, your own tasks last.

- Under each task and what it is for, it prints a line per tier with the model, its context
  and your note. Tiers with the same model share a line (`low/mid`), and tasks with the same
  lines are listed together above them, so an agent reads each line once.
- Each line has the command that runs one prompt on the model and exits, on the harness
  `--cmd` goes by (`modelcmp quiet opencode run --model opencode/ling-3.1-flash-free
  "<prompt>"`, `modelcmp quiet pi --model ... -p "<prompt>"`), so an agent in one harness
  reaches a model only another has.
- `modelcmp quiet <command>` runs the command with what it prints on stderr (the harness's
  steps) left out, and says the last 20 lines of it when the run fails, with the command's
  exit code, so the agent reads the answer alone, or the error.
- The command has the flags that let claude, codex, gemini or copilot edit files and run
  commands (tests, builds) without asking, as opencode, pi and omp do.
- With `--via` the model is named by the id that harness takes. A favorite the harness lacks
  follows the tier's own pick, with the command that starts it where it runs: `(★ favorite
  Ling 3.1 Flash is not on claude, start: modelcmp quiet opencode run --model
  opencode/ling-3.1-flash-free "<prompt>")`.
- `--min` and `--not` leave a favorite out too. A tier's favorite they leave out gives way to
  the task's, and a task of your own whose model is out says `no model`.
- When no task has a model (`--via ollama`, whose copies are picked only as favorites, or a
  `--min` nothing meets), `pick` fails and says to choose none.
- `--task` and `--tier` print only that task or tier, for an agent that chose it already.

### Naming a model

Model names match by substring, among the models you have first. The shortest match wins only
when every other contains it (`opus-4.5` over its `-thinking` variant), else the name is
ambiguous.

| Exit code | When |
|---|---|
| 1 | an unknown `--dev` or `--via`, rather than an empty list |
| 2 | an unknown task or harness, or a `models-dir` that is not a folder |
| 3 | an ambiguous model name, and it lists the candidates |

## What you have

A refresh asks each harness on your `PATH` which models it has, and the Via column names them:

- `opencode models`, `pi --list-models` and `omp models` list theirs.
- The `claude`, `codex` and `gemini` binaries each count as their own provider.
- `copilot` gives the models GitHub Copilot's CLI takes on your plan, asked of the CLI itself
  with `copilot --acp`, under its own login. Each refresh leaves an empty folder in
  `~/.copilot/session-state/`. The VS Code extension alone is not enough, install the CLI.
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
A tag of ollama's own library (`llama3.2:1b`) is the instruct model, as that is what ollama holds under it.
A repo that repeats the developer in its name (`bartowski/Qwen_Qwen3.5-4B-GGUF`) matches none.

`modelcmp get <model>` finds the most downloaded GGUF copy of the model's Hugging Face repo and
starts `llama-cli -hf <repo>` or `ollama run hf.co/<repo>`, which download it. Both say the size
of the download first, as `2.5 GB`. With neither installed it asks before installing one:
llama.cpp with Homebrew, else ollama with its own script on Linux. A model with no repo on
Hugging Face has no such option, and it does not check that the model fits your machine. In
the TUI, `x` on a model and `download and run` does the same, with each runner you have, and
`install, download and run` with each one you lack that it can install, for a model no runner here has.

## Files

The data is downloaded on first run, which a command says on stderr as it can take half a minute, and cached for 24 hours under `~/.cache/modelcmp/` on Linux
(`~/Library/Caches/modelcmp/` on macOS). Your selection, favorites, exclusions, notes and settings live in
`~/.config/modelcmp/user.json` (`~/Library/Application Support/modelcmp/` on macOS), and the `?`
help shows the path. Each refresh also
asks GitHub for the latest release, to say when a new version is out, and `UU` in the TUI
upgrades to it.

## Data age

The TUI's bottom border always shows the refresh state: `⟳ refreshing 7/9, waiting for opencode` (the downloads
and harnesses that answered, and the last one or two still awaited), `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h. Then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`. It is indented at a terminal and one line in a pipe.

```sh
opencode -m $(modelcmp list --task coding --tier mid --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite for the tier or task when set, else the tier's
```

Each model carries these fields:

| Field | What it is |
|---|---|
| `key` | the model's key |
| `selected` | on your shortlist |
| `excluded` | you excluded it |
| `favorite_for` | the tasks it is the user's favorite for, and `task:tier` for a tier's |
| `price` | the offer you'd pay, as below |
| `context` | in tokens |
| `eci` | the overall index of `source` |
| `source` | `epoch` or `aa` |
| `tasks` | each task's table column: ECI points with `epoch`, the benchmark's 0-100 score with `aa`, overall and vision the `eci`, value a 0-100 percentile |
| `task_cost_usd` | `$task`, with `epoch`, when the model has one |
| `tokens_per_second`, `ttft_seconds` | with `aa` |

`price` has:

| Field | What it is |
|---|---|
| `id` | the provider's model id, which opencode takes after `provider/` |
| `input_per_mtok`, `output_per_mtok` | null when no provider lists a price, which the tables show as `-`, not `free` |
| `cache_read_per_mtok` | null when input is never discounted |
| `listed` | true when yours lists none and they are the list prices of other providers, `~` in the tables |

`show` and `compare` add:

| Field | What it is |
|---|---|
| `benchmarks` | every score, as a fraction: 0.545 is 54.5% |
| `providers` | every provider's price |
| `pages` | site to URL, for the sites that have a page for it |
| `url` | the first of `pages`, null when none has |

`recommend --json` has a list of tasks, each with its `when`, its `frontier`, the user's
`favorite` and `tier_favorites`, and their harnesses as `via` and `tier_via`.

- A frontier entry carries its `context` in tokens, `via` (the harnesses that have the model,
  as in `list --json`), and the user's `note` when the model has one.
- A favorite that is on the line only as the favorite has `"recommended": false`, and the text
  output marks it `not recommended`.
- The text output prints each frontier entry as `name [key] $price (value)`, the value being
  the task's table column, the same as `tasks`.

A task of your own follows the built-in ones with the same fields and `"custom": true`:

| Field | What it is |
|---|---|
| `about` | what the user wrote, `your own task` when nothing |
| `when` | says it wins over a built-in task that fits too |
| `favorite`, `tier_favorites` | the models the user gave it |
| `via`, `tier_via` | the harnesses they run on when the user chose one, null and `{}` otherwise, as for a built-in task |
| `frontier` | those models, cheapest first, with a null `score` |
