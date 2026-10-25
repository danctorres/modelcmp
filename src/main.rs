mod app;
mod cli;
mod data;
mod fit;
mod store;
mod tui;
mod view;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};
use cli::Exit;
use store::Store;

/// Pick the right LLM: prices from models.dev and benchmarks from Epoch AI or Artificial Analysis,
/// for the models you can already use. Those are the models opencode, pi and omp list, Anthropic's
/// for claude, OpenAI's for codex, Google's for gemini, the ones your Copilot plan includes, and the
/// ones ollama and llama-cli have on your machine. The Via column shows which
/// harness has each model. Run without a command for the interactive TUI.
#[derive(Parser)]
#[command(
    version,
    after_help = "Data: models.dev (prices), Epoch AI (benchmarks, CC-BY) or Artificial Analysis (benchmarks, https://artificialanalysis.ai/), Arena (agent, chat and WebDev leaderboards, CC-BY). Cached for 24h."
)]
struct Args {
    /// Re-download data now instead of using the cache
    #[arg(long, global = true)]
    refresh: bool,
    /// The percent of input tokens read from the prompt cache, for the Price column: 90 suits an agent session, 0 a one-off prompt (`%` in the TUI)
    #[arg(long, global = true, value_name = "PERCENT", default_value_t = 90, value_parser = clap::value_parser!(u8).range(0..=100))]
    cache: u8,
    /// Where benchmarks come from: epoch (Epoch AI) or aa (Artificial Analysis, which needs ARTIFICIAL_ANALYSIS_API_KEY or a key saved in the TUI). Defaults to aa when a working key is set, else epoch
    #[arg(long, global = true, value_parser = PossibleValuesParser::new(data::Source::ALL.map(|s| s.id())))]
    source: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the models you have access to (every model with --all), filtered, sorted or ranked for a task
    #[command(alias = "ls")]
    List {
        /// The best model at each price level for a task (overall, coding, agentic, reasoning, vision, value), cheapest first: each row costs more and scores higher than the one before (`R` then `enter` on a task in the TUI). Excluded models are left out. A task of your own (see `fav`) lists the favorites you gave it
        #[arg(short, long, value_parser = store::task_name, conflicts_with = "sort")]
        task: Option<String>,
        /// Print one model from the task's list: your favorite for the tier, if it costs no more than the tier's cap. Otherwise the best model the tier allows: free = only free models, low = up to $2 per 1M tokens, mid = up to $5 (`modelcmp tier` changes these), high = any price. It fails when no model fits the tier. When that model is not your favorite, a model your machine runs or can download (`modelcmp memory`) replaces it if it scores higher, since it runs for free. A task of your own gives the tier's favorite
        #[arg(long, requires = "task", value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
        /// Sort by a column, best first: cheapest, or highest score (`s` in the TUI)
        #[arg(short, long, value_parser = PossibleValuesParser::new(app::COLS.map(|c| c.id)))]
        sort: Option<String>,
        /// Keep models at or above a value, e.g. --min coding=60. It takes the columns --sort takes. ctx is in thousands of tokens (or in tokens from 10000 up) and release is a date (2026-06). Repeatable. It replaces a `>` bound set in the TUI on the same column
        #[arg(long, value_parser = |s: &str| bound(s, false))]
        min: Vec<(usize, f64)>,
        /// Keep models at or below a value, e.g. --max price=2. A release date includes its whole month or year. Repeatable. It replaces a `<` bound set in the TUI on the same column, and --max price also replaces the Price level picked there
        #[arg(long, value_parser = |s: &str| bound(s, true))]
        max: Vec<(usize, f64)>,
        /// Include models you have no access to
        #[arg(short, long)]
        all: bool,
        /// Only the selected models, your shortlist (`S` in the TUI)
        #[arg(short = 'm', long, alias = "marked")]
        selected: bool,
        /// Only these developers or countries, e.g. --dev anthropic --dev china. It replaces what is picked in the TUI's Dev dropdown (`d`)
        #[arg(long)]
        dev: Vec<String>,
        /// Only models you have through these harnesses (opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli), at the price they charge, which Price, --max price and --tier go by. Repeatable. It replaces what is picked in the TUI's Via dropdown (`d`)
        #[arg(long)]
        via: Vec<String>,
        /// Max rows, 0 = no limit
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Print only each model's id, one per line. With --via, the id that harness takes, or an error when it does not have the model: `opencode -m $(modelcmp list --task coding --tier mid --via opencode --id)`. Without --via, the id for the harness --cmd starts on it, else the model's own id
        #[arg(long, conflicts_with = "json")]
        id: bool,
        /// Ignore your favorites and print the tier's best model, for when the favorite is on no harness you can start
        #[arg(long, requires = "tier")]
        no_fav: bool,
        /// Print only the command that starts a harness on each model, one per line: `$(modelcmp list --task coding --tier mid --cmd)`. The harness is the one set on the model (`modelcmp harness <harness> <model>`), else your default harness, else the model's developer's own (claude for a Claude model), else the first one that has the model, out of the --via ones when given. With --tier, only one whose own price fits the tier counts
        #[arg(long, conflicts_with_all = ["json", "id"])]
        cmd: bool,
    },
    /// For agents, in one call: every task with what it is for, and the model for each of its tiers
    ///
    /// Each tier (free, low, mid, high) gets one model, with its context size and your note. It is your favorite if that fits the tier's price, else the best model at that price
    Pick {
        /// The harness you will start (opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli): prints the ids it takes, picked at the price it charges. A favorite it does not have comes after the tier's pick, with the command that runs it elsewhere. Without --via, prints the command that runs a prompt on each model, on a harness that has it at the tier's price
        #[arg(long)]
        via: Vec<String>,
        /// Only models at or above a value, favorites included, e.g. --min ctx=600 for a prompt of 600,000 tokens
        #[arg(long, value_parser = |s: &str| bound(s, false))]
        min: Vec<(usize, f64)>,
        /// Leave out a model, even a favorite, e.g. one your note rules out for this work. Takes its name or the printed id. Repeatable
        #[arg(long, value_name = "MODEL")]
        not: Vec<String>,
        /// Only this task, when you have already chosen one
        #[arg(short, long, value_parser = store::task_name)]
        task: Option<String>,
        /// Only this tier
        #[arg(long, value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
    },
    /// Run a command that `pick` printed, for agents: its stderr (the harness's progress) is hidden, and its last 20 lines are shown if it fails
    Quiet {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Everything about one model
    Show {
        model: String,
        /// The price at every provider, not only the ones you have a harness for
        #[arg(short, long)]
        all: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Compare models side by side, starting with a verdict: cheapest, best value, and per task the best then each cheaper one worth paying for
    Compare {
        #[arg(num_args = 2.., required = true)]
        models: Vec<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Open the model's web page (`o` in the TUI)
    Open {
        model: String,
        /// The site: models.dev, epoch.ai, artificialanalysis.ai (or aa), openrouter.ai,
        /// huggingface.co (or hf) or arena.ai (its agent leaderboard). A prefix is enough. Without it, the first of these with a page for the model
        #[arg(long)]
        on: Option<String>,
    },
    /// Download a copy of the model to your machine and run it there (`x` in the TUI)
    ///
    /// It runs the copy you already have, or downloads the model's GGUF copy from Hugging Face with `llama-cli -hf` or `ollama run`. When neither is installed, it asks before installing one
    Get {
        model: String,
        /// What runs it: ollama or llama-cli. Without it, the one you have
        #[arg(long)]
        via: Option<String>,
        /// Wait for enter after an error, which a terminal opened for this would close on
        #[arg(long, hide = true)]
        pause: bool,
    },
    /// Select a model to add it to your shortlist, which `list --selected` shows (space in the TUI)
    #[command(alias = "mark")]
    Select {
        model: String,
        /// Deselect it instead
        #[arg(long)]
        rm: bool,
    },
    /// Exclude a model you have but cannot use, so --task, pick and recommend leave it out (`e` in the TUI)
    Exclude {
        model: String,
        /// Include it again
        #[arg(long)]
        rm: bool,
    },
    /// Show or set a model's note (`i` in the TUI). Agents read it when choosing a model
    Note {
        model: String,
        #[arg(conflicts_with = "rm")]
        text: Option<String>,
        /// Delete the note
        #[arg(long)]
        rm: bool,
    },
    /// Show or set your default harness, or the harness a model runs on (`P` and `H` in the TUI)
    ///
    /// One of opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli. `list --cmd`, `list --id` and `pick` run a model on the harness you gave it, else on your default harness, else on its developer's own (claude for a Claude model), else on the first that has it. `modelcmp harness claude opus-5.5 sonnet-5` runs those two on claude
    Harness {
        name: Option<String>,
        /// The models to run on it, else it is your default harness
        models: Vec<String>,
        /// Clear your default harness, or the harness of the models named
        #[arg(long)]
        rm: bool,
    },
    /// Show every tier's price cap, or set the low or mid tier's
    ///
    /// A tier picks the best model at or under its cap. By default low is up to $2 per 1M tokens and mid up to $5. free takes only free models and high any price
    Tier {
        #[arg(value_parser = PossibleValuesParser::new(["low", "mid"]))]
        name: Option<String>,
        /// The highest price it allows, in $ per 1M tokens
        #[arg(requires = "name", conflicts_with = "rm")]
        usd: Option<f64>,
        /// Reset it to its default
        #[arg(long, requires = "name")]
        rm: bool,
    },
    /// Show or set the folder of the `.gguf` files you downloaded
    ///
    /// llama.cpp runs the models in it, and a refresh lists them. `LLAMA_ARG_MODELS_DIR` is used instead when it is set
    ModelsDir {
        #[arg(conflicts_with = "rm")]
        dir: Option<String>,
        /// Clear it
        #[arg(long)]
        rm: bool,
    },
    /// Show or set the memory local models run in, in GB: your GPU's, or a Mac's RAM
    ///
    /// A tier takes the best model your machine runs or can download when it scores higher than the tier's pick. A download fits when it takes at most 80% of this memory, leaving the rest for its context. When not set, it is the memory of your NVIDIA GPUs, else the RAM
    Memory {
        #[arg(conflicts_with = "rm")]
        gb: Option<f64>,
        /// Go back to the memory found on this machine
        #[arg(long)]
        rm: bool,
    },
    /// Show or set your favorite model for a tier of a task (`f` in the TUI)
    ///
    /// It must fit the tier's price. `list --tier` and `pick` choose it, and `recommend` marks it ★. With no task, shows every favorite
    Fav {
        /// overall, coding, agentic, reasoning, vision or value. Any other name is a task of your own, e.g. debugging, whose only models are the favorites you give it
        #[arg(value_parser = store::task_name)]
        task: Option<String>,
        #[arg(requires_all = ["task", "tier"], conflicts_with = "rm")]
        model: Option<String>,
        /// The tier it is the favorite for, e.g. a cheap model for low and a strong one for high
        #[arg(long, requires = "task", value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
        /// Clear the task's favorites, or only the tier's with --tier. A task of your own is deleted with its last favorite, unless it has an --about text (--about "" clears that)
        #[arg(long, requires = "task")]
        rm: bool,
        /// Rename a task of your own. Its favorites stay
        #[arg(long, value_name = "NAME", requires = "task", conflicts_with_all = ["model", "tier", "rm"], value_parser = store::task_name)]
        rename: Option<String>,
        /// Describe a task of your own in your words. Agents choose the task by this text, even over a built-in task that also fits. It can create a task before it has a model, but agents only see the task once it has one. "" clears it
        #[arg(long, value_name = "TEXT", requires = "task", conflicts_with_all = ["rm", "rename"])]
        about: Option<String>,
    },
    /// The best model at each price for each task, with what the task measures and when to use it (`R` in the TUI)
    Recommend {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `coding=60` for --min and --max: the column's index in `app::COLS` and the value.
fn bound(s: &str, max: bool) -> Result<(usize, f64), String> {
    let ids = || app::COLS.map(|c| c.id).join(", ");
    let (id, v) =
        s.split_once('=').ok_or_else(|| format!("expected column=value, e.g. coding=60, columns: {}", ids()))?;
    let col =
        app::COLS.iter().position(|c| c.id == id).ok_or_else(|| format!("no column '{id}', columns: {}", ids()))?;
    let v = (app::COLS[col].read)(v, max).ok_or_else(|| format!("'{v}' is not a valid value for {id}"))?;
    Ok((col, v))
}

fn main() {
    let args = Args::parse();
    data::set_cached(f64::from(args.cache) / 100.0);
    let store = Store::load();
    data::set_models_dir(&store.models_dir);
    let source = args.source.as_deref().and_then(data::Source::parse);
    data::set_source(source.unwrap_or_else(data::Source::preferred));
    data::set_chosen(source.is_some());
    let result = match args.cmd {
        None => {
            // Asked the first time the TUI opens, and not with a key there: Artificial Analysis
            // is then the source.
            let ask = source.is_none() && store.seen.is_empty() && data::aa_key().is_none();
            tui::run(store, args.refresh, ask).map_err(Exit::from)
        }
        Some(cmd) => {
            // Die quietly when a pipe closes early, as in `modelcmp list | head`, instead of
            // panicking in println. Not in the TUI: it writes to clipboard tools that may exit.
            #[cfg(unix)]
            #[allow(unsafe_code)]
            // SAFETY: nothing else is running yet to observe the signal disposition change.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            }
            if let Some(w) = &store.warning {
                warn(w);
            }
            // The one model to use, chosen without the exclusions and favorites the file held:
            // a failure, as the warning is said once and a `$(...)` shows it to nobody.
            let one = matches!(cmd, Cmd::Pick { .. })
                || matches!(cmd, Cmd::List { ref tier, id, cmd, .. } if tier.is_some() || id || cmd);
            if store.warning.is_some() && one {
                Err(Exit::from(
                    "no model picked, as your favorites and exclusions may not apply: see the warning above, then run again"
                        .to_string(),
                ))
            } else {
                run(cmd, args.refresh, source.is_none())
            }
        }
    };
    if let Err(e) = result {
        eprintln!("modelcmp: {}", e.msg);
        std::process::exit(e.code);
    }
}

/// One line per warning: a refresh joins its warnings with `data::SEP`, which the TUI lists as bullets.
fn warn(w: &str) {
    let (from, w) = w.strip_prefix(data::LAST_REFRESH).map_or(("", w), |w| (data::LAST_REFRESH, w));
    for one in w.split(data::SEP) {
        eprintln!("warning: {from}{one}");
    }
}

/// Epoch has no speed: a bound on it would drop every model, a sort do nothing.
fn shown(mut cols: impl Iterator<Item = usize>) -> Result<(), Exit> {
    match cols.find(|&c| app::hidden(c + app::TEXT) || app::unmeasured(c + app::TEXT)) {
        Some(c) => {
            let (id, src) = (app::COLS[app::by_role(c)].id, app::COLS[c].from());
            Err(Exit::from(format!("{id} needs --source {}: only {} has it", src.id(), src.label())))
        }
        None => Ok(()),
    }
}

/// `auto`: no `--source` was given, so a key that does not work leaves Epoch AI.
fn run(cmd: Cmd, force: bool, auto: bool) -> Result<(), Exit> {
    // These name no model: they neither wait for a download nor fail without one.
    let bare = match &cmd {
        // `--rm` takes every name as a model's.
        Cmd::Harness { name, models, rm } => models.is_empty() && !(*rm && name.is_some()),
        _ => matches!(
            cmd,
            Cmd::Tier { .. }
                | Cmd::ModelsDir { .. }
                | Cmd::Memory { .. }
                | Cmd::Quiet { .. }
                | Cmd::Fav { task: Some(_), rename: Some(_), .. }
        ),
    };
    let mut data = if bare {
        data::Data::default()
    } else {
        let (data, warn_of) = data::load(force, auto).map_err(|e| e.to_string())?;
        // The other source's columns sort and bound too, with its data there.
        data::set_lent(data.lent);
        if let Some(w) = warn_of {
            warn(&w);
        }
        // Epoch AI by default for want of a key, which the TUI's frame says too. Not when asked
        // for, and only where benchmarks are what is shown: not for the id or command a script
        // takes from `$(...)`.
        let shown = matches!(cmd, Cmd::List { id: false, cmd: false, .. } | Cmd::Recommend { .. });
        if shown && data::keyless() {
            eprintln!(
                "warning: no {} API key, so benchmarks are from Epoch AI (set {}, or press K in the TUI)",
                data::Source::Aa.label(),
                data::AA_KEY_ENV
            );
        }
        if !data.any_available() {
            eprintln!("note: {}", view::NO_ACCESS);
        }
        data
    };
    // A command that saves holds the lock from load to save.
    let _lock = matches!(
        cmd,
        Cmd::Select { .. }
            | Cmd::Exclude { .. }
            | Cmd::Note { .. }
            | Cmd::Fav { .. }
            | Cmd::Harness { .. }
            | Cmd::Tier { .. }
            | Cmd::ModelsDir { .. }
            | Cmd::Memory { .. }
    )
    .then(|| store::lock(&store::path()))
    .transpose()
    .map_err(|e| Exit::from(format!("cannot lock {}: {e}", store::path().display())))?;
    // Loaded after the download, which can take a minute: what the TUI or an agent saved meanwhile is kept.
    let mut store = Store::load();
    match cmd {
        Cmd::List { task: t, tier, sort, min, max, all, selected, dev, via, limit, json, id, cmd, no_fav } => {
            let sort = sort.and_then(|s| app::COLS.iter().position(|c| c.id == s)).map(app::by_role);
            let bounds: Vec<_> = min
                .into_iter()
                .map(|(c, v)| (app::by_role(c), v, f64::INFINITY))
                .chain(max.into_iter().map(|(c, v)| (app::by_role(c), f64::NEG_INFINITY, v)))
                .collect();
            shown(sort.into_iter().chain(bounds.iter().map(|b| b.0)))?;
            // A name that is no built-in task is one of your own.
            let task = t.as_deref().and_then(fit::task);
            let custom = t.filter(|_| task.is_none());
            let opts = cli::ListOpts {
                task,
                custom,
                tier,
                sort,
                bounds,
                all,
                selected,
                dev,
                via,
                limit,
                json,
                id,
                cmd,
                no_fav,
                prompt: false,
                not: vec![],
            };
            cli::on_via(&mut data, &opts.via);
            cli::list(&data, &store, &opts)
        }
        Cmd::Pick { via, min, not, task, tier } => {
            let bounds: Vec<_> = min.into_iter().map(|(c, v)| (app::by_role(c), v, f64::INFINITY)).collect();
            shown(bounds.iter().map(|b| b.0))?;
            // The task asked for, built in or yours, goes by its name.
            let opts = cli::ListOpts { via, bounds, custom: task, tier, ..Default::default() };
            cli::on_via(&mut data, &opts.via);
            cli::pick(&data, &store, &opts, &not)
        }
        Cmd::Quiet { cmd } => cli::quiet(&cmd),
        Cmd::Show { model, all, json } => cli::show(&data, &store, &model, json, all),
        Cmd::Compare { models, json } => cli::compare(&data, &store, &models, json),
        Cmd::Open { model, on } => cli::open(&data, &model, on.as_deref()),
        Cmd::Get { model, via, pause } => cli::get(&data, &model, via.as_deref(), pause).inspect_err(|e| {
            if pause {
                eprintln!("modelcmp: {}", e.msg);
                eprintln!("press enter to close");
                let _ = std::io::stdin().read_line(&mut String::new());
                std::process::exit(e.code);
            }
        }),
        Cmd::Select { model, rm } => cli::select(&data, &mut store, &model, rm),
        Cmd::Exclude { model, rm } => cli::exclude(&data, &mut store, &model, rm),
        Cmd::Harness { name, models, rm } => cli::harness(&data, &mut store, name.as_deref(), &models, rm),
        Cmd::Tier { name, usd, rm } => cli::tier(&mut store, name.as_deref(), usd, rm),
        Cmd::ModelsDir { dir, rm } => cli::models_dir(&mut store, dir.as_deref(), rm),
        Cmd::Memory { gb, rm } => cli::memory(&mut store, gb, rm),
        Cmd::Note { model, text, rm } => cli::note(&data, &mut store, &model, text.as_deref(), rm),
        Cmd::Fav { task: Some(task), rename: Some(new), .. } => cli::rename(&mut store, &task, &new),
        Cmd::Fav { task, model, tier, rm, about, .. } => {
            cli::fav(&data, &mut store, task.as_deref(), tier.as_deref(), model.as_deref(), rm, about.as_deref())
        }
        Cmd::Recommend { json } => cli::recommend(&data, &store, json),
    }
}
