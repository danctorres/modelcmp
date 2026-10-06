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

/// Pick the right LLM: prices (models.dev) + benchmarks (Epoch AI, or Artificial Analysis), filtered to the
/// models you can already use: the ones your harnesses list (opencode models, pi --list-models, omp models;
/// claude, codex and gemini give their own provider's, copilot what its CLI takes on your plan, ollama and llama-cli the ones on your machine).
/// The VIA column says which. Run without a command for the interactive TUI.
#[derive(Parser)]
#[command(
    version,
    after_help = "Data: models.dev (prices), Epoch AI (benchmarks, CC-BY) or Artificial Analysis (benchmarks, https://artificialanalysis.ai/). Cached for 24h."
)]
struct Args {
    /// Re-download data now instead of using the cache
    #[arg(long, global = true)]
    refresh: bool,
    /// Percent of input tokens read from the prompt cache in Price: 90 fits an agent session, 0 a one-off prompt (`%` in the TUI)
    #[arg(long, global = true, value_name = "PERCENT", default_value_t = 90, value_parser = clap::value_parser!(u8).range(0..=100))]
    cache: u8,
    /// Benchmarks from: epoch (Epoch AI) or aa (Artificial Analysis, needs ARTIFICIAL_ANALYSIS_API_KEY or a key saved with `B`); overrides `B` in the TUI for this run. With neither picked: aa when its key is there, else epoch
    #[arg(long, global = true, value_parser = PossibleValuesParser::new(data::Source::ALL.map(|s| s.id())))]
    source: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List models you have access to (all with --all): rank, bound and sort them
    #[command(alias = "ls")]
    List {
        /// Best model per price level for a task (overall, coding, agentic, reasoning, value, vision): cheapest first, each row costing more and scoring higher; excluded models are left out (`R` then `enter` in the TUI). A task of your own (`fav`) gives the models you gave it
        #[arg(short, long, value_parser = store::task_name, conflicts_with = "sort")]
        task: Option<String>,
        /// One model from the task's list: your favorite for the tier, else for the task; else low = cheapest within 8 months of progress of your best model, mid = cheapest within 3, high = your best; a task of your own gives the tier's model, else the task's
        #[arg(long, requires = "task", value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
        /// Sort by a column, best first: cheapest, or highest score (`s` in the TUI)
        #[arg(short, long, value_parser = PossibleValuesParser::new(app::COLS.map(|c| c.id)))]
        sort: Option<String>,
        /// Keep models at or above a value, e.g. --min coding=155; columns as in --sort, ctx in thousands of tokens (or in tokens from 10000), release as a date (2026-06); repeatable (`>` in the TUI)
        #[arg(long, value_parser = |s: &str| bound(s, false))]
        min: Vec<(usize, f64)>,
        /// Keep models at or below a value, e.g. --max price=2; a release up to the end of its month or year; repeatable (`<` in the TUI)
        #[arg(long, value_parser = |s: &str| bound(s, true))]
        max: Vec<(usize, f64)>,
        /// Include models you have no access to
        #[arg(short, long)]
        all: bool,
        /// Selected only: your shortlist (`S` in the TUI)
        #[arg(short = 'm', long, alias = "marked")]
        selected: bool,
        /// Only these developers or countries, e.g. --dev anthropic --dev china (the Dev dropdown, `d`, in the TUI)
        #[arg(long)]
        dev: Vec<String>,
        /// Only models you have through these harnesses (opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli); repeatable (the Via dropdown, `d`, in the TUI)
        #[arg(long)]
        via: Vec<String>,
        /// Max rows, 0 = no limit
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Only the provider/model ids opencode takes, one per line: `opencode -m $(modelcmp list --task coding --tier mid --id)`; a favorite given a harness (`fav --via`) prints the id that one takes, else your default harness's (`modelcmp harness`). --via with a harness prints the id that one takes instead (`--via claude --id`), an error when it lacks the model
        #[arg(long, conflicts_with = "json")]
        id: bool,
        /// The tier's pick without your favorites: the model to use when a favorite is on no harness you can start
        #[arg(long, requires = "tier")]
        no_fav: bool,
        /// Only the command that starts a harness on each model, one per line: `$(modelcmp list --task coding --tier mid --cmd)`; the harness of a favorite given one (`fav --via`), else your default one (`modelcmp harness`), else the first that has the model, among --via's when given
        #[arg(long, conflicts_with_all = ["json", "id"])]
        cmd: bool,
    },
    /// For an agent, one call: each task with what it is for, and the model for its low, mid and high tier (your favorite, else the cheapest good enough), with its context and your note
    Pick {
        /// The harness you will start (opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli): prints the ids it takes, and a favorite it lacks after the tier's own pick. Without it, the command that runs a prompt on each model, on the harness you have it on
        #[arg(long)]
        via: Vec<String>,
        /// Only models at or above a value, favorites too: --min ctx=600 for a prompt of 600,000 tokens
        #[arg(long, value_parser = |s: &str| bound(s, false))]
        min: Vec<(usize, f64)>,
        /// Leave a model out, a favorite too, e.g. one your note rules out for this work: its name or the id printed; repeatable
        #[arg(long, value_name = "MODEL")]
        not: Vec<String>,
        /// Only this task, for one chosen already
        #[arg(short, long, value_parser = store::task_name)]
        task: Option<String>,
        /// Only this tier
        #[arg(long, value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
    },
    /// Run a command as `pick` prints it, for an agent: what it says on stderr (a harness's steps) is left out, and its last 20 lines are said when it fails
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
    /// Side-by-side comparison, opening with a verdict: cheapest, best at coding, best value
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
        /// The site: models.dev, epoch.ai, artificialanalysis.ai (or aa), openrouter.ai or
        /// huggingface.co (or hf), a prefix will do; the first of them to have the model when left out
        #[arg(long)]
        on: Option<String>,
    },
    /// Download a copy of the model to your machine and run it there (`x` in the TUI)
    ///
    /// The GGUF one Hugging Face has of it, by `llama-cli -hf` or `ollama run`, after installing one of them, once you agree, when you have neither
    Get {
        model: String,
        /// The one that runs it (ollama or llama-cli), the one you have when left out
        #[arg(long)]
        via: Option<String>,
        /// Wait for enter after an error, which a terminal opened for this would close on
        #[arg(long, hide = true)]
        pause: bool,
    },
    /// Select a model, to shortlist it until the TUI closes: `list --selected` shows them (space in the TUI)
    #[command(alias = "mark")]
    Select {
        model: String,
        /// Remove it instead
        #[arg(long)]
        rm: bool,
    },
    /// Exclude a model you have but cannot use: --task and recommend leave it out (`e` in the TUI)
    Exclude {
        model: String,
        /// Include it again
        #[arg(long)]
        rm: bool,
    },
    /// Show a model's note, or set it (`n` in the TUI); agents read it when choosing
    Note {
        model: String,
        #[arg(conflicts_with = "rm")]
        text: Option<String>,
        /// Delete the note
        #[arg(long)]
        rm: bool,
    },
    /// Your default harness, alone shows it (`H` in the TUI)
    ///
    /// One of opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli: `list --cmd` and `--id` go by it when it has the model and --via names none, after a favorite's own harness (`fav --via`)
    Harness {
        #[arg(conflicts_with = "rm")]
        name: Option<String>,
        /// Clear it
        #[arg(long)]
        rm: bool,
    },
    /// The folder of the `.gguf` files you downloaded, alone shows it
    ///
    /// llama.cpp runs the models in it and a refresh lists them, when `LLAMA_ARG_MODELS_DIR` is not set
    ModelsDir {
        #[arg(conflicts_with = "rm")]
        dir: Option<String>,
        /// Clear it
        #[arg(long)]
        rm: bool,
    },
    /// Your favorite model for a task, or for one tier of it: --tier picks it and recommend marks it ★; alone, shows them (`f` in the TUI)
    Fav {
        /// overall, coding, agentic, reasoning, value or vision; any other name is a task of your own, e.g. debugging, which has only the models you give it
        #[arg(value_parser = store::task_name)]
        task: Option<String>,
        #[arg(requires = "task", conflicts_with = "rm")]
        model: Option<String>,
        /// Only for `list --tier` with this tier, e.g. a cheap model for low and a strong one for the task
        #[arg(long, requires = "task", value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
        /// The harness you run it on (opencode, pi, omp, claude, codex, gemini, copilot, ollama, llama-cli): `list --task --id` prints the id that one takes
        #[arg(long, requires = "model")]
        via: Option<String>,
        /// Clear the task's favorite, or with --tier the tier's; a task of your own is gone with its last model
        #[arg(long, requires = "task")]
        rm: bool,
        /// Give a task of your own this name instead; its models stay
        #[arg(long, value_name = "NAME", requires = "task", conflicts_with_all = ["model", "tier", "rm"], value_parser = store::task_name)]
        rename: Option<String>,
        /// What a task of your own is about, in your words: agents pick the task by it, and over a built-in task that fits too; "" clears it
        #[arg(long, value_name = "TEXT", requires = "task", conflicts_with_all = ["tier", "rm", "rename"])]
        about: Option<String>,
    },
    /// The best model per price for each task, what the task measures and when to use it (`R` in the TUI)
    Recommend {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `coding=155` for --min and --max: the column's index in `app::COLS` and the value.
fn bound(s: &str, max: bool) -> Result<(usize, f64), String> {
    let ids = || app::COLS.map(|c| c.id).join(", ");
    let (id, v) =
        s.split_once('=').ok_or_else(|| format!("expected column=value, e.g. coding=155; columns: {}", ids()))?;
    let col =
        app::COLS.iter().position(|c| c.id == id).ok_or_else(|| format!("no column '{id}'; columns: {}", ids()))?;
    let v = (app::COLS[col].read)(v, max).ok_or_else(|| format!("'{v}' is not a value for {id}"))?;
    Ok((col, v))
}

fn main() {
    let args = Args::parse();
    data::set_cached(f64::from(args.cache) / 100.0);
    let store = Store::load();
    data::set_models_dir(&store.models_dir);
    let picked = args.source.as_deref().unwrap_or(&store.source);
    let source = if picked.is_empty() { Some(data::Source::preferred()) } else { data::Source::parse(picked) };
    data::set_source(source.unwrap_or_default());
    let result = match args.cmd {
        None => {
            let ask = args.source.is_none() && store.source.is_empty();
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
                eprintln!("warning: {w}");
            }
            run(cmd, args.refresh)
        }
    };
    if let Err(e) = result {
        eprintln!("modelcmp: {}", e.msg);
        std::process::exit(e.code);
    }
}

/// Epoch has no speed, Artificial Analysis no cost of a task: a bound on one would drop every
/// model, a sort do nothing.
fn shown(mut cols: impl Iterator<Item = usize>) -> Result<(), Exit> {
    match cols.find(|&c| app::hidden(c + app::TEXT)) {
        Some(c) => {
            let (col, src) = (&app::COLS[c], app::COLS[c].only.unwrap_or_default());
            Err(Exit::from(format!("{} needs --source {}: only {} has it", col.id, src.id(), src.label())))
        }
        None => Ok(()),
    }
}

fn run(cmd: Cmd, force: bool) -> Result<(), Exit> {
    // These name no model: they neither wait for a download nor fail without one.
    let bare = matches!(
        cmd,
        Cmd::Harness { .. }
            | Cmd::ModelsDir { .. }
            | Cmd::Quiet { .. }
            | Cmd::Fav { task: Some(_), rename: Some(_), .. }
    );
    let data = if bare {
        data::Data::default()
    } else {
        let (data, warn) = data::load(force).map_err(|e| e.to_string())?;
        if let Some(w) = warn {
            eprintln!("warning: {w}");
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
            | Cmd::ModelsDir { .. }
    )
    .then(|| store::lock(&store::path()))
    .transpose()
    .map_err(|e| Exit::from(format!("cannot lock {}: {e}", store::path().display())))?;
    // Loaded after the download, which can take a minute: what the TUI or an agent saved meanwhile is kept.
    let mut store = Store::load();
    match cmd {
        Cmd::List { task: t, tier, sort, min, max, all, selected, dev, via, limit, json, id, cmd, no_fav } => {
            let sort = sort.and_then(|s| app::COLS.iter().position(|c| c.id == s));
            shown(sort.into_iter().chain(min.iter().chain(&max).map(|b| b.0)))?;
            let bounds = min
                .into_iter()
                .map(|(c, v)| (c, v, f64::INFINITY))
                .chain(max.into_iter().map(|(c, v)| (c, f64::NEG_INFINITY, v)))
                .collect();
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
            cli::list(&data, &store, &opts)
        }
        Cmd::Pick { via, min, not, task, tier } => {
            shown(min.iter().map(|b| b.0))?;
            let bounds = min.into_iter().map(|(c, v)| (c, v, f64::INFINITY)).collect();
            // The task asked for, built in or yours, goes by its name.
            let opts = cli::ListOpts { via, bounds, custom: task, tier, ..Default::default() };
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
        Cmd::Harness { name, rm } => cli::harness(&mut store, name.as_deref(), rm),
        Cmd::ModelsDir { dir, rm } => cli::models_dir(&mut store, dir.as_deref(), rm),
        Cmd::Note { model, text, rm } => cli::note(&data, &mut store, &model, text.as_deref(), rm),
        Cmd::Fav { task: Some(task), rename: Some(new), .. } => cli::rename(&mut store, &task, &new),
        Cmd::Fav { task, model, tier, via, rm, about, .. } => {
            let (task, tier) = (task.as_deref(), tier.as_deref());
            // As `list --via` takes a harness: in any case.
            let via = via.map(|h| h.to_lowercase());
            let model = model.as_deref().map(|m| (m, via.as_deref()));
            cli::fav(&data, &mut store, task, tier, model, rm, about.as_deref())
        }
        Cmd::Recommend { json } => cli::recommend(&data, &store, json),
    }
}
