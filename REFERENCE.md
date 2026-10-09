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
| ECI | Epoch Capabilities Index. From Epoch AI |
| AAII | Artificial Analysis Intelligence Index. From Artificial Analysis |
| Arena | net improvement in real agent sessions, in percent. From Arena's Agent leaderboard |
| Coding, Agentic, Reason | the [task scores](#task-columns) |
| Value | Coding score per dollar of Price, ranked 0-100 |
| $task | USD per coding task, [measured](#task) by Epoch AI |
| Tok/task | output tokens per coding task, [measured](#task) by Epoch AI |
| Tok/s | output tokens per second. From Artificial Analysis |
| TTFT | seconds to the first answer token, after any thinking. From Artificial Analysis |
| Via | the harnesses that have the model, by a [short name](#what-you-have) |
| Notes | your note on it |

One table shows the columns of both sources. The ones of the source not in use show when both
are cached, which takes an Artificial Analysis key, and were fetched within a day of each other.
In the TUI AAII, Tok/s and TTFT show without that data too, empty, and their descriptions say
they need a key, or a refresh with one. A sort or a bound on an empty one is refused.
ECI and AAII keep their places, and the table starts sorted by the one of the source in use.
The task scores, Value and the recommendations stay with that source. In the TUI,
`|` picks the columns to show.

The source in use is Artificial Analysis when its key is there and works, else Epoch AI.

- The TUI's first start asks for the key, under the wordmark: `enter` saves it, `esc` skips it.
  A click on `artificialanalysis.ai` there opens the page where a free key is made. The bottom
  of that screen names keys to try on the next one: `o`, `x`, `R` and `?`.
- The key is `ARTIFICIAL_ANALYSIS_API_KEY`, else the one saved in `aa_key` next to `user.json`,
  readable by you alone. `K` in the TUI asks for one later, to add it or replace the saved
  one, as `modelcmp --source aa` does with none saved.
- A saved key that Artificial Analysis rejects is asked for again, and `esc` there leaves
  Epoch AI and removes the saved key. One rejected from the environment leaves Epoch AI at
  once, and a message says so. The command line does the same, unless `--source aa` asked
  for Artificial Analysis, which is then an error when no cached data is left to show.

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
- `Tok/task` is the output tokens that same run took per task, for an allowance counted in
  tokens and not in dollars. Epoch lists no input tokens, which an agent spends far more of, so
  it compares models and does not total what a task takes. `--sort tokens` and
  `--max tokens=50000` take it.
- For a model that shows `-`, [Artificial Analysis](https://artificialanalysis.ai) charts the
  input and output tokens its own benchmarks took on the model's page, which
  `modelcmp open MODEL --on aa` opens. Its API does not list them and its terms forbid scraping
  the site, so modelcmp cannot show them.

### Task columns

A task column is a benchmark score, 0-100, blank for a model not tested on it.

With Epoch AI, the default, it is one widely run benchmark about the task:

- Coding: WeirdML
- Agentic: APEX-Agents
- Reason: LMCA

With Artificial Analysis (its key, or `--source aa`):

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

- Both sources, Artificial Analysis only with its key.
- Each source's sitemap, with or without a key, to link the models that have a page there (`o`).
- OpenRouter's list of models, for the Hugging Face repo it names for each.
- For an open model OpenRouter names no repo for, the repo Hugging Face has under an id a
  provider gives it. Hugging Face is asked after the early data, 400 ids a refresh, and an id
  with no repo again a month later.

Epoch AI (the default without an Artificial Analysis key, <https://epoch.ai/data/benchmark_data.zip>, no key):

- ECI is Epoch's Capabilities Index as Epoch publishes it. modelcmp does not compute it.
- A benchmark score is the best one Epoch lists for any version of the model.
- Coding, Agentic and Reason are one benchmark's score as published, 0-100. modelcmp does not
  compute them.
- A task's other benchmarks are in its column's dropdown (`d`), only those Epoch still runs on
  new models.
- `$task` and `Tok/task` are the mean cost and the mean output tokens of a task that Epoch
  lists for the model's best DeepSWE run, as published. modelcmp does not compute them.

Artificial Analysis (`--source aa`, its API with your key):

- AAII is its Intelligence Index as it publishes it.
- Agentic and Reason are one benchmark's score as published, 0-100. Coding is the mean of
  two, computed by modelcmp. Tok/s and TTFT are its measurements.
- A model's numbers are those of one of its reasoning settings, chosen as
  [Task columns](#task-columns) says.
- It scores more models than Epoch AI does, most of all on Agentic.
- Its API lists no cost of a task, so `$task` and `Tok/task` are hidden with it.

Arena (its Agent leaderboard, no key needed):

- Arena is the score arena.ai publishes, its net improvement in real agent sessions, in
  percent. It can be negative, and higher is better. modelcmp does not compute it.
- Arena lists a model once per reasoning setting, and the column has the best of them.
- The column's dropdown (`d`) has the signals the score is made of, each as Arena publishes
  it, in percent: task outcome, tool hallucination, steerability, bash recovery and praise.
  One that a refresh could not get is empty until the next.
- It shows with either source, and counts toward no task score and no recommendation.
- A refresh that cannot reach it keeps the scores of the last one, and with none to keep it
  leaves the column empty, and the first start or command an hour later tries again. The
  column is empty once Arena's newest leaderboard is more than two months old.

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
modelcmp list --task coding --tier mid --via opencode --id    # only its provider/model, for opencode -m $(...)
modelcmp list --task coding --tier mid --cmd   # the command that opens a harness on it
modelcmp list --min coding=60 --sort price    # good enough, cheapest first
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
| `--source epoch\|aa` | the benchmarks for this run. The default is `aa` when its key is there and works, else `epoch` |
| `--all` | on `list`, includes the models you have no access to. On `show`, adds the price at providers you have no harness for |
| `--json` | on `list`, `show`, `compare` and `recommend`, as [JSON](#json) says |

### Tasks

The built-in tasks are `overall`, `coding`, `agentic`, `reasoning`, `vision` and `value`. For a
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
| `cost` | `$task`, from Epoch AI |
| `tokens` | `Tok/task`, from Epoch AI, in tokens |
| `tps`, `ttft` | from Artificial Analysis |
| `arena` | from Arena's Agent leaderboard, in percent |
| `other` | the other source's index |

A column of the source not in use needs both sources cached, as in the [table](#columns).

`list` prints every match unless `-n` limits it, and then says how many it left out. It shows
excluded models marked `✗` and your selection `✓`. `--task` and `recommend` leave excluded
models out.

### Tiers

`--task` keeps the best model of each price level (free, up to $0.5, $2, $5, $15, and above).
`--tier` picks one model from that list, the best at the tier's price:

| Tier | Its pick |
|---|---|
| `free` | the best free model, and none when the list has no free one |
| `low` | the best up to $2 per 1M tokens |
| `mid` | the best up to $5 |
| `high` | your best. It is the best to the whole point: of two models a fraction of a point apart, the cheaper |

- Two tiers pick the same model when none dearer that the next may take is better. A tier
  never takes a worse model to differ from the next.
- With no model at its price, `low` or `mid` picks the cheapest of the list. `free` never
  holds a model that costs, your favorite included.
- `value` lists the models within 8 months of progress of your best on coding. A month of
  progress is a twelfth of what the best coding score rose in the last year, among the models
  listed here that the source scored.

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

Via names them short, so a model's harnesses fit on its line: `oc` opencode, `pi`, `omp`, `cc`
claude, `cx` codex, `gem` gemini, `cp` copilot, `oll` ollama and `llm` llama-cli. The Via
dropdown (`d`) says which is which, and a search (`/`) takes either name.

Under WSL the harnesses are asked in a new session, as `x` launches them, so a key exported by
hand in your shell does not count. A harness that fails to list its models keeps the ones from
the last refresh, and the refresh says so.

## Models on your machine

The models ollama and llama.cpp run here show as free, with `oll` or `llm` in Via. `ollama`, or
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
(`~/Library/Caches/modelcmp/` on macOS). A download that fails is asked for up to three times, unless the
site says the request itself is wrong. Your selection, favorites, exclusions, notes and settings live in
`~/.config/modelcmp/user.json` (`~/Library/Application Support/modelcmp/` on macOS), and the `?`
help shows the path. Each refresh also
asks GitHub for the latest release, to say when a new version is out, and `UU` in the TUI
upgrades to it.

Both are safe to delete: the cache is downloaded again on the next run, and removing `user.json`
resets your selection, favorites, exclusions, notes and settings.

## Data age

The TUI's bottom border always shows the refresh state: `⟳ refreshing 7/9, waiting for opencode` (the downloads
and harnesses that answered, and the last one or two still awaited), `refresh failed` in red
with the age of the data still shown, or the data age alone, red once it is past 24h. Then
the status bar says it too, `data 25h old` in red, and hints `r refresh`.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`. It is indented at a terminal and one line in a pipe.

```sh
opencode -m $(modelcmp list --task coding --tier mid --via opencode --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite for the tier or task when set, else the tier's
```

Each model carries these fields:

| Field | What it is |
|---|---|
| `key` | the model's key |
| `name`, `developer` | as the table shows them |
| `available` | a harness of yours has it |
| `via` | the harnesses that have it |
| `selected` | on your shortlist |
| `excluded` | you excluded it |
| `note` | your note on it, null when it has none |
| `favorite_for` | the tasks it is the user's favorite for, and `task:tier` for a tier's |
| `price` | the offer you'd pay, as below |
| `context`, `max_output` | in tokens |
| `tool_call`, `reasoning`, `vision`, `open_weights` | what it can do, true or false |
| `release`, `knowledge` | its release date and knowledge cutoff, empty when unknown |
| `url` | its page, the first of `pages` below, null when no site has one |
| `eci` | the overall index of `source` |
| `source` | `epoch` or `aa` |
| `tasks` | each task's table column: the 0-100 score of its benchmark with `source`, overall and vision the `eci`, value a 0-100 percentile |
| `other_index` | the other source's index, AAII with `epoch` and ECI with `aa`, when both are cached |
| `arena_agent` | Arena, in percent, when Arena ranks the model |
| `task_cost_usd` | `$task`, from Epoch AI, when the model has one |
| `task_output_tokens` | `Tok/task`, in tokens, from Epoch AI, when the model has one |
| `tokens_per_second`, `ttft_seconds` | from Artificial Analysis, when the model has them |

`price` has:

| Field | What it is |
|---|---|
| `provider` | the provider the price is from |
| `id` | the provider's model id, which opencode takes after `provider/` |
| `available`, `via` | whether a harness of yours has this offer, and which |
| `input_per_mtok`, `output_per_mtok` | null when no provider lists a price, which the tables show as `-`, not `free` |
| `cache_read_per_mtok` | null when input is never discounted |
| `listed` | true when yours lists none and they are the list prices of other providers, `~` in the tables |

`show` and `compare` add:

| Field | What it is |
|---|---|
| `benchmarks` | every score, as a fraction: 0.545 is 54.5% |
| `providers` | every provider's price |
| `pages` | site to URL, for the sites that have a page for it |

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
