# modelcmp reference

The columns, every command and the JSON fields. The [README](README.md) covers install and
the basics, [KEYS.md](KEYS.md) every TUI key, and `modelcmp --help` (or `modelcmp <command>
--help`) every flag.

## Columns

One row per model. In the TUI, move the column cursor and the top border says what the column
means. Green and red mark the best and worst value in a column, here and in compare.

| Column | What it is |
|---|---|
| Model, Dev | the model and its developer |
| Released | year and month |
| Price | $ per 1M tokens, blended as [below](#price) |
| $in | $ per 1M input tokens |
| $cache | $ per 1M cached input tokens, `$in` when a provider has no discount |
| $out | $ per 1M output tokens |
| Ctx | context window, in tokens |
| ECI | Epoch Capabilities Index. From Epoch AI, or grey when estimated from the model's benchmarks |
| Coding, Agentic | the [task scores](#task-columns) |
| Tok/s | output tokens per second in OpenRouter's GPQA diamond runs, end to end with the thinking. From OpenRouter |
| Via | the harnesses that have the model, by a [short name](#what-you-have). `↓` with the download's size when ollama or llama.cpp can download it, grey when it is too big for your machine's memory. A harness left out in the Via dropdown is grey, here and in `x`, the details and compare. The one the model runs on (`H`) is dark on its colour, here, in the details and compare |
| Notes | your note on it |

The table starts sorted by ECI. In the TUI, `|` picks the columns to show.

### Price

- It blends input and output 3:1, with 90% of the input read from the prompt cache, as in an
  agent session. `%` in the TUI or `--cache` changes the share.
- A provider without a cache price pays full input.
- A provider that charges more past a context size is priced as at 100k tokens.
- A provider that charges more for output while the model thinks is priced at that.
- A price after a `~` is other providers' list price, as yours lists none.

### Task columns

Coding is the model's FrontierCode score, in percent: Cognition's coding tasks written by
project maintainers, solved well enough to merge. Every model is scored on the same tasks, each
in its developer's own agent, so one model's score is not from other benchmarks than another's.

Agentic is Arena's agent score: the net improvement arena.ai measures in real agent sessions,
in percent, the same measure for every model (see [Scores](#scores)). It needs tool calling.

A model with none of a task's benchmarks has no score for it, and is not recommended for it.

Each column's dropdown (`d`) lists other benchmarks, each shown alone when picked:

- ECI: FrontierMath T4, SimpleQA Verified, Chess Puzzles, Mystery Game Puzzles, LMCA and
  Furniture Assembly, general benchmarks that still tell the best models apart, and Arena's
  chat rating
- Coding: CursorBench, DeepSWE, FrontierSWE, MirrorCode, SWE-Bench verified, Aider polyglot,
  Terminal Bench, GSO-Bench, WeirdML v3, WeirdML, Surface Evolver Bench and Arena's WebDev rating
- Agentic: the signals Arena's agent score is made of, then APEX-Agents, Remote Labor Index,
  OSWorld 2.0, OSWorld, EBR-bench, SpireClimb, METR Time Horizons, The Agent Company, GDPval and
  Terminal Bench

The overall score, in ECI, is Epoch's own ECI, or in grey the same fit over every benchmark the
model has. Vision is ranked by overall. At the same score, a measured ECI sorts before a grey one.
`modelcmp show` says what each task's score is from, as `--json`'s `basis`.

A row has no scores when its offers are another release than the one scored, as a `-latest`
id is.

`modelcmp recommend --json` names each task's benchmarks in `benchmarks`. A benchmark picked in
the task's column in the TUI takes their place, and the column then shows that benchmark's
score alone, 0-100.

[Scores](#scores) says how each of these numbers is obtained.

## Scores

Where the numbers in the score columns come from.

What a refresh downloads:

- Epoch AI's benchmark data.
- Epoch AI's and Artificial Analysis's sitemaps, to link the models that have a page there (`o`).
- OpenRouter's list of models, for the Hugging Face repo it names for each.
- OpenRouter's benchmark leaderboard (<https://openrouter.ai/api/v1/datasets/exports/README.md>,
  CC BY 4.0), for the speed of its GPQA diamond runs. A refresh that cannot get it says so and
  goes on.
- For an open model OpenRouter names no repo for, the Hugging Face repo under an id a provider
  gives it. Hugging Face is asked once the early data is in, for up to 400 ids a refresh, and an
  id with no repo is asked about again a month later.

Epoch AI (<https://epoch.ai/data/benchmark_data.zip>):

- ECI is Epoch's Capabilities Index as Epoch publishes it. modelcmp does not compute it.
- A benchmark score is the best one Epoch lists for any version of the model, of every
  benchmark in Epoch's index, and of CursorBench and DeepSWE, which Epoch lists outside it.
- The overall of a model Epoch has not given an ECI yet is computed by modelcmp with Epoch's own
  model of benchmark scores: each benchmark has a difficulty and a slope on the ECI scale
  (`edi_scores.csv`), a score is normalized between the benchmark's random baseline and its
  ceiling, and the estimate is the capability that fits the model's scores. Over every benchmark
  it recovers Epoch's ECI within a point for most models.
- An estimate needs at least one score between 10% and 90% of the way from the random baseline
  to the ceiling. Near either end a point moves the estimate far: one such score alone can be
  off by tens of ECI points. With one in that range, the model's other scores count too.
- A task's benchmarks are in its column's dropdown (`d`). `B` finds a benchmark of any task
  without knowing which task it is for.

OpenRouter (its benchmark leaderboard, updated daily):

- Tok/s is the output tokens a GPQA diamond run produced over the time it took, the median over
  the providers that ran the model in the last 90 days, or OpenRouter's auto-routing run when no
  provider has one: an end-to-end rate, the thinking and the time to the first token included,
  which a provider's streaming rate is not.
- Its accuracy is not used: GPQA diamond is too easy for today's best models to tell them apart.
  Nor are its tau-bench results.

Arena (its Agent, Text and WebDev leaderboards):

- Agentic is the score arena.ai publishes, its net improvement in real agent sessions, in
  percent. It can be negative, and higher is better. modelcmp does not compute it.
- Arena lists a model once per reasoning setting, and the column has the best of them.
- Agentic's dropdown (`d`) has the signals the agent score is made of, each as Arena publishes
  it, in percent: task outcome, tool hallucination, steerability, bash reliability and praise.
  ECI's has Arena's Chat rating and Coding's its WebDev rating, from people's votes in its Text
  and WebDev leaderboards, around 1500 and higher is better. A click on one opens its
  leaderboard. One that a refresh could not get is empty until the next, and a rating is empty
  once its leaderboard is more than two months old.
- It is the agentic task's score, so it ranks agentic in recommend, `pick` and `--task`.
- A refresh that cannot reach it keeps the last refresh's scores. With none to keep, the
  column is empty, and the first start or command an hour later tries again. The column is
  also empty once Arena's newest leaderboard is more than two months old.
- The TUI shows the table before Arena has answered, with the last refresh's scores until then.

A row has the scores of the model Epoch lists under its name, else under the id most of the
row's offers are sold as, and none when those offers are another release.

## Commands

```sh
# list
modelcmp list                                  # models you have access to
modelcmp list --task coding                    # best model per price level, cheapest first
modelcmp list --task coding --tier mid         # just one, the tier's pick
modelcmp list --task coding --tier mid --via opencode --id    # only its provider/model, for opencode -m $(...)
modelcmp list --task coding --tier mid --cmd   # the command that opens a harness on it
modelcmp list --min coding=60 --sort price    # good enough, cheapest first
modelcmp list --max price=2 --via opencode --dev anthropic --dev openai
modelcmp list --dev china                      # every model of a developer from there

# one model
modelcmp show sonnet-5                         # everything about it, --all adds the price at providers you have no harness for
modelcmp compare sonnet-5 gpt-5 --json         # side by side, with a verdict
modelcmp open sonnet-5 --on epoch              # its web page: models.dev, epoch, aa, openrouter or hf
modelcmp get gemma-3-4b-it --via ollama        # download its GGUF copy and run it here
modelcmp models-dir ~/models                   # the folder of the .gguf files you downloaded
modelcmp memory 24                             # the GB your GPU has, when the one found is wrong

# your choices
modelcmp select sonnet-5                       # shortlist it, list --selected shows them
modelcmp note sonnet-5 "fast enough for refactors"
modelcmp exclude llama-4-maverick              # have it, can't use it
modelcmp harness opencode                      # your default harness (`P` in the TUI)
modelcmp harness claude opus-5.5 sonnet-5      # the harness these run on (`H` in the TUI)
modelcmp fav coding gemini-3.7-flash --tier low   # your favorite for one tier of a task, at its price or under
modelcmp tier mid 8                            # mid takes models up to $8, `modelcmp tier` shows every cap

# tasks of your own
modelcmp fav debugging opus-5.5 --tier high --about "finding and fixing a bug"
modelcmp fav debugging --about "bugs and flaky tests"   # rewrite what it is about, "" clears it
modelcmp fav debugging gemini-3.7-flash --tier low   # its model for one tier
modelcmp fav debugging --rename triage         # another name, its models stay

# recommendations
modelcmp recommend                             # best model per price for each task, and when to use it
modelcmp recommend --cache 0                   # priced as one-off prompts
modelcmp pick                                  # for an agent: every task and tier, with the command to run
modelcmp pick --via opencode                   # the same among opencode's models, by the id opencode takes
modelcmp pick --via claude --min ctx=600       # only models that hold a 600,000-token prompt
modelcmp pick --not opus-5.5                   # without a model, as one your note rules out
```

`--rm` undoes `select`, `note`, `exclude`, `harness`, `tier`, `models-dir`, `memory` and `fav`. Alone, `harness`,
`tier`, `models-dir`, `memory` and `fav` show what is set, and `note <model>` without text shows the note. Agents
weigh a note when choosing.

- `open` without `--on` opens the first site to have a page for the model.
- `get` downloads the model's copy from Hugging Face and runs it with ollama or `llama-cli`:
  the one `--via` names, else the one you have. With neither, it installs one first if you
  agree. It says the size of the download before it starts.
- `models-dir` is read at the next `--refresh`.

### Common flags

| Flag | What it does |
|---|---|
| `--refresh` | re-downloads the data first |
| `--cache PERCENT` | the percent of input tokens read from the prompt cache, for Price. 90 by default |
| `--all` | on `list`, includes the models you have no access to. On `show`, adds the price at providers you have no harness for |
| `--json` | on `list`, `show`, `compare` and `recommend`, as [JSON](#json) says |

### Tasks

The built-in tasks are `overall`, `coding`, `agentic` and `vision`. For a large prompt, bound
the context instead: `--min ctx=200`.

Any other name given to `fav` is a task of your own (`debugging`, and `"Tool Dispatch"` is
stored as `tool-dispatch`).

- No benchmark ranks it. It has the models you give its tiers, and a tier you give none has none.
- `--task` lists them, cheapest first. With `--tier` it returns that tier's model when it fits
  the tier's cap, unless you excluded it or lost access to it.
- `recommend` lists it after the built-in tasks, with what you wrote it is about and the model
  of each tier.
- It is the task to pick for work that fits it, even when a built-in task fits too: for a bug,
  your `debugging` over `coding`.
- `--about` alone makes one with no model yet, as recommend's `+ new task` does in the TUI.
  Agents see it only once it has a model.
- It is gone when its last model and what it is about are cleared.
- `fav <task> --rm` and then `fav <task> --about ""` delete it, as `D` on its name in recommend does.
- `--rename` gives it another name, one no task has yet.

### Sorting and bounds

`--sort`, `--min` and `--max` take these columns:

| Column | Notes |
|---|---|
| `price`, `in`, `cache`, `out` | $ per 1M tokens |
| `ctx` | thousands of tokens, and a value of 10000 or more is read as tokens |
| `release` | a date: `--min release=2026-06` is June 2026 on, `--max release=2026-06` up to the end of June |
| `eci`, `coding`, `agentic` | the scores as the table shows them, `eci` and `coding` in whole points, `agentic` Arena's agent score in percent, to a tenth |
| `tps` | tokens per second, from OpenRouter's GPQA runs |

`list` prints every match unless `-n` limits it, and then says how many it left out. It shows
excluded models marked `✗` and your selected ones `✓`. `--task`, `pick` and `recommend` leave
excluded models out.

Excluded also counts the models the TUI's filters hide. The TUI keeps those filters across
restarts, and only it takes them back, with `XX` or `c`. They are an entry left out of the Dev,
Price or Via dropdown, the entries picked in one, which hide the models of the others, and a
bound typed with `>` or `<`. `exclude --rm` on such a model says which filter hides it, as
`Coding≥60`, and the commands that leave those models out name the filters on stderr. `--dev`,
`--via`, `--min` and `--max` each replace their own: `--dev` the Dev pick, `--max price` the
Price level and `--min coding` a `>` on Coding. `--via ollama` or `--via llama-cli` on a task
keeps a model's local copy off the ranking, which picking them in the TUI's Via dropdown does not.
A bound or the Price level on a column hidden with `|` hides nothing and is kept for when the
column is back.

`recommend` gives the same picks as the TUI's `R` on the `mine` tab with no search, as the TUI
keeps neither its tab nor its search. The commands ignore a pick of `↓ downloadable` in Via, as
only the TUI knows what can be downloaded, and leaving it out only keeps the downloads off the
tiers of its `all` and `✓ selected` tabs. A benchmark picked in a task column's dropdown ranks
that task in both, and a bound on that column is on the benchmark. Both are kept across restarts
until `c`, and the commands say on stderr which tasks a pick ranks. Each prices at its own cache
share, the TUI's set with `%` or `--cache`, so with different shares a Price level or bound can
hide different models.

### Tiers

`--task` keeps the best model of each price level (free, up to $0.5, $2, $5, $15, and above).
`--tier` picks one model from that list, the best at the tier's price:

| Tier | Its pick |
|---|---|
| `free` | the best free model, and none when the list has no free one |
| `low` | the best up to $2 per 1M tokens |
| `mid` | the best up to $5 |
| `high` | the best at any price. Scores count as the table shows them, whole points or Arena's tenths, so of two models with the same shown score it takes the cheaper |

- `modelcmp tier low 1` or `modelcmp tier mid 8` changes a cap, and `--rm` puts it back to its
  default, as an empty cap on the tier's header in the TUI's recommend does. `low` can't cost more
  than `mid`. The list splits at the caps too, so a tier picks the best model up to its cap.
- Two tiers pick the same model when the higher tier's cap lets in no better one. A tier never
  takes a worse model just to differ from the next.
- A model your machine runs takes a tier when it scores higher than the tier's pick, as it is
  free there. See [models on your machine](#models-on-your-machine).
- A tier never holds a model dearer than its cap, your favorite included. With no model at its
  price it has none, as `free` does when there is no free model, and `pick` says `no model` there.
- A tier goes by the price paid where the model runs. With `--via` that is what those harnesses
  charge, which the Price column, `--max price` and the JSON's `price` show too. `--id`, `--cmd`
  and `pick` start a tier's pick on a harness whose own price fits the tier, among `--via`'s when
  given, so a model opencode has free runs there and not on pi, which charges for it.

### Favorites

- A model you `fav` for a tier beats the tier's pick when it costs no more than the tier's cap.
  `fav` needs `--tier` with a model and refuses one over the cap. Without `--tier`, `fav <task>`
  shows the task's favorites and `--rm` clears them all.
- Every favorite of the task that you have and did not exclude sits on the task's list,
  whatever `--dev`, `--via`, `--min`, `--max` or `--selected` narrow it to.
- `recommend` marks it `★`, whether or not it is on the frontier or has a score for the task,
  which then shows as `-`.
- `--tier` with `--no-fav` gives the tier's pick without your favorites, for when a favorite is
  on no harness you can start (`--tier mid --via claude --no-fav --id`). A task of your own
  has no pick without your favorites.

### `--id` and `--cmd`

`--id` prints only the model's id, the first of these that applies:

1. With `--via` naming a harness, the id that harness takes (`--via claude --id` gives
   `claude-opus-5-5`, `--via opencode` the id opencode takes, for a script that always starts
   opencode). It is an error when the harness lacks the model.
2. The id the model's harness takes (`modelcmp harness claude opus-5.5`, `H`), while it has the model.
3. The id your default harness takes (`modelcmp harness`, `P`), when it has the model.
4. The id its developer's own harness takes (claude for a Claude model), as `--cmd` starts it.
5. The id the harness running modelcmp takes, when an agent in claude, opencode, pi, omp, codex
   or gemini runs it (copilot does not say so).
6. The id the first harness to have the model takes, in Via's order: opencode, pi, omp, and for
   a model only your machine runs, the tag `ollama run` takes, or the repo (`-hf`) or file
   (`-m`) `llama-cli` takes.
7. The id at the provider you'd pay.

`--cmd` prints the command that starts the harness on the model instead (`claude --model
claude-opus-5-5`). The harness is the model's, else your default one, else its developer's own
(claude for a Claude model, codex for OpenAI's, gemini for Google's), else the one running
modelcmp, else the first that has the model in Via's order, among `--via`'s when given. Without
`--via` these last three skip a harness you left out in the TUI's Via dropdown while another has
the model, for `--id` too. With `--tier` only the harnesses whose own price fits the tier count.
A model no harness has is an error.

- With `--tier`, `--id` or `--cmd`, no match is an error (exit code 1), so a harness is never
  started on an empty model. Without them `list` says `no models match` and exits with 0.
- With `--tier`, `--id` and `--cmd` also say the model's context and your note on stderr
  (`Claude Opus 5.5: context 1000000, note: slow`), where an agent sees them and a `$(...)`
  does not capture them.

### `pick`

`pick` is `list --task --tier` for every task and tier at once, your own tasks last.

- Under each task and what it is for, it prints a line per tier with the model, its context
  and your note. Tiers with the same model share a line (`low/mid`), and tasks with the same
  lines are listed together above them, so an agent reads each line once.
- Each line has the command that runs one prompt on the model and exits, on the harness
  `--cmd` goes by (`modelcmp quiet opencode run --model opencode/ling-3.1-flash-free
  "<prompt>"`, `modelcmp quiet pi --model ... -p "<prompt>"`), so an agent in one harness
  reaches a model only another has.
- `modelcmp quiet <command>` runs the command and hides what it prints on stderr (the
  harness's steps), so the agent reads only the answer. When the run fails, it prints the last
  20 lines of stderr instead and exits with the command's exit code.
- The command has the flags that let claude, codex, gemini or copilot edit files and run
  commands (tests, builds) without asking, as opencode, pi and omp do. `ollama run` gets
  `--nowordwrap --hidethinking`, so it prints the answer alone.
- With `--via` the model is named by the id that harness takes. A favorite the harness lacks
  is shown after the tier's own pick, with the command that starts it where it runs: `(★ favorite
  Ling 3.1 Flash is not on claude, run it with: modelcmp quiet opencode run --model
  opencode/ling-3.1-flash-free "<prompt>")`.
- `--min` and `--not` leave a favorite out too. A tier's favorite they leave out gives way to
  the tier's pick, and a task of your own whose model is out says `no model`.
- When no task has a model (`--via ollama`, whose copies are picked only as favorites, or a
  `--min` nothing meets), `pick` fails and tells the agent to choose no model and tell the
  user. With `--task`, it tells the agent to try another task.
- `--task` and `--tier` print only that task or tier, for an agent that chose it already.

### Naming a model

Model names match by substring, among the models you have first. The shortest match wins only
when every other contains it (`opus-4.5` over its `-thinking` variant), else the name is
ambiguous. An id as a harness takes it matches too, `pick`'s `llama-cli -hf` repo included.

| Exit code | When |
|---|---|
| 1 | an unknown `--dev` or `--via`, rather than an empty list |
| 2 | an unknown task or harness, or a `models-dir` that is not a folder |
| 3 | an ambiguous model name, and it lists the candidates |

## What you have

A refresh asks each harness on your `PATH` which models it has, and the Via column names them:

- `opencode models`, `pi --list-models` and `omp models` list theirs.
- The `claude`, `codex` and `gemini` binaries each count as their own provider, once logged in.
  One logged out has no models, and `!` says so. A check that cannot tell counts as logged in.
  gemini has no status command: with no Google login saved and no key or other auth type set, it
  is logged out.
- `copilot` gives the models GitHub Copilot's CLI takes on your plan, asked of the CLI itself
  with `copilot --acp`, under its own login. Each refresh leaves an empty folder in
  `~/.copilot/session-state/`. The VS Code extension alone is not enough, install the CLI.
  Logged out, it lists no models, and `!` says so.
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

A tier takes the best model your machine runs when it scores higher than the tier's pick, as it
is free there, and `recommend --json` names it under `local` for each task. `pick` and `--cmd`
start such a model on your machine with ollama or llama.cpp, even when a harness offers it at a
price. On `mine`, which `recommend` and `pick` are, a model you have not downloaded never takes a
tier, however well it would fit: it is not one you have. On the TUI's `all` tab, and on
`✓ selected` when you selected it, a model whose download takes at most 80% of your machine's
memory, leaving the rest for the context, takes a tier too, marked `↓` with the size, as
`modelcmp get` or `x` has to download it first.

The memory is that of the NVIDIA GPUs (`nvidia-smi`, looked up once a week), else the RAM, which a
Mac's GPU shares. `modelcmp memory 24` sets it by hand, in GB as a GPU's memory is given, for
another GPU or a model split between GPU and RAM, and `modelcmp memory` shows it. The TUI shows a
download's size in Via, in `x` and in the details, grey when it is over that 80% and in the text
colour when it fits, on a greyed row too.

The scores are those of the full weights, which a quantized copy falls a little short of, and the
context shown is not the copy's either. With `--via` a tier never takes a local model this way, and
`--via ollama` and `--via llama-cli` on a task pick a copy only as your favorite. Leaving out
`ollama` and `llama-cli` in the TUI's Via dropdown keeps the models on your machine off the tiers,
in `recommend` and `pick` too, and leaving out `↓ downloadable` does the same for the downloads on
`all` and `✓ selected`.

`ollama run` and `llama-cli` answer a prompt with text alone. For the model to edit files and run
commands, as an agent, run it in pi, opencode or omp. Add your runner to the harness's config
under the provider `ollama` (`llama-cli` for llama.cpp, with `llama-server` and its own address),
as modelcmp knows the model by that name alone. For pi, that is `~/.pi/agent/models.json` as
below, then refresh, and Via shows `pi` beside `oll`.

```json
{ "providers": { "ollama": {
  "baseUrl": "http://localhost:11434/v1", "api": "openai-completions", "apiKey": "ollama",
  "models": [{ "id": "qwen3.5:4b" }]
} } }
```

`modelcmp fav coding qwen3.5-4b --tier free` then makes `pick` start it in pi for that task as any other
model, `modelcmp quiet pi --model ollama/qwen3.5:4b -p "<prompt>"`. The run is a process of its
own with its own context, so the agent that starts it reads the answer alone.

ollama gives a model 4096 tokens of context unless told otherwise, too few for pi's prompt and
tools, so the model writes a command instead of running it. Set `OLLAMA_CONTEXT_LENGTH=16384`
where ollama serves (`systemctl edit ollama` on Linux), and `ollama ps` then shows 16384 under
CONTEXT. With 16384, Qwen3.5 4B counted files with a tool in 70 seconds.

A model is matched by its tag, repo or file name, without the quantization, so the name must be the
one the model has here (`modelcmp list --all` shows them): `gemma-3-4b-it-Q4_K_M.gguf` is Gemma 3 4B IT.
A name that says `it` is the instruction-tuned model, never the base one beside it.
A tag of ollama's own library (`llama3.2:1b`) is the instruct model, as that is what ollama holds under it.
A repo that repeats the developer in its name (`bartowski/Qwen_Qwen3.5-4B-GGUF`) matches none.

`modelcmp get <model>` finds the most downloaded GGUF copy of the model's Hugging Face repo and
starts `llama-cli -hf <repo>` or `ollama run hf.co/<repo>`, which download it. Both say the size
of the download first, as `2.5 GB`. With neither installed it asks before installing one:
llama.cpp with Homebrew, else ollama with its own script on Linux. It cannot get a model with no
repo on Hugging Face, and it does not check that the model fits your machine. In the TUI, `x` on
a model and then `download and run` does the same, with an entry for each runner you have. For a
model no runner here has, there is also an `install, download and run` entry for each runner you
lack that it can install.

## Files

The data is downloaded on the first run, which can take up to a minute, so a command says so on
stderr. It is cached for 24 hours under `~/.cache/modelcmp/` on Linux (`~/Library/Caches/modelcmp/`
on macOS). The TUI's first start of a day also refreshes it, in the background behind the table. A
failed download is tried again up to three times, unless the site says the request itself is
wrong. Your selected models, favorites, exclusions, notes and settings live in
`~/.config/modelcmp/user.json` (`~/Library/Application Support/modelcmp/` on macOS), and the `?`
help shows the path. Each refresh and each start of the TUI also asks GitHub for the latest
release, to say when a new version is out, and `UU` in the TUI upgrades to it.

Both are safe to delete: the cache is downloaded again on the next run, and removing `user.json`
resets your selected models, favorites, exclusions, notes and settings.

## Data age

The TUI's bottom border always shows the refresh state, one of these:

- `⟳ refreshing 7/9, waiting for opencode`: how many downloads and harnesses have answered, and
  the last one or two still awaited.
- `refresh failed` in red, with the age of the data still shown.
- `⚠ refresh had issues` in yellow, until a refresh has none.
- The data age alone, red once it is past 24h. The status bar then says it too, as `data 25h old`
  in red, with the hint `r refresh`.

`!` or a click on the `⚠` lists the issues, and the status bar hints `! issues` while there are
any. The status bar itself says only what a key or a refresh you started did.

When Epoch AI sends no scores for one of the benchmarks a task uses, as when its index names no
file for it, a warning names the benchmark, in `!` and at every command. The first start or
command an hour later downloads the data again.

## JSON

`list`, `show`, `compare` and `recommend` take `--json`. It is indented at a terminal and one line in a pipe.

```sh
opencode -m $(modelcmp list --task coding --tier mid --via opencode --id)   # --id prints just provider/model
modelcmp recommend --json                      # choose the task by its "when"
modelcmp list --task coding --tier mid --json  # the one model to use: the user's favorite for the tier when set, else the tier's
```

Each model carries these fields:

| Field | What it is |
|---|---|
| `key` | the model's key |
| `name`, `developer` | as the table shows them |
| `available` | a harness of yours has it |
| `via` | the harnesses that have it |
| `selected` | on your shortlist |
| `excluded` | you excluded it, or one of the TUI's filters hides it |
| `note` | your note on it, null when it has none |
| `favorite_for` | the tiers it is the user's favorite for, as `task:tier` |
| `price` | the offer you'd pay, as below |
| `context`, `max_output` | in tokens |
| `tool_call`, `reasoning`, `vision`, `open_weights` | what it can do, true or false |
| `release`, `knowledge` | its release date and knowledge cutoff, empty when unknown |
| `url` | its page, the first of `pages` below, null when no site has one |
| `eci` | the Epoch Capabilities Index, as Epoch publishes it |
| `tasks` | each task's table column: coding's FrontierCode score, 0-100, agentic's Arena agent score in percent, the others on the ECI scale, or a benchmark's 0-100 score when one is picked in the TUI |
| `basis` | what each task's score is from: `ECI`, `3 benchmarks`, `FrontierCode` or `Arena` |
| `arena_agent` | Arena, in percent, when Arena ranks the model |
| `arena_chat`, `arena_webdev` | Arena's chat and WebDev ratings, when Arena rates the model |
| `tokens_per_second` | from OpenRouter's GPQA diamond runs, when it ran the model |

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

`recommend --json` has a list of tasks, each with its `when`, its `frontier`, the `tiers` the
TUI's `R` shows (the key each of `free`, `low`, `mid` and `high` picks, or one `pick` for a task
with one), and the user's `tier_favorites`.

- A frontier entry carries its `context` in tokens, `via` (the harnesses that have the model,
  as in `list --json`), the `harness` the user runs it on when they chose one that has it, and
  the user's `note` when the model has one.
- A favorite that is on the line only as the favorite has `"recommended": false`, and the text
  output marks it `not recommended`.
- The text output prints each tier's pick as `name [key] · $price · score`, the score being
  the task's table column, the same as `tasks`. Under a favorite that is not the tier's own
  pick, a `recommended:` line names that pick, which the JSON leaves out.

A task of your own follows the built-in ones with the same fields and `"custom": true`:

| Field | What it is |
|---|---|
| `when` | what the user wrote, and that it wins over a built-in task that fits too |
| `tier_favorites` | the models the user gave its tiers |
| `frontier` | those models, cheapest first, with a null `score` |
