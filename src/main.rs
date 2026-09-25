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
    /// List models (only ones you have access to, unless --all)
    #[command(alias = "ls")]
    List {
        /// Rank by task fit
        #[arg(short, long, value_parser = tasks())]
        task: Option<String>,
        /// Include models you have no access to
        #[arg(short, long)]
        all: bool,
        /// Favorites only
        #[arg(short, long)]
        favorites: bool,
        /// Only these developers, e.g. --dev anthropic --dev openai (the Dev dropdown, `d`, in the TUI)
        #[arg(long)]
        dev: Vec<String>,
        /// Only models you have through these harnesses (opencode, claude, codex, gemini) or env; repeatable (the Via dropdown, `d`, in the TUI)
        #[arg(long)]
        via: Vec<String>,
        /// Max blended price ($/1M tokens, 3:1 input:output)
        #[arg(long)]
        max_price: Option<f64>,
        /// Best model per price level for --task: cheapest first, each row costing more and scoring higher (`p` in the TUI)
        #[arg(long, requires = "task")]
        frontier: bool,
        /// Max rows, 0 = no limit
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Top models for a task
    Recommend {
        #[arg(value_parser = tasks())]
        task: String,
        /// Include models you have no access to
        #[arg(short, long)]
        all: bool,
        /// Only models you have through these harnesses (opencode, claude, codex, gemini) or env; repeatable (the Via dropdown, `d`, in the TUI)
        #[arg(long)]
        via: Vec<String>,
        /// Max blended price ($/1M tokens, 3:1 input:output)
        #[arg(long)]
        max_price: Option<f64>,
        /// Max rows, 0 = no limit
        #[arg(short = 'n', long, default_value_t = 5)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
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
    /// Open the model's web page
    Open { model: String },
    /// Add or remove a favorite
    Fav {
        #[arg(value_parser = ["add", "rm"])]
        action: String,
        model: String,
    },
    /// Show a model's note, or set it (empty text deletes)
    Note { model: String, text: Option<String> },
    /// What each task measures and when to pick a model high on it (`t` in the TUI)
    Tasks,
    /// Re-download data
    Refresh,
}

fn main() {
    let args = Args::parse();
    let result = match args.cmd {
        None => tui::run(args.refresh).map_err(Exit::from),
        Some(cmd) => run(cmd, args.refresh),
    };
    if let Err(e) = result {
        eprintln!("modelcmp: {}", e.msg);
        std::process::exit(e.code);
    }
}

fn run(cmd: Cmd, force: bool) -> Result<(), Exit> {
    if let Cmd::Tasks = cmd {
        cli::tasks();
        return Ok(());
    }
    if let Cmd::Refresh = cmd {
        let data = data::refresh()?;
        let with_bench = data.models.iter().filter(|m| m.eci.is_some()).count();
        println!("{} models ({} with benchmarks)", data.models.len(), with_bench);
        return Ok(());
    }
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
        Cmd::List { task: t, all, favorites, dev, via, max_price, frontier, limit, json } => cli::list(
            &data,
            &store,
            &cli::ListOpts { task: task(t), all, favorites, dev, via, limit, max_price, frontier, json },
        ),
        Cmd::Recommend { task: t, all, via, max_price, limit, json } => cli::list(
            &data,
            &store,
            &cli::ListOpts {
                task: task(Some(t)),
                all,
                favorites: false,
                dev: vec![],
                via,
                limit,
                max_price,
                frontier: false,
                json,
            },
        ),
        Cmd::Show { model, json } => cli::show(&data, &store, &model, json),
        Cmd::Compare { models, json } => cli::compare(&data, &store, &models, json),
        Cmd::Open { model } => cli::open(&data, &model),
        Cmd::Fav { action, model } => cli::fav(&data, &mut store, action == "add", &model),
        Cmd::Note { model, text } => cli::note(&data, &mut store, &model, text.as_deref()),
        Cmd::Tasks | Cmd::Refresh => unreachable!(),
    }
}
