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

fn tasks() -> PossibleValuesParser {
    PossibleValuesParser::new(fit::task_names())
}

/// Pick the right LLM: prices (models.dev) + benchmarks (Epoch AI), filtered to the
/// models you can already use: the ones your harnesses list (opencode models; claude,
/// codex and gemini give their own provider's), plus providers you have API keys for.
/// The VIA column says which. Run without a command for the interactive TUI.
#[derive(Parser)]
#[command(version, after_help = "Data: models.dev (prices), Epoch AI (benchmarks, CC-BY). Cached for 24h.")]
struct Args {
    /// Re-download data now instead of using the cache
    #[arg(long, global = true)]
    refresh: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List models you have access to (all with --all): rank, bound and sort them
    #[command(alias = "ls")]
    List {
        /// Best model per price level for a task: cheapest first, each row costing more and scoring higher; excluded models are left out (`t` in the TUI)
        #[arg(short, long, value_parser = tasks(), conflicts_with = "sort")]
        task: Option<String>,
        /// One model from the task's list: low = cheapest scoring 50+, mid = cheapest 75+, high = the best; the best when none reaches the floor
        #[arg(long, requires = "task", value_parser = PossibleValuesParser::new(view::TIERS.map(|t| t.0)))]
        tier: Option<String>,
        /// Sort by a column, best first: cheapest, or highest score (`s` in the TUI)
        #[arg(short, long, value_parser = PossibleValuesParser::new(app::COLS.map(|c| c.id)))]
        sort: Option<String>,
        /// Keep models at or above a value, e.g. --min coding=70; columns as in --sort, ctx in thousands of tokens; repeatable (`>` in the TUI)
        #[arg(long, value_parser = bound)]
        min: Vec<(usize, f64)>,
        /// Keep models at or below a value, e.g. --max price=2; repeatable (`<` in the TUI)
        #[arg(long, value_parser = bound)]
        max: Vec<(usize, f64)>,
        /// Include models you have no access to
        #[arg(short, long)]
        all: bool,
        /// Marked only: your shortlist (`M` in the TUI)
        #[arg(short, long)]
        marked: bool,
        /// Only these developers, e.g. --dev anthropic --dev openai (the Dev dropdown, `d`, in the TUI)
        #[arg(long)]
        dev: Vec<String>,
        /// Only models you have through these harnesses (opencode, claude, codex, gemini) or env; repeatable (the Via dropdown, `d`, in the TUI)
        #[arg(long)]
        via: Vec<String>,
        /// Max rows, 0 = no limit
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Only the model ids a harness takes (provider/model), one per line: `codex -m $(modelcmp list --task coding --tier mid --id)`
        #[arg(long, conflicts_with = "json")]
        id: bool,
    },
    /// Everything about one model
    Show {
        model: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Side-by-side comparison, opening with a verdict: cheapest, best at coding, most coding per $
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
        /// The site: models.dev, epoch.ai or openrouter.ai; a prefix will do
        #[arg(long, default_value = "openrouter")]
        on: String,
    },
    /// Mark a model, to shortlist it: `list --marked` shows them (`m` in the TUI)
    Mark {
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
    /// Show a model's note, or set it (`n` in the TUI)
    Note {
        model: String,
        #[arg(conflicts_with = "rm")]
        text: Option<String>,
        /// Delete the note
        #[arg(long)]
        rm: bool,
    },
    /// Your favorite model for a task: --tier picks it and recommend marks it ★; alone, shows them (`f` in the TUI)
    Fav {
        #[arg(value_parser = tasks())]
        task: Option<String>,
        #[arg(requires = "task", conflicts_with = "rm")]
        model: Option<String>,
        /// Clear the task's favorite
        #[arg(long, requires = "task")]
        rm: bool,
    },
    /// The best model per price for each task, what the task measures and when to use it (`R` in the TUI)
    Recommend {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `coding=70` for --min and --max: the column's index in `app::COLS` and the value.
fn bound(s: &str) -> Result<(usize, f64), String> {
    let ids = || app::COLS.map(|c| c.id).join(", ");
    let (id, v) =
        s.split_once('=').ok_or_else(|| format!("expected column=value, e.g. coding=70; columns: {}", ids()))?;
    let col =
        app::COLS.iter().position(|c| c.id == id).ok_or_else(|| format!("no column '{id}'; columns: {}", ids()))?;
    Ok((col, v.parse().map_err(|_| format!("'{v}' is not a number"))?))
}

fn main() {
    let args = Args::parse();
    let result = match args.cmd {
        None => tui::run(args.refresh).map_err(Exit::from),
        Some(cmd) => {
            // Die quietly when a pipe closes early, as in `modelcmp list | head`, instead of
            // panicking in println. Not in the TUI: it writes to clipboard tools that may exit.
            #[cfg(unix)]
            // SAFETY: nothing else is running yet to observe the signal disposition change.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            }
            run(cmd, args.refresh)
        }
    };
    if let Err(e) = result {
        eprintln!("modelcmp: {}", e.msg);
        std::process::exit(e.code);
    }
}

fn run(cmd: Cmd, force: bool) -> Result<(), Exit> {
    let (data, warn) = data::load(force)?;
    if let Some(w) = warn {
        eprintln!("warning: {w}");
    }
    if !data.models.iter().any(|m| m.available) {
        eprintln!("note: no harness models or provider API keys found, showing all models");
    }
    let mut store = Store::load();
    // clap has already validated task names against fit::TASKS.
    let task = |t: Option<String>| t.and_then(|t| fit::task(&t));
    match cmd {
        Cmd::List { task: t, tier, sort, min, max, all, marked, dev, via, limit, json, id } => {
            let bounds = min
                .into_iter()
                .map(|(c, v)| (c, v, f64::INFINITY))
                .chain(max.into_iter().map(|(c, v)| (c, f64::NEG_INFINITY, v)))
                .collect();
            let sort = sort.and_then(|s| app::COLS.iter().position(|c| c.id == s));
            let opts = cli::ListOpts { task: task(t), tier, sort, bounds, all, marked, dev, via, limit, json, id };
            cli::list(&data, &store, &opts)
        }
        Cmd::Show { model, json } => cli::show(&data, &store, &model, json),
        Cmd::Compare { models, json } => cli::compare(&data, &store, &models, json),
        Cmd::Open { model, on } => cli::open(&data, &model, &on),
        Cmd::Mark { model, rm } => cli::mark(&data, &mut store, &model, rm),
        Cmd::Exclude { model, rm } => cli::exclude(&data, &mut store, &model, rm),
        Cmd::Note { model, text, rm } => cli::note(&data, &mut store, &model, text.as_deref(), rm),
        Cmd::Fav { task, model, rm } => cli::fav(&data, &mut store, task.as_deref(), model.as_deref(), rm),
        Cmd::Recommend { json } => cli::recommend(&data, &store, json),
    }
}
