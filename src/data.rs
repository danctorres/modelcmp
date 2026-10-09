//! Fetching, caching and merging models.dev (prices) with Epoch AI (benchmarks).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MODELS_URL: &str = "https://models.dev/api.json";
/// Epoch's scores; with Artificial Analysis's, only its names, to link a model there.
const EPOCH_URL: &str = "https://epoch.ai/data/benchmark_data.zip";
/// Epoch's model pages, as it has none for some models it scored and some for models it did not.
/// The index of Epoch's sitemaps, which names those of its model pages (`epoch_pages`).
const EPOCH_PAGES_URL: &str = "https://epoch.ai/sitemap-index.xml";
/// Artificial Analysis's page names, to link a model there that its API did not name one for.
const AA_URL: &str = "https://artificialanalysis.ai/sitemap.xml";
/// OpenRouter's models, for the Hugging Face repo it names for each (`hf_listed`).
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
/// Artificial Analysis's scores, with an API key (`aa_key`).
const AA_API_URL: &str = "https://artificialanalysis.ai/api/v2/data/llms/models";
/// Arena's Agent leaderboard, the newest one, from the dataset it publishes on Hugging Face
/// (CC-BY): no key needed (`Arena`).
// ponytail: one page of 100 rows, twice what the leaderboard has, and a warning past that
// (`Arena::cut`). Page with `offset` then.
macro_rules! arena_url {
    ($config:literal) => {
        concat!(
            "https://datasets-server.huggingface.co/rows?dataset=lmarena-ai/leaderboard-dataset&config=",
            $config,
            "&split=latest&offset=0&length=100"
        )
    };
}
const ARENA_URL: &str = arena_url!("agent");
/// The signals Arena's agent score is made of, each a leaderboard of its own with the same rows:
/// its name in the Arena column's dropdown, and where it is (`Model::arena_more`).
pub const ARENA_MORE: [(&str, &str); 5] = [
    ("Task outcome", arena_url!("agent_task_outcome_explicit")),
    ("Tool hallucination", arena_url!("agent_tool_hallucination")),
    ("Steerability", arena_url!("agent_steerability")),
    ("Bash recovery", arena_url!("agent_bash_recovery_steps")),
    ("Praise", arena_url!("agent_praise_complaint")),
];
/// The leaderboard itself, which `o` and a click on a model's Arena score open (`arena_page`).
const ARENA_PAGE: &str = "https://arena.ai/leaderboard/agent";
/// modelcmp's newest release, fetched with the data so a new version is told once a day at most.
const RELEASE_URL: &str = "https://api.github.com/repos/danctorres/modelcmp/releases/latest";
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// How old a cache without Arena's scores may be when their download did not come
/// (`Data::arena_missed`): tried again long before `MAX_AGE`.
const ARENA_RETRY: Duration = Duration::from_secs(3600);
/// Bumped when the cached fields change meaning, so an older cache refreshes.
/// 2: `Offer::unpriced`, where a missing price used to read as free. 3: `Model::aa`.
/// 4: task fit from Epoch's per-benchmark fit instead of mean percentiles. 5: `Model::epoch`.
/// 6: `Model::shown`. 7: no deprecated offers, no fine-tunes folded into their base.
/// 8: with Artificial Analysis, a row naming a reasoning setting has that setting's scores.
/// 9: `Model::md` and `Model::openrouter` for `Model::url`, and `Model::epoch` a page name.
/// 10: Artificial Analysis's agentic score is Terminal-Bench 4.0, where it was Hard.
/// 11: prices at the tier an agent's session reaches. 12: a row's scores are of the release
/// its offers are. 13: Artificial Analysis's coding score is `fit::AA_CODING`, where it was its
/// Coding Index, and `Model::ttft` is to the first answer token. 14: its scores are all of one
/// reasoning setting. 15: Epoch's tasks without the benchmarks it no longer runs, and no task
/// score for a model without an ECI scored on few benchmarks. 16: `Model::hf`.
/// 18: `Model::task_cost`. 19: `Model::task_tokens`. 20: context and max output are the
/// ones most offers give, where they were the most any gave. 21: `Model::arena`.
/// 22: Epoch's task scores are one benchmark's, in percent, where they were fitted in ECI points.
/// 23: `Model::arena_more`.
const FORMAT: u32 = 23;
/// Share of input tokens read from the prompt cache by default: an agent resends the whole
/// conversation every turn, so most of what it sends was sent before. A one-off prompt caches
/// nothing: `--cache 0`, or `%` in the TUI.
pub const AGENT_CACHED: f64 = 0.9;
/// The share in use, as f64 bits: one process-wide setting that every price reads.
#[cfg(not(test))]
static CACHED: AtomicU64 = AtomicU64::new(AGENT_CACHED.to_bits());
// Tests run in parallel threads: each gets its own share, so one that sets it cannot race the rest.
#[cfg(test)]
thread_local!(static CACHED: AtomicU64 = const { AtomicU64::new(AGENT_CACHED.to_bits()) });

/// Share of input tokens read from the prompt cache, 0..1.
pub fn cached() -> f64 {
    #[cfg(test)]
    return CACHED.with(|c| f64::from_bits(c.load(Relaxed)));
    #[cfg(not(test))]
    f64::from_bits(CACHED.load(Relaxed))
}

/// Set the cache share, clamped to 0..1. Prices derived from it (`fit::add_value`,
/// `App::vals`) must be recomputed by the caller.
pub fn set_cached(share: f64) {
    let bits = share.clamp(0.0, 1.0).to_bits();
    #[cfg(test)]
    CACHED.with(|c| c.store(bits, Relaxed));
    #[cfg(not(test))]
    CACHED.store(bits, Relaxed);
}

/// Where the benchmark scores come from: Epoch AI needs nothing, Artificial Analysis an API key.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    #[default]
    Epoch,
    Aa,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Epoch, Source::Aa];

    /// Its name in `--source`, the store and the cache: "epoch", "aa".
    pub fn id(self) -> &'static str {
        match self {
            Source::Epoch => "epoch",
            Source::Aa => "aa",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::Epoch => "Epoch AI",
            Source::Aa => "Artificial Analysis",
        }
    }

    /// The overall index's column name and what it is.
    pub fn index(self) -> (&'static str, &'static str) {
        match self {
            Source::Epoch => ("ECI", "Epoch AI's overall capability index"),
            Source::Aa => ("AAII", "Artificial Analysis Intelligence Index"),
        }
    }

    /// Its website, as `Model::links` names it.
    pub fn site(self) -> &'static str {
        match self {
            Source::Epoch => "epoch.ai",
            Source::Aa => "artificialanalysis.ai",
        }
    }

    /// The one to use without `--source`: Artificial Analysis once its key is there.
    pub fn preferred() -> Source {
        if aa_key().is_some() { Source::Aa } else { Source::Epoch }
    }

    /// By `id`.
    pub fn parse(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|x| x.id() == s)
    }

    /// The one this is not, whose columns show beside its own (`Data::borrow`).
    pub fn other(self) -> Source {
        match self {
            Source::Epoch => Source::Aa,
            Source::Aa => Source::Epoch,
        }
    }

    /// The benchmarks its tasks use; a cache made with others is stale.
    fn benches(self) -> Vec<&'static str> {
        match self {
            Source::Epoch => crate::fit::task_benches(),
            Source::Aa => crate::fit::aa_fields(),
        }
    }
}

/// The source in use: one process-wide setting, like `CACHED`.
#[cfg(not(test))]
static SOURCE: AtomicU8 = AtomicU8::new(0);
#[cfg(test)]
thread_local!(static SOURCE: AtomicU8 = const { AtomicU8::new(0) });

pub fn source() -> Source {
    #[cfg(test)]
    let i = SOURCE.with(|s| s.load(Relaxed));
    #[cfg(not(test))]
    let i = SOURCE.load(Relaxed);
    Source::ALL[i as usize]
}

pub fn set_source(s: Source) {
    let i = Source::ALL.iter().position(|x| *x == s).unwrap_or(0) as u8;
    #[cfg(test)]
    SOURCE.with(|x| x.store(i, Relaxed));
    #[cfg(not(test))]
    SOURCE.store(i, Relaxed);
}

/// Whether the data shown has the other source's columns too (`Data::lent`): one process-wide
/// setting, like `SOURCE`.
#[cfg(not(test))]
static LENT: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
thread_local!(static LENT: AtomicBool = const { AtomicBool::new(false) });

pub fn lent() -> bool {
    #[cfg(test)]
    return LENT.with(|l| l.load(Relaxed));
    #[cfg(not(test))]
    LENT.load(Relaxed)
}

pub fn set_lent(on: bool) {
    #[cfg(test)]
    LENT.with(|l| l.store(on, Relaxed));
    #[cfg(not(test))]
    LENT.store(on, Relaxed);
}

/// Artificial Analysis's API key: `AA_KEY_ENV`, else the one saved with `save_aa_key`.
pub const AA_KEY_ENV: &str = "ARTIFICIAL_ANALYSIS_API_KEY";
/// Where its key is made, free with an account: what a click on the key prompt's link opens.
pub const AA_KEY_URL: &str = "https://artificialanalysis.ai/login";

/// Why a refresh failed: the two the TUI answers by asking for a key, and the rest.
#[derive(Clone, PartialEq, Debug)]
pub enum Failure {
    /// Artificial Analysis is the source and there is no API key.
    NoKey,
    /// Artificial Analysis turned the key down.
    BadKey,
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Failure::NoKey => {
                write!(
                    f,
                    "Artificial Analysis needs an API key: set {AA_KEY_ENV}, or press K in the TUI, which asks for it"
                )
            }
            Failure::BadKey => f.write_str("Artificial Analysis rejected the API key"),
            Failure::Other(e) => f.write_str(e),
        }
    }
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        Failure::Other(e)
    }
}

impl From<&str> for Failure {
    fn from(e: &str) -> Self {
        Failure::Other(e.into())
    }
}

#[cfg(not(test))]
fn aa_key_path() -> PathBuf {
    crate::store::path().with_file_name("aa_key")
}

/// Blank counts as unset, in the environment as in the file.
fn nonblank(k: &str) -> Option<String> {
    Some(k.trim().to_string()).filter(|k| !k.is_empty())
}

// Tests set the environment's key and the saved one here, never reading or writing yours.
#[cfg(test)]
thread_local!(pub static TEST_KEYS: std::cell::RefCell<[Option<String>; 2]> = const { std::cell::RefCell::new([None, None]) });

pub fn aa_key() -> Option<String> {
    #[cfg(test)]
    let saved = TEST_KEYS.with(|k| k.borrow()[1].clone());
    #[cfg(not(test))]
    let saved = std::fs::read_to_string(aa_key_path()).ok();
    aa_key_env().or_else(|| saved.as_deref().and_then(nonblank))
}

/// The key set in `AA_KEY_ENV`, which a saved one would not replace.
pub fn aa_key_env() -> Option<String> {
    #[cfg(test)]
    let env = TEST_KEYS.with(|k| k.borrow()[0].clone());
    #[cfg(not(test))]
    let env = std::env::var(AA_KEY_ENV).ok();
    env.as_deref().and_then(nonblank)
}

/// Saved apart from user.json, readable by you alone, so the key is never shared with your settings.
pub fn save_aa_key(key: &str) -> std::io::Result<()> {
    #[cfg(test)]
    {
        TEST_KEYS.with(|k| k.borrow_mut()[1] = Some(key.trim().to_string()));
        Ok(())
    }
    #[cfg(not(test))]
    crate::store::write_private(&aa_key_path(), key.trim().as_bytes())
}
/// How a harness tells which models it can use.
enum Probe {
    /// A command that prints the `provider/model` ids it has access to, one per line.
    List(&'static [&'static str]),
    /// A command that prints a table with a header row, then provider and model as the first two columns.
    Table(&'static [&'static str]),
    /// A command that prints JSON: `models`, each with its `provider/model` id as `selector`.
    Json(&'static [&'static str]),
    /// A command that prints a table of the models it runs on this machine under a `NAME ...`
    /// header, the name first (`name_ids`).
    Names(&'static [&'static str]),
    /// A command that prints a numbered list of the models it has downloaded to this machine
    /// (`cache_ids`), with the files downloaded by hand to its folder (`gguf_ids`).
    Cache(&'static [&'static str]),
    /// No such command: being installed means access to every model of this provider.
    Provider(&'static str),
    /// No such command either, but its CLI lists what the account may use to an Agent Client
    /// Protocol client (`copilot_ids`).
    Copilot,
}

/// models.dev's provider for the models GitHub Copilot serves.
const COPILOT: &str = "github-copilot";

const HARNESSES: &[(&str, Probe)] = &[
    ("opencode", Probe::List(&["models"])),
    ("pi", Probe::Table(&["--list-models"])),
    ("omp", Probe::Json(&["models", "--json"])),
    ("claude", Probe::Provider("anthropic")),
    ("codex", Probe::Provider("openai")),
    ("gemini", Probe::Provider("google")),
    ("copilot", Probe::Copilot),
    (OLLAMA, Probe::Names(&["list"])),
    (LLAMA, Probe::Cache(&["--cache-list"])),
];

/// Ollama, and the provider of the offer made for a model it runs here (`Offer::local`).
pub const OLLAMA: &str = "ollama";
/// llama.cpp's CLI, and the provider of the offer made for a model it runs here.
pub const LLAMA: &str = "llama-cli";
/// models.dev's provider for the models Hugging Face serves, by their repo there.
const HF: &str = "huggingface";
/// The harnesses that run models on this machine, and what each is called.
const LOCAL: &[(&str, &str)] = &[(OLLAMA, "Ollama"), (LLAMA, "llama.cpp")];

/// llama.cpp's CLI as this machine has it: `llama-cli`, else the one binary of newer builds, else
/// its server, which lists and loads the same models.
pub fn llama_cmd() -> Option<&'static [&'static str]> {
    if cfg!(test) {
        return Some(&[LLAMA]);
    }
    [&[LLAMA][..], &["llama", "cli"], &["llama-server"]].into_iter().find(|c| installed(c[0]))
}

/// Whether `harness` is installed here.
pub fn has(harness: &str) -> bool {
    if harness == LLAMA { llama_cmd().is_some() } else { installed(harness) }
}

/// The command that installs `runner`, ollama or llama.cpp, where one is known to: Homebrew's
/// formula, or on Linux ollama's own script. None where it is yours to install.
pub fn install_cmd(runner: &str) -> Option<&'static [&'static str]> {
    let brew = installed("brew");
    match runner {
        LLAMA if brew => Some(&["brew", "install", "llama.cpp"]),
        OLLAMA if cfg!(target_os = "linux") => Some(&["sh", "-c", "curl -fsSL https://ollama.com/install.sh | sh"]),
        OLLAMA if brew => Some(&["brew", "install", "ollama"]),
        _ => None,
    }
}

/// The command by which `runner` downloads the GGUF `repo` from Hugging Face and runs it.
pub fn get_cmd(runner: &str, repo: &str) -> Vec<String> {
    if runner == LLAMA {
        // Not installed yet, it is `llama-cli` that the install brings.
        let bin = llama_cmd().unwrap_or(&[LLAMA]);
        return [bin, &["-hf", repo]].concat().into_iter().map(String::from).collect();
    }
    vec![OLLAMA.into(), "run".into(), format!("hf.co/{repo}")]
}

/// The GGUF repo to download for the model `key`, whose weights are in the Hugging Face repo
/// `base`: `base` when it is one, else the most downloaded one quantized from it whose name is
/// the model's (`local_tags`), else is `base`'s (`gguf_named`). None when Hugging Face has no
/// such copy, an error when it did not answer.
pub fn gguf_repo(base: &str, key: &str) -> Result<Option<String>, String> {
    if base.to_ascii_lowercase().ends_with("-gguf") {
        return Ok(Some(base.to_string()));
    }
    let url = format!(
        "https://huggingface.co/api/models?filter=gguf&filter=base_model:quantized:{base}&sort=downloads&direction=-1&limit=20"
    );
    let list = fetch_within(&url, None, HF_WAIT).map_err(|e| e.to_string())?;
    Ok(gguf_named(&list, base, key))
}

/// The repo to download out of Hugging Face's `[{"id": "owner/name"}]`: the first whose name is
/// the model `key`'s, else the first named as the repo `base` it is quantized from, as
/// `bartowski/Meta-Llama-3.1-8B-Instruct-GGUF` for the model `llama318b`. Its name is then
/// `base`'s and no more, but for what of `base`'s owner some put before it
/// (`deepseek-ai_DeepSeek-R1-0528-GGUF`): a name with more to it (`-abliterated`) is another
/// model's, as a fine-tune's copy is listed among them at times.
fn gguf_named(list: &[u8], base: &str, key: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Repo {
        id: String,
    }
    let keys = HashSet::from([key]);
    let named = |id: &&String| local_tags(&[format!("{LLAMA}/{id}")], &keys, &HashMap::new()).contains_key(key);
    let base = base.to_ascii_lowercase();
    let (owner, base) = base.split_once('/').unwrap_or(("", &base));
    let of_base = |id: &&String| {
        let id = id.to_ascii_lowercase();
        let name = id.rsplit('/').next().unwrap_or(&id).trim_end_matches("-gguf");
        name.strip_suffix(base).is_some_and(|before| owner.starts_with(before.trim_end_matches(['-', '_', '.'])))
    };
    let ids: Vec<String> = serde_json::from_slice::<Vec<Repo>>(list).ok()?.into_iter().map(|r| r.id).collect();
    ids.iter().find(named).or_else(|| ids.iter().find(of_base)).cloned()
}

/// A file of modelcmp's cache folder.
fn cache_file(name: &str) -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("modelcmp").join(name)
}

/// A map kept in modelcmp's cache folder from one run to the next, empty when there is none.
fn kept<T: serde::de::DeserializeOwned>(name: &str) -> HashMap<String, T> {
    std::fs::read(cache_file(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Keeps `map` for `kept`. Not being able to is no error: what it holds is asked for again.
fn keep<T: Serialize>(name: &str, map: &HashMap<String, T>) {
    if let Ok(json) = serde_json::to_vec(map) {
        let _ = crate::store::write_atomic(&cache_file(name), &json);
    }
}

/// Model key -> the GGUF repo `modelcmp get` downloaded it from, in lower case: a copy of that
/// repo on this machine is that model, whatever the repo is called (`local_tags`). As
/// `repos.json` had them when the data was last loaded or refreshed (`load_repos`).
static REPOS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Reads `REPOS` anew: `get` in another terminal may have downloaded one since.
fn load_repos() {
    *REPOS.lock().unwrap_or_else(PoisonError::into_inner) = Some(kept("repos.json"));
}

/// The model `key` is downloaded from `repo`: kept, so that the copy is that model here.
pub fn keep_repo(key: &str, repo: &str) {
    // Held from the read to the write, so two downloads started at once both stay.
    let _held = crate::store::lock(&cache_file("repos.json"));
    let mut all: HashMap<String, String> = kept("repos.json");
    all.insert(key.into(), repo.to_ascii_lowercase());
    keep("repos.json", &all);
}

/// The Hugging Face repo a tag of ollama's or llama.cpp's was pulled from, in lower case:
/// `unsloth/qwen3.5-4b-gguf` of `hf.co/unsloth/Qwen3.5-4B-GGUF:Q4_K_M`.
fn tag_repo(tag: &str) -> String {
    let tag = tag.strip_prefix("hf.co/").or_else(|| tag.strip_prefix("huggingface.co/")).unwrap_or(tag);
    tag.split_once(':').map_or(tag, |(repo, _)| repo).to_ascii_lowercase()
}

/// How long Hugging Face has to say which repo to download, and its size: it answers in a
/// fraction of a second, and `x` and `get` wait on it.
const HF_WAIT: Duration = Duration::from_secs(10);

/// The bytes the GGUF `repo` takes to download, as Hugging Face lists its files: none when it
/// lists no `.gguf` one, so there is nothing to download.
pub fn gguf_size(repo: &str) -> Result<Option<u64>, String> {
    let url = format!("https://huggingface.co/api/models/{repo}/tree/main?recursive=true");
    let tree = fetch_within(&url, None, HF_WAIT).map_err(|e| e.to_string())?;
    Ok(gguf_bytes(&tree))
}

/// The size of the files ollama and llama.cpp take from a repo named with no quantization, out
/// of Hugging Face's `[{"path": "x-Q4_K_M.gguf", "size": 1}]`: the first Q4_K_M one, with every
/// part of it when it is split (`x-Q4_K_M-00001-of-00002.gguf`), else the first `.gguf`.
// ponytail: the vision projector (`mmproj`) llama.cpp fetches with it is not counted, and which
// file is first is a guess at theirs: ask the runner's own resolver if the size has to be exact.
fn gguf_bytes(tree: &[u8]) -> Option<u64> {
    #[derive(Deserialize)]
    struct File {
        path: String,
        #[serde(default)]
        size: u64,
    }
    let files: Vec<File> = serde_json::from_slice(tree).ok()?;
    let lower = |f: &File| f.path.to_ascii_lowercase();
    let weights = || files.iter().filter(|f| lower(f).ends_with(".gguf") && !lower(f).contains("mmproj"));
    // What a split file's parts share: its path up to `-00001-of-00002.gguf`.
    let whole = |f: &File| {
        let p = lower(f);
        let mut ends = p.trim_end_matches(".gguf").rsplitn(4, '-');
        let num = |s: Option<&str>| s.is_some_and(|s| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit()));
        let split = num(ends.next()) && ends.next() == Some("of") && num(ends.next());
        ends.next().filter(|_| split).map(str::to_string)
    };
    let first = weights().find(|f| lower(f).contains("q4_k_m")).or_else(|| weights().next())?;
    Some(match whole(first) {
        Some(stem) => weights().filter(|f| whole(f).as_ref() == Some(&stem)).map(|f| f.size).sum(),
        None => first.size,
    })
}

/// A download's size as said to you: `4.7 GB`, or `640 MB` under one.
pub fn gb(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1e9 { format!("{:.0} MB", b / 1e6) } else { format!("{:.1} GB", b / 1e9) }
}

/// The size in bytes of each model's download, by key, as Hugging Face said it in a run
/// before, so it is asked once for a model.
// ponytail: kept for good, so a repo uploaded again at another size reads stale: date the
// entries and ask again past some age if that ever shows.
pub fn load_sizes() -> HashMap<String, u64> {
    kept("sizes.json")
}

/// Keeps `sizes` for the next run.
pub fn save_sizes(sizes: &HashMap<String, u64>) {
    keep("sizes.json", sizes);
}

/// How long Hugging Face's "no GGUF copy" of a model is gone by before it is asked again, in
/// seconds: one may be uploaded since.
const GONE_FOR: u64 = 7 * 24 * 3600;

/// When Hugging Face said it had no GGUF copy of each model, in seconds, by key: the answers
/// of a run before that are no older than `GONE_FOR`, so those models are not asked for again.
pub fn load_gone() -> HashMap<String, u64> {
    let mut gone: HashMap<String, u64> = kept("gone.json");
    gone.retain(|_, at| now().saturating_sub(*at) < GONE_FOR);
    gone
}

/// Hugging Face said just now that it has no GGUF copy of the model `key`: kept with `gone`.
pub fn save_gone(gone: &mut HashMap<String, u64>, key: &str) {
    gone.insert(key.into(), now());
    keep("gone.json", gone);
}

/// Forgets them all: a refresh you ask for has Hugging Face asked again.
pub fn clear_gone(gone: &mut HashMap<String, u64>) {
    gone.clear();
    keep("gone.json", gone);
}

/// Whether `harness` runs its models on this machine.
pub fn runs_here(harness: &str) -> bool {
    LOCAL.iter().any(|l| l.0 == harness)
}

/// The key of the model an ollama tag names, `qwen3.5:4b` or `llama3.2:1b-instruct-Q4_K_M-128k`:
/// without its quantization or a context size after it. A tag that says no size
/// (`llama3.2:latest`) names none.
// ponytail: by the tag alone; read the size from `ollama show` if `:latest` tags should match.
fn local_key(tag: &str) -> String {
    let digit = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());
    // A Hugging Face repo says the size in its name, so ollama's `:latest` for it is no part of it.
    let hub = tag.starts_with("hf.co/") || tag.starts_with("huggingface.co/");
    let extra = |p: &str| {
        p == "qat"
            || (hub && p == "latest")
            || ["q", "fp", "bf", "f"].iter().any(|q| p.strip_prefix(q).is_some_and(digit))
            || (digit(p) && p.ends_with('k'))
    };
    // Whoever it was pulled from, `hf.co/bartowski/Qwen3.5-4B-GGUF:Q4_K_M` or `someone/qwen3.5:4b`.
    let tag = tag.rsplit('/').next().unwrap_or(tag).to_ascii_lowercase();
    let (name, variant) = match tag.strip_suffix(".gguf") {
        // A file downloaded by hand, `gemma-3-4b-it-Q4_K_M.gguf`: its name, then its quantization.
        Some(file) => file.rsplit_once(['-', '.']).unwrap_or((file, "")),
        None => tag.split_once(':').unwrap_or((&tag, "")),
    };
    let name = name.trim_end_matches("-gguf");
    norm(&format!("{name}{}", variant.split('-').filter(|p| !extra(p)).collect::<String>()))
}

/// Model key -> the tag that runs it, of the `ollama/tag` or `llama-cli/repo:quant` ids `listed`:
/// the shortest tag naming a model, so the one ollama pulls by default, whatever was pulled last.
/// One naming no model in `keys` names its instruct one, which is what ollama's plain tags hold:
/// a tag of ollama's own library names that one first, even with the base one in `keys`.
/// One that says `it` names the instruction-tuned model, never the base one beside it: under
/// that name, else as the instruct one, else as the only one there is. A tag pulled from a repo
/// of `repos`, model key -> repo, names that model too, whatever the repo is called.
fn local_tags<'a>(
    listed: &'a [String],
    keys: &HashSet<&str>,
    repos: &HashMap<String, String>,
) -> HashMap<String, &'a str> {
    let mut tags: Vec<(&str, &str)> = listed.iter().filter_map(|i| i.split_once('/')).collect();
    tags.sort_by_key(|(_, t)| (t.len(), *t));
    let mut by_key = HashMap::new();
    for (harness, tag) in tags {
        let key = local_key(tag);
        // A tag of ollama's own library, `llama3.2:1b`: the instruct model, not the base one beside it.
        let plain = harness == OLLAMA && !tag.contains('/');
        let chat = plain.then(|| [format!("{key}instruct"), format!("{key}it")]);
        let tuned = key.strip_suffix("it").map(|k| [format!("{k}instruct"), k.to_string()]);
        let named = chat.into_iter().flatten().chain([key.clone()]).chain(tuned.into_iter().flatten());
        let key = named.into_iter().find(|k| keys.contains(k.as_str())).unwrap_or_else(|| format!("{key}instruct"));
        by_key.entry(key).or_insert(tag);
        let from = tag_repo(tag);
        for (key, _) in repos.iter().filter(|(k, repo)| **repo == from && keys.contains(k.as_str())) {
            by_key.entry(key.clone()).or_insert(tag);
        }
    }
    by_key
}

/// pi's names for providers models.dev names otherwise, paired by the model ids they share.
/// omp, a fork of pi, names them the same.
const PI_PROVIDERS: &[(&str, &str)] = &[
    ("azure-openai-responses", "azure"),
    ("fireworks", "fireworks-ai"),
    ("kimi-coding", "kimi-code-plan-global"),
    ("openai-codex", "openai"),
    ("qwen-token-plan", "alibaba-token-plan"),
    ("qwen-token-plan-cn", "alibaba-token-plan-cn"),
    ("qwen-token-plan-individual", "alibaba-token-plan"),
    ("together", "togetherai"),
    ("vercel-ai-gateway", "vercel"),
    ("zai-coding-cn", "zhipuai-coding-plan"),
];

/// A `provider/model` id `harness` listed, with the provider as models.dev names it.
pub fn canonical(harness: &str, id: &str) -> String {
    match id.split_once('/') {
        Some((p, rest)) if matches!(harness, "pi" | "omp") => {
            format!("{}/{rest}", PI_PROVIDERS.iter().find(|a| a.0 == p).map_or(p, |a| a.1))
        }
        _ => id.to_string(),
    }
}

/// Every name the Via column can show: the harnesses.
pub fn vias() -> impl Iterator<Item = &'static str> {
    HARNESSES.iter().map(|h| h.0)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Offer {
    pub provider: String,
    pub provider_name: String,
    pub id: String,
    pub input: f64,
    pub output: f64,
    /// $ per 1M cached input tokens, when the provider lists a discount.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// models.dev lists no price for it: `input` and `output` are 0 but mean unknown, not free.
    #[serde(default)]
    pub unpriced: bool,
    #[serde(skip)]
    pub available: bool,
    /// Made for a model ollama or llama.cpp runs on this machine (`Data::apply_available`), free: models.dev
    /// lists none.
    #[serde(skip)]
    pub local: bool,
    /// Where you have access: the harnesses listing it.
    #[serde(skip)]
    pub via: Vec<String>,
}

impl Offer {
    /// $ per 1M input tokens with `cached()` of them read from the cache.
    pub fn input_cached(&self) -> f64 {
        let share = cached();
        share * self.cache_read.unwrap_or(self.input) + (1.0 - share) * self.input
    }

    /// $ per 1M tokens, 3:1 input:output with `cached()` of the input cached.
    pub fn blended(&self) -> f64 {
        (3.0 * self.input_cached() + self.output) / 4.0
    }
}

fn paid(o: &Offer) -> bool {
    !o.unpriced && o.input + o.output > 0.0
}

/// Of offers you can use, the one you'd pay: the cheapest paid one, else a free one, else one
/// with no listed price.
fn cheapest<'a>(offers: impl Iterator<Item = &'a Offer>) -> Option<&'a Offer> {
    let offers: Vec<&Offer> = offers.collect();
    let by_price = offers.iter().copied().filter(|o| paid(o)).min_by(|a, b| a.blended().total_cmp(&b.blended()));
    by_price.or_else(|| offers.iter().copied().find(|o| !o.unpriced)).or(offers.first().copied())
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Model {
    pub key: String,
    pub name: String,
    /// Who trained it: Epoch's organization, else the vendor prefix of an OpenRouter-style id.
    #[serde(default)]
    pub developer: String,
    /// Where its developer is from, as Epoch says: "China", "USA". Empty when Epoch knows no
    /// model of the developer's.
    #[serde(default)]
    pub country: String,
    pub context: u64,
    pub max_output: u64,
    pub tool_call: bool,
    pub reasoning: bool,
    pub vision: bool,
    pub open_weights: bool,
    pub release: String,
    pub knowledge: String,
    /// The model's page name on models.dev, `zhipuai/glm-5.3`: the one most of its offers name.
    #[serde(default)]
    pub md: Option<String>,
    /// Its id on OpenRouter, `anthropic/claude-opus-5.5`, when OpenRouter lists it.
    #[serde(default)]
    pub openrouter: Option<String>,
    /// Its repo on Hugging Face, `moonshotai/Kimi-K2-Instruct`, when OpenRouter names one for
    /// it (`hf_listed`), else the one Hugging Face has under an id of its own (`hf_found`).
    #[serde(default)]
    pub hf: Option<String>,
    pub offers: Vec<Offer>,
    /// Epoch Capabilities Index: overall capability, roughly 100..170.
    pub eci: Option<f64>,
    /// Benchmark name -> best score (0..1) across reasoning-effort settings, or of the one
    /// the name states (`aa_named`).
    pub scores: BTreeMap<String, f64>,
    /// Task name -> 0..100 percentile (see fit.rs), which ranks it.
    pub fit: BTreeMap<String, f64>,
    /// Task name -> the value it shows: the task's benchmark score, 0..100. See `fit::shown`.
    #[serde(default)]
    pub shown: BTreeMap<String, f64>,
    /// Task name -> months of progress behind the source's best (`fit::add_lag`), which the
    /// tiers go by.
    #[serde(skip)]
    pub lag: BTreeMap<String, f64>,
    /// The model's page name on artificialanalysis.ai, when it has one (`aa_pages`).
    #[serde(default)]
    pub aa: Option<String>,
    /// The model's page name on epoch.ai, when it has one (`epoch_listed`), though no task uses
    /// the benchmarks Epoch ran it on.
    #[serde(default)]
    pub epoch: Option<String>,
    /// Output tokens per second and seconds to the first token of the answer, after any
    /// thinking: medians on the developer's own API, else across providers. Artificial Analysis
    /// measures them, Epoch does not.
    #[serde(default)]
    pub tps: Option<f64>,
    #[serde(default)]
    pub ttft: Option<f64>,
    /// USD one task of `COST_BENCH` cost the model, at the setting its score there is of. Epoch
    /// lists it, Artificial Analysis's API does not.
    #[serde(default)]
    pub task_cost: Option<f64>,
    /// Output tokens that task took it, of the same run. Epoch lists no input tokens.
    #[serde(default)]
    pub task_tokens: Option<f64>,
    /// Its score on Arena's Agent leaderboard, in percent: the net improvement arena.ai measures
    /// in real agent sessions, the best of its reasoning settings (`Arena`).
    #[serde(default)]
    pub arena: Option<f64>,
    /// Its score on each of `ARENA_MORE` that ranks it, by name, in percent as `arena`.
    #[serde(default)]
    pub arena_more: BTreeMap<String, f64>,
    /// The other source's overall index, with its data at hand too (`Data::borrow`).
    #[serde(skip)]
    pub other_index: Option<f64>,
    /// The other source's `scores`, with its data at hand too (`Data::borrow`).
    #[serde(skip)]
    pub other_scores: BTreeMap<String, f64>,
    #[serde(skip)]
    pub available: bool,
    /// Every `Offer::via` of the model, once each.
    #[serde(skip)]
    pub via: Vec<String>,
}

impl Model {
    /// The overall index `of` gives it: the one in use has it in `eci`, the other in `other_index`.
    pub fn index(&self, of: Source) -> Option<f64> {
        if of == source() { self.eci } else { self.other_index }
    }

    /// Its score (0..1) on the benchmark `b`, of whichever source runs it.
    pub fn bench(&self, b: &str) -> Option<f64> {
        let of = if crate::fit::bench_source(b) == source() { &self.scores } else { &self.other_scores };
        of.get(b).copied()
    }

    /// What a column that picked `b` in its dropdown shows: `bench` in percent, or one of
    /// `arena_more` as it is.
    pub fn picked(&self, b: &str) -> Option<f64> {
        self.arena_more.get(b).copied().or_else(|| self.bench(b).map(|s| s * 100.0))
    }

    /// The offer you'd actually use: cheapest available paid one, else a free one of yours,
    /// else one of yours with no listed price, else the `list` one. It may be `unpriced`,
    /// still naming the id to use; `priced_offer` is the one with prices to show.
    pub fn price(&self) -> Option<&Offer> {
        // Your machine's copy is the price of a model nothing else runs for you: free, it
        // would be what one of yours with no listed price costs.
        let local = self.local();
        let yours = |o: &&Offer| local || !o.local;
        let free = || self.offers.iter().filter(yours).find(|o| !o.unpriced);
        cheapest(self.offers.iter().filter(yours).filter(|o| o.available))
            .or_else(|| self.list())
            .or_else(free)
            .or(self.offers.first())
    }

    /// The offer at the most common list price among the paid ones, whoever it is from. Not a
    /// free one: another provider's free tier says nothing of what yours charges.
    fn list(&self) -> Option<&Offer> {
        // ponytail: mode of prices ≈ list price; resellers with odd pricing are outvoted.
        // Ordered by price bits so that on a tie the cheapest wins, deterministically.
        let mut counts: BTreeMap<(u64, u64, u64), (usize, &Offer)> = BTreeMap::new();
        for o in self.offers.iter().filter(|o| paid(o)) {
            let cache = o.input_cached().to_bits();
            counts.entry((o.input.to_bits(), o.output.to_bits(), cache)).or_insert((0, o)).0 += 1;
        }
        counts.into_values().rev().max_by_key(|(n, _)| *n).map(|(_, o)| o)
    }

    /// The offer `harness` runs the model on: of the ones it reaches, the one you'd pay.
    pub fn offer_via(&self, harness: &str) -> Option<&Offer> {
        cheapest(self.offers.iter().filter(|o| o.via.iter().any(|v| v == harness)))
    }

    /// `price`, when its prices are known, else the `list` one: prices to show, `listed` as not yours.
    pub fn priced_offer(&self) -> Option<&Offer> {
        self.quoted().map(|q| q.0)
    }

    /// `priced_offer` and `listed` in one search through the offers, for what asks both of a model.
    pub fn quoted(&self) -> Option<(&Offer, bool)> {
        match self.price().filter(|o| !o.unpriced) {
            Some(o) => Some((o, false)),
            None => self.list().map(|o| (o, true)),
        }
    }

    /// Whether the prices shown are the list ones, yours having none: what a `~` before them says.
    pub fn listed(&self) -> bool {
        self.quoted().is_some_and(|q| q.1)
    }

    /// `Offer::blended` of `priced_offer`; 0 when it is free, none when nobody lists a price.
    pub fn cost(&self) -> Option<f64> {
        self.quoted().map(|q| q.0.blended())
    }

    /// What the Dev dropdown and `--dev` pick the model by: its developer and that one's country.
    pub fn devs(&self) -> impl Iterator<Item = &str> {
        [self.developer.as_str(), self.country.as_str()].into_iter().filter(|s| !s.is_empty())
    }

    /// The model's page on models.dev, which lists every provider's price for it.
    fn md_page(&self) -> Option<String> {
        self.md.as_ref().map(|id| format!("https://models.dev/models/{id}/"))
    }

    /// Where models.dev shows the price you'd pay: the model's page, else the provider's, which
    /// lists its models with their prices. Your own machine is no provider of its.
    pub fn price_page(&self) -> Option<String> {
        let provider = || self.price().filter(|o| !o.local);
        self.md_page().or_else(|| Some(format!("https://models.dev/providers/{}/", provider()?.provider)))
    }

    /// Only your own machine runs it for you: a quantized copy with the context ollama gives
    /// it, which the scores and the context here are not of.
    pub fn local(&self) -> bool {
        let mut yours = self.offers.iter().filter(|o| o.available);
        self.offers.iter().any(|o| o.local) && yours.all(|o| o.local)
    }

    /// Your machine runs a copy of it, through ollama or llama.cpp, whoever else offers it.
    pub fn here(&self) -> bool {
        self.via.iter().any(|h| runs_here(h))
    }

    /// The model's page on a benchmark source, when it has one there (`epoch`, `aa`).
    pub fn page(&self, s: Source) -> Option<String> {
        let id = match s {
            Source::Epoch => self.epoch.as_ref()?,
            Source::Aa => self.aa.as_ref()?,
        };
        Some(format!("https://{}/models/{id}", s.site()))
    }

    /// The model's Hugging Face repo: the one the copy on this machine was pulled from,
    /// `unsloth/Qwen3.5-4B-GGUF`, when its tag says (ollama's `hf.co/owner/repo:quant`,
    /// llama.cpp's `owner/repo:quant`), else the one Hugging Face serves it from, whose id
    /// there is the repo, else the one OpenRouter names (`hf`). A tag of ollama's own library
    /// or a file downloaded by hand names none.
    pub fn hf_repo(&self) -> Option<&str> {
        let pulled = self.offers.iter().filter(|o| o.local && !o.id.ends_with(".gguf"));
        let served = self.offers.iter().filter(|o| o.provider == HF);
        let offered = pulled.chain(served).find_map(|o| {
            let id = o.id.split_once(':').map_or(o.id.as_str(), |(repo, _)| repo);
            let repo = match o.provider.as_str() {
                OLLAMA => id.strip_prefix("hf.co/").or_else(|| id.strip_prefix("huggingface.co/"))?,
                _ => id,
            };
            is_repo(repo).then_some(repo)
        });
        offered.or(self.hf.as_deref())
    }

    /// Arena's Agent leaderboard, for a model it ranks (`arena`): Arena has no page per model.
    pub fn arena_page(&self) -> Option<String> {
        self.arena.map(|_| ARENA_PAGE.to_string())
    }

    /// The model's pages, (site, url), of the sites that have one: models.dev, the benchmark
    /// sources, OpenRouter, Hugging Face (`hf_repo`), then Arena's leaderboard (`arena_page`).
    /// None is a guess: a model no site has a page for has no link.
    pub fn links(&self) -> Vec<(&'static str, String)> {
        let md = self.md_page().map(|url| ("models.dev", url));
        let sources = Source::ALL.into_iter().filter_map(|s| Some((s.site(), self.page(s)?)));
        let or = self.openrouter.as_ref().map(|id| ("openrouter.ai", format!("https://openrouter.ai/{id}")));
        let hf = self.hf_repo().map(|repo| ("huggingface.co", format!("https://huggingface.co/{repo}")));
        let arena = self.arena_page().map(|url| ("arena.ai", url));
        md.into_iter().chain(sources).chain(or).chain(hf).chain(arena).collect()
    }

    /// The first of `links`, which `o` then `enter` opens. Err says that no site has the model.
    pub fn url(&self) -> Result<String, String> {
        let first = self.links().into_iter().next().map(|(_, url)| url);
        first.ok_or_else(|| format!("no site has a page for {}", self.name))
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Data {
    /// `FORMAT` when fetched.
    #[serde(default)]
    pub format: u32,
    pub fetched: u64,
    /// Harness -> the `provider/model` ids it reported access to at the last refresh;
    /// `provider/*` stands for all of a provider's models.
    #[serde(default)]
    pub harness: BTreeMap<String, Vec<String>>,
    /// The harnesses that gave no listing at the last refresh and kept the one before in
    /// `harness`: kept once, so one silent again loses it.
    #[serde(default)]
    pub kept: Vec<String>,
    /// `fit::task_benches()` when fetched; a cache made with other benchmarks is stale.
    #[serde(default)]
    pub benches: Vec<String>,
    /// modelcmp's newest release at the last refresh, "0.2.0"; empty when unknown.
    #[serde(default)]
    pub latest: String,
    /// The day Arena published the leaderboard `Model::arena` is of, "2026-10-02"; empty with none.
    #[serde(default)]
    pub arena_date: String,
    /// huggingface.co did not send the leaderboard and no kept scores stood in: the Arena column
    /// is empty, and the cache is stale after `ARENA_RETRY`.
    #[serde(default)]
    pub arena_missed: bool,
    pub models: Vec<Model>,
    /// What went wrong in a refresh that still gave data, for the caller to show.
    #[serde(skip)]
    pub warning: Option<String>,
    /// A harness still asked for its models has no listing, not even the cache's: a refresh's
    /// early data, too soon to say that there is access to no model.
    #[serde(skip)]
    pub listing: bool,
    /// The rows have what the other source measures too (`borrow`), so its columns show.
    #[serde(skip)]
    pub lent: bool,
}

/// A row of the other source's data, as much of it as `Data::borrow` takes.
#[derive(Deserialize)]
struct Lent {
    key: String,
    eci: Option<f64>,
    #[serde(default)]
    tps: Option<f64>,
    #[serde(default)]
    ttft: Option<f64>,
    #[serde(default)]
    task_cost: Option<f64>,
    #[serde(default)]
    task_tokens: Option<f64>,
    #[serde(default)]
    scores: BTreeMap<String, f64>,
}

impl From<&Model> for Lent {
    fn from(m: &Model) -> Self {
        Lent {
            key: m.key.clone(),
            eci: m.eci,
            tps: m.tps,
            ttft: m.ttft,
            task_cost: m.task_cost,
            task_tokens: m.task_tokens,
            scores: m.scores.clone(),
        }
    }
}

/// The other source's cache, read for `Lent` alone.
#[derive(Deserialize)]
struct LentData {
    #[serde(default)]
    format: u32,
    fetched: u64,
    models: Vec<Lent>,
}

impl Data {
    pub fn age(&self) -> Duration {
        Duration::from_secs(now().saturating_sub(self.fetched))
    }

    /// Whether you have access to any model: with none, every model counts as in reach. One
    /// only your machine runs is none, as no task picks it (`Model::local`).
    pub fn any_available(&self) -> bool {
        self.models.iter().any(|m| m.available && !m.local())
    }

    /// The newest release when it is newer than this build.
    pub fn update(&self) -> Option<&str> {
        let v = |s: &str| s.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
        (v(&self.latest) > v(env!("CARGO_PKG_VERSION"))).then_some(self.latest.as_str())
    }

    /// Gives each row of `src`'s data what only the other source measures, and its index, from
    /// that one's rows, which go by the same keys: one table shows both. With none, the row has
    /// neither, whatever its cache kept.
    fn borrow(&mut self, src: Source, other: Option<Vec<Lent>>) {
        self.lent = other.is_some();
        let mut by_key: HashMap<String, Lent> =
            other.into_iter().flatten().map(|mut l| (std::mem::take(&mut l.key), l)).collect();
        for m in &mut self.models {
            let l = by_key.remove(m.key.as_str());
            m.other_index = l.as_ref().and_then(|l| l.eci);
            match src {
                Source::Epoch => (m.tps, m.ttft) = l.as_ref().map_or((None, None), |l| (l.tps, l.ttft)),
                Source::Aa => {
                    (m.task_cost, m.task_tokens) = l.as_ref().map_or((None, None), |l| (l.task_cost, l.task_tokens))
                }
            }
            m.other_scores = l.map(|l| l.scores).unwrap_or_default();
        }
    }

    pub fn stale(&self) -> bool {
        // Dated ahead of the clock, it would never grow old.
        let unranked = self.arena_missed && self.age() > ARENA_RETRY;
        self.age() > MAX_AGE
            || unranked
            || self.fetched > now()
            || self.format != FORMAT
            || self.benches != source().benches()
    }

    /// Mark offers the user can use, and where: listed by an installed harness. A model ollama
    /// or llama.cpp runs here gets an offer of its own from each, free, for the tag that names it
    /// (`local_key`).
    pub fn apply_available(&mut self) {
        let keys: HashSet<&str> = self.models.iter().map(|m| m.key.as_str()).collect();
        let repos = REPOS.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_default();
        let tags = |h: &str| Some(local_tags(self.harness.get(h)?, &keys, &repos));
        let local: Vec<_> = LOCAL.iter().filter_map(|(h, name)| Some((*h, *name, tags(h)?))).collect();
        let harness: Vec<(&str, HashSet<String>)> =
            self.harness.iter().map(|(h, ids)| (h.as_str(), ids.iter().map(|i| canonical(h, i)).collect())).collect();
        for m in &mut self.models {
            m.via.clear();
            // Made anew each time, as the cache does not hold it.
            m.offers.retain(|o| !o.local);
            for (h, name, tags) in &local {
                let Some(tag) = tags.get(&m.key) else { continue };
                // The scores are the full weights', which a quantized copy falls short of.
                let provider_name = format!("{name} (local quant)");
                let (provider, id) = (h.to_string(), tag.to_string());
                m.offers.push(Offer { provider, provider_name, id, local: true, ..Default::default() });
            }
            for o in &mut m.offers {
                let (id, all) = (format!("{}/{}", o.provider, o.id), format!("{}/*", o.provider));
                o.via = harness
                    .iter()
                    .filter(|(_, ids)| ids.contains(id.as_str()) || ids.contains(all.as_str()))
                    .map(|(h, _)| h.to_string())
                    .collect();
                o.available = !o.via.is_empty();
                for v in &o.via {
                    if !m.via.contains(v) {
                        m.via.push(v.clone());
                    }
                }
            }
            m.available = !m.via.is_empty();
        }
        // Value counts the models close to your best on coding, and ranks the price you would
        // pay, which availability picks.
        crate::fit::add_lag(&mut self.models, now() as f64);
        crate::fit::add_value(&mut self.models);
    }

    /// Resolve a user-typed model name. Err = list of candidates (empty if none). A partial
    /// name matches the models you have first, and all of them only if none of yours match.
    pub fn find(&self, query: &str) -> Result<&Model, Vec<&Model>> {
        let q = norm(query);
        // "--" or "日本" has nothing a key is made of, and an empty string is part of every key.
        if q.is_empty() {
            return Err(vec![]);
        }
        // By key, or by a provider's id for it, bare or as `list --id` prints it:
        // "granite-4.0-h-micro" or "openrouter/ibm-granite/granite-4.0-h-micro" is "Granite 4.0 Micro".
        // Or as pi and omp name the provider, which is the id `pick` prints for them.
        let pi = norm(&canonical("pi", query));
        let by_id = |m: &&Model| {
            m.offers.iter().any(|o| {
                let full = norm(&format!("{}/{}", o.provider, o.id));
                norm(&slug(&o.id)) == q || full == q || full == pi
            })
        };
        // Or by the name shown, which says more than the key when it is Epoch's: "Kimi K2 (Sep 2025)".
        let by_name = |m: &&Model| norm(&m.name) == q;
        let all = || self.models.iter();
        // Two models can go by one bare id at different providers: yours first.
        let id = || all().filter(by_id).min_by_key(|m| !m.available);
        if let Some(m) = all().find(|m| m.key == q).or_else(id).or_else(|| all().find(by_name)) {
            return Ok(m);
        }
        // A version is whole: "sonnet-5" is no part of "claude-sonnet-5.5".
        let digit = |c: char| c.is_ascii_digit();
        let part =
            |k: &str| k.match_indices(&q).any(|(i, _)| !(q.ends_with(digit) && k[i + q.len()..].starts_with(digit)));
        let hits = |mine: bool| -> Vec<&Model> {
            self.models.iter().filter(|m| (!mine || m.available) && part(&m.key)).collect()
        };
        let mut hits = Some(hits(true)).filter(|h| !h.is_empty()).unwrap_or_else(|| hits(false));
        hits.sort_by_key(|m| m.key.len());
        match hits.as_slice() {
            [one] => Ok(one),
            // The shortest match wins when it is part of every other: "opus-4.5" -> claude-opus-4.5,
            // not ...-thinking; "claude" matching both Opus and Sonnet stays ambiguous.
            [a, rest @ ..] if rest.iter().all(|b| b.key.len() > a.key.len() && b.key.contains(&a.key)) => Ok(a),
            _ => Err(hits),
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// One per source, so a switch back needs no download: data.json for Epoch, data-aa.json.
fn cache_path(src: Source) -> PathBuf {
    let name = match src {
        Source::Epoch => "data.json".to_string(),
        s => format!("data-{}.json", s.id()),
    };
    cache_file(&name)
}

pub fn load_cache() -> Option<Data> {
    load_repos();
    let bytes = std::fs::read(cache_path(source())).ok()?;
    // One written in another format reads wrong here (a missing price as free), so it is no fallback.
    let mut d = serde_json::from_slice::<Data>(&bytes).ok().filter(|d| d.format == FORMAT)?;
    // The other source's, unless fetched more than a day apart: its numbers would be older
    // than the table says.
    let other = std::fs::read(cache_path(source().other())).ok();
    let other = other.and_then(|b| serde_json::from_slice::<LentData>(&b).ok());
    let other = other.filter(|o| o.format == FORMAT && o.fetched.abs_diff(d.fetched) <= MAX_AGE.as_secs());
    d.borrow(source(), other.map(|o| o.models));
    // Value is derived here from cached fields, so a change to the formula applies without a re-download.
    d.apply_available();
    Some(d)
}

fn installed(bin: &str) -> bool {
    // One that cannot run is not installed: a harness found by its file alone has every model of its provider.
    let runs = |f: std::path::PathBuf| {
        #[cfg(unix)]
        return std::fs::metadata(f)
            .is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0);
        #[cfg(not(unix))]
        f.is_file()
    };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| runs(d.join(bin))))
}

/// The shell line that runs `cmd` in the shell's place, each argument quoted whatever it holds.
fn exec_line(cmd: &[String]) -> String {
    let quoted: Vec<String> = cmd.iter().map(|a| format!("'{}'", a.replace('\'', r"'\''"))).collect();
    format!("exec {}", quoted.join(" "))
}

/// `cmd` as a new WSL session starts it, under WSL with `wsl.exe` on `PATH`: bare, so a login
/// shell sets up PATH and keys as for a typed command. A harness is asked for its models the way
/// it is launched, as a key exported by hand in this shell, which opencode would list a provider
/// for, is not in the new tab.
pub fn wsl_session(cmd: &[String]) -> Option<Vec<String>> {
    let distro = std::env::var("WSL_DISTRO_NAME").ok().filter(|_| installed("wsl.exe"))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let cwd = std::env::current_dir().ok()?.to_string_lossy().into_owned();
    Some(["wsl.exe", "-d", &distro, "--cd", &cwd, "-e", &shell, "-lic", &exec_line(cmd)].map(String::from).to_vec())
}

/// What `bin` prints for `args`, asked in a new session under WSL (`wsl_session`), else here.
fn listing(bin: &str, args: &[&str], stop: &AtomicBool) -> Option<String> {
    let cmd: Vec<String> = std::iter::once(bin).chain(args.iter().copied()).map(String::from).collect();
    let argv = wsl_session(&cmd).unwrap_or(cmd);
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    run(&argv[0], &args, Duration::from_secs(30), stop)
}

/// What a harness answered: its ids, or none when its listing failed, hung or was cut short.
type Listing = (String, Option<Vec<String>>);

/// What each harness `asked` says it can use, sent to `answers` as it answers: its ids, or
/// none when its listing failed, hung or was cut short by `stop`, so a refresh always finishes.
/// All are asked at once, so the slowest is the wait.
fn harness_models(asked: &[&(&'static str, Probe)], stop: &AtomicBool, steps: &Steps, answers: &Sender<Listing>) {
    let ids = |bin: &str, probe: &Probe| match probe {
        Probe::Provider(p) => Some(vec![format!("{p}/*")]),
        Probe::List(args) => {
            let out = listing(bin, args, stop)?;
            Some(out.lines().map(str::trim).filter(|l| l.contains('/')).map(String::from).collect())
        }
        Probe::Table(args) => table_ids(&listing(bin, args, stop)?),
        // One that does not answer runs none: no model is kept for a server that is down.
        Probe::Names(args) => Some(listing(bin, args, stop).and_then(|out| name_ids(&out)).unwrap_or_default()),
        Probe::Cache(args) => {
            let cmd = [llama_cmd()?, args].concat();
            let cached = listing(cmd[0], &cmd[1..], stop).and_then(|out| cache_ids(&out)).unwrap_or_default();
            Some(cached.into_iter().chain(gguf_ids()).collect())
        }
        Probe::Json(args) => selector_ids(&listing(bin, args, stop)?),
        Probe::Copilot => copilot_ids(stop),
    };
    std::thread::scope(|s| {
        for (bin, probe) in asked {
            step(steps, bin);
            let ids = &ids;
            s.spawn(move || {
                let _ = answers.send((bin.to_string(), ids(bin, probe)));
                answered(steps, bin);
            });
        }
    });
}

/// The listings a refresh keeps, and the harnesses that gave none: one that did not answer
/// keeps what it listed `before`, as one bad run is not a day without its models.
fn keep_listed(
    now: BTreeMap<String, Option<Vec<String>>>,
    before: impl FnOnce() -> BTreeMap<String, Vec<String>>,
) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let silent: Vec<String> = now.iter().filter(|(_, ids)| ids.is_none()).map(|(h, _)| h.clone()).collect();
    let mut before = if silent.is_empty() { BTreeMap::new() } else { before() };
    let kept = now.into_iter().filter_map(|(h, ids)| Some((ids.or_else(|| before.remove(&h))?, h)));
    (kept.map(|(ids, h)| (h, ids)).collect(), silent)
}

/// What the harnesses listed at the last refresh, read from its cache whatever the format.
#[derive(Deserialize, Default)]
struct Listed {
    #[serde(default)]
    harness: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    kept: Vec<String>,
}

impl Listed {
    fn of(cache: &[u8]) -> Self {
        serde_json::from_slice(cache).unwrap_or_default()
    }

    /// Its listings. Not the ones that refresh kept from the one before, unless `kept`: a
    /// harness silent twice in a row, as one you logged out of, no longer has the models it
    /// once listed.
    fn harness(&self, kept: bool) -> BTreeMap<String, Vec<String>> {
        let listings = self.harness.iter().filter(|(h, _)| kept || !self.kept.contains(h));
        listings.map(|(h, ids)| (h.clone(), ids.clone())).collect()
    }
}

/// The pages linked at the last refresh, Artificial Analysis's then Epoch's, from its cache: the
/// sites have those, so they stand for a site's list when that did not come, as one bad
/// download is not a day without its links. None from a cache in another format, whose Epoch
/// names were not checked.
fn cached_pages(cache: &[u8]) -> [Vec<String>; 2] {
    #[derive(Deserialize)]
    struct Linked {
        format: u32,
        models: Vec<Pages>,
    }
    #[derive(Deserialize)]
    struct Pages {
        aa: Option<String>,
        epoch: Option<String>,
    }
    let before = serde_json::from_slice::<Linked>(cache).ok().filter(|d| d.format == FORMAT);
    let (aa, epoch): (Vec<_>, Vec<_>) = before.into_iter().flat_map(|d| d.models).map(|m| (m.aa, m.epoch)).unzip();
    [aa, epoch].map(|pages| pages.into_iter().flatten().collect())
}

/// The `provider/model` ids of a table under a `provider model ...` header, as `pi --list-models`
/// prints; `None` without that header. Only lines with as many columns as the header are rows.
fn table_ids(out: &str) -> Option<Vec<String>> {
    let mut lines = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .skip_while(|f| !f.starts_with(&["provider", "model"]));
    let cols = lines.next()?.len();
    Some(lines.filter(|f| f.len() == cols).map(|f| format!("{}/{}", f[0], f[1])).collect())
}

/// The `ollama/tag` ids of a table under a `NAME ...` header, as `ollama list` prints; `None`
/// without that header.
fn name_ids(out: &str) -> Option<Vec<String>> {
    let mut names = out.lines().filter_map(|l| l.split_whitespace().next()).skip_while(|n| *n != "NAME");
    names.next()?;
    Some(names.map(|n| format!("{OLLAMA}/{n}")).collect())
}

/// The `llama-cli/repo:quant` ids of the numbered list under `number of models in cache`, as
/// `llama-cli --cache-list` prints; `None` without that line.
fn cache_ids(out: &str) -> Option<Vec<String>> {
    let mut lines = out.lines().skip_while(|l| !l.starts_with("number of models in cache"));
    lines.next()?;
    Some(lines.filter_map(|l| l.split_whitespace().nth(1)).map(|n| format!("{LLAMA}/{n}")).collect())
}

/// The folder saved with `modelcmp models-dir`, set once at the start.
static MODELS_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_models_dir(dir: &str) {
    let _ = MODELS_DIR.set(dir.to_string());
}

/// The `llama-cli/path` ids of the `.gguf` files in llama.cpp's own models folder
/// (`LLAMA_ARG_MODELS_DIR`), else in the one saved here (`set_models_dir`): the ones downloaded
/// by hand, which its cache does not list. Not a vision projector, which is no model.
fn gguf_ids() -> Vec<String> {
    let saved = || MODELS_DIR.get().filter(|d| !d.is_empty()).map(Into::into);
    let dir = std::env::var_os("LLAMA_ARG_MODELS_DIR")
        .filter(|d| !d.is_empty())
        .or_else(saved)
        .and_then(|d| std::fs::read_dir(d).ok());
    let files = dir.into_iter().flatten().flatten().map(|e| e.path());
    let model = |p: &PathBuf| {
        p.extension().is_some_and(|e| e == "gguf")
            && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("mmproj"))
    };
    files.filter(model).map(|p| format!("{LLAMA}/{}", p.display())).collect()
}

/// The `provider/model` ids of `{"models": [{"selector": ...}]}`, as `omp models --json` prints;
/// `None` when it is not that. Lines around the JSON are skipped, a brace in one too.
fn selector_ids(out: &str) -> Option<Vec<String>> {
    let models = out.match_indices('{').find_map(|(at, _)| {
        let json = serde_json::Deserializer::from_str(&out[at..]).into_iter::<serde_json::Value>().next()?.ok()?;
        json.get("models")?.as_array().cloned()
    })?;
    Some(models.iter().filter_map(|m| Some(m["selector"].as_str()?.to_string())).collect())
}

/// The `github-copilot/model` ids Copilot's CLI takes on your plan, asked of the CLI itself
/// over the Agent Client Protocol, which has no command for it: a new session's answer lists
/// them. In the temp dir, so it loads no repository's instructions.
fn copilot_ids(stop: &AtomicBool) -> Option<Vec<String>> {
    let dir = std::env::temp_dir();
    let asks = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": dir.to_str()?, "mcpServers": []}}),
    ];
    let asks = format!("{}\n{}\n", asks[0], asks[1]);
    copilot_enabled(&answer("copilot", &["--acp"], &dir, &asks, 2, Duration::from_secs(30), stop)?)
}

/// The ids of a new session's answer that Copilot's CLI takes, under models.dev's provider
/// for them: the ones no policy keeps off, switched off or not yet on. None for an answer
/// without models, as an error or the one of a CLI not logged in.
fn copilot_enabled(answer: &serde_json::Value) -> Option<Vec<String>> {
    let taken = |m: &&serde_json::Value| {
        let state = &m["_meta"]["copilotEnablement"];
        state.is_null() || state == "enabled"
    };
    let ids = answer["result"]["models"]["availableModels"].as_array()?.iter().filter(taken);
    Some(ids.filter_map(|m| m["modelId"].as_str()).map(|id| format!("{COPILOT}/{id}")).collect())
}

/// The JSON-RPC response `id` of `bin args`, run in `cwd` and sent the lines `asks`, or `None`
/// when it is missing, ends without answering, or is killed at `limit` or on `stop`, which
/// also keeps it from starting.
fn answer(
    bin: &str,
    args: &[&str],
    cwd: &std::path::Path,
    asks: &str,
    id: u64,
    limit: Duration,
    stop: &AtomicBool,
) -> Option<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError::Timeout;
    if stop.load(Relaxed) {
        return None;
    }
    let (io, null) = (Stdio::piped, Stdio::null());
    let mut child = Command::new(bin).args(args).current_dir(cwd).stdin(io()).stdout(io()).stderr(null).spawn().ok()?;
    let (mut stdin, out) = (child.stdin.take()?, child.stdout.take()?);
    // One that has already gone takes nothing, and has no answer to read either.
    let _ = stdin.write_all(asks.as_bytes()).and_then(|()| stdin.flush());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Not its notifications, nor a request of its own, which has a method.
        let lines = BufReader::new(out).lines().map_while(Result::ok);
        let mut answers = lines.filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok());
        if let Some(answer) = answers.find(|v| v["id"] == id && v.get("method").is_none()) {
            let _ = tx.send(answer);
        }
        // Read to its end: it writes on after answering, which a closed pipe would fail.
        answers.for_each(drop);
    });
    let start = Instant::now();
    let answer = loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(answer) => break Some(answer),
            Err(Timeout) if start.elapsed() < limit && !stop.load(Relaxed) => {}
            Err(_) => break None,
        }
    };
    // Its input closed, it ends on its own: killed only when it has not, or gave no answer.
    drop(stdin);
    let end = Instant::now() + Duration::from_secs(2);
    while answer.is_some() && !stop.load(Relaxed) && Instant::now() < end && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    answer
}

/// `bin args` stdout, or `None` when it is missing, fails, or is killed at `limit` or on `stop`,
/// which also keeps it from starting.
fn run(bin: &str, args: &[&str], limit: Duration, stop: &AtomicBool) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError::Timeout;
    if stop.load(Relaxed) {
        return None;
    }
    let mut child =
        Command::new(bin).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Read while it runs: output larger than the pipe holds would otherwise block it.
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = tx.send(out.read_to_string(&mut text).map(|_| text));
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() >= limit || stop.load(Relaxed) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // A process it left running may hold the pipe open: wait for the rest no longer than the
    // limit, nor past `stop`.
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(text) => return text.ok(),
            Err(Timeout) if start.elapsed() < limit && !stop.load(Relaxed) => {}
            Err(_) => return None,
        }
    }
}

/// The steps of a refresh, its downloads and its harnesses: how many, and the ones still awaited.
pub type Steps = Arc<Mutex<(usize, Vec<&'static str>)>>;

/// Counts `name` as a step, awaited until `answered`.
fn step(steps: &Steps, name: &'static str) {
    let mut s = steps.lock().unwrap_or_else(PoisonError::into_inner);
    s.0 += 1;
    s.1.push(name);
}

fn answered(steps: &Steps, name: &str) {
    let mut s = steps.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(i) = s.1.iter().position(|n| *n == name) {
        s.1.remove(i);
    }
}

/// How far the refresh counting in `steps` is, as `progress_text`; empty before it has any.
pub fn progress(steps: &Steps) -> String {
    let s = steps.lock().unwrap_or_else(PoisonError::into_inner);
    if s.0 == 0 { String::new() } else { progress_text(s.0, &s.1) }
}

/// The steps answered of `total`, and with one or two left what is `awaited`: `7/9, waiting for opencode`.
fn progress_text(total: usize, awaited: &[&str]) -> String {
    let mut names: Vec<&str> = vec![];
    for n in awaited {
        if !names.contains(n) {
            names.push(n);
        }
    }
    let count = format!("{}/{total}", total - awaited.len());
    if names.is_empty() || names.len() > 2 { count } else { format!("{count}, waiting for {}", names.join(", ")) }
}

/// Download the sources and ask the harnesses in parallel, merge, write cache. `steps` counts
/// them for `progress`. `early` gets the data as soon as it is downloaded, when harnesses are
/// still listing their models, which have the ones the cache had until the whole answer: they
/// are not waited for to show the rest.
pub fn refresh(steps: &Steps, early: Option<impl FnOnce(Data)>) -> Result<Data, Failure> {
    load_repos();
    let src = source();
    // Asked for under Epoch too when it is there: one refresh serves both sources.
    let key = aa_key();
    if src == Source::Aa && key.is_none() {
        return Err(Failure::NoKey);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, answers) = std::sync::mpsc::channel();
    // The installed ones: one not installed is left out.
    let asked: Vec<_> = HARNESSES.iter().filter(|h| has(h.0)).collect();
    let harness = {
        let (asked, stop, steps) = (asked.clone(), Arc::clone(&stop), Arc::clone(steps));
        std::thread::spawn(move || harness_models(&asked, &stop, &steps, &tx))
    };
    let res = download(src, key.as_deref(), steps);
    // A refresh that cannot finish kills the harnesses rather than wait for them.
    stop.store(res.is_err(), Relaxed);
    let (mut data, mut other, [aa, epoch, openrouter, arena], more, release) = match res {
        Ok(d) => d,
        Err(e) => {
            let _ = harness.join();
            return Err(e);
        }
    };
    // Before the early data, so the other source's columns do not go to come back.
    let lent: Option<Vec<Lent>> =
        other.as_ref().and_then(|(_, o)| Some(o.as_ref().ok()?.models.iter().map(Lent::from).collect()));
    data.borrow(src, lent);
    // Only links hang on them, so without one the refresh still succeeds, and says so.
    let text = |xml: Result<Vec<u8>, Failure>| String::from_utf8_lossy(&xml.unwrap_or_default()).into_owned();
    let (aa, epoch) = (text(aa), text(epoch));
    let (mut aa, mut epoch) = (sitemap(&aa, Source::Aa), sitemap(&epoch, Source::Epoch));
    // Read once, and only by a refresh that a list of pages or a harness's models did not reach,
    // or not yet.
    let cache = std::cell::LazyCell::new(|| std::fs::read(cache_path(src)).unwrap_or_default());
    // Only a page a site lists is linked; without its list, only one linked at the last refresh.
    let before = if aa.is_empty() || epoch.is_empty() { cached_pages(&cache) } else { Default::default() };
    let was = std::cell::LazyCell::new(|| Listed::of(&cache));
    let mut unpaged = [None, None];
    for (i, (pages, s)) in [(&mut aa, Source::Aa), (&mut epoch, Source::Epoch)].into_iter().enumerate() {
        if pages.is_empty() {
            let so = if before[i].is_empty() { "may be missing" } else { "are kept from the last refresh" };
            unpaged[i] = Some(format!("{} did not list its pages: links to it {so}", s.site()));
            pages.extend(before[i].iter().map(String::as_str));
        }
    }
    aa_pages(&mut data.models, &aa);
    epoch_listed(&mut data.models, &epoch);
    // ponytail: no repo is kept from the last refresh, as the pages are. Read them from the
    // cache in `cached_pages` if a refresh without OpenRouter should keep its links.
    let openrouter = openrouter.unwrap_or_default();
    let unnamed = (!hf_listed(&mut data.models, &openrouter))
        .then(|| "openrouter.ai did not list its models: links to huggingface.co may be missing".to_string());
    // Only the Arena column hangs on it: without it, the scores of the last refresh are kept,
    // and none show once Arena has published no leaderboard for `ARENA_MAX_AGE`.
    let ranked = Arena::parse(&arena.unwrap_or_default());
    let unranked = ranked.is_none().then_some("huggingface.co did not send Arena's agent leaderboard");
    let arena = ranked.or_else(|| Arena::cached(&cache)).unwrap_or_default();
    let unranked = match (unranked, arena.current(now())) {
        (Some(e), true) => Some(format!("{e}: its scores are kept from the last refresh")),
        (Some(e), false) => {
            data.arena_missed = true;
            Some(format!("{e}: the Arena column is empty"))
        }
        (None, false) => Some(format!(
            "Arena's agent leaderboard is from {}, too old to show: the Arena column is empty",
            arena.date
        )),
        (None, true) => arena
            .cut
            .then(|| "Arena's agent leaderboard has more rows than were read: some models have no Arena score".into()),
    };
    let mut arena = if arena.current(now()) { arena } else { Arena::default() };
    // A leaderboard kept from the last refresh has its signals too.
    // ponytail: with the leaderboard new, a signal that did not come, or is of another day, is
    // empty until the next refresh, with no warning. Keep that one from the cache if it shows.
    for ((name, _), rows) in ARENA_MORE.into_iter().zip(more) {
        let signal = rows.ok().and_then(|r| Arena::parse(&r)).filter(|s| s.date == arena.date);
        arena.more.extend(signal.map(|s| (name, s.scores)));
    }
    arena.score(&mut data.models);
    data.arena_date.clone_from(&arena.date);
    if let Some((_, Ok(o))) = &mut other {
        aa_pages(&mut o.models, &aa);
        epoch_listed(&mut o.models, &epoch);
        hf_listed(&mut o.models, &openrouter);
        arena.score(&mut o.models);
        (o.arena_date, o.arena_missed) = (arena.date.clone(), data.arena_missed);
    }
    // Only the update notice hangs on it, so without it the refresh still succeeds.
    data.latest = release
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|r| Some(r["tag_name"].as_str()?.trim_start_matches('v').to_string()))
        .unwrap_or_default();
    let mut listed: BTreeMap<_, _> = answers.try_iter().collect();
    let awaited: Vec<_> = asked.iter().map(|h| h.0).filter(|h| !listed.contains_key(*h)).collect();
    // Built only for who shows it: a command waits for the whole of it anyway.
    if let Some(early) = early.filter(|_| !awaited.is_empty()) {
        let mut first = Data { warning: None, ..data.clone() };
        first.harness = keep_listed(listed.clone(), || was.harness(false)).0;
        // A harness still listing keeps what the cache has for it, and so what the table shows,
        // whether or not the last refresh kept it: its marks do not go to come back.
        let mut shown = was.harness(true);
        first.harness.extend(awaited.iter().filter_map(|h| Some((h.to_string(), shown.remove(*h)?))));
        first.listing = awaited.iter().any(|h| !first.harness.contains_key(*h));
        first.apply_available();
        early(first);
    }
    // After the early data, which does not wait for it, and while the harnesses answer.
    step(steps, "huggingface.co");
    let (mut found, mut asks): (Asked, _) = (kept("hf.json"), HF_ASKS);
    hf_found(&mut data.models, &mut found, &mut asks, hf_spelled);
    if let Some((_, Ok(o))) = &mut other {
        hf_found(&mut o.models, &mut found, &mut asks, hf_spelled);
    }
    keep("hf.json", &found);
    answered(steps, "huggingface.co");
    // Until the last harness has answered. One that never does, its thread dead, is silent.
    listed.extend(answers.iter());
    let _ = harness.join();
    for h in awaited {
        listed.entry(h.to_string()).or_insert(None);
    }
    let silent;
    (data.harness, silent) = keep_listed(listed, || was.harness(false));
    let lost;
    (data.kept, lost) = silent.into_iter().partition(|h| data.harness.contains_key(h));
    let json = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    // Its own source's file, though a switch may have happened meanwhile.
    // An unwritable cache costs the next start a download, not this one its data.
    let uncached = crate::store::write_atomic(&cache_path(src), &json)
        .err()
        .map(|e| format!("could not cache the data in {}: {e}", cache_path(src).display()));
    // The other source's cache, with the same listings, so that a switch to it needs no refresh.
    let unswitched = other.and_then(|(s, o)| {
        let written = o.and_then(|mut o| {
            (o.latest, o.harness, o.kept) = (data.latest.clone(), data.harness.clone(), data.kept.clone());
            let json = serde_json::to_vec(&o).map_err(|e| e.to_string())?;
            Ok(crate::store::write_atomic(&cache_path(s), &json).map_err(|e| e.to_string())?)
        });
        written.err().map(|e: Failure| format!("{}'s data was not refreshed ({e})", s.label()))
    });
    let unlisted = |hs: &[String], kept: &str| {
        (!hs.is_empty()).then(|| format!("{} did not list its models{kept}", hs.join(", ")))
    };
    let renamed = data.warning.take();
    let warnings =
        [renamed, unlisted(&data.kept, ": kept the ones from the last refresh"), unlisted(&lost, ""), uncached];
    let warnings = warnings.into_iter().chain(unpaged).chain([unnamed, unranked, unswitched]);
    data.warning = Some(warnings.flatten().collect::<Vec<_>>().join("; ")).filter(|w| !w.is_empty());
    data.apply_available();
    Ok(data)
}

/// What `download` gives: the merged data, the other source's when it was asked for, then
/// Artificial Analysis's and Epoch's sitemaps, OpenRouter's models, Arena's agent leaderboard,
/// its signals (`ARENA_MORE`) and modelcmp's newest release, which a refresh can do without.
type Downloaded = (
    Data,
    Option<(Source, Result<Data, Failure>)>,
    [Result<Vec<u8>, Failure>; 4],
    [Result<Vec<u8>, Failure>; 5],
    Result<Vec<u8>, Failure>,
);

/// How long the downloads a refresh can do without are waited for once it has the others.
const GRACE: Duration = Duration::from_secs(10);

/// The sources, downloaded in parallel and merged. It returns at the first failure of a download
/// it cannot do without, models.dev's and the source's scores, leaving the others to end on
/// their own: none is waited for once the refresh cannot finish, and the links and the newest
/// release no longer than `GRACE` once it can.
fn download(src: Source, key: Option<&str>, steps: &Steps) -> Result<Downloaded, Failure> {
    const URLS: [&str; 13] = [
        MODELS_URL,
        EPOCH_URL,
        AA_API_URL,
        AA_URL,
        EPOCH_PAGES_URL,
        OPENROUTER_URL,
        RELEASE_URL,
        ARENA_URL,
        ARENA_MORE[0].1,
        ARENA_MORE[1].1,
        ARENA_MORE[2].1,
        ARENA_MORE[3].1,
        ARENA_MORE[4].1,
    ];
    let needed = [0, if src == Source::Aa { 2 } else { 1 }];
    let (tx, rx) = std::sync::mpsc::channel();
    let mut sites = vec![];
    for (i, url) in URLS.into_iter().enumerate() {
        // Artificial Analysis's scores are asked for only with its key.
        let key = if url == AA_API_URL { key.map(String::from) } else { None };
        if url == AA_API_URL && key.is_none() {
            continue;
        }
        let (tx, steps) = (tx.clone(), Arc::clone(steps));
        let pages = url == EPOCH_PAGES_URL;
        // Counted under its site's name, as the warnings name it.
        let site =
            url.split('/').nth(2).unwrap_or(url).trim_start_matches("api.").trim_start_matches("datasets-server.");
        step(&steps, site);
        sites.push(site);
        std::thread::spawn(move || {
            let res = if pages { epoch_pages(url) } else { fetch(url, key.as_deref()) };
            answered(&steps, site);
            tx.send((i, res))
        });
    }
    drop(tx);
    let mut got: [Result<Vec<u8>, Failure>; 13] = URLS.map(|url| Err(format!("{url}: no reply").into()));
    // Once the first three that were asked for are in, the rest get `GRACE` and no more. The
    // other source's scores are among them, though not needed: they fill its cache, and Epoch's
    // names decide which key a merged row keeps, which your favorites and notes hang on.
    let (mut missing, mut end) = (2 + usize::from(key.is_some()), None::<Instant>);
    loop {
        let next = match end {
            None => rx.recv().ok(),
            Some(end) => rx.recv_timeout(end.saturating_duration_since(Instant::now())).ok(),
        };
        let Some((i, res)) = next else { break };
        if needed.contains(&i) {
            res.as_ref().map_err(Failure::clone)?;
        }
        if i < 3 {
            missing -= 1;
            if missing == 0 {
                end = Some(Instant::now() + GRACE);
            }
        }
        got[i] = res;
    }
    // A download not waited for is awaited no more, before the merge and not after it.
    sites.into_iter().for_each(|site| answered(steps, site));
    let [models, epoch, api, aa, epoch_pages, openrouter, release, arena, more @ ..] = got;
    let models = models?;
    let ep = epoch.and_then(|z| Ok(parse_epoch(&z)?));
    let by_epoch = || -> Result<Data, Failure> {
        let ep = ep.as_ref().map_err(Failure::clone)?;
        let mut data = merge(&models, ep, None)?;
        epoch_named(&mut data.models, ep);
        Ok(data)
    };
    let by_aa = || -> Result<Data, Failure> {
        // Without Epoch's the refresh still succeeds, with fewer epoch.ai links and Artificial
        // Analysis's names deciding the keys.
        let mut data = merge(&models, &parse_aa(api.as_ref().map_err(Failure::clone)?)?, ep.as_ref().ok())?;
        match &ep {
            Ok(ep) => epoch_named(&mut data.models, ep),
            // Said, as a favorite or a note on a key that changed is not found until the next refresh.
            Err(e) => {
                data.warning = Some(format!("epoch.ai's names did not come ({e}): some models go by another key"))
            }
        }
        Ok(data)
    };
    let (data, other) = match src {
        Source::Epoch => (by_epoch()?, key.map(|_| (Source::Aa, by_aa()))),
        Source::Aa => (by_aa()?, Some((Source::Epoch, by_epoch()))),
    };
    Ok((data, other, [aa, epoch_pages, openrouter, arena], more, release))
}

/// Said after why Artificial Analysis's key did nothing: what is shown instead.
pub const FELL_BACK: &str = " · benchmarks from Epoch AI";

/// Cached data, refreshing if missing or stale. Falls back to stale cache when offline.
/// `auto`: no `--source` was given, so a key that does not work leaves Epoch AI.
pub fn load(force: bool, auto: bool) -> Result<(Data, Option<String>), Failure> {
    match load_cache() {
        Some(d) if !force && !d.stale() => Ok((d, None)),
        cached => {
            // One refresh at a time: agents that find the cache stale together wait for the
            // first and read what it wrote.
            let _held = crate::store::lock(&cache_path(source()));
            if !force && let Some(d) = load_cache().filter(|d| !d.stale()) {
                return Ok((d, None));
            }
            // A refresh can take half a minute, which with nothing said looks like a hang.
            eprintln!("downloading model data, cached for 24h...");
            match refresh(&Steps::default(), None::<fn(Data)>) {
                Ok(mut d) => {
                    let w = d.warning.take();
                    Ok((d, w))
                }
                Err(e @ Failure::BadKey) if auto => {
                    eprintln!("warning: {e}{FELL_BACK}");
                    set_source(Source::Epoch);
                    load(force, false)
                }
                Err(e) => match cached {
                    Some(d) => {
                        let w = format!("refresh failed ({e}); using data {} old", crate::view::age(d.age()));
                        Ok((d, Some(w)))
                    }
                    None => Err(match e {
                        Failure::Other(e) => Failure::Other(format!("could not download model data: {e}")),
                        key => key,
                    }),
                },
            }
        }
    }
}

/// What every download goes through, given up at `limit`.
fn agent(limit: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(limit)).build().into()
}

/// `url`'s body; `key` goes in Artificial Analysis's `x-api-key` header.
fn fetch(url: &str, key: Option<&str>) -> Result<Vec<u8>, Failure> {
    fetch_within(url, key, Duration::from_secs(60))
}

/// `fetch`, given up on after `limit`.
fn fetch_within(url: &str, key: Option<&str>, limit: Duration) -> Result<Vec<u8>, Failure> {
    request(url, key, limit).map_err(|e| match e {
        ureq::Error::StatusCode(401 | 403) if key.is_some() => Failure::BadKey,
        e => format!("{url}: {e}").into(),
    })
}

/// How many times a download is asked for: a site that fails once often answers the next time.
const TRIES: u32 = 3;

/// Whether asking again can help: not for an answer that says the request itself is wrong.
fn passing(e: &ureq::Error) -> bool {
    !matches!(e, ureq::Error::StatusCode(c) if (400..500).contains(c) && !matches!(c, 408 | 429))
}

/// `fetch_within`, with the site's own error for one that reads its status. A failure that can
/// pass is tried again, up to `TRIES` times and all within `limit`.
fn request(url: &str, key: Option<&str>, limit: Duration) -> Result<Vec<u8>, ureq::Error> {
    // One for every download, so a second request to a site goes over the first's connection.
    static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| agent(Duration::from_secs(60)));
    let end = Instant::now() + limit;
    let mut tried = 0;
    loop {
        tried += 1;
        let left = end.saturating_duration_since(Instant::now());
        let mut req = AGENT.get(url).config().timeout_global(Some(left)).build();
        if let Some(k) = key {
            req = req.header("x-api-key", k);
        }
        let pause = Duration::from_millis(500 * u64::from(tried));
        match req.call().and_then(|mut r| r.body_mut().with_config().limit(200 << 20).read_to_vec()) {
            Err(e) if tried < TRIES && passing(&e) && Instant::now() + pause < end => std::thread::sleep(pause),
            res => return res,
        }
    }
}

/// The sitemaps of model pages that Epoch's `index` names: one, and more once Epoch splits it.
fn model_sitemaps(index: &str) -> impl Iterator<Item = &str> {
    let urls = index.split("<loc>").skip(1).filter_map(|s| s.split_once('<').map(|(url, _)| url));
    urls.filter(|url| url.starts_with("https://epoch.ai/sitemap-models-"))
}

/// Epoch's sitemaps of model pages, one after the other. All of them or none: a part of the
/// list would drop the links to the pages it lacks without a word.
fn epoch_pages(index: &str) -> Result<Vec<u8>, Failure> {
    let index = fetch(index, None)?;
    let mut all = Vec::new();
    for url in model_sitemaps(&String::from_utf8_lossy(&index)) {
        all.extend(fetch(url, None)?);
    }
    Ok(all)
}

/// Repos OpenRouter names that Hugging Face has no page for, gone or private, as of 2026-10-05.
// ponytail: kept by hand, as Hugging Face lists no repos to check against and limits requests.
// Ask it for each repo once a refresh if these keep turning up.
const HF_GONE: &[&str] = &[
    "inference-net/schematron-v2-llama-3.2-3b",
    "inference-net/schematron-v2-granite-4.0-h-micro",
    "microsoft/WizardLM-2-8x22B",
];

/// Whether `id` is shaped as a Hugging Face repo, `owner/name`.
fn is_repo(id: &str) -> bool {
    let part = |p: &str| !p.is_empty() && !p.contains(['/', ' ']);
    id.split_once('/').is_some_and(|(owner, name)| part(owner) && part(name))
}

/// Gives each model OpenRouter lists the Hugging Face repo its `list` names for it, when it
/// names one that has a page (`HF_GONE`): `{"data": [{"id": ..., "hugging_face_id": ...}]}`.
/// A model it does not list gets the repo named as the model is, `Qwen/Qwen3.8-Flash-Next` for
/// `qwen38flashnext`. False when it is not that.
fn hf_listed(models: &mut [Model], list: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Rows {
        data: Vec<Row>,
    }
    #[derive(Deserialize)]
    struct Row {
        id: String,
        hugging_face_id: Option<String>,
    }
    let Ok(rows) = serde_json::from_slice::<Rows>(list) else { return false };
    let named = rows
        .data
        .into_iter()
        .filter_map(|r| Some((r.id, r.hugging_face_id.filter(|id| is_repo(id) && !HF_GONE.contains(&id.as_str()))?)));
    let named: HashMap<String, String> = named.collect();
    // A model OpenRouter lists under another name ("Qwen 3.8 Flash") has its repo's too.
    let by_name: HashMap<String, &String> =
        named.values().filter_map(|repo| Some((norm(repo.split_once('/')?.1), repo))).collect();
    for m in models {
        let listed = m.openrouter.as_ref().and_then(|id| named.get(id));
        m.hf = listed.or_else(|| by_name.get(&m.key).copied()).cloned();
    }
    true
}

/// How old Arena's newest leaderboard may be, in months, for its scores to show: one it no
/// longer publishes is not shown as today's.
const ARENA_MAX_AGE: f64 = 2.0;

/// Arena's Agent leaderboard: the day it was published, each model's score by `Model::key`, in
/// percent to a decimal, and whether it has rows that were not read.
#[derive(Default, PartialEq, Debug)]
struct Arena {
    date: String,
    scores: HashMap<String, f64>,
    cut: bool,
    /// Each of `ARENA_MORE` that came with it, by name: its scores, as `scores`.
    more: BTreeMap<&'static str, HashMap<String, f64>>,
}

impl Arena {
    /// From the rows of its dataset, none when they are not that: `{"num_rows_total": 50, "rows":
    /// [{"row": {"model_name": "Claude Opus 5 (High)", "score": 0.0867, "category": "overall",
    /// "leaderboard_publish_date": "2026-10-02"}}]}`. Arena has a row per reasoning setting,
    /// named in brackets: the model gets its best, as with Epoch's benchmarks.
    fn parse(rows: &[u8]) -> Option<Arena> {
        #[derive(Deserialize)]
        struct Rows {
            #[serde(default)]
            num_rows_total: usize,
            rows: Vec<Wrapped>,
        }
        #[derive(Deserialize)]
        struct Wrapped {
            row: Row,
        }
        // Any of them may be null: the row is then left out, not the leaderboard.
        #[derive(Deserialize)]
        struct Row {
            model_name: Option<String>,
            score: Option<f64>,
            category: Option<String>,
            leaderboard_publish_date: Option<String>,
        }
        let all = serde_json::from_slice::<Rows>(rows).ok()?;
        let mut arena = Arena { cut: all.num_rows_total > all.rows.len(), ..Default::default() };
        for r in all.rows.into_iter().map(|w| w.row).filter(|r| r.category.as_deref() == Some("overall")) {
            let (Some(name), Some(score)) = (r.model_name, r.score.filter(|s| s.is_finite())) else { continue };
            // No -0.0, which would show apart from 0.0.
            let pct = (score * 1000.0).round() / 10.0 + 0.0;
            arena.scores.entry(norm(&clean_name(&name))).and_modify(|b| *b = b.max(pct)).or_insert(pct);
            arena.date = arena.date.max(r.leaderboard_publish_date.unwrap_or_default());
        }
        (!arena.scores.is_empty()).then_some(arena)
    }

    /// The one of the last refresh, from its cache, when this refresh got none: one bad download
    /// is not a day without the column.
    fn cached(cache: &[u8]) -> Option<Arena> {
        #[derive(Deserialize)]
        struct Kept {
            format: u32,
            #[serde(default)]
            arena_date: String,
            models: Vec<Row>,
        }
        #[derive(Deserialize)]
        struct Row {
            key: String,
            #[serde(default)]
            arena: Option<f64>,
            #[serde(default)]
            arena_more: BTreeMap<String, f64>,
        }
        let kept = serde_json::from_slice::<Kept>(cache).ok().filter(|d| d.format == FORMAT)?;
        let mut more: BTreeMap<_, HashMap<_, _>> = BTreeMap::new();
        for (name, _) in ARENA_MORE {
            let scored = kept.models.iter().filter_map(|m| Some((m.key.clone(), *m.arena_more.get(name)?)));
            more.insert(name, scored.collect());
        }
        let scores: HashMap<_, _> = kept.models.into_iter().filter_map(|m| Some((m.key, m.arena?))).collect();
        (!scores.is_empty()).then_some(Arena { date: kept.arena_date, scores, more, cut: false })
    }

    /// Whether it was published within `ARENA_MAX_AGE` of `now`, in seconds since 1970.
    fn current(&self, now: u64) -> bool {
        let age = crate::fit::months(&self.date).map(|then| now as f64 / (30.44 * 86400.0) - then);
        age.is_some_and(|a| a <= ARENA_MAX_AGE)
    }

    /// Gives each model its score, none to one Arena does not rank.
    fn score(&self, models: &mut [Model]) {
        for m in models {
            m.arena = self.scores.get(&m.key).copied();
            m.arena_more = self.more.iter().filter_map(|(n, s)| Some((n.to_string(), *s.get(&m.key)?))).collect();
        }
    }
}

/// The most repos Hugging Face is asked for in a refresh: it answers 500 requests in five
/// minutes, and `x` asks it too. The rest are asked for by the next refresh.
const HF_ASKS: usize = 400;

/// How long Hugging Face's "no such repo" is gone by before it is asked again, in seconds.
const HF_NONE_FOR: u64 = 30 * 24 * 3600;

/// What Hugging Face said of each id it was asked for, in lower case: the repo as it spells
/// it, or none, and when it said so, in seconds. Kept in `hf.json`, so an id with a repo is
/// asked for once, and one with none again after `HF_NONE_FOR`.
// ponytail: a repo found is kept for good, so one deleted since stays linked: ask again past
// some age if dead links show.
type Asked = HashMap<String, (Option<String>, u64)>;

/// The repo Hugging Face has under `id`, as it spells it, `Qwen/QwQ-32B` for `qwen/qwq-32b`:
/// Some(None) when it has none, None when it did not say.
fn hf_spelled(id: &str) -> Option<Option<String>> {
    #[derive(Deserialize)]
    struct Repo {
        id: String,
    }
    match request(&format!("https://huggingface.co/api/models/{id}"), None, HF_WAIT) {
        // A page in place of its answer, a proxy's, says nothing of the repo.
        Ok(repo) => Some(Some(serde_json::from_slice::<Repo>(&repo).ok()?.id)),
        // Its answers for a repo it does not have, or keeps private.
        Err(ureq::Error::StatusCode(401 | 404)) => Some(None),
        Err(_) => None,
    }
}

/// What a provider or models.dev calls a model, without the provider's tag: `glm-5.2` of
/// `z-ai/glm-5.2:thinking`, and the id whole. A size is no tag: `deepseek-r1:8b` is another
/// model than `deepseek-r1`.
fn id_name(id: &str) -> (&str, &str) {
    let sized = |tag: &str| tag.starts_with(|c: char| c.is_ascii_digit());
    let id = id.split_once(':').filter(|(_, tag)| !sized(tag)).map_or(id, |(id, _)| id);
    (id, id.rsplit_once('/').map_or(id, |(_, name)| name))
}

/// The ids Hugging Face may have the open model `m` under, in lower case: the ones its
/// providers and models.dev give it, then their names under `org`, where its developer keeps
/// its repos (`Qwen/qwen3.5-2b` of Alibaba's `qwen3.5-2b`).
fn hf_guesses(m: &Model, org: Option<&str>) -> Vec<String> {
    let ids = || m.md.iter().chain(m.offers.iter().map(|o| &o.id)).map(|id| id_name(id));
    let said = ids().map(|(id, _)| id.to_ascii_lowercase());
    let under = ids().filter_map(|(_, name)| Some(format!("{}/{name}", org?).to_ascii_lowercase()));
    let plain = |id: &str| is_repo(id) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b));
    let mut seen = HashSet::new();
    said.chain(under).filter(|id| plain(id) && seen.insert(id.clone())).collect()
}

/// Gives the open models left without a repo (`hf_listed`) the one Hugging Face has under an
/// id of theirs (`hf_guesses`), the one named as they are first, then their developer's own,
/// else the one another model has that is named as they are or as a provider names them
/// (`zai-org/GLM-5.2` for `zhipuai/glm-5.2`).
/// `ask` is `hf_spelled`, asked for at most `asks` ids not in `found`, which it adds to.
// ponytail: a repo under another name than any provider's (`CohereLabs/c4ai-command-r-08-2024`)
// stays unlinked. Search Hugging Face by the developer's org if those are missed.
fn hf_found(
    models: &mut [Model],
    found: &mut Asked,
    asks: &mut usize,
    ask: impl Fn(&str) -> Option<Option<String>> + Sync,
) {
    let org_of = |repo: &str| repo.split_once('/').map(|(org, _)| org.to_string());
    let mut repos: HashMap<(&str, String), usize> = HashMap::new();
    for m in models.iter().filter(|m| !m.developer.is_empty()) {
        // Not `hf_repo`: a copy pulled here is whoever's quantized it.
        if let Some(org) = m.hf.as_deref().and_then(org_of) {
            *repos.entry((&m.developer, org)).or_default() += 1;
        }
    }
    // Developer -> where most of its repos are.
    let mut repos: Vec<_> = repos.into_iter().map(|((dev, org), n)| (n, org, dev.to_string())).collect();
    repos.sort();
    let orgs: HashMap<String, String> = repos.into_iter().map(|(_, org, dev)| (dev, org)).collect();

    let open = |m: &Model| m.open_weights && m.hf_repo().is_none();
    let guesses = |(i, m): (usize, &Model)| (i, hf_guesses(m, orgs.get(&m.developer).map(String::as_str)));
    let guessed: Vec<(usize, Vec<String>)> = models.iter().enumerate().filter(|(_, m)| open(m)).map(guesses).collect();
    let at = now();
    let unasked =
        |id: &&str| found.get(*id).is_none_or(|(repo, was)| repo.is_none() && at.saturating_sub(*was) >= HF_NONE_FOR);
    let new: BTreeSet<&str> = guessed.iter().flat_map(|(_, ids)| ids).map(String::as_str).filter(unasked).collect();
    let new: Vec<&str> = new.into_iter().take(*asks).collect();
    *asks -= new.len();
    // Hugging Face not answering one, down or asked too much, is not asked for the rest.
    let (ask, down) = (&ask, &AtomicBool::new(false));
    let said: Vec<(String, Option<String>)> = std::thread::scope(|s| {
        let some = |ids: &[&str]| {
            let one = |id: &&str| {
                let said = ask(id);
                down.fetch_or(said.is_none(), Relaxed);
                Some((id.to_string(), said?))
            };
            ids.iter().take_while(|_| !down.load(Relaxed)).map_while(one).collect::<Vec<_>>()
        };
        let asking: Vec<_> = new.chunks(new.len().div_ceil(8).max(1)).map(|ids| s.spawn(move || some(ids))).collect();
        asking.into_iter().flat_map(|t| t.join().unwrap_or_default()).collect()
    });
    found.extend(said.into_iter().map(|(id, repo)| (id, (repo, at))));

    for (i, ids) in &guessed {
        let has = || ids.iter().filter_map(|id| found.get(id)?.0.as_ref());
        // One provider calls a distill by the name of the model it is distilled from.
        let named = |repo: &str| norm(id_name(repo).1) == models[*i].key;
        let own = |repo: &str| org_of(repo).as_ref() == orgs.get(&models[*i].developer);
        models[*i].hf = has().min_by_key(|repo| (!named(repo), !own(repo))).cloned();
    }
    let mut by_name: HashMap<String, String> = HashMap::new();
    for repo in models.iter().filter_map(Model::hf_repo) {
        by_name.entry(norm(id_name(repo).1)).or_insert_with(|| repo.to_string());
    }
    for m in models.iter_mut().filter(|m| open(m)) {
        let names = m.md.iter().chain(m.offers.iter().map(|o| &o.id)).map(|id| norm(id_name(id).1));
        m.hf = std::iter::once(m.key.clone()).chain(names).find_map(|name| by_name.get(&name)).cloned();
    }
}

/// "Claude Opus 4.5" -> ["claude", "opus", "4", "5"]: its lowercase alphanumeric runs, a "+"
/// being "plus" as in `norm`.
fn words(s: &str) -> Vec<String> {
    let s = s.replace('+', " plus ");
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// The model pages a site's sitemap lists, by name: `claude-opus-5-5`. None when it is not one.
fn sitemap(xml: &str, site: Source) -> HashSet<&str> {
    xml.split(&format!("<loc>https://{}/models/", site.site()))
        .skip(1)
        .filter_map(|s| s.split_once('<').map(|(p, _)| p))
        .filter(|p| !p.is_empty() && !p.contains('/'))
        .collect()
}

/// The keys of the models that cannot reason. A row naming a reasoning setting of one is a
/// model of its own: "Phi-4-reasoning" is not Phi-4 set to reason.
fn plain(models: &[Model]) -> HashSet<String> {
    models.iter().filter(|m| !m.reasoning).map(|m| m.key.clone()).collect()
}

/// Whether `name` ends in a reasoning setting of a model in `plain`.
fn own_reasoner(name: &str, plain: &HashSet<String>) -> bool {
    let w = words(name);
    let n = setting_at(&w);
    setting(&w[n..]).0 == Some(true) && plain.contains(&w[..n].concat())
}

/// Set each model's `aa` from Artificial Analysis's `pages`: the page named as Epoch or
/// models.dev names the model, else the one with the same words in any order, leaving out the
/// vendor and "instruct": "Claude Sonnet 4.5" is `claude-4-5-sonnet`, "Llama 3.3 70B"
/// `llama-3-3-instruct-70b`. Dates, "preview" and "exp" count, as they name another release:
/// "Gemini 2.5 Pro (Jun 2025)" is not `gemini-2-5-pro`. A reasoning setting need not match:
/// "Claude Haiku 4.5 Thinking" is `claude-4-5-haiku-reasoning`, the page that reasons as the
/// name says, else the model's shortest: "o4 Mini High" is `o4-mini`.
// ponytail: the sitemap misses some pages, so those models get no link rather than a guessed one.
fn aa_pages(models: &mut [Model], pages: &HashSet<&str>) {
    let mut by_key: HashMap<Vec<String>, &str> = HashMap::new();
    // Each model's pages, whatever their setting, the shortest first.
    let mut by_model: HashMap<Vec<String>, Vec<&str>> = HashMap::new();
    for &p in pages {
        let e = by_key.entry(aa_words(p, false)).or_insert(p);
        if (p.len(), p) < (e.len(), *e) {
            *e = p;
        }
        by_model.entry(aa_words(p, true)).or_default().push(p);
    }
    by_model.values_mut().for_each(|ps| ps.sort_by_key(|p| (p.len(), *p)));
    // Whether a name says it reasons. An effort alone does not: "-low" is a page beside the model's.
    let spelled = |s: &str| {
        let w = words(s);
        reasons(&w[setting_at(&w)..])
    };
    let plain = plain(models);
    // The API's own page for a model it scored stands.
    for m in models.iter_mut().filter(|m| m.aa.is_none()) {
        let slug = words(&m.name).join("-");
        let any_setting = || {
            let of_model = by_model.get(&aa_words(&slug, true))?;
            let asked = spelled(&slug);
            let said = of_model.iter().find(|p| asked.is_some() && spelled(p) == asked);
            said.or(of_model.first().filter(|_| !own_reasoner(&m.name, &plain))).copied()
        };
        let page = pages.get(slug.as_str()).or_else(|| by_key.get(&aa_words(&slug, false))).copied();
        m.aa = page.or_else(any_setting).map(String::from);
    }
}

/// A name as Epoch's pages spell it: "Claude Opus 4.5" is `claude-opus-4-5`, and "Command R+"
/// `command-r`, as Epoch drops a "+", a bracket and a letter that is not ASCII.
fn epoch_slug(name: &str) -> String {
    words(&name.chars().filter(|c| c.is_ascii_alphanumeric() || " ._-/".contains(*c)).collect::<String>()).join("-")
}

/// Keep each model's `epoch` only when it is one of Epoch's `pages`, as Epoch has none for
/// some models it scored, and sends a few of those names to another model's page. A model
/// Epoch did not score has the page that is named as it, unless that names another model too:
/// `command-a` is "Command A+", and could be Command A. Without `pages` none is kept, as a
/// name alone may be a page Epoch does not have: `refresh` then gives the last refresh's.
fn epoch_listed(models: &mut [Model], pages: &HashSet<&str>) {
    let mut rows: HashMap<String, usize> = HashMap::new();
    for m in models.iter_mut() {
        m.epoch = m.epoch.take().filter(|p| pages.contains(p.as_str()));
        *rows.entry(m.epoch.clone().unwrap_or_else(|| epoch_slug(&m.name))).or_default() += 1;
    }
    for m in models.iter_mut().filter(|m| m.epoch.is_none()) {
        let slug = epoch_slug(&m.name);
        m.epoch = (pages.contains(slug.as_str()) && rows[&slug] == 1).then_some(slug);
    }
}

/// A name's words as Artificial Analysis's pages are matched: names sorted, then the numbers in
/// the order written, "qwen3" as "qwen 3", the
/// vendor and "instruct" left out. `effort` leaves out the reasoning setting too, as the API
/// lists "claude-opus-5-5-high" and "-non-reasoning" apart from "claude-opus-5-5": only at the
/// end of a name with a version, as "Magistral Medium" is a model and not a setting of Magistral.
// ponytail: a setting before a date ("…-reasoning-04-2025") stays, so that release is its own model.
fn aa_words(slug: &str, effort: bool) -> Vec<String> {
    const SKIP: &[&str] = &["instruct", "hosted", "amazon", "cohere", "nvidia", "anthropic", "google"];
    let mut w = words(slug);
    if effort {
        w.truncate(setting_at(&w));
    }
    let mut k: Vec<String> = w
        .iter()
        .flat_map(|w| match w.find(|c: char| c.is_ascii_digit()) {
            // "qwen3" is "qwen 3", as AA writes it both ways.
            Some(i) if i > 0 && w[..i].bytes().all(|b| b.is_ascii_alphabetic()) => {
                vec![w[..i].to_string(), w[i..].to_string()]
            }
            _ => vec![w.clone()],
        })
        .filter(|w| !SKIP.contains(&w.as_str()))
        .collect();
    names_first(&mut k);
    k
}

/// Names first, sorted; numbers keep their order, as "gpt-4-5" is not "gpt-5-4".
fn names_first(k: &mut [String]) {
    k.sort_by_key(|w| w.starts_with(|c: char| c.is_ascii_digit()));
    let names = k.iter().take_while(|w| !w.starts_with(|c: char| c.is_ascii_digit())).count();
    k[..names].sort();
}

/// Where the reasoning setting a name ends in starts among its words: their count when it ends
/// in none.
fn setting_at(w: &[String]) -> usize {
    let digit = |x: &String| x.bytes().any(|b| b.is_ascii_digit());
    let said = |x: &str| x == "non" || REASONS.contains(&x) || EFFORTS.contains(&x);
    let mut n = w.len();
    while n > 0 && said(&w[n - 1]) && w[..n - 1].iter().any(digit) {
        n -= 1;
    }
    n
}

/// The words of a reasoning setting: that the model reasons, and how hard.
const REASONS: &[&str] = &["reasoning", "thinking", "adaptive"];
const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];

/// A reasoning setting as a name spells it: whether the model reasons, and at which effort.
/// `None` is unsaid.
type Setting = (Option<bool>, Option<String>);

/// Whether `words` say the model reasons: "non" is that it does not. `None` is unsaid.
fn reasons(words: &[String]) -> Option<bool> {
    let said = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    if said(&["non"]) { Some(false) } else { said(REASONS).then_some(true) }
}

/// The setting `words` spell: "non" is no reasoning, and an effort without it is reasoning.
/// "max" is an effort here and not where a name ends (`setting_at`), as "Qwen 3.8 Max" is a model.
fn setting(words: &[String]) -> Setting {
    let effort = words.iter().find(|w| EFFORTS.contains(&w.as_str()) || *w == "max").cloned();
    (reasons(words).or(effort.as_ref().map(|_| true)), effort)
}

/// The setting of one of the API's entries: what its name says in brackets, "Claude Opus 4.6
/// (Non-reasoning, High Effort)", then what its slug and its name end in.
fn aa_setting(slug: &str, name: &str) -> Setting {
    let mut said = words(name.split_once('(').map_or("", |(_, brackets)| brackets));
    for w in [words(slug), words(&clean_name(name))] {
        said.extend_from_slice(&w[setting_at(&w)..]);
    }
    setting(&said)
}

/// The entries among `all`, one model's, that are the setting `name` ends in, as one
/// (`aa_fold`): "Grok 4.20 Non-Reasoning" is not scored as Grok 4.20 reasoning. `None` when the
/// name ends in no setting. One the API did not measure has no scores, and the page of the
/// entries that reason as it does, whatever their effort: "Thinking Low" is not on the
/// non-reasoning page. An entry that does not say counts when none says it, as a model with
/// one setting is listed bare.
fn aa_named(name: &str, all: &[AaEntry]) -> Option<AaEntry> {
    let w = words(name);
    let asked = setting(&w[setting_at(&w)..]);
    if asked == (None, None) {
        return None;
    }
    // 1 when the entry says what is asked, 0 when it does not say, none when it says otherwise.
    fn part<T: PartialEq>(asked: &Option<T>, said: &Option<T>) -> Option<usize> {
        match (asked, said) {
            (Some(a), Some(s)) => (a == s).then_some(1),
            _ => Some(0),
        }
    }
    let says = |e: &AaEntry| Some(part(&asked.0, &e.setting.0)? + part(&asked.1, &e.setting.1)?);
    let most = all.iter().filter_map(says).max();
    let measured = aa_fold(all.iter().filter(|e| most.is_some() && says(e) == most));
    let page = || aa_fold(all.iter().filter(|e| e.setting.0 == asked.0)).map(|e| e.slug).unwrap_or_default();
    Some(measured.unwrap_or_else(|| AaEntry { slug: page(), ..Default::default() }))
}

/// The entries among `all` that came out with the model, the one with the shortest slug: "Kimi
/// K2 Thinking", four months after Kimi K2, is not Kimi K2 set to think.
fn same_release(all: &[AaEntry]) -> impl Iterator<Item = &AaEntry> {
    let model = all.iter().min_by_key(|e| (e.slug.len(), &e.slug));
    all.iter().filter(move |e| model.is_some_and(|m| m.release == e.release))
}

/// The settings of one model as one: the setting with the best index, the first on a tie, all
/// of it, scores and speed, as the best of each is a model no setting is. Among those scored
/// on the most benchmarks: "Gemini 3.7 Flash (Medium)" has the higher index and no
/// Terminal-Bench 4.0, which the index now counts. Its page is that of the setting-less slug,
/// else the shortest.
fn aa_fold<'a>(entries: impl IntoIterator<Item = &'a AaEntry>) -> Option<AaEntry> {
    let entries: Vec<&AaEntry> = entries.into_iter().collect();
    let rank = |e: &AaEntry| (e.scores.len(), e.index.unwrap_or(f64::NEG_INFINITY));
    let best = entries.iter().copied().reduce(|best, e| if rank(e) > rank(best) { e } else { best })?;
    let page = entries.iter().map(|e| &e.slug).min_by_key(|s| (s.len(), *s))?;
    Some(AaEntry { slug: page.clone(), ..best.clone() })
}

/// Lowercase alphanumerics only: "Claude Opus 4.5" == "claude-opus-4-5" == "claude_opus_4.5".
/// A "+" is "plus": "Command R+" == "command-r-plus", and is not Command R.
pub fn norm(s: &str) -> String {
    s.replace('+', "plus").chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// "Anthropic: Claude Opus 4.5 (latest)" -> "Claude Opus 4.5". A trailing "Free" goes too, as
/// "(free)" does: a free tier is an offer of the model, not another model.
fn clean_name(s: &str) -> String {
    let s = s.rsplit_once(": ").map_or(s, |(_, r)| r);
    let mut out = String::new();
    let mut depth: i32 = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = [" free", "-free", ":free"].iter().find_map(|f| {
        let n = out.len().checked_sub(f.len())?;
        (n > 0 && out.is_char_boundary(n) && out[n..].eq_ignore_ascii_case(f)).then_some(n)
    });
    match cut {
        Some(n) => out[..n].to_string(),
        None => out,
    }
}

/// The row an offer goes under: its cleaned name, else its id's. A "contributor" id is Meta's
/// tier that trains on your data, its own model, though OpenCode names one "Muse Spark 1.3 Free".
fn offer_name(id: &str, name: &str) -> String {
    let n = clean_name(if name.is_empty() { id } else { name });
    match id.to_ascii_lowercase().contains("contributor") && !n.to_ascii_lowercase().contains("contributor") {
        true => format!("{n} Contributor"),
        false => n,
    }
}

/// Rows whose offers mostly use the same model id are one model under several names:
/// Cloudflare's "Granite 4.0 H Micro" and OpenRouter's "granite-4.0-micro" are both
/// `granite-4.0-h-micro`. Only each row's most common id counts, as providers mislabel models
/// (Vercel calls `gpt-5.2-pro` "GPT 5.2") and one such offer must not merge two models.
/// The row OpenRouter or Epoch knows (`known`) absorbs the others, else the first by key.
/// So are rows named by the same words in another order, as providers write "Claude 4.7 Opus"
/// for "Claude Opus 4.7".
/// Returns each absorbed key and the key it went into, in the order they went.
// ponytail: ids without a digit ("deepseek-chat", "sonar") are too generic to trust and never merge.
fn merge_same_ids(by_key: &mut HashMap<String, Model>, known: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    // The names sorted, then the numbers as written, those side by side as one: "Grok 4.1 Fast"
    // is not "Grok 4 Fast 1". A name without a digit is too generic, as an id without one.
    // ponytail: where a number sat among the names is lost; compare the positions too if two
    // models ever differ only by that.
    let any_order = |m: &Model| {
        let w = words(&m.name);
        let num = |w: &String| w.starts_with(|c: char| c.is_ascii_digit());
        let mut k: Vec<String> = w.iter().filter(|w| !num(w)).cloned().collect();
        k.sort();
        k.extend(w.chunk_by(|a, b| num(a) && num(b)).filter(|r| num(&r[0])).map(|r| r.join(".")));
        let k = k.join("-");
        k.bytes().any(|b| b.is_ascii_digit()).then_some(k)
    };
    let mut moved = merge_by(by_key, &known, main_id);
    moved.extend(merge_by(by_key, &known, any_order));
    moved
}

/// The id most of a row's offers go by, without its vendor. A clear winner only: a row of
/// several ids, one offer each, has no main id; nor is one without a digit a model's.
fn main_id(m: &Model) -> Option<String> {
    most_sold(m, str::to_string).filter(|main| main.bytes().any(|b| b.is_ascii_digit()))
}

/// The id most of a row's offers go by, each without its vendor and as `id` writes it.
fn most_sold(m: &Model, id: impl Fn(&str) -> String) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for o in &m.offers {
        *counts.entry(id(&slug(&o.id))).or_default() += 1;
    }
    winner(counts)
}

/// The id most of a row's offers go by, as its words: `grok-4.1-fast` and `grok-4-1-fast` are
/// one. A clear winner only, as with `main_id`, though one without a digit counts: it says
/// what is sold, and names no model.
fn sold_as(m: &Model) -> Option<String> {
    most_sold(m, |id| words(id).join("-"))
}

/// Whether a row sold as `id` is another release than the `versions` a source scored: a
/// "-latest", which moves on, or against each of them a number where the version has another
/// as long ("ministral-8b-2512" is not `ministral-8b-2410`, though "mistral-medium-3-5" may be
/// `mistral-medium-2604`). More words on one side alone say nothing: "claude-opus-4-5" is
/// `claude-opus-4-5-20251101`; nor do two numbers one side writes as one, a date being "01-25"
/// or "0125". Nor is one sold as non-reasoning a model scored with reasoning
/// too, whose scores are its best setting's, or one sold as reasoning a model scored without.
fn other_release(id: &str, versions: Option<&BTreeSet<String>>) -> bool {
    let id = words(id);
    if id.last().is_some_and(|w| w == "latest") {
        return true;
    }
    let num = |w: &String| w.bytes().all(|b| b.is_ascii_digit());
    let nums = |w: &[String]| -> HashSet<String> { w.iter().filter(|w| num(w)).cloned().collect() };
    // The numbers of `a` that `b` lacks, but for those `b` writes as one or `a` as two.
    let left = |a: &[String], b: &[String]| -> Vec<String> {
        let pairs = |w: &[String]| -> Vec<Vec<String>> {
            w.windows(2).filter(|p| num(&p[0]) && num(&p[1])).map(<[String]>::to_vec).collect()
        };
        let (has, joins, splits) = (nums(b), pairs(b), pairs(a));
        let one = |n: &String| joins.iter().any(|p| p.concat() == *n);
        let two = |n: &String| splits.iter().any(|p| p.contains(n) && has.contains(&p.concat()));
        nums(a).into_iter().filter(|n| !has.contains(n) && !one(n) && !two(n)).collect()
    };
    let other = |v: &String| {
        let v = words(&slug(v));
        let (mine, its) = (left(&id, &v), left(&v, &id));
        mine.iter().any(|a| its.iter().any(|b| a.len() == b.len()))
    };
    let mut settings = versions.into_iter().flatten().map(|v| reasons(&words(&slug(v))));
    let setting = match reasons(&id) {
        Some(false) => settings.any(|s| s == Some(true)),
        Some(true) => versions.is_some() && settings.all(|s| s == Some(false)),
        None => false,
    };
    setting || versions.is_some_and(|vs| vs.iter().all(other))
}

/// Each row's group in `ep`, and whether the row takes the group's name. By the row's name,
/// its own key then the ones it absorbed (`absorbed`), unless its offers are another release
/// (`other_release`): "Mistral Medium" sold as `mistral-medium-latest` is not the
/// `mistral-medium-2312` Epoch scored. Else by a model id, the row's key or the one its offers
/// go by, when that id and the group say all the row's name does: `deepseek-r1-0528` is
/// "DeepSeek-R1 (May 2025)", but "Qwen 3.6 Plus Uncensored" is not the `qwen-3-6-plus` it is
/// sold as. A group a row has by name is another row's by id only when that row's name or id
/// adds to the group's: "Llama-3.3-70B-Instruct" is "Llama 3.3 70B", and keeps its own name,
/// but "Grok 4.3" is not the "Grok 4.3 Beta" listed beside it.
fn joined<'a>(
    models: &[Model],
    absorbed: &HashMap<String, Vec<String>>,
    ep: &'a Scores,
) -> Vec<Option<(&'a String, bool)>> {
    let keys = |m: &Model| std::iter::once(m.key.clone()).chain(absorbed.get(&m.key).into_iter().flatten().cloned());
    let main: Vec<Option<String>> = models.iter().map(sold_as).collect();
    let fits = |id: &Option<String>, g: &String| !id.as_ref().is_some_and(|id| other_release(id, ep.versions.get(g)));
    let named = |m: &Model, id: &Option<String>| {
        // Artificial Analysis orders a name's words its own way: "Claude 4.5 Sonnet".
        let aa = (ep.source == Source::Aa).then(|| aa_words(&m.name, true).join("-"));
        keys(m).chain(aa).filter_map(|k| ep.group(&k)).find(|g| fits(id, g))
    };
    let named: Vec<Option<&String>> = models.iter().zip(&main).map(|(m, id)| named(m, id)).collect();
    let owned: HashSet<&String> = named.iter().flatten().copied().collect();
    let rows: HashSet<&str> = models.iter().map(|m| m.key.as_str()).collect();
    let by_id = |m: &Model, id: &Option<String>| {
        let group = |g: &String| Some(words(&clean_name(&ep.groups.get(g)?.0)));
        let sold = id.as_ref().and_then(|id| {
            let g = ep.ids.get(&norm(id))?;
            let said = [words(id), group(g)?].concat();
            words(&m.name).iter().all(|w| said.contains(w)).then_some(g)
        });
        let g = keys(m).find_map(|k| ep.ids.get(&k)).or(sold)?;
        let group = group(g)?;
        let adds = |s: &str| {
            let said = words(s);
            group.iter().all(|w| said.contains(w))
        };
        let free = !owned.contains(g);
        // Nor does it take the name of a row that lost the group to its own offers.
        let rename = free && !rows.contains(g.as_str());
        (fits(id, g) && (free || adds(&m.name) || id.as_deref().is_some_and(adds))).then_some((g, rename))
    };
    models
        .iter()
        .zip(&main)
        .zip(named)
        .map(|((m, id), named)| named.map(|g| (g, true)).or_else(|| by_id(m, id)))
        .collect()
}

/// Merges the rows `same` gives the same value into the first of them, the known ones first.
fn merge_by(
    by_key: &mut HashMap<String, Model>,
    known: &impl Fn(&str) -> bool,
    same: impl Fn(&Model) -> Option<String>,
) -> Vec<(String, String)> {
    let mut order: Vec<String> = by_key.keys().cloned().collect();
    order.sort_by_cached_key(|k| (!known(k), k.clone()));
    let mut owner: HashMap<String, String> = HashMap::new(); // value -> key of the row absorbing it
    let mut moved = Vec::new();
    for k in order {
        let Some(main) = same(&by_key[&k]) else { continue };
        let to = owner.entry(main).or_insert_with(|| k.clone()).clone();
        if to != k {
            let m = by_key.remove(&k).unwrap();
            let t = by_key.get_mut(&to).unwrap();
            t.tool_call |= m.tool_call;
            t.reasoning |= m.reasoning;
            t.open_weights |= m.open_weights;
            t.vision |= m.vision;
            if t.knowledge.is_empty() {
                t.knowledge = m.knowledge;
            }
            t.offers.extend(m.offers);
            moved.push((k, to));
        }
    }
    moved
}

/// The entry with more votes than any other: none on a tie for the most.
fn winner<T>(votes: impl IntoIterator<Item = (T, usize)>) -> Option<T> {
    let mut ranked: Vec<(T, usize)> = votes.into_iter().collect();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
    let tie = ranked.len() > 1 && ranked[0].1 == ranked[1].1;
    ranked.into_iter().next().filter(|_| !tie).map(|r| r.0)
}

/// "meta-llama/Llama-4-Maverick" -> "llama-4-maverick": a model id without its vendor.
fn slug(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_lowercase()
}

/// Prefixes that name a vendor or reseller only in model names, not as a provider or an id's
/// namespace (`orgs`): "au.anthropic...", "coding-glm-5.1", "Dola Seed", "Doubao Seed".
const NAME_ORGS: &[&str] = &["au", "coding", "dola", "doubao"];

/// "openaigpt41" -> "gpt41" when "gpt41" exists: providers that prefix the vendor
/// ("OpenAI: GPT-4.1", "Anthropic Claude Opus 4.7") fold into the plain entry. A stealth
/// "Space Bunny Alpha" folds the same way into "Space Bunny" when a provider lists that.
/// Only a prefix in `orgs` folds, so a fine-tune ("Shisa v2 Llama 3.3 70B", "GrayLine Qwen3
/// 8B") keeps its own entry and prices.
fn fold_vendor(key: &str, keys: &HashSet<String>, orgs: &HashSet<String>) -> String {
    let mut k = key;
    const VENDORS: &[&str] = &["openai", "anthropic", "google", "xai", "meta", "mistral"];
    // ponytail: suffix must be ≥5 chars with a digit (or follow a known vendor, for "o3"),
    // a cheap guard against false merges.
    while let Some(r) = (2..k.len()).map(|i| &k[i..]).find(|r| {
        let prefix = &k[..k.len() - r.len()];
        let long = r.len() >= 5 && r.bytes().any(|b| b.is_ascii_digit());
        let vendor = VENDORS.contains(&prefix);
        (long && orgs.contains(prefix) || vendor) && keys.contains(*r)
    }) {
        k = r;
    }
    k.strip_suffix("alpha").filter(|b| keys.contains(*b)).unwrap_or(k).to_string()
}

// ---------- models.dev ----------

#[derive(Deserialize)]
struct MdProvider {
    #[serde(default)]
    name: String,
    /// Ordered, as `merge` keeps the first id and the last name it meets.
    #[serde(default)]
    models: BTreeMap<String, MdModel>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MdModel {
    id: String,
    name: String,
    tool_call: bool,
    reasoning: bool,
    open_weights: bool,
    release_date: String,
    knowledge: String,
    modalities: MdModalities,
    limit: MdLimit,
    cost: Option<MdCost>,
    /// "deprecated" for a retired endpoint, whose id no longer works.
    status: String,
    /// The model's page on models.dev, "zhipuai/glm-5.3", whichever provider offers it.
    canonical_model_id: String,
    /// "auto" or "model-router" for a router.
    family: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MdModalities {
    input: Vec<String>,
    output: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MdLimit {
    context: u64,
    output: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MdCost {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    #[serde(deserialize_with = "tiers")]
    tiers: Vec<MdTier>,
}

/// A model's tiers, or none when they come in another shape: they only adjust a price, and must
/// not fail the whole reply.
fn tiers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MdTier>, D::Error> {
    Ok(serde_json::from_value(serde_json::Value::deserialize(d)?).unwrap_or_default())
}

/// The prices from a context size on: "over 32k tokens, twice as much". One it does not list
/// stays as it was.
#[derive(Deserialize, Default)]
#[serde(default)]
struct MdTier {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    tier: MdTierFrom,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MdTierFrom {
    #[serde(rename = "type")]
    kind: String,
    size: f64,
}

/// The context an agent's long conversation is priced at: a tier that starts by then applies.
/// Most start past 200k tokens, which few sessions reach, and stay out.
// ponytail: one size for every session, and a one-off prompt (`%` off) is priced at it too;
// the tiers kept on the offer and picked when it is priced, if that bites.
const AGENT_CONTEXT: f64 = 100_000.0;

impl MdCost {
    /// (input, output, cache read) at `AGENT_CONTEXT`: of the last tier it reaches, else the
    /// first prices. A tier never costs less, so one priced below them is none. Its cache price
    /// is its own: without one the cache costs as input, not the first prices' cache, unless
    /// the input's price stays too.
    fn agent(&self) -> (f64, f64, Option<f64>) {
        let prices = |t: &MdTier| (t.input.unwrap_or(self.input), t.output.unwrap_or(self.output));
        let reached = |t: &&MdTier| {
            let (input, output) = prices(t);
            t.tier.kind == "context" && t.tier.size <= AGENT_CONTEXT && input >= self.input && output >= self.output
        };
        match self.tiers.iter().filter(reached).max_by(|a, b| a.tier.size.total_cmp(&b.tier.size)) {
            Some(t) => (prices(t).0, prices(t).1, t.cache_read.or(self.cache_read.filter(|_| t.input.is_none()))),
            None => (self.input, self.output, self.cache_read),
        }
    }
}

// ---------- Epoch ----------

/// (display name, overall index, bench -> score)
type Group = (String, Option<f64>, BTreeMap<String, f64>);
type CsvRow = HashMap<String, String>;

/// A benchmark source's models, as `merge` joins them to models.dev's.
#[derive(Default, Debug)]
struct Scores {
    source: Source,
    /// Keyed by norm(name) for Epoch, `aa_words` for Artificial Analysis. Ordered so that merge
    /// results are deterministic.
    groups: BTreeMap<String, Group>,
    /// "Gemini 2.5 Pro (Jun 2025)" is reachable as "gemini25pro"; newest dated version wins.
    alias: HashMap<String, String>,
    /// Epoch's model ids, for a model it names otherwise: `grok-4.3` is "Grok 4.3 Beta". Apart
    /// from `alias`, as an id only finds the scores: it does not choose which row keeps its key.
    ids: HashMap<String, String>,
    /// group -> the model ids Epoch benchmarked, which say what release its scores are of
    /// (`other_release`).
    versions: HashMap<String, BTreeSet<String>>,
    /// group -> organization.
    org: HashMap<String, String>,
    /// Developer (`dev_key`) -> its country, Epoch's only.
    country: HashMap<String, String>,
    /// group -> its best score on `COST_BENCH`, and what a task cost there and the output tokens
    /// it took, when the run says, Epoch's only.
    cost: HashMap<String, (f64, Option<f64>, Option<f64>)>,
    /// group -> task -> percentile, and the value it shows.
    fit: crate::fit::Fit,
    /// group -> its page on artificialanalysis.ai.
    page: HashMap<String, String>,
    /// group -> (tokens/s, time to first answer token) of that page's setting.
    speed: HashMap<String, (Option<f64>, Option<f64>)>,
    /// group -> each of its reasoning settings, which Artificial Analysis lists apart.
    settings: HashMap<String, Vec<AaEntry>>,
    /// What a setting's scores are ranked among: the groups'.
    pools: crate::fit::Pools,
}

impl Scores {
    /// Rank each group's scores among them all (`fit::scored`).
    fn rank(&mut self) {
        self.pools = crate::fit::pools(self.groups.values().map(|(_, i, s)| (*i, s)));
        for (key, (_, i, s)) in &self.groups {
            let (fit, shown) = crate::fit::scored(self.source, *i, s, &self.pools);
            self.fit.0.insert(key.clone(), fit);
            self.fit.1.insert(key.clone(), shown);
        }
    }
}

/// One entry of Artificial Analysis's API: a model at one reasoning setting.
#[derive(Clone, Default, Debug)]
struct AaEntry {
    /// Its page on artificialanalysis.ai.
    slug: String,
    /// When it came out: a "Thinking" entry of another date is a later model, not a setting.
    release: String,
    setting: Setting,
    index: Option<f64>,
    /// Field -> score, 0..1.
    scores: BTreeMap<String, f64>,
    /// (tokens/s, time to first answer token).
    speed: (Option<f64>, Option<f64>),
}

/// Whether `provider` is the developer itself, not a reseller of its models.
fn is_lab(provider: &str, developer: &str) -> bool {
    let dev = norm(developer);
    !dev.is_empty() && norm(&short_org(provider)) == dev
}

/// Short developer names, the same whichever source named them.
fn short_org(s: &str) -> String {
    // "Z.ai (Zhipu AI),Tsinghua University" -> "Z.ai"; "~anthropic" -> "anthropic".
    let s = s.split(',').next().unwrap_or_default();
    let s = s.split('(').next().unwrap_or_default().trim().trim_start_matches('~');
    match s.to_lowercase().as_str() {
        "" => String::new(),
        "openai" => "OpenAI".into(),
        "google" | "google deepmind" => "Google".into(),
        "qwen" | "alibaba" => "Alibaba".into(),
        "meta" | "meta-llama" | "meta ai" => "Meta".into(),
        "mistral" | "mistralai" | "mistral ai" => "Mistral".into(),
        "deepseek" => "DeepSeek".into(),
        "xai" | "x-ai" | "spacexai" => "xAI".into(),
        "z-ai" | "zhipuai" | "thudm" => "Z.ai".into(),
        "bytedance" | "bytedance-seed" => "ByteDance".into(),
        "ibm" | "ibm-granite" => "IBM".into(),
        "xiaomimimo" => "Xiaomi".into(),
        "inclusionai" => "inclusionAI".into(),
        "moonshot" | "moonshotai" | "kimi" => "Moonshot".into(),
        "minimax" => "MiniMax".into(),
        "nvidia" => "NVIDIA".into(),
        _ => {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// A developer's name without what `unify_developers` drops: one key for all its spellings.
fn dev_key(d: &str) -> String {
    let mut k = norm(d);
    while let Some(s) = ["ai", "org", "labs", "corp", "research", "inc"]
        .iter()
        .find_map(|x| k.strip_suffix(x).filter(|s| !s.is_empty()))
    {
        k = s.to_string();
    }
    k
}

/// Epoch's country names, the long ones as they are said.
fn short_country(c: &str) -> &str {
    match c {
        "United States of America" => "USA",
        "United Kingdom" => "UK",
        "United Arab Emirates" => "UAE",
        c => c,
    }
}

/// One spelling per developer: names equal once case, punctuation and a trailing "AI", "org",
/// "Labs", "Corp", "Research" or "Inc" are dropped ("Zai-org" and "Z.ai", "Deepseek-ai" and
/// "DeepSeek") all take the most used one, capitalised ones winning ties.
fn unify_developers(models: &mut [Model]) {
    use dev_key as key;
    let mut counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for m in models.iter().filter(|m| !m.developer.is_empty()) {
        *counts.entry(key(&m.developer)).or_default().entry(m.developer.clone()).or_default() += 1;
    }
    let best: HashMap<String, String> = counts
        .into_iter()
        .map(|(k, names)| {
            let upper = |s: &str| s.chars().filter(char::is_ascii_uppercase).count();
            (k, names.into_iter().max_by_key(|(s, n)| (*n, upper(s))).unwrap().0)
        })
        .collect();
    for m in models.iter_mut().filter(|m| !m.developer.is_empty()) {
        m.developer = best[&key(&m.developer)].clone();
    }
}

/// The developer a model family's name implies, for ids with no vendor prefix.
fn developer_from_name(name: &str) -> &'static str {
    const FAMILIES: &[(&str, &str)] = &[
        ("claude", "Anthropic"),
        ("gemini", "Google"),
        ("gemma", "Google"),
        ("gpt", "OpenAI"),
        ("llama", "Meta"),
        ("musespark", "Meta"),
        ("qwen", "Alibaba"),
        ("deepseek", "DeepSeek"),
        ("grok", "xAI"),
        ("mistral", "Mistral"),
        ("codestral", "Mistral"),
        ("devstral", "Mistral"),
        ("kimi", "Moonshot"),
        ("glm", "Z.ai"),
        ("minimax", "MiniMax"),
        ("nemotron", "NVIDIA"),
        ("mimo", "Xiaomi"),
        ("ling", "inclusionAI"),
        ("ministral", "Mistral"),
        ("pixtral", "Mistral"),
        ("mixtral", "Mistral"),
        ("voxtral", "Mistral"),
        ("leanstral", "Mistral"),
        ("nvidia", "NVIDIA"),
        ("phi", "Microsoft"),
        ("maicode", "Microsoft"),
        ("codex", "OpenAI"),
        ("doubao", "ByteDance"),
        ("ernie", "Baidu"),
        ("hunyuan", "Tencent"),
        ("tencent", "Tencent"),
        ("qvq", "Alibaba"),
        ("nova", "Amazon"),
        ("sonar", "Perplexity"),
        ("perplexity", "Perplexity"),
        ("jamba", "AI21"),
        ("command", "Cohere"),
        ("aya", "Cohere"),
        ("longcat", "Meituan"),
        ("mercury", "Inception Labs"),
        ("meta", "Meta"),
        ("google", "Google"),
    ];
    let n = norm(name);
    FAMILIES.iter().find(|(f, _)| n.starts_with(f)).map_or("", |(_, d)| d)
}

/// The developer a provider's model id implies: the prefix of "google/gemini-2.5-flash",
/// or the provider itself when it sells only its own models.
fn developer_hint(pid: &str, mid: &str) -> String {
    const FIRST_PARTY: &[&str] = &[
        "anthropic",
        "google",
        "openai",
        "xai",
        "mistral",
        "deepseek",
        "cohere",
        "ai21",
        "upstage",
        "perplexity",
        "stepfun",
        "inception",
    ];
    // Cloudflare's "@cf/ibm-granite/granite-4.0" names its own namespace before the vendor.
    let mid = mid.strip_prefix('@').and_then(|m| m.split_once('/')).map_or(mid, |(_, rest)| rest);
    // Bedrock writes "nvidia.nemotron-nano" or, with a region, "us.amazon.nova-premier";
    // SAP AI Core writes "anthropic--claude-4.6-sonnet".
    let vendor = match pid {
        "amazon-bedrock" => {
            let m = ["us.", "eu.", "apac.", "global."].iter().find_map(|r| mid.strip_prefix(r)).unwrap_or(mid);
            m.split_once('.')
        }
        _ => mid.split_once('/').or_else(|| mid.split_once("--")),
    };
    match vendor {
        Some((vendor, _)) => short_org(vendor),
        None if FIRST_PARTY.contains(&pid) => short_org(pid),
        None => String::new(),
    }
}

/// A CSV column, or an error naming it: an upstream schema change must fail the refresh
/// (and leave the cache alone), never panic.
fn col<'a>(r: &'a CsvRow, name: &str) -> Result<&'a str, String> {
    r.get(name).map(String::as_str).ok_or_else(|| format!("epoch csv: missing column '{name}'"))
}

/// None if the file is missing or unreadable.
fn csv_rows(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<CsvRow>> {
    let mut buf = Vec::new();
    zip.by_name(name).ok()?.take(200 << 20).read_to_end(&mut buf).ok()?;
    let mut rdr = csv::Reader::from_reader(buf.as_slice());
    let headers = rdr.headers().ok()?.clone();
    let rows = rdr
        .records()
        .flatten()
        .map(|r| headers.iter().map(String::from).zip(r.iter().map(String::from)).collect())
        .collect();
    Some(rows)
}

impl Scores {
    /// The group a models.dev key joins: its own, or the one it is an alias of.
    fn group(&self, key: &str) -> Option<&String> {
        self.groups.get_key_value(key).map(|(k, _)| k).or_else(|| self.alias.get(key))
    }
}

/// The page Epoch would have for each model it knows (`joined`), whichever source scores it;
/// `epoch_listed` keeps the ones it has.
fn epoch_named(models: &mut [Model], ep: &Scores) {
    let groups = joined(models, &HashMap::new(), ep);
    for (m, g) in models.iter_mut().zip(groups) {
        m.epoch = g.map(|(k, _)| epoch_slug(&ep.groups[k].0));
    }
}

/// The benchmark whose runs say what a task cost, and the columns that say it, in USD and in
/// output tokens: every model is run there on one harness, so the costs compare.
// ponytail: one benchmark, some 30 models; mean a second one in if Epoch lists a cost as wide.
const COST_BENCH: (&str, &str, &str) = ("DeepSWE", "Mean cost (USD)", "Mean output tokens");

fn parse_epoch(bytes: &[u8]) -> Result<Scores, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("epoch zip: {e}"))?;
    let meta = csv_rows(&mut zip, "model_metadata.csv").ok_or("epoch zip: missing model_metadata.csv")?;
    let mut ep = Scores::default();
    let mut dates: HashMap<String, String> = HashMap::new();
    let mut version_group: HashMap<String, String> = HashMap::new();
    // The groups Epoch files the bare id under: `gemini-2.5-flash` is "Gemini 2.5 Flash (Jun 2025)".
    let mut bare: HashSet<String> = HashSet::new();
    for r in &meta {
        let (version, group) = (col(r, "model_version")?, col(r, "model_group")?);
        if version.is_empty() || group.is_empty() {
            continue;
        }
        if norm(&slug(version)) == norm(&clean_name(group)) {
            bare.insert(norm(group));
        }
        dates.insert(norm(group), col(r, "date")?.to_string());
        version_group.insert(version.to_string(), group.to_string());
        if let Some(o) = r.get("organization").filter(|o| !o.is_empty()) {
            ep.org.entry(norm(group)).or_insert_with(|| o.clone());
            // A model of several countries is of several organizations, and does not say which is whose.
            if let Some(c) = r.get("country").filter(|c| !c.is_empty() && !c.contains(',')) {
                // An organization with no name left is no developer: a model with none takes no country.
                for key in o.split(',').map(|org| dev_key(&short_org(org))).filter(|k| !k.is_empty()) {
                    ep.country.entry(key).or_insert_with(|| short_country(c).into());
                }
            }
        }
    }
    let group_of = |version: &str| {
        version_group.get(version).cloned().unwrap_or_else(|| version.split('_').next().unwrap_or(version).to_string())
    };

    let task_benches = crate::fit::task_benches();
    let mut unscored: HashMap<String, BTreeSet<String>> = HashMap::new();
    let benches = csv_rows(&mut zip, "benchmark_metadata.csv").ok_or("epoch zip: missing benchmark_metadata.csv")?;
    for b in &benches {
        let (file, score_col, bench) = (col(b, "source_file")?, col(b, "score_column")?, col(b, "benchmark")?);
        if file.is_empty() || score_col.is_empty() {
            continue;
        }
        // "nan" and "inf" parse as numbers, and are none.
        let num =
            |c: &str, or: f64| b.get(c).and_then(|v| v.parse().ok()).filter(|v: &f64| v.is_finite()).unwrap_or(or);
        col(b, "scale")?;
        let scale = num("scale", 1.0);
        let task = task_benches.contains(&bench);
        let Some(rows) = csv_rows(&mut zip, file) else { continue };
        for r in &rows {
            // A row without a version is no model's: together they would be one that outscores most.
            let (Some(v), Some(s)) = (r.get("Model version").filter(|v| !v.is_empty()), r.get(score_col)) else {
                continue;
            };
            let Some(s) = s.trim_end_matches('%').parse::<f64>().ok().filter(|s| s.is_finite()) else { continue };
            let g = group_of(v);
            // The ids its task scores are of, else those of any score.
            let scored = if task { &mut ep.versions } else { &mut unscored };
            scored.entry(norm(&g)).or_default().insert(v.clone());
            // Any benchmark result may give the model a page on Epoch; only a task's scores it.
            let e = ep.groups.entry(norm(&g)).or_insert_with(|| (g, None, BTreeMap::new()));
            if !task {
                continue;
            }
            let best = e.2.entry(bench.to_string()).or_insert(0.0);
            *best = best.max(s * scale);
            // The cost of the run the score is of, never a lower run's, and the cheaper of two that tie.
            if bench == COST_BENCH.0 {
                let num = |c| r.get(c).and_then(|c| c.parse::<f64>().ok()).filter(|c| c.is_finite() && *c > 0.0);
                let (usd, tokens) = (num(COST_BENCH.1), num(COST_BENCH.2));
                let kept = ep.cost.entry(norm(&e.0)).or_insert((s, usd, tokens));
                if s > kept.0 || s == kept.0 && usd.is_some_and(|u| kept.1.is_none_or(|k| u < k)) {
                    *kept = (s, usd, tokens);
                }
            }
        }
    }
    for (g, versions) in unscored {
        ep.versions.entry(g).or_insert(versions);
    }
    for r in csv_rows(&mut zip, "epoch_capabilities_index/eci_scores.csv").unwrap_or_default() {
        let Some(eci) = col(&r, "eci")?.parse::<f64>().ok().filter(|e| e.is_finite()) else { continue };
        let g = col(&r, "Model")?.to_string();
        dates.entry(norm(&g)).or_insert_with(|| r.get("date").cloned().unwrap_or_default());
        if let Some(o) = r.get("Organization").filter(|o| !o.is_empty()) {
            ep.org.entry(norm(&g)).or_insert_with(|| o.clone());
        }
        ep.groups.entry(norm(&g)).or_insert_with(|| (g, None, BTreeMap::new())).1 = Some(eci);
    }
    let mut newest: HashMap<String, (bool, bool, bool, &str)> = HashMap::new();
    for (k, (name, eci, scores)) in &ep.groups {
        let short = norm(&clean_name(name));
        let date = dates.get(k).map_or("", String::as_str);
        let rank = (eci.is_some(), !scores.is_empty(), bare.contains(k), date);
        // One with an index wins, then one scored on a task, then the one filed under the bare
        // id, then the strictly newer; on ties the first in key order keeps it. A newer one with
        // no index yet is scored on little, and would leave the row out of what is recommended.
        if short != *k && newest.get(&short).is_none_or(|r| rank > *r) {
            newest.insert(short.clone(), rank);
            ep.alias.insert(short, k.clone());
        }
    }
    // A model's id reaches its group too, where the names differ: `grok-4.3_high` is "Grok 4.3
    // Beta" and `gemini-3-pro-preview` "Gemini 3 Pro". The id without a setting first, then in
    // order; one without a digit ("deepseek-chat") is whichever release it points to, and none.
    // The provider before a `/` is not the model's, and what follows a `_` is a setting only at
    // the end, as a word or a budget: `InternVL2_5-78B` is whole.
    let base = |v: &str| {
        let v = slug(v);
        let setting = |s: &str| {
            s.bytes().all(|b| b.is_ascii_lowercase()) || s.strip_suffix('k').is_some_and(|n| n.parse::<u32>().is_ok())
        };
        v.rsplit_once('_').filter(|(_, s)| !s.is_empty() && setting(s)).map_or(v.clone(), |(b, _)| b.to_string())
    };
    // An id that says it is another kind of model than its group is not joined: Epoch files
    // `gpt-5-chat` under "GPT-5" and `Qwen2.5-VL-72B-Instruct` under "Qwen2.5-72B".
    // ponytail: the two words seen; a list of the words an id may add, if more turn up.
    let other =
        |v: &str, g: &str| words(v).iter().any(|w| ["chat", "vl"].contains(&w.as_str()) && !words(g).contains(w));
    let ids = version_group.iter().filter(|(v, g)| !other(v, g) && ep.groups.contains_key(&norm(g)));
    // Of several groups an id reaches, the one `alias` would keep: with an index, then scored
    // on a task, then the newest.
    let rank = |g: &String| {
        let (_, eci, scores) = &ep.groups[g];
        std::cmp::Reverse((eci.is_some(), !scores.is_empty(), dates.get(g).cloned().unwrap_or_default()))
    };
    let mut ids: Vec<_> = ids
        .map(|(v, g)| {
            let g = norm(g);
            (norm(&base(v)), base(v) != slug(v), rank(&g), g)
        })
        .collect();
    ids.sort();
    for (id, _, _, g) in ids {
        if id.bytes().any(|b| b.is_ascii_digit()) {
            ep.ids.entry(id).or_insert(g);
        }
    }
    if ep.groups.values().all(|(_, eci, scores)| eci.is_none() && scores.is_empty()) {
        return Err("epoch zip: no benchmark data found".into());
    }
    ep.rank();
    Ok(ep)
}

// ---------- Artificial Analysis ----------

/// The API's models, the reasoning settings of one model folded into one group, its best
/// (`aa_words`, `aa_fold`), as Epoch's are, and kept apart in `settings` for the
/// rows that name one. Scores are 0..1, as Epoch's; the index stays 0..100.
fn parse_aa(bytes: &[u8]) -> Result<Scores, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("artificial analysis: {e}"))?;
    let list = v["data"].as_array().or(v.as_array()).ok_or("artificial analysis: no model list")?;
    let fields = crate::fit::aa_fields();
    let mut sc = Scores { source: Source::Aa, ..Default::default() };
    let mut names: BTreeMap<String, _> = BTreeMap::new();
    for m in list {
        let (Some(slug), Some(name)) = (m["slug"].as_str(), m["name"].as_str()) else { continue };
        let num = |f: &str| m["evaluations"][f].as_f64().filter(|x| x.is_finite());
        let key = aa_words(slug, true).join("-");
        if key.is_empty() {
            continue;
        }
        sc.groups.entry(key.clone()).or_insert_with(|| (clean_name(name), None, BTreeMap::new()));
        // Its name reaches it too, where the slug says more or less: `step-5` is "Step 5
        // Preview" and `claude-35-sonnet` "Claude 3.5 Sonnet". Of several, the newest release.
        // A setting with a slug of its own ("…-reasoning-0925") is the longer one.
        let named =
            (m["release_date"].as_str().unwrap_or_default(), std::cmp::Reverse((slug.len(), slug)), key.clone());
        let e = names.entry(aa_words(&clean_name(name), true).join("-")).or_insert_with(|| named.clone());
        *e = named.max(e.clone());
        // Benchmarks are 0..1, though one above 1 is a percentage.
        let score = |f: &&str| Some((f.to_string(), num(f).map(|x| if x > 1.0 { x / 100.0 } else { x })?));
        let mut scores: BTreeMap<String, f64> = fields.iter().filter_map(score).collect();
        if let (Some(tb), Some(sci)) = (scores.get("terminalbench_v4_0"), scores.get("scicode")) {
            scores.insert(crate::fit::AA_CODING.into(), (tb + sci) / 2.0);
        }
        let top = |f: &str| m[f].as_f64().filter(|x| x.is_finite() && *x > 0.0);
        sc.settings.entry(key.clone()).or_default().push(AaEntry {
            slug: slug.to_string(),
            release: m["release_date"].as_str().unwrap_or_default().to_string(),
            setting: aa_setting(slug, name),
            index: num(crate::fit::AA_INDEX),
            scores,
            // To the answer, not to the first token: that one is of the thinking, for the
            // models that stream it.
            speed: (top("median_output_tokens_per_second"), top("median_time_to_first_answer_token")),
        });
        if let Some(o) = m["model_creator"]["name"].as_str() {
            sc.org.entry(key).or_insert_with(|| o.to_string());
        }
    }
    for (key, g) in &mut sc.groups {
        let Some(all) = sc.settings.get(key).and_then(|all| aa_fold(same_release(all))) else { continue };
        (g.1, g.2) = (all.index, all.scores);
        sc.page.insert(key.clone(), all.slug);
        if all.speed != (None, None) {
            sc.speed.insert(key.clone(), all.speed);
        }
    }
    sc.groups.retain(|_, (_, i, s)| i.is_some() || !s.is_empty());
    sc.alias =
        names.into_iter().map(|(name, (.., key))| (name, key)).filter(|(_, k)| sc.groups.contains_key(k)).collect();
    if sc.groups.is_empty() {
        return Err("artificial analysis: no benchmark data found".into());
    }
    sc.rank();
    Ok(sc)
}

// ---------- merge ----------

/// `epoch`: Epoch's models when the scores are another source's, as its names decide which of
/// two rows keeps its key (`merge_same_ids`) whatever the source: user.json is keyed by them.
fn merge(models_json: &[u8], ep: &Scores, epoch: Option<&Scores>) -> Result<Data, String> {
    // Ordered, so that two refreshes of the same data merge the same way.
    let providers: BTreeMap<String, MdProvider> =
        serde_json::from_slice(models_json).map_err(|e| format!("models.dev: {e}"))?;
    let mut by_key: HashMap<String, Model> = HashMap::new();
    let mut openrouter: HashMap<String, String> = HashMap::new();
    // OpenRouter ids by their last part: "llama-4-maverick" -> "meta-llama/llama-4-maverick".
    let mut or_slug: HashMap<String, String> = HashMap::new();
    // Developer votes per model, one per offer: resellers prefix ids with their own names
    // ("@cf/meta/...", "novita/..."), so the first offer alone is not to be trusted.
    let mut dev_votes: HashMap<String, HashMap<String, usize>> = HashMap::new();
    // Release dates per model, (provider, date) of each offer: a reseller dates a model by its
    // own listing (Kilo's "stealth" previews, Azure's later launch), so the first or earliest
    // is not it.
    let mut dates: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    // Context and max output per model, (provider, context, output) of each offer.
    let mut limits: HashMap<String, Vec<(&String, u64, u64)>> = HashMap::new();
    // models.dev's page per model, one vote per offer that names one: a reseller's alias and a
    // router's many models are outvoted.
    let mut page_votes: HashMap<String, HashMap<&str, usize>> = HashMap::new();

    let entries: Vec<(&String, &String, &MdModel, String)> = providers
        .iter()
        .flat_map(|(pid, p)| p.models.iter().map(move |(mid, md)| (pid, mid, md)))
        // Text generation models only: skip image/video/embedding endpoints, and retired ones.
        .filter(|(_, _, md)| md.modalities.output.is_empty() || md.modalities.output.iter().any(|o| o == "text"))
        .filter(|(_, _, md)| md.status != "deprecated")
        // Nor routers, which pass each prompt to some model: models.dev's family says so, else
        // the id ("trustedrouter/auto"). Not the name, which would hide a real "XRouter 7B": a
        // router left in has no score, so only a favorite's place recommends it.
        .filter(|(_, mid, md)| {
            !["auto", "model-router"].contains(&md.family.as_str()) && *mid != "auto" && !mid.ends_with("/auto")
        })
        .map(|(pid, mid, md)| (pid, mid, md, offer_name(mid, &md.name)))
        // ponytail: models.dev lists an embedding's output as text, so only its name tells.
        .filter(|e| !["embed", "rerank"].iter().any(|w| e.3.to_lowercase().contains(w)))
        .filter(|e| !norm(&e.3).is_empty())
        .collect();
    let keys: HashSet<String> = entries.iter().map(|e| norm(&e.3)).collect();
    // Providers and the namespaces of ids ("Pro/zai-org/GLM-5": pro, zaiorg).
    let orgs: HashSet<String> = entries
        .iter()
        .flat_map(|e| std::iter::once(e.0.as_str()).chain(e.1.split('/').rev().skip(1)))
        .map(norm)
        .chain(NAME_ORGS.iter().map(|s| s.to_string()))
        .collect();

    for (pid, mid, md, name) in entries {
        let p = &providers[pid];
        let raw = norm(&name);
        let key = fold_vendor(&raw, &keys, &orgs);
        if pid == "openrouter" {
            openrouter.entry(key.clone()).or_insert_with(|| mid.clone());
            // Its own models ("openrouter/<stealth>") are no model's page.
            if !mid.starts_with("openrouter/") {
                or_slug.entry(slug(mid)).or_insert_with(|| mid.clone());
            }
        }
        let m = by_key.entry(key.clone()).or_insert_with(|| Model {
            key: key.clone(),
            name: name.clone(),
            ..Default::default()
        });
        if raw == key {
            m.name = name.clone();
        }
        let hint = developer_hint(pid, mid);
        if !hint.is_empty() {
            *dev_votes.entry(key.clone()).or_default().entry(hint).or_default() += 1;
        }
        limits.entry(key.clone()).or_default().push((pid, md.limit.context, md.limit.output));
        m.tool_call |= md.tool_call;
        m.reasoning |= md.reasoning;
        m.open_weights |= md.open_weights;
        m.vision |= md.modalities.input.iter().any(|i| i == "image");
        if !md.release_date.is_empty() {
            dates.entry(key.clone()).or_default().push((pid, &md.release_date));
        }
        if !md.canonical_model_id.is_empty() {
            *page_votes.entry(key.clone()).or_default().entry(&md.canonical_model_id).or_default() += 1;
        }
        if m.knowledge.is_empty() {
            m.knowledge = md.knowledge.clone();
        }
        let cost = md.cost.as_ref().map(MdCost::agent);
        m.offers.push(Offer {
            provider: pid.clone(),
            provider_name: p.name.clone(),
            id: if md.id.is_empty() { mid.clone() } else { md.id.clone() },
            input: cost.map_or(0.0, |c| c.0),
            output: cost.map_or(0.0, |c| c.1),
            // A few list a paid model's cache as 0, a placeholder; a cache never costs more than input.
            cache_read: cost.and_then(|c| c.2.filter(|&r| r > 0.0).map(|r| r.min(c.0))),
            unpriced: cost.is_none(),
            ..Default::default()
        });
    }

    let names = epoch.unwrap_or(ep);
    let moved = merge_same_ids(&mut by_key, |k| {
        openrouter.contains_key(k) || names.groups.contains_key(k) || names.alias.contains_key(k)
    });
    // The keys each row absorbed: the source may score the model under one of them.
    let mut absorbed: HashMap<String, Vec<String>> = HashMap::new();
    for (from, to) in moved {
        // A row merged into one that is merged in turn takes what it absorbed along.
        let mut keys = absorbed.remove(&from).unwrap_or_default();
        keys.push(from.clone());
        absorbed.entry(to.clone()).or_default().extend(keys);
        if let Some(id) = openrouter.remove(&from) {
            openrouter.entry(to.clone()).or_insert(id);
        }
        for (d, n) in dev_votes.remove(&from).unwrap_or_default() {
            *dev_votes.entry(to.clone()).or_default().entry(d).or_default() += n;
        }
        let moved = dates.remove(&from).unwrap_or_default();
        dates.entry(to.clone()).or_default().extend(moved);
        let moved = limits.remove(&from).unwrap_or_default();
        limits.entry(to.clone()).or_default().extend(moved);
        for (page, n) in page_votes.remove(&from).unwrap_or_default() {
            *page_votes.entry(to.clone()).or_default().entry(page).or_default() += n;
        }
    }

    // A reply in another shape parses as no providers, and must not replace a cache that has them.
    if by_key.is_empty() {
        return Err("models.dev: no models found".into());
    }
    let mut models: Vec<Model> = by_key.into_values().collect();
    let plain = plain(&models);
    let groups = joined(&models, &absorbed, ep);
    // One row takes a group's name, the others that reach it keep their own: the row most
    // providers sell, then the first by key, whatever order this refresh has them in.
    let mut named: HashMap<&String, (std::cmp::Reverse<usize>, String)> = HashMap::new();
    for (m, g) in models.iter().zip(&groups).filter_map(|(m, g)| Some((m, g.filter(|g| g.1)?.0))) {
        let row = (std::cmp::Reverse(m.offers.len()), m.key.clone());
        named.entry(g).and_modify(|n| *n = row.clone().min(n.clone())).or_insert(row);
    }
    for (m, group) in models.iter_mut().zip(groups) {
        // The family in the name is surest; else what most offers' ids say.
        m.developer = match developer_from_name(&m.name) {
            "" => dev_votes.remove(&m.key).and_then(|v| Some(v.into_iter().max_by_key(|(d, n)| (*n, d.clone()))?.0)),
            d => Some(d.into()),
        }
        .unwrap_or_default();
        // Its developer's own date, the earliest when it lists the model twice. Else the one
        // most offers give, a month counting for a day in it, and the earliest on a tie.
        let dates = dates.remove(&m.key).unwrap_or_default();
        let votes = |d: &str| dates.iter().filter(|(_, v)| d.starts_with(v)).count();
        let own = dates.iter().filter(|(p, _)| is_lab(p, &m.developer)).map(|(_, d)| *d).min();
        let most = || dates.iter().map(|(_, d)| *d).max_by_key(|d| (votes(d), std::cmp::Reverse(*d)));
        m.release = own.or_else(most).unwrap_or_default().to_string();
        // Its developer's own limits too, else the ones most offers give and the smaller on a
        // tie: one reseller's claim of more is not what the provider you call takes.
        let limits = limits.remove(&m.key).unwrap_or_default();
        let usual = |get: fn(&(&String, u64, u64)) -> u64| {
            let of = |own: bool| {
                let all = || limits.iter().filter(|l| !own || is_lab(l.0, &m.developer)).map(get).filter(|&n| n > 0);
                all().max_by_key(|&n| (all().filter(|&x| x == n).count(), std::cmp::Reverse(n)))
            };
            of(true).or_else(|| of(false)).unwrap_or_default()
        };
        (m.context, m.max_output) = (usual(|l| l.1), usual(|l| l.2));
        m.md = page_votes.remove(&m.key).and_then(winner).map(String::from);
        // The model's OpenRouter page. Else that of an id its offers go by, when OpenRouter has
        // it: Helicone's "llama-4-maverick" is OpenRouter's, under whatever name Helicone gives
        // it. Not of any id, as one mislabelled offer is not the model.
        let or_id = openrouter.get(&m.key).or_else(|| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for o in &m.offers {
                *counts.entry(slug(&o.id)).or_default() += 1;
            }
            // The commonest id, or one that is it with more or less at its end: a date, a size.
            // ponytail: "-mini" is such an end too; a list of the ends that name a model if it bites.
            let end =
                |long: &str, short: &str| long.strip_prefix(short).is_some_and(|e| e.starts_with(['-', ':', '@']));
            // On a tie the shortest, when the others only add an end to it: "kimi-k3-tee" and
            // "kimi-k3". Ids that differ otherwise, one offer each, name no model.
            let most = counts.values().copied().max()?;
            let mut top = counts.iter().filter(|(_, n)| **n == most).map(|(s, _)| s.as_str());
            let main = top.clone().min_by_key(|s| s.len())?;
            if !top.all(|t| t == main || end(t, main)) {
                return None;
            }
            let near = |s: &str| s == main || end(s, main) || end(main, s);
            // An id without a version is not a row's whose name has one: "mistral-large" is
            // whichever release OpenRouter files under it, not "Mistral Large 2411".
            let digit = |s: &str| s.bytes().any(|b| b.is_ascii_digit());
            let versioned = |s: &str| digit(s) || !digit(&m.name);
            let listed = counts.iter().filter(|(s, _)| near(s) && versioned(s));
            let listed = listed.filter_map(|(s, n)| Some((*n, or_slug.get(s)?)));
            listed.max_by_key(|(n, id)| (*n, std::cmp::Reverse(*id))).map(|(_, id)| id)
        });
        // The standard page rather than the free tier's, when OpenRouter has both.
        let standard = |id: &String| {
            let base = id.strip_suffix(":free")?;
            or_slug.get(&slug(base)).filter(|s| *s == base)
        };
        m.openrouter = or_id.map(|id| standard(id).unwrap_or(id).clone());
        let gk = group.map(|(k, _)| k.as_str());
        // A reasoning model of its own is not the model it is named after, unless Artificial
        // Analysis lists it as that model's reasoning setting.
        let reasons = |k: &&str| ep.settings.get(*k).is_some_and(|all| all.iter().any(|e| e.setting.0 == Some(true)));
        let gk = gk.filter(|k| ep.source != Source::Aa || !own_reasoner(&m.name, &plain) || reasons(k));
        if let Some(k) = gk
            && let Some((gname, eci, scores)) = ep.groups.get(k)
        {
            let (mut eci, mut scores) = (*eci, scores.clone());
            let mut fit = (ep.fit.0.get(k).cloned().unwrap_or_default(), ep.fit.1.get(k).cloned().unwrap_or_default());
            if ep.source == Source::Aa {
                m.aa = ep.page.get(k).cloned();
                (m.tps, m.ttft) = ep.speed.get(k).copied().unwrap_or_default();
                // A row naming a reasoning setting is that setting, not the best of them
                // all; one the API did not measure has the model's page and no scores.
                // Nor is a row that cannot reason its reasoning settings.
                let plain = |all: &Vec<AaEntry>| {
                    let off = same_release(all).filter(|e| e.setting.0 != Some(true));
                    aa_fold(off).filter(|_| !m.reasoning)
                };
                if let Some(own) = ep.settings.get(k).and_then(|all| aa_named(&m.name, all).or_else(|| plain(all))) {
                    fit = crate::fit::scored(Source::Aa, own.index, &own.scores, &ep.pools);
                    (m.tps, m.ttft) = own.speed;
                    (eci, scores) = (own.index, own.scores);
                    if !own.slug.is_empty() {
                        m.aa = Some(own.slug);
                    }
                }
            }
            // Scored on no task, a page is all the source adds; its name for the model may be a bare id.
            if eci.is_none() && scores.is_empty() {
                continue;
            }
            // Epoch's names are the cleaner; Artificial Analysis's put the version first.
            if ep.source == Source::Epoch && group.is_some_and(|g| g.1 && named[g.0].1 == m.key) {
                m.name = gname.clone();
            }
            if let Some(o) = ep.org.get(k) {
                m.developer = short_org(o);
            }
            (m.eci, m.scores) = (eci, scores);
            (m.fit, m.shown) = fit;
            (m.task_cost, m.task_tokens) = ep.cost.get(k).map_or((None, None), |c| (c.1, c.2));
        }
    }
    unify_developers(&mut models);
    let country = epoch.map_or(&ep.country, |e| &e.country);
    for m in &mut models {
        m.country = country.get(&dev_key(&m.developer)).cloned().unwrap_or_default();
    }
    models.sort_by(|a, b| {
        b.eci
            .unwrap_or(0.0)
            .total_cmp(&a.eci.unwrap_or(0.0))
            .then(b.release.cmp(&a.release))
            .then_with(|| a.key.cmp(&b.key))
    });
    let benches = ep.source.benches().into_iter().map(String::from).collect();
    Ok(Data { format: FORMAT, fetched: now(), benches, models, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row has what the other source measures and its index, by its key, or neither.
    #[test]
    fn a_download_that_fails_once_is_asked_for_again() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", server.local_addr().unwrap());
        std::thread::spawn(move || {
            for (stream, reply) in server.incoming().zip(["503 Busy", "200 OK", "404 Not Found", "200 OK"]) {
                let mut stream = stream.unwrap();
                let _ = stream.read(&mut [0; 1024]);
                let _ = write!(stream, "HTTP/1.1 {reply}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
        });
        let s5 = Duration::from_secs(5);
        assert_eq!(request(&url, None, s5).unwrap(), b"ok", "a 503 is tried again");
        assert!(matches!(request(&url, None, s5), Err(ureq::Error::StatusCode(404))), "a 404 is not");
    }

    #[test]
    fn arena_scores_go_to_the_model_at_its_best_setting() {
        let row = |key: &str| Model { key: key.into(), arena: Some(1.0), ..Default::default() };
        let mut ms = [row("claudeopus5"), row("deepseekv4pro"), row("gpt6mini"), row("gpt6"), row("flat")];
        let rows = r#"{"num_rows_total": 7, "rows": [
            {"row": {"model_name": "Claude Opus 5 (High)", "score": 0.0867, "category": "overall", "leaderboard_publish_date": "2026-10-02"}},
            {"row": {"model_name": "Claude Opus 5 (Max)", "score": 0.0792, "category": "overall", "leaderboard_publish_date": "2026-10-02"}},
            {"row": {"model_name": "Claude Opus 5 (Max)", "score": 0.5, "category": "coding"}},
            {"row": {"model_name": "DeepSeek V4 Pro (High) (0813)", "score": -0.0119, "category": "overall"}},
            {"row": {"model_name": "GPT 6 (High) Mini", "score": 0.02, "category": "overall"}},
            {"row": {"model_name": "Flat", "score": -0.0004, "category": "overall"}},
            {"row": {"model_name": "New", "score": null, "category": null}}]}"#;
        let arena = Arena::parse(rows.as_bytes()).unwrap();
        arena.score(&mut ms);
        let got = ms.each_ref().map(|m| m.arena);
        assert_eq!(got, [Some(8.7), Some(-1.2), Some(2.0), None, Some(0.0)], "the best setting, in percent");
        assert!(ms[4].arena.unwrap().is_sign_positive(), "and no -0.0");
        // A signal goes to the models it ranks, by its name.
        let mut signals = Arena { more: [("Steerability", arena.scores.clone())].into(), ..Default::default() };
        signals.more.insert("Praise", Default::default());
        signals.score(&mut ms);
        assert_eq!((ms[0].arena, ms[0].picked("Steerability"), ms[0].picked("Praise")), (None, Some(8.7), None));
        arena.score(&mut ms);
        assert_eq!((arena.date.as_str(), arena.cut), ("2026-10-02", false), "a row with nulls is left out, not all");
        assert!(Arena::parse(rows.replace("7,", "8,").as_bytes()).unwrap().cut, "more rows than were read");
        assert_eq!(Arena::parse(b"<html>"), None);
        // Kept from the cache when it did not come, until Arena has published none for too long.
        let cache = format!(
            r#"{{"format": {FORMAT}, "fetched": 0, "arena_date": "2026-10-02",
            "models": [{{"key": "claudeopus5", "arena": 8.7, "arena_more": {{"Praise": 3.1}}}}, {{"key": "gpt6"}}]}}"#
        );
        let kept = Arena::cached(cache.as_bytes()).unwrap();
        assert_eq!((kept.more["Praise"]["claudeopus5"], kept.more["Steerability"].len()), (3.1, 0), "its signals too");
        assert_eq!((kept.date.as_str(), kept.scores.len(), kept.scores["claudeopus5"]), ("2026-10-02", 1, 8.7));
        assert_eq!(Arena::cached(cache.replace(&FORMAT.to_string(), "1").as_bytes()), None, "not of another format");
        let day = |d: u64| (56 * 365 + 14 + 273 + d) * 86400; // 2026-10-01 and d days
        assert!(kept.current(day(7)) && !kept.current(day(120)) && !Arena::default().current(day(7)));
        // A cache that missed them is tried again within the hour, not the day.
        let aged = |mins: u64, arena_missed| Data {
            format: FORMAT,
            fetched: now() - mins * 60,
            benches: source().benches().into_iter().map(String::from).collect(),
            arena_missed,
            ..Default::default()
        };
        assert!(aged(120, true).stale() && !aged(10, true).stale() && !aged(120, false).stale());
    }

    #[test]
    fn a_row_borrows_the_other_sources_columns() {
        let row = |key: &str| Model { key: key.into(), tps: Some(1.0), task_cost: Some(2.0), ..Default::default() };
        let mut d = Data { models: vec![row("a"), row("b")], ..Default::default() };
        let mut a = Model { key: "a".into(), eci: Some(60.0), tps: Some(90.0), ..Default::default() };
        a.scores.insert("scicode".into(), 0.3);
        let aa = [Lent::from(&a)];
        d.borrow(Source::Epoch, Some(aa.into()));
        assert_eq!((d.models[0].bench("scicode"), d.models[1].bench("scicode")), (Some(0.3), None));
        let got = |d: &Data, i: usize| (d.models[i].other_index, d.models[i].tps, d.models[i].task_cost);
        assert_eq!(got(&d, 0), (Some(60.0), Some(90.0), Some(2.0)), "its own cost stays");
        assert_eq!(got(&d, 1), (None, None, Some(2.0)), "a row it does not have: not what the cache kept");
        assert!(d.lent);
        d.borrow(Source::Epoch, None);
        assert_eq!((got(&d, 0), d.lent), ((None, None, Some(2.0)), false));
        assert_eq!(d.models[0].bench("scicode"), None);
    }

    #[test]
    fn exec_line_quotes_every_argument() {
        let cmd = ["opencode", "--model", "p/it's $HOME"].map(String::from);
        assert_eq!(exec_line(&cmd), r"exec 'opencode' '--model' 'p/it'\''s $HOME'");
    }

    #[test]
    fn table_ids_start_after_the_header() {
        let out = "provider x: token expired\nprovider  model  context\nanthropic  claude-x  1M\n\n\
                   openrouter  a/b:free  8K\npi 1.0 is out: run pi update\n";
        assert_eq!(table_ids(out).unwrap(), ["anthropic/claude-x", "openrouter/a/b:free"], "only the table's rows");
        assert_eq!(table_ids("no models\n"), None, "no header, no ids");
    }

    #[test]
    fn selector_ids_are_read_from_the_json() {
        let out = "omp 19 is out, see {changelog}\n{\"models\": [{\"provider\": \"openai-codex\", \"selector\": \"openai-codex/gpt\"}]}\n";
        assert_eq!(selector_ids(out).unwrap(), ["openai-codex/gpt"]);
        assert_eq!(canonical("omp", "openai-codex/gpt"), "openai/gpt", "the provider as models.dev names it");
        assert_eq!(selector_ids("no models\n"), None);
        assert_eq!(selector_ids("{\"error\": 1}"), None, "no list, no ids");
    }

    #[test]
    fn a_harness_that_does_not_answer_keeps_its_last_listing() {
        let ids = |s: &str| vec![s.to_string()];
        let now = BTreeMap::from([
            ("claude".to_string(), Some(ids("anthropic/*"))),
            ("opencode".to_string(), None),
            ("pi".to_string(), None),
        ]);
        let before =
            || BTreeMap::from([("opencode".to_string(), ids("google/flash")), ("codex".into(), ids("openai/*"))]);
        let (kept, silent) = keep_listed(now, before);
        assert_eq!(kept["claude"], ids("anthropic/*"));
        assert_eq!(kept["opencode"], ids("google/flash"), "what it listed before");
        assert!(!kept.contains_key("pi") && !kept.contains_key("codex"), "never listed, or no longer installed");
        assert_eq!(silent, ["opencode", "pi"]);
        let all = BTreeMap::from([("claude".to_string(), Some(ids("anthropic/*")))]);
        assert_eq!(keep_listed(all, || unreachable!("every harness answered")).1, [""; 0]);
        // Kept once: what the last refresh itself kept is not there to keep again.
        let cache = br#"{"harness": {"opencode": ["google/flash"], "pi": ["openai/gpt"]}, "kept": ["pi"]}"#;
        let was = Listed::of(cache);
        assert_eq!(was.harness(false), BTreeMap::from([("opencode".to_string(), ids("google/flash"))]));
        assert_eq!(was.harness(true).len(), 2, "all of them for one still listing");
    }

    #[test]
    fn copilot_lists_what_its_cli_takes() {
        let session = serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": "s", "models": {
            "currentModelId": "gpt-5-mini",
            "availableModels": [
                {"modelId": "gpt-5-mini", "name": "GPT-5 mini"},
                {"modelId": "claude-haiku-4.5", "_meta": {"copilotEnablement": "enabled"}},
                {"modelId": "claude-opus-5.5", "_meta": {"copilotEnablement": "disabled"}},
                {"modelId": "gpt-6-sol", "_meta": {"copilotEnablement": "unconfigured"}},
            ],
        }}});
        let ids = ["github-copilot/gpt-5-mini", "github-copilot/claude-haiku-4.5"].map(String::from).to_vec();
        assert_eq!(copilot_enabled(&session), Some(ids), "not the ones a policy keeps off");
        let error = serde_json::json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32000, "message": "no"}});
        assert_eq!(copilot_enabled(&error), None, "not a listing");
        let out = serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": "s"}});
        assert_eq!(copilot_enabled(&out), None, "nor is a session without models, as when logged out");
    }

    #[test]
    fn answer_returns_the_response_and_gives_up_on_hangs() {
        let (go, dir, s5) = (&AtomicBool::new(false), std::env::temp_dir(), Duration::from_secs(5));
        let ask =
            |bin: &str, args: &[&str], limit, stop| answer(bin, args, &dir, "{\"id\":1}\n{\"id\":2}\n", 2, limit, stop);
        // It answers once it has read both lines, after a notification and a request of its own.
        let cli = r#"read a; read b; echo '{"id":1,"result":1}'; echo '{"method":"session/update"}'
            echo '{"id":2,"method":"ask"}'; echo "not json"; echo '{"id":2,"result":{"cwd":"'$PWD'"}}'; cat >/dev/null"#;
        let t = Instant::now();
        let cwd = ask("sh", &["-c", cli], s5, go).map(|v| v["result"]["cwd"].as_str().map(PathBuf::from));
        assert_eq!(cwd.flatten().and_then(|p| p.canonicalize().ok()), dir.canonicalize().ok(), "run in its cwd");
        assert!(t.elapsed() < Duration::from_secs(1), "it ends when its input closes, with no wait to kill it");
        assert_eq!(ask("sh", &["-c", "echo '{\"id\":1}'"], s5, go), None, "one that exits without answering");
        assert_eq!(ask("no-such-binary-xyz", &[], s5, go), None, "so does a missing harness");
        assert_eq!(ask("sleep", &["10"], Duration::from_millis(200), go), None, "a hang is killed");
        assert_eq!(ask("sleep", &["10"], s5, &AtomicBool::new(true)), None, "so is a stopped one");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn run_returns_output_and_gives_up_on_hangs() {
        let go = &AtomicBool::new(false);
        assert_eq!(run("sh", &["-c", "echo a/b"], Duration::from_secs(5), go).as_deref(), Some("a/b\n"));
        assert_eq!(run("sh", &["-c", "exit 1"], Duration::from_secs(5), go), None, "a failure reports nothing");
        let big = run("sh", &["-c", "head -c 200000 /dev/zero | tr '\\0' a"], Duration::from_secs(5), go);
        assert_eq!(big.map(|s| s.len()), Some(200_000), "more than a pipe holds");
        assert_eq!(run("no-such-binary-xyz", &[], Duration::from_secs(5), go), None, "so does a missing harness");
        let t = Instant::now();
        assert_eq!(run("sleep", &["10"], Duration::from_millis(200), go), None, "a hang is killed");
        assert_eq!(run("sleep", &["10"], Duration::from_secs(10), &AtomicBool::new(true)), None, "so is a stopped one");
        let left = run("sh", &["-c", "sleep 10 &"], Duration::from_millis(200), go);
        assert_eq!(left, None, "nor is a pipe it left open waited for past the limit");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn names_join() {
        assert_eq!(norm(&clean_name("Anthropic: Claude Opus 4.5 (latest)")), norm("Claude Opus 4.5"));
        assert_eq!(norm("claude-fable-5-1"), norm("Claude Fable 5.1"));
        for free in ["Nemotron 3 Ultra Free", "nemotron-3-ultra-free", "NVIDIA: Nemotron 3 Ultra (free)"] {
            assert_eq!(norm(&clean_name(free)), "nemotron3ultra", "{free}");
        }
        assert_eq!(clean_name("Free"), "Free", "a name is never cut to nothing");
        assert_ne!(norm("GPT-5.5"), norm("GPT-5.5 Pro"));
        assert_eq!(norm("Command R+"), norm("command-r-plus"));
        assert_eq!(words("Command A+"), words("command-a-plus"));
        assert_ne!(aa_words("gpt-4-5", true), aa_words("gpt-5-4", true), "a version is not its digits in any order");
        assert_eq!(offer_name("muse-spark-1.3-contributor-free", "Muse Spark 1.3 Free"), "Muse Spark 1.3 Contributor");
        assert_eq!(
            offer_name("meta/muse-spark-1.3-contributor", "Meta: Muse Spark 1.3 Contributor"),
            "Muse Spark 1.3 Contributor"
        );
        assert_eq!(offer_name("muse-spark-1.3", "Muse Spark 1.3"), "Muse Spark 1.3");
    }

    #[test]
    fn rows_sharing_an_id_merge() {
        let row = |key: &str, ids: &[&str]| {
            let offers = ids.iter().map(|i| Offer { id: i.to_string(), ..Default::default() }).collect();
            (key.to_string(), Model { key: key.into(), offers, ..Default::default() })
        };
        let mut by_key: HashMap<String, Model> = [
            row("granite40hmicro", &["@cf/ibm-granite/granite-4.0-h-micro"]),
            row("granite40micro", &["ibm-granite/granite-4.0-h-micro"]),
            row("autoroute", &["auto"]),
            row("openrouterauto", &["openrouter/auto"]),
            row("gpt52", &["openai/gpt-5.2", "gpt-5.2", "gpt-5.2-pro"]),
            row("gpt52pro", &["openai/gpt-5.2-pro", "gpt-5.2-pro"]),
            row("coherecommanda", &["cohere/command-a-plus", "command-a-reasoning-08-2025"]),
            row("commandareasoning", &["command-a-reasoning-08-2025"]),
        ]
        .into();
        let moved = merge_same_ids(&mut by_key, |k| k == "granite40micro");
        assert_eq!(moved, [("granite40hmicro".to_string(), "granite40micro".to_string())], "the known row absorbs");
        assert_eq!(by_key["granite40micro"].offers.len(), 2);
        assert_eq!(by_key.len(), 7, "no merge on generic ids, one mislabelled offer, or a tie");
    }

    #[test]
    fn a_model_scored_under_an_absorbed_key_keeps_its_scores() {
        // One id, so one row, under the shorter key as OpenRouter knows it; Epoch names the longer.
        let json = br#"{"openrouter": {"models": {"google/gemma-9-it": {"name": "Gemma 9"}}},
                        "b": {"models": {"gemma-9-it": {"name": "Gemma 9 IT"}}}}"#;
        let mut ep = Scores::default();
        ep.groups.insert("gemma9it".into(), ("Gemma 9 IT".into(), Some(140.0), BTreeMap::new()));
        let d = merge(json, &ep, None).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.eci)).collect();
        assert_eq!(rows, [("gemma9", Some(140.0))]);
        assert!(merge(b"{}", &ep, None).is_err(), "no models is a reply in another shape, not data");
        // The same words in another order, under ids of its own: still one row, and scored.
        let json = br#"{"openrouter": {"models": {"x/opus-9": {"name": "Claude Opus 9"}}},
                        "b": {"models": {"opus9-a": {"name": "Claude 9 Opus"}, "opus9-b": {"name": "Claude 9 Opus"}}}}"#;
        ep.groups.insert("claudeopus9".into(), ("Claude Opus 9".into(), Some(150.0), BTreeMap::new()));
        let d = merge(json, &ep, None).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.eci, m.offers.len())).collect();
        assert_eq!(rows, [("claudeopus9", Some(150.0), 3)]);
        // Numbers apart are not numbers side by side, and a name without one merges with none.
        let json = br#"{"b": {"models": {"a": {"name": "Grok 4.1 Fast"}, "b": {"name": "Grok 4 Fast 1"},
                        "c": {"name": "Sonar Pro"}, "d": {"name": "Pro Sonar"}}}}"#;
        assert_eq!(merge(json, &ep, None).unwrap().models.len(), 4);
        // An id reaches a group no row has by name, and only then.
        ep.groups.insert("grok9beta".into(), ("Grok 9 Beta".into(), Some(160.0), BTreeMap::new()));
        ep.ids.insert("grok9".into(), "grok9beta".into());
        let eci = |json: &[u8]| {
            let d = merge(json, &ep, None).unwrap();
            d.models.iter().map(|m| (m.key.clone(), m.eci)).collect::<Vec<_>>()
        };
        assert_eq!(eci(br#"{"b": {"models": {"a": {"name": "Grok 9"}}}}"#), [("grok9".into(), Some(160.0))]);
        let both = eci(br#"{"b": {"models": {"a": {"name": "Grok 9"}, "b": {"name": "Grok 9 Beta"}}}}"#);
        assert!(
            both.contains(&("grok9".into(), None)) && both.contains(&("grok9beta".into(), Some(160.0))),
            "{both:?}"
        );
    }

    #[test]
    fn a_row_is_the_release_its_offers_are() {
        let vs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let other = |id: &str, v: &[&str]| other_release(id, Some(&vs(v)));
        assert!(other("mistral-medium-latest", &["mistral-medium-2312"]), "a -latest moves on");
        assert!(other("ministral-8b-2512", &["ministral-8b-2410"]));
        assert!(other("command-a-reasoning-08-2025", &["c4ai-command-a-03-2025"]));
        assert!(!other("claude-opus-4-5", &["claude-opus-4-5-20251101"]), "one says more, not otherwise");
        assert!(!other("mistral-medium-3-5", &["mistral-medium-2604"]), "a version is not a date");
        assert!(!other("gpt-4o-2024-11-20", &["gpt-4o-2024-08-06", "gpt-4o-2024-11-20"]), "one of them");
        assert!(!other("qwen-max-2025-01-25", &["qwen-max-0125"]), "one date, written two ways");
        assert!(other("gpt-4-1", &["gpt-4-5-2025-04-14"]), "a 1 is not the 14 of a date");
        let both = ["grok-4-1-fast-reasoning", "grok-4-1-fast-non-reasoning"];
        assert!(other("grok-4-1-fast-non-reasoning", &both), "the scores are the reasoning setting's");
        assert!(!other("grok-4-1-fast-reasoning", &both) && !other("grok-4-1-fast", &both));

        let mut ep = Scores::default();
        for (k, name, eci, v) in [
            ("mistralmedium", "Mistral Medium", 120.0, "mistral-medium-2312"),
            ("llama970b", "Llama 9 70B", 127.0, "Llama-9-70B-Instruct"),
            ("r1may2025", "R1 (May 2025)", 141.0, "r1-0528"),
            ("qwen9plus", "Qwen 9 Plus", 147.0, "qwen-9-plus"),
        ] {
            ep.groups.insert(k.into(), (name.into(), Some(eci), BTreeMap::new()));
            ep.versions.insert(k.into(), vs(&[v]));
            ep.ids.insert(norm(v), k.into());
        }
        let json = br#"{"p": {"models": {
            "mistral-medium-latest": {"name": "Mistral Medium"}, "mistral-medium-2312": {"name": "mistral-medium-2312"},
            "x/llama-9-70b-fp8": {"name": "Llama 9 70B"}, "llama-9-70b-instruct": {"name": "Llama-9-70B-Instruct"},
            "r1-0528": {"name": "R1 0528"}, "qwen-9-plus": {"name": "Qwen 9 Plus Uncensored"}}}}"#;
        let d = merge(json, &ep, None).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.name.as_str(), m.eci)).collect();
        let named = [
            ("R1 (May 2025)", Some(141.0)),
            ("Llama 9 70B", Some(127.0)),
            ("Llama-9-70B-Instruct", Some(127.0)),
            ("mistral-medium-2312", Some(120.0)),
            ("Mistral Medium", None),
            ("Qwen 9 Plus Uncensored", None),
        ];
        assert_eq!(rows, named, "by the id sold, under its own name beside a row named as the group, and no fine-tune");

        // Two rows reach a group no row has by name: the one more providers sell takes its name.
        ep.groups.insert("kimi9sep2025".into(), ("Kimi 9 (Sep 2025)".into(), Some(150.0), BTreeMap::new()));
        ep.ids.insert("kimi90905".into(), "kimi9sep2025".into());
        ep.ids.insert("kimi9instruct0905".into(), "kimi9sep2025".into());
        let json = br#"{"p": {"models": {"kimi-9-0905": {"name": "Kimi 9 0905"},
                                         "kimi-9-instruct-0905": {"name": "Kimi-9-Instruct-0905"}}},
                        "q": {"models": {"kimi-9-0905": {"name": "Kimi 9 0905"}}}}"#;
        let d = merge(json, &ep, None).unwrap();
        let mut rows: Vec<_> = d.models.iter().map(|m| (m.name.as_str(), m.eci)).collect();
        rows.sort_by_key(|r| r.0);
        assert_eq!(rows, [("Kimi 9 (Sep 2025)", Some(150.0)), ("Kimi-9-Instruct-0905", Some(150.0))]);

        // A row named as another release than it sells is the group of a key it absorbed.
        ep.groups.insert("mistralmedium9".into(), ("Mistral Medium 9".into(), Some(150.0), BTreeMap::new()));
        ep.versions.insert("mistralmedium9".into(), vs(&["mistral-medium-2609"]));
        let offers = vec![Offer { id: "mistral-medium-2609".into(), ..Default::default() }];
        let row = Model { key: "mistralmedium".into(), name: "Mistral Medium".into(), offers, ..Default::default() };
        let absorbed = HashMap::from([("mistralmedium".to_string(), vec!["mistralmedium9".to_string()])]);
        assert_eq!(joined(&[row], &absorbed, &ep), [Some((&"mistralmedium9".to_string(), true))]);
    }

    #[test]
    fn retired_and_fine_tuned_models_keep_out_of_the_base() {
        let json = br#"{"p": {"models": {
            "gpt-4.1": {"name": "GPT-4.1", "cost": {"input": 2, "output": 8}},
            "old/gpt-4.1": {"name": "GPT-4.1", "status": "deprecated", "cost": {"input": 1, "output": 1}},
            "shisa/shisa-v2-gpt-4.1": {"name": "Shisa v2 GPT-4.1", "cost": {"input": 0.5, "output": 0.5}}
        }}}"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let offers = |k: &str| d.models.iter().find(|m| m.key == k).map(|m| m.offers.len());
        assert_eq!((offers("gpt41"), offers("shisav2gpt41")), (Some(1), Some(1)));
    }

    #[test]
    fn the_release_date_is_the_one_most_offers_give() {
        let json = br#"{
            "a": {"models": {"x-1": {"name": "X 1", "release_date": "2026-04-16"}, "y-1": {"release_date": "2026-02"}}},
            "b": {"models": {"x-1": {"name": "X 1", "release_date": "2026-04-16"}, "y-1": {"release_date": "2026-01"}}},
            "stealth": {"models": {"x-1": {"name": "X 1", "release_date": "2025-08-26"}, "z-1": {}}}
        }"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let date = |k: &str| d.models.iter().find(|m| m.key == k).unwrap().release.as_str();
        assert_eq!(date("x1"), "2026-04-16", "not the earliest, one reseller's own");
        assert_eq!((date("y1"), date("z1")), ("2026-01", ""), "the earliest on a tie, none when no offer has one");
        let json = br#"{
            "openai": {"models": {"gpt-9": {"release_date": "2026-04-16"}}},
            "stealth": {"models": {"gpt-9": {"release_date": "2025-08-26"}, "w-1": {"release_date": "2026-01-05"}}},
            "a": {"models": {"gpt-9": {"release_date": "2025-08-26"}, "w-1": {"release_date": "2026-02"}}},
            "b": {"models": {"w-1": {"release_date": "2026-02-10"}}}
        }"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let date = |k: &str| d.models.iter().find(|m| m.key == k).unwrap().release.as_str();
        assert_eq!(date("gpt9"), "2026-04-16", "its developer's own, whatever the resellers say");
        assert_eq!(date("w1"), "2026-02-10", "a month counts for a day in it");
    }

    #[test]
    fn the_context_is_the_one_most_offers_give() {
        let json = br#"{
            "a": {"models": {"x-1": {"limit": {"context": 400000, "output": 128000}}, "y-1": {"limit": {"context": 200000}}}},
            "b": {"models": {"x-1": {"limit": {"context": 400000, "output": 128000}}, "y-1": {"limit": {"context": 1000000}}}},
            "big": {"models": {"x-1": {"limit": {"context": 1047576, "output": 1047576}}, "gpt-9": {"limit": {"context": 1000000}}}},
            "c": {"models": {"gpt-9": {"limit": {"context": 1000000}}}},
            "openai": {"models": {"gpt-9": {"limit": {"context": 400000}}}}
        }"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let of = |k: &str| d.models.iter().find(|m| m.key == k).map(|m| (m.context, m.max_output)).unwrap();
        assert_eq!(of("x1"), (400_000, 128_000), "not the most one reseller claims");
        assert_eq!(of("y1"), (200_000, 0), "the smaller on a tie, none when no offer has one");
        assert_eq!(of("gpt9").0, 400_000, "its developer's own, whatever the resellers say");
    }

    #[test]
    fn folds_vendor_prefix() {
        let keys: HashSet<String> =
            ["gpt41", "openaigpt41", "prozaiorgglm5", "zaiorgglm5", "glm5x", "o3", "openaio3", "spacebunny"]
                .map(String::from)
                .into();
        let orgs: HashSet<String> = ["pro", "zaiorg"].map(String::from).into();
        let fold = |k| fold_vendor(k, &keys, &orgs);
        assert_eq!(fold("openaigpt41"), "gpt41");
        assert_eq!(fold("prozaiorgglm5"), "zaiorgglm5");
        assert_eq!(fold("gpt41"), "gpt41");
        assert_eq!(fold("openaio3"), "o3");
        assert_eq!(fold("spacebunnyalpha"), "spacebunny");
        assert_eq!(fold("oxalpha"), "oxalpha", "no plain entry, no fold");
        assert_eq!(fold("shisav2gpt41"), "shisav2gpt41", "a fine-tune is not its base");
    }

    #[test]
    fn links_to_each_site_that_has_the_model() {
        let mut m = Model { openrouter: Some("anthropic/claude-opus-5.5".into()), ..Default::default() };
        let sites = |m: &Model| m.links().into_iter().map(|(s, _)| s).collect::<Vec<_>>();
        assert_eq!(sites(&m), ["openrouter.ai"], "only the sites that have it");
        m.md = Some("anthropic/claude-opus-5-5".into());
        m.epoch = Some("claude-opus-5-5".into());
        m.aa = Some("claude-opus-5-5".into());
        assert_eq!(
            m.links(),
            [
                ("models.dev", "https://models.dev/models/anthropic/claude-opus-5-5/".into()),
                ("epoch.ai", "https://epoch.ai/models/claude-opus-5-5".into()),
                ("artificialanalysis.ai", "https://artificialanalysis.ai/models/claude-opus-5-5".into()),
                ("openrouter.ai", "https://openrouter.ai/anthropic/claude-opus-5.5".into()),
            ]
        );
        m.openrouter = None;
        assert_eq!(sites(&m).len(), 3, "no search on OpenRouter for a model it does not list");
        assert!(Model::default().links().is_empty(), "no site has it: no link");
        let ranked = Model { arena: Some(8.7), hf: Some("a/b".into()), ..Default::default() };
        let arena = ("arena.ai", "https://arena.ai/leaderboard/agent".to_string());
        assert_eq!(ranked.links().last(), Some(&arena), "Arena's leaderboard last, for a model it ranks");
        let local = |provider: &str, id: &str| Model {
            offers: vec![Offer { provider: provider.into(), id: id.into(), local: true, ..Default::default() }],
            ..Default::default()
        };
        let hf = [("huggingface.co", "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF".to_string())];
        assert_eq!(local(LLAMA, "unsloth/Qwen3.5-4B-GGUF:Q4_K_M").links(), hf, "the repo llama.cpp pulled");
        assert_eq!(local(OLLAMA, "hf.co/unsloth/Qwen3.5-4B-GGUF:Q4_K_M").links(), hf, "and ollama");
        for (provider, id) in [(OLLAMA, "qwen3.5:4b"), (OLLAMA, "someone/qwen3.5:4b"), (LLAMA, "models/qwen.gguf")] {
            assert!(local(provider, id).links().is_empty(), "{id} names no repo");
        }
        let mut own = local(LLAMA, "models/gemma-3-4b-it-Q4_K_M.gguf");
        own.offers.push(Offer { provider: HF.into(), id: "google/gemma-3-4b-it".into(), ..Default::default() });
        let served = [("huggingface.co", "https://huggingface.co/google/gemma-3-4b-it".to_string())];
        assert_eq!(own.links(), served, "else the repo Hugging Face serves it from");
        own.offers.push(Offer {
            provider: OLLAMA.into(),
            id: "hf.co/unsloth/Qwen3.5-4B-GGUF".into(),
            local: true,
            ..Default::default()
        });
        assert_eq!(own.links(), hf, "the copy you have first");
        let mut ms = [
            Model { openrouter: Some("moonshotai/kimi-k2".into()), ..Default::default() },
            Model { openrouter: Some("openai/gpt-5.5".into()), hf: Some("stale/repo".into()), ..Default::default() },
            Model { key: "kimik2instruct".into(), ..Default::default() },
            Model { openrouter: Some("a/b".into()), ..Default::default() },
            Model { openrouter: Some("a/c".into()), ..Default::default() },
            Model { openrouter: Some("a/d".into()), ..Default::default() },
        ];
        let list = br#"{"data": [{"id": "moonshotai/kimi-k2", "hugging_face_id": "moonshotai/Kimi-K2-Instruct"},
            {"id": "openai/gpt-5.5", "hugging_face_id": ""}, {"id": "x/y", "hugging_face_id": null}, {"id": "x/z"},
            {"id": "a/b", "hugging_face_id": "not a repo"}, {"id": "a/c", "hugging_face_id": "a/b/c"},
            {"id": "a/d", "hugging_face_id": "microsoft/WizardLM-2-8x22B"}]}"#;
        assert!(hf_listed(&mut ms, list) && !hf_listed(&mut [], b"<html>"), "only a list of models is one");
        assert_eq!(
            ms.each_ref().map(|m| m.hf.as_deref()),
            [Some("moonshotai/Kimi-K2-Instruct"), None, Some("moonshotai/Kimi-K2-Instruct"), None, None, None],
            "the third by the repo's name, OpenRouter not listing it"
        );
        // Hugging Face is asked for the ids of the open models still without one, once each.
        let open = |dev: &str, key: &str, ids: &[&str]| Model {
            key: key.into(),
            developer: dev.into(),
            open_weights: true,
            offers: ids.iter().map(|id| Offer { id: id.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        };
        let mut found =
            Asked::from([("stale/qwq-32b".to_string(), (None, 0)), ("fresh/qwq-32b".to_string(), (None, now()))]);
        let (asked, up) = (Mutex::new(Vec::new()), AtomicBool::new(true));
        let ask = |id: &str| {
            asked.lock().unwrap().push(id.to_string());
            let has = ["Qwen/QwQ-32B", "cortecs/QwQ-32B", "zai-org/GLM-5.2"];
            up.load(Relaxed).then(|| has.into_iter().find(|r| r.eq_ignore_ascii_case(id)).map(String::from))
        };
        let mut asked_for = [
            Model { hf: Some("Qwen/Qwen3-32B".into()), developer: "Alibaba".into(), ..Default::default() },
            open("Alibaba", "qwq32b", &["cortecs/qwq-32b", "qwq-32b:free", "~odd", "stale/qwq-32b", "fresh/qwq-32b"]),
            open("Z.ai", "glm52", &["zai-org/GLM-5.2"]),
            open("Z.ai", "glm52fast", &["z-ai/glm-5.2:fast"]),
            open("Alibaba", "qwenmax", &["qwen/max", "qwen3:8b"]),
            Model {
                key: "closed".into(),
                offers: vec![Offer { id: "zai-org/glm-5.2".into(), ..Default::default() }],
                ..Default::default()
            },
        ];
        let mut asks = 9;
        hf_found(&mut asked_for, &mut found, &mut asks, ask);
        assert_eq!(
            asked_for.each_ref().map(|m| m.hf.as_deref()),
            [
                Some("Qwen/Qwen3-32B"),
                Some("Qwen/QwQ-32B"),
                Some("zai-org/GLM-5.2"),
                Some("zai-org/GLM-5.2"),
                None,
                None
            ],
            "its developer's repo over a provider's copy, and the fourth by the name its provider gives it"
        );
        let mut ids = std::mem::take(&mut *asked.lock().unwrap());
        ids.sort();
        let all = ["cortecs/qwq-32b", "qwen/max", "qwen/qwq-32b", "stale/qwq-32b", "z-ai/glm-5.2", "zai-org/glm-5.2"];
        assert_eq!((ids, asks), (all.map(String::from).to_vec(), 3), "the ids shaped as a repo, not the fresh none");
        // Hugging Face down says nothing of an id, which the next refresh asks for again.
        found.remove("qwen/max");
        up.store(false, Relaxed);
        hf_found(&mut asked_for, &mut found, &mut 9, ask);
        assert_eq!(*asked.lock().unwrap(), ["qwen/max"], "the others are asked for once");
        assert!(!found.contains_key("qwen/max") && found["z-ai/glm-5.2"].0.is_none());
        let named = ("huggingface.co", "https://huggingface.co/moonshotai/Kimi-K2-Instruct".to_string());
        assert_eq!(ms[0].links().last(), Some(&named), "the repo OpenRouter names, where no offer names one");
        let paid = |p: &str, price: f64| Offer {
            provider: p.into(),
            available: true,
            input: price,
            output: price,
            ..Default::default()
        };
        m.offers = vec![paid("anthropic", 5.0), paid("302ai", 1.0)];
        let page = "https://models.dev/models/anthropic/claude-opus-5-5/";
        assert_eq!(m.price_page().as_deref(), Some(page), "the model's page lists every provider's price");
        let offers = std::mem::take(&mut m.offers);
        assert_eq!(m.price_page().as_deref(), Some(page), "with no offer to pay too");
        m.offers = offers;
        m.md = None;
        let page = "https://models.dev/providers/302ai/";
        assert_eq!(m.price_page().as_deref(), Some(page), "without one, the page of the provider you'd pay");
    }

    #[test]
    fn links_are_what_most_offers_name() {
        let json = br#"{
            "zai": {"models": {"glm-5.3": {"name": "GLM-5.3", "canonical_model_id": "zhipuai/glm-5.3"}}},
            "a": {"models": {
                "z/glm-5.3": {"name": "GLM-5.3", "canonical_model_id": "zhipuai/glm-5.3"},
                "coding-router": {"name": "Coding Router", "canonical_model_id": "x/one"},
                "qwen3.8-max": {"name": "Qwen 3.8 Max"},
                "gemma-4-it": {"name": "Gemma 4"},
                "sonar": {"name": "Sonar"},
                "bigstral-large": {"name": "Bigstral Large 2411"},
                "llama-9-fp8": {"name": "Llama 9 FP8"},
                "phi-9-31b": {"name": "Phi 9"},
                "mini-7.5": {"name": "Mini 7.5"}
            }},
            "b": {"models": {
                "glm-latest": {"name": "GLM-5.3", "canonical_model_id": "zai/glm-latest"},
                "coding-router": {"name": "Coding Router", "canonical_model_id": "x/two"},
                "qwen3.8-max": {"name": "Qwen 3.8 Max"},
                "llama-9-fp8": {"name": "Llama 9 FP8"},
                "phi-9": {"name": "Phi 9"},
                "mini-7.5": {"name": "Mini 7.5"}
            }},
            "c": {"models": {
                "Qwen/Qwen3.8-2.4T": {"name": "Qwen 3.8 Max"},
                "llama-9": {"name": "Llama 9 FP8"},
                "phi9": {"name": "Phi 9"},
                "mini-7": {"name": "Mini 7.5"},
                "kimi-k9-tee": {"name": "Kimi K9 TEE"}
            }},
            "d": {"models": {"kimi-k9": {"name": "Kimi K9 TEE"}}},
            "openrouter": {"models": {
                "qwen/qwen3.8-2.4t": {"name": "Qwen3.8 2.4T"},
                "microsoft/phi-9-31b": {"name": "Phi 9 31B"},
                "x/mini-7": {"name": "Mini 7"},
                "moonshotai/kimi-k9": {"name": "Kimi K9"},
                "google/gemma-4-it:free": {"name": "Gemma 4 (free)"},
                "google/gemma-4-it": {"name": "Gemma 4 IT"},
                "perplexity/sonar": {"name": "Perplexity Sonar"},
                "x/bigstral-large": {"name": "Bigstral Large"},
                "meta-llama/llama-9": {"name": "Llama 9"},
                "openrouter/auto": {"name": "Auto Router"}
            }},
            "kilo": {"models": {
                "kilo-auto/small": {"name": "Auto Small", "family": "auto"},
                "brick": {"name": "Brick v1", "family": "model-router"},
                "z-ai/autoglm-9b": {"name": "AutoGLM 9B"},
                "x/auto-coder-2": {"name": "Auto-Coder 2"},
                "x/auto": {"name": "Auto"},
                "nanogpt/coding-router": {"name": "Coding Router"},
                "x/xrouter-7b": {"name": "XRouter 7B"}
            }}
        }"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let row = |k: &str| d.models.iter().find(|m| m.key == k).unwrap();
        assert_eq!(row("glm53").md.as_deref(), Some("zhipuai/glm-5.3"), "the page, not provider/id nor an alias");
        let routers = ["autorouter", "autosmall", "brickv1", "auto"].map(|k| d.models.iter().any(|m| m.key == k));
        assert_eq!(routers, [false; 4], "a router is no model");
        let models =
            ["autoglm9b", "autocoder2", "xrouter7b", "codingrouter"].map(|k| d.models.iter().any(|m| m.key == k));
        assert_eq!(models, [true; 4], "but a model named Auto... is, and the name alone never hides one");
        assert_eq!(row("codingrouter").md, None, "a router left in names a page per offer: none is its");
        assert_eq!(row("codingrouter").openrouter, None, "nor is another model's OpenRouter id");
        assert_eq!(row("qwen38max").openrouter, None, "one mislabelled offer of three is not the model");
        assert_eq!(row("llama9fp8").openrouter.as_deref(), Some("meta-llama/llama-9"), "but the id cut short is");
        assert_eq!(row("phi9").openrouter, None, "an id per offer: none is the row's");
        assert_eq!(row("kimik9tee").openrouter.as_deref(), Some("moonshotai/kimi-k9"), "unless they differ by an end");
        assert_eq!(row("mini75").openrouter, None, "cut inside a version, it is another model");
        assert_eq!(row("gemma4").openrouter.as_deref(), Some("google/gemma-4-it"), "the standard page over :free");
        assert_eq!(row("sonar").openrouter.as_deref(), Some("perplexity/sonar"), "an id with no digit is one too");
        assert_eq!(row("bigstrallarge2411").openrouter, None, "but not a row's whose name has a version");
    }

    #[test]
    fn aa_pages_match_names_in_any_order() {
        let pages = [
            "claude-4-5-sonnet",
            "claude-4-5-sonnet-thinking",
            "claude-opus-4-5",
            "claude-opus-4-5-thinking",
            "llama-3-3-instruct-70b",
            "gemini-2-5-pro",
            "qwen3-8-max",
            "qwen3-8-max-0803",
            "deepseek-v3-2",
            "claude-4-5-haiku",
            "claude-4-5-haiku-reasoning",
            "grok-3-mini-reasoning",
            "o4-mini",
            "phi-4",
            "nvidia-nemotron-3-nano-30b",
            "gpt-5",
            "gpt-5-low",
        ];
        let xml: String =
            pages.iter().map(|p| format!("<url><loc>https://artificialanalysis.ai/models/{p}</loc></url>")).collect();
        let other =
            "<url><loc>https://artificialanalysis.ai/models/comparisons/a-vs-b</loc></url><loc>https://x/y</loc>";
        let xml = xml + other;
        let pages = sitemap(&xml, Source::Aa);
        assert_eq!(pages.len(), 17, "the model pages alone");
        assert!(sitemap("<html>not found</html>", Source::Aa).is_empty());
        let index = "<sitemap><loc>https://epoch.ai/sitemap-data-0.xml</loc></sitemap>\
                     <sitemap><loc>https://epoch.ai/sitemap-models-0.xml</loc></sitemap>\
                     <sitemap><loc>https://epoch.ai/sitemap-models-1.xml</loc></sitemap>\
                     <sitemap><loc>https://else.where/sitemap-models-2.xml</loc></sitemap>";
        assert_eq!(
            model_sitemaps(index).collect::<Vec<_>>(),
            ["https://epoch.ai/sitemap-models-0.xml", "https://epoch.ai/sitemap-models-1.xml"],
            "every sitemap of model pages, and only Epoch's own"
        );
        let names = [
            ("Claude Sonnet 4.5", Some("claude-4-5-sonnet")),
            ("Claude Opus 4.5", Some("claude-opus-4-5")),
            ("Llama 3.3 70B", Some("llama-3-3-instruct-70b")),
            ("Gemini 2.5 Pro (Jun 2025)", None),
            ("Qwen 3.8 Max", Some("qwen3-8-max")),
            ("Qwen 3.8 Max 0803", Some("qwen3-8-max-0803")),
            ("DeepSeek V3.2 Exp", None),
            ("Claude Sonnet 4", None),
            ("Claude Haiku 4.5 Thinking", Some("claude-4-5-haiku-reasoning")),
            ("Claude Sonnet 4.5 Thinking", Some("claude-4-5-sonnet-thinking")),
            ("Grok-3 mini", Some("grok-3-mini-reasoning")),
            ("o4 Mini High", Some("o4-mini")),
            ("Nemotron 3 Nano 30B", Some("nvidia-nemotron-3-nano-30b")),
            ("Phi-4", Some("phi-4")),
            ("Phi-4-reasoning", None),
            ("GPT-5 Thinking", Some("gpt-5")),
        ];
        let mut models: Vec<Model> =
            names.map(|(n, _)| Model { key: norm(n), name: n.into(), ..Default::default() }).into();
        aa_pages(&mut models, &pages);
        assert_eq!(
            models.iter().map(|m| m.aa.as_deref()).collect::<Vec<_>>(),
            names.map(|(_, page)| page),
            "the page with the same words, dates and all, at the setting named, else the model's; \
             none rather than another release's or, for a reasoning model of its own, its base's"
        );
    }

    #[test]
    fn find_prefers_shortest() {
        let mk = |k: &str| Model { key: k.into(), ..Default::default() };
        let granite = Model {
            offers: vec![Offer {
                provider: "openrouter".into(),
                id: "ibm-granite/granite-4.0-h-micro".into(),
                ..Default::default()
            }],
            ..mk("granite40micro")
        };
        let d = Data {
            fetched: 0,
            models: vec![mk("claudeopus45thinking"), mk("claudeopus45"), mk("gpt55"), granite],
            ..Default::default()
        };
        assert_eq!(d.find("opus-4.5").unwrap().key, "claudeopus45");
        assert_eq!(d.find("GPT 5.5").unwrap().key, "gpt55");
        assert_eq!(d.find("granite-4.0-h-micro").unwrap().key, "granite40micro", "by a provider's id");
        assert_eq!(
            d.find("openrouter/ibm-granite/granite-4.0-h-micro").unwrap().key,
            "granite40micro",
            "as list --id prints"
        );
        assert!(d.find("nope").unwrap_err().is_empty());
        let d5 = Data { models: vec![mk("claudesonnet55"), mk("claudesonnet5")], ..Default::default() };
        assert_eq!(d5.find("sonnet-5").unwrap().key, "claudesonnet5");
        let d5 = Data { models: vec![mk("claudesonnet55")], ..Default::default() };
        assert!(d5.find("sonnet-5").is_err(), "a version is whole: 5 is not 5.5");
        assert_eq!(d5.find("sonnet").unwrap().key, "claudesonnet55");
        assert!(d.find("--").unwrap_err().is_empty(), "nothing to match by is no match, not every model");
        let gpt = Model {
            name: "GPT-5.5 (Apr 2026)".into(),
            offers: vec![Offer { provider: "openai".into(), id: "gpt-5.5".into(), ..Default::default() }],
            ..mk("gpt55")
        };
        let d = Data { fetched: 0, models: vec![gpt], ..Default::default() };
        assert_eq!(d.find("GPT-5.5 (Apr 2026)").unwrap().key, "gpt55", "by the name shown, longer than the key");
        assert_eq!(d.find("openai-codex/gpt-5.5").unwrap().key, "gpt55", "as pi names the provider, and pick prints");
        let mine = |k: &str| Model { available: true, ..mk(k) };
        let d = Data {
            models: vec![
                mk("claudesonnet4"),
                mk("claudesonnet45"),
                mine("claudesonnet5"),
                mine("claudesonnet5thinking"),
            ],
            ..Default::default()
        };
        assert_eq!(d.find("sonnet").unwrap().key, "claudesonnet5", "yours first, then the shortest");
        assert_eq!(d.find("sonnet4").unwrap().key, "claudesonnet4", "all models when none of yours match");
        assert_eq!(d.find("claudesonnet45").unwrap().key, "claudesonnet45", "an exact key wins");
        let d = Data { models: vec![mine("claudeopus5"), mine("claudefable5")], ..Default::default() };
        assert_eq!(d.find("claude").unwrap_err().len(), 2, "shorter is not enough: ambiguous");
    }

    #[test]
    fn developer_from_id_prefix_or_first_party() {
        assert_eq!(developer_hint("openrouter", "google/gemini-2.5-flash"), "Google");
        assert_eq!(developer_hint("openrouter", "meta-llama/llama-4"), "Meta");
        assert_eq!(developer_hint("anthropic", "claude-opus-5"), "Anthropic");
        assert_eq!(developer_hint("opencode", "space-bunny-free"), "");
        assert_eq!(developer_hint("cloudflare-workers-ai", "@cf/ibm-granite/granite-4.0-h-micro"), "IBM");
        assert_eq!(developer_hint("amazon-bedrock", "nvidia.nemotron-nano-3-30b"), "NVIDIA");
        assert_eq!(developer_hint("amazon-bedrock", "us.amazon.nova-premier-v1:0"), "Amazon");
        assert_eq!(developer_hint("sap-ai-core", "anthropic--claude-4.6-sonnet"), "Anthropic");
        assert_eq!(developer_from_name("ministral-3b-2512"), "Mistral");
        assert_eq!(short_org("Google DeepMind"), "Google");
        assert_eq!(short_org("Thinking Machines"), "Thinking Machines");
        assert_eq!(short_org("Z.ai (Zhipu AI),Tsinghua University"), "Z.ai");
        assert_eq!(short_org("~anthropic"), "Anthropic");
        assert_eq!(
            (short_org("SpaceXAI"), short_org("Kimi")),
            ("xAI".into(), "Moonshot".into()),
            "as either source says"
        );
        assert_eq!(developer_from_name("Nemotron 3 Ultra Free"), "NVIDIA");
        assert_eq!(developer_from_name("Space Bunny Free"), "");
    }

    #[test]
    fn developer_spellings_merge() {
        let mut ms: Vec<Model> = ["Z.ai", "Z.ai", "Zai-org", "Deepseek-ai", "DeepSeek", "OpenAI", "xAI", ""]
            .map(|d| Model { developer: d.into(), ..Default::default() })
            .into();
        unify_developers(&mut ms);
        let devs: Vec<&str> = ms.iter().map(|m| m.developer.as_str()).collect();
        assert_eq!(devs, ["Z.ai", "Z.ai", "Z.ai", "DeepSeek", "DeepSeek", "OpenAI", "xAI", ""]);
    }

    #[test]
    fn harness_marks_exact_offer_only() {
        let o = |p: &str, id: &str| Offer { provider: p.into(), id: id.into(), ..Default::default() };
        let m = |offers: Vec<Offer>| Model { offers, ..Default::default() };
        let mut d = Data {
            harness: BTreeMap::from([
                ("opencode".into(), vec!["anthropic/claude-opus-5".into()]),
                ("claude".into(), vec!["anthropic/*".into()]),
                ("pi".into(), vec!["openai-codex/gpt-5".into()]),
            ]),
            models: vec![
                m(vec![o("anthropic", "claude-opus-5"), o("openrouter", "claude-opus-5")]),
                m(vec![o("anthropic", "claude-haiku-4-5")]),
                m(vec![o("openai", "gpt-5")]),
                m(vec![o("openai", "gpt-4")]),
            ],
            ..Default::default()
        };
        d.apply_available();
        assert!(d.models[0].available && d.models[0].offers[0].available && !d.models[0].offers[1].available);
        assert_eq!(d.models[0].via, ["claude", "opencode"]);
        assert_eq!(d.models[1].via, ["claude"], "a provider-wide harness covers every model of it");
        assert_eq!(d.models[2].via, ["pi"], "pi's openai-codex is models.dev's openai");
        assert!(!d.models[3].available && d.models[3].via.is_empty());
    }

    #[test]
    fn a_gguf_repo_named_as_the_model_is_the_one_to_download() {
        let list = br#"[{"id": "prism-ml/Ternary-Bonsai-27B-gguf"}, {"id": "unsloth/Qwen3.6-27B-MTP-GGUF"},
            {"id": "unsloth/Qwen3.6-27B-GGUF"}, {"id": "bartowski/Qwen3.6-27B-GGUF"}]"#;
        let found = |list, base, key| gguf_named(list, base, key);
        let first = Some("unsloth/Qwen3.6-27B-GGUF".to_string());
        assert_eq!(found(list, "Qwen/Qwen3.6-27B", "qwen3627b"), first, "the first so named");
        assert_eq!(found(list, "zai-org/GLM-5.3", "glm53"), None);
        assert_eq!(found(b"[]", "zai-org/GLM-5.3", "glm53"), None);
        // A model keyed by a shorter name than its repo's: the first copy named as the repo and
        // no more, but for what of its owner's name some put before it.
        let list = br#"[{"id": "TheDrummer/Rocinante-X-12B-v1-GGUF"}, {"id": "meta-llama/Llama-3.1-8B-Instruct-abliterated-GGUF"},
            {"id": "x/Llama-3.1-8B-Instruct-GGUF-UD"}, {"id": "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF"}]"#;
        let copy = Some("bartowski/Meta-Llama-3.1-8B-Instruct-GGUF".to_string());
        assert_eq!(found(list, "meta-llama/Llama-3.1-8B-Instruct", "llama318b"), copy, "not a fine-tune's");
        let list = br#"[{"id": "mlabonne/DeepSeek-R1-0528-abliterated-GGUF"}, {"id": "bartowski/deepseek-ai_DeepSeek-R1-0528-GGUF"}]"#;
        let copy = Some("bartowski/deepseek-ai_DeepSeek-R1-0528-GGUF".to_string());
        assert_eq!(found(list, "deepseek-ai/DeepSeek-R1-0528", "r10528"), copy, "its owner before it");
        // Downloaded, it is that model here, by the repo kept for it, and the one its name says.
        let listed = ["x/hf.co/bartowski/Meta-Llama-3.1-8B-Instruct-GGUF:latest".to_string()];
        let keys = HashSet::from(["llama318b", "metallama318binstruct", "glm53"]);
        let repos = [("llama318b", "bartowski/meta-llama-3.1-8b-instruct-gguf"), ("glm53", "a/b-gguf")];
        let repos = repos.map(|(k, r)| (k.to_string(), r.to_string())).into();
        let mut named: Vec<_> = local_tags(&listed, &keys, &repos).into_keys().collect();
        named.sort();
        assert_eq!(named, ["llama318b", "metallama318binstruct"]);
        let tree = br#"[{"path":"README.md","size":9},{"path":"mmproj-F16.gguf","size":800},
            {"path":"m-Q8_0.gguf","size":4270000000},{"path":"Q4_K_M/m-Q4_K_M-00001-of-00002.gguf","size":2000000000},
            {"path":"Q4_K_M/m-Q4_K_M-00002-of-00002.gguf","size":500000000}]"#;
        assert_eq!(gguf_bytes(tree).map(gb).as_deref(), Some("2.5 GB"), "both parts of the Q4_K_M one");
        let other =
            br#"[{"type":"directory","path":"x"},{"path":"mmproj.gguf","size":8},{"path":"m-Q8_0.gguf","size":7}]"#;
        assert_eq!((gguf_bytes(other), gguf_bytes(b"[]")), (Some(7), None), "else the first, the projector aside");
        let two = br#"[{"path":"imatrix/m-Q4_K_M.gguf","size":3},{"path":"m-Q4_K_M.gguf","size":5},{"path":"m-ablit-Q4_K_M.gguf","size":9}]"#;
        assert_eq!(gguf_bytes(two), Some(3), "one file, not every one so quantized");
        assert_eq!(
            (gb(40_000_000), gb(999_000_000), gb(1_000_000_000)),
            ("40 MB".into(), "999 MB".into(), "1.0 GB".into())
        );
        let own = Ok(Some("unsloth/GLM-5.3-GGUF".to_string()));
        assert_eq!(gguf_repo("unsloth/GLM-5.3-GGUF", "glm53"), own, "one already");
        assert_eq!(get_cmd(LLAMA, "a/b-GGUF"), ["llama-cli", "-hf", "a/b-GGUF"]);
        assert_eq!(get_cmd(OLLAMA, "a/b-GGUF"), ["ollama", "run", "hf.co/a/b-GGUF"]);
        assert_eq!(local_key("hf.co/unsloth/Qwen3.6-27B-GGUF:latest"), "qwen3627b", "as ollama lists that copy");
        assert_eq!(local_key("llama3.2:latest"), "llama32latest", "its own library's still names no size");
    }

    #[test]
    fn ollama_tags_mark_the_models_they_name() {
        let out = "motd\nNAME  ID  SIZE  MODIFIED\nqwen3.5:4b-fp16  aa  9 GB  now\nqwen3.5:4b  d8b0  3.3 GB  2 hours ago\nllama3.2:latest  baf6  1.3 GB  now\nphi5:3b  cc  2 GB  now\n";
        let ids = name_ids(out).unwrap();
        assert_eq!(ids, ["ollama/qwen3.5:4b-fp16", "ollama/qwen3.5:4b", "ollama/llama3.2:latest", "ollama/phi5:3b"]);
        assert_eq!(name_ids("Error: could not connect\n"), None, "no header, no ids");
        for tag in ["llama3.2:1b-instruct-q4_K_M", "llama3.2:1b-instruct-Q4_K_M-128k", "llama3.2:1b-instruct-qat"] {
            assert_eq!(local_key(tag), "llama321binstruct", "{tag} without its quantization");
        }
        let paid =
            |p: &str| Offer { provider: p.into(), id: "m".into(), input: 1.0, output: 1.0, ..Default::default() };
        let fit = BTreeMap::from([("coding".to_string(), 50.0)]);
        let m = |key: &str, p: &str| Model {
            key: key.into(),
            offers: vec![paid(p)],
            fit: fit.clone(),
            ..Default::default()
        };
        let mut d = Data {
            harness: BTreeMap::from([(OLLAMA.into(), ids), ("opencode".into(), vec!["alibaba/m".into()])]),
            models: vec![m("qwen354b", "alibaba"), m("llama32", "meta"), m("phi53binstruct", "azure")],
            ..Default::default()
        };
        d.apply_available();
        d.apply_available();
        let q = &d.models[0];
        assert_eq!(
            (q.offers.len(), &q.via),
            (2, &vec!["opencode".to_string(), OLLAMA.into()]),
            "one offer, however often"
        );
        assert_eq!(q.offers[1].id, "qwen3.5:4b", "the default tag, not the one pulled last");
        assert_eq!((q.cost(), q.local()), (Some(1.0), false), "the harness that pays may be the one started");
        assert_eq!(d.models[1].offers.len(), 1, "a tag without a size names no model");
        let phi = &d.models[2];
        assert_eq!((phi.cost(), phi.local()), (Some(0.0), true), "a plain tag is the instruct model, free here alone");
        assert_eq!(phi.price_page(), None, "models.dev has no page for your machine");
        assert_eq!((phi.fit.get("value"), d.models[0].fit.contains_key("value")), (None, true), "nor a value");
        for tag in ["hf.co/bartowski/Llama3.2-1B-Instruct-GGUF:Q4_K_M", "someone/llama3.2:1b-instruct-f16"] {
            assert_eq!(local_key(tag), "llama321binstruct", "{tag} whoever it is from");
        }
        assert_eq!(local_key("gemma3:4b-it-qat"), "gemma34bit", "the instruction-tuned one, as it says");
        d.models[0].offers[0].unpriced = true;
        assert_eq!(d.models[0].cost(), None, "your machine's copy is not the price of one with none listed");
        d.harness.remove("opencode");
        d.apply_available();
        assert!(d.models[0].local() && !d.any_available(), "ollama alone leaves every model in reach");
    }

    #[test]
    fn llama_cache_marks_the_models_it_names() {
        let out = "0.00.000.797 I srv  llama_server: initializing ...\nnumber of models in cache: 2\n   1. ggml-org/tinygemma3-GGUF:Q8_0\n   2. unsloth/Qwen3.5-4B-GGUF:Q4_K_M\n";
        let ids = cache_ids(out).unwrap();
        assert_eq!(ids, ["llama-cli/ggml-org/tinygemma3-GGUF:Q8_0", "llama-cli/unsloth/Qwen3.5-4B-GGUF:Q4_K_M"]);
        assert_eq!(cache_ids("error: invalid argument\n"), None, "no count, no ids");
        let mut d = Data {
            harness: BTreeMap::from([(LLAMA.into(), ids), (OLLAMA.into(), vec!["ollama/qwen3.5:4b".into()])]),
            models: vec![Model { key: "qwen354b".into(), ..Default::default() }],
            ..Default::default()
        };
        d.apply_available();
        let m = &d.models[0];
        let offers: Vec<_> = m.offers.iter().map(|o| (o.provider.as_str(), o.id.as_str())).collect();
        assert_eq!(offers, [("ollama", "qwen3.5:4b"), ("llama-cli", "unsloth/Qwen3.5-4B-GGUF:Q4_K_M")]);
        assert_eq!((&m.via, m.local()), (&vec![OLLAMA.to_string(), LLAMA.into()], true), "an offer from each");
        let cmd = crate::app::launch_cmd(m, LLAMA, &d.harness).unwrap();
        assert_eq!(cmd, ["llama-cli", "-hf", "unsloth/Qwen3.5-4B-GGUF:Q4_K_M"]);
        for file in ["/mnt/c/Downloads/Qwen3.5-4B-Q4_K_M.gguf", "qwen3.5-4b.Q4_K_M.gguf", "qwen3.5-4b.gguf"] {
            assert_eq!(local_key(file), "qwen354b", "{file} by its name");
        }
        let file = "/mnt/c/Downloads/Qwen3.5-4B-Q4_K_M.gguf";
        d.harness = BTreeMap::from([(LLAMA.into(), vec![format!("{LLAMA}/{file}")])]);
        d.apply_available();
        let cmd = crate::app::launch_cmd(&d.models[0], LLAMA, &d.harness).unwrap();
        assert_eq!(cmd, ["llama-cli", "-m", file], "a file is loaded, not downloaded");
        // The instruction-tuned model, by whichever name it has: not the base one beside it.
        let listed = ["gemma3:4b-it-qat", "gemma-3-12b-it-Q4_K_M.gguf", "gemma-4-12b-it-Q4_K_M.gguf", "gemma3:1b"];
        let listed = listed.map(|t| format!("x/{t}"));
        let keys = HashSet::from(["gemma34b", "gemma34bit", "gemma312b", "gemma412b", "gemma412binstruct", "gemma31b"]);
        let mut named: Vec<_> = local_tags(&listed, &keys, &HashMap::new()).into_keys().collect();
        named.sort();
        assert_eq!(named, ["gemma312b", "gemma31b", "gemma34bit", "gemma412binstruct"]);
        // ollama's plain tag is the instruct model where the base one is listed too, a repo's is as named.
        let keys = HashSet::from(["llama321b", "llama321binstruct"]);
        let named = |id: &str| local_tags(&[id.to_string()], &keys, &HashMap::new()).into_keys().collect::<Vec<_>>();
        assert_eq!(named("ollama/llama3.2:1b"), ["llama321binstruct"]);
        assert_eq!(named("ollama/hf.co/callgg/llama-3.2-1b-gguf:latest"), ["llama321b"]);
        assert_eq!(named("ollama/someone/llama3.2:1b"), ["llama321b"], "nor is another's upload");
        assert_eq!(named("llama-cli/callgg/llama-3.2-1b-gguf:Q4_K_M"), ["llama321b"]);
    }

    #[test]
    fn price_prefers_available_then_list() {
        let o = |p: &str, i: f64, a: bool| Offer {
            provider: p.into(),
            input: i,
            output: i,
            available: a,
            ..Default::default()
        };
        let mut m =
            Model { offers: vec![o("a", 5.0, false), o("b", 5.0, false), o("c", 1.0, false)], ..Default::default() };
        assert_eq!(m.price().unwrap().input, 5.0);
        m.offers[1].available = true;
        m.offers.push(o("d", 4.0, true));
        assert_eq!(m.price().unwrap().provider, "d");
        // Tied counts: the cheaper price wins, whatever the offer order.
        let tie = Model { offers: vec![o("x", 9.0, false), o("y", 2.0, false)], ..Default::default() };
        assert_eq!(tie.price().unwrap().provider, "y");
        // Cached input is billed at the cache price: 3 × (0.9 × 0.5 + 0.1 × 5) + 25, over 4.
        let cached = Offer {
            provider: "c".into(),
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.5),
            available: true,
            ..Default::default()
        };
        let plain = Offer { provider: "p".into(), input: 4.0, output: 20.0, available: true, ..Default::default() };
        assert!((cached.blended() - 6.9625).abs() < 1e-9 && plain.blended() == 8.0, "no discount: full input");
        let m = Model { offers: vec![plain, cached], ..Default::default() };
        assert_eq!(m.price().unwrap().provider, "c", "the dearer list price is cheaper once cached");
        assert!(
            crate::app::base_col_about(crate::app::PRICE)
                .contains(&format!("{:.0}% of the input cached", crate::data::cached() * 100.0))
        );
        // A one-off prompt caches nothing: the cheaper list price wins again.
        set_cached(0.0);
        assert_eq!(m.price().unwrap().provider, "p");
        assert!(crate::app::base_col_about(crate::app::PRICE).contains(" 0% of the input cached"));
        set_cached(2.0);
        assert_eq!(crate::data::cached(), 1.0, "clamped");
        // A price nobody lists is unknown, not free: yours still names the id, but costs nothing known.
        let unknown = Offer { provider: "u".into(), unpriced: true, available: true, ..Default::default() };
        let free = Offer { provider: "f".into(), available: true, ..Default::default() };
        let m = Model { offers: vec![unknown.clone()], ..Default::default() };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost(), m.listed()), ("u", None, false));
        let m = Model { offers: vec![unknown.clone(), free], ..Default::default() };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost()), ("f", Some(0.0)), "a free offer is priced");
        assert!(!m.listed(), "and it is yours");
        // Yours has no price but others list one: the id is still yours, the price the list one.
        let m = Model {
            offers: vec![unknown, o("a", 5.0, false), o("b", 5.0, false), o("c", 1.0, false)],
            ..Default::default()
        };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost(), m.listed()), ("u", Some(5.0), true));
        let m = Model { offers: vec![m.offers[0].clone(), o("z", 0.0, false)], ..Default::default() };
        assert_eq!((m.cost(), m.listed()), (None, false), "another provider's free tier is not your price");
        let marked: Vec<_> =
            crate::app::COLS.iter().enumerate().filter(|c| crate::app::on_price(c.0)).map(|c| c.1.id).collect();
        assert_eq!(marked, ["price", "in", "cache", "out", "value"], "the columns a ~ goes on");
    }

    #[test]
    fn progress_counts_steps_and_names_the_last_ones() {
        assert_eq!(progress_text(9, &["models.dev", "epoch.ai", "pi"]), "6/9");
        assert_eq!(progress_text(9, &["epoch.ai", "epoch.ai", "pi"]), "6/9, waiting for epoch.ai, pi", "a site once");
        assert_eq!(progress_text(9, &[]), "9/9");
        let steps = Steps::default();
        ["opencode", "pi", "pi"].into_iter().for_each(|n| step(&steps, n));
        answered(&steps, "pi");
        answered(&steps, "codex");
        assert_eq!(*steps.lock().unwrap(), (3, vec!["opencode", "pi"]), "one answer ends one step");
        assert_eq!(
            (progress(&steps), progress(&Steps::default())),
            ("1/3, waiting for opencode, pi".into(), String::new())
        );
    }

    #[test]
    fn epoch_pages_link_by_group_or_alias() {
        let mut ep = Scores::default();
        for (k, name) in [
            ("gemini25pro062025", "Gemini 2.5 Pro (Jun 2025)"),
            ("commandrplus", "Command R+"),
            ("gpt5codex", "GPT-5-Codex"),
        ] {
            ep.groups.insert(k.into(), (name.into(), None, BTreeMap::new()));
        }
        ep.alias.insert("gemini25pro".into(), "gemini25pro062025".into());
        let names = ["Gemini 2.5 Pro", "Command R+", "GPT-5-Codex", "Command R", "Devstral Small 2", "A+", "A", ""];
        let mut ms = names.map(|n| Model { key: norm(n), name: n.into(), ..Default::default() });
        ms[7].epoch = Some("stale".into());
        let pages = |ms: &[Model]| ms.iter().map(|m| m.epoch.clone().unwrap_or_default()).collect::<Vec<_>>();
        epoch_named(&mut ms, &ep);
        let named = ["gemini-2-5-pro-jun-2025", "command-r", "gpt-5-codex", "", "", "", "", ""];
        assert_eq!(pages(&ms), named, "as Epoch spells its group, a + dropped; no group, no page");
        epoch_listed(&mut ms, &["gemini-2-5-pro-jun-2025", "command-r", "devstral-small-2", "a"].into());
        assert_eq!(
            pages(&ms),
            ["gemini-2-5-pro-jun-2025", "command-r", "", "", "devstral-small-2", "", "", ""],
            "only the pages Epoch has, one it did not score by its name, and none that two rows could be"
        );
        ms[0].aa = Some("gemini-2-5-pro".into());
        let cache = |format| serde_json::to_vec(&Data { format, models: ms.to_vec(), ..Default::default() }).unwrap();
        let [aa, mut kept] = cached_pages(&cache(FORMAT));
        kept.sort();
        assert_eq!(aa, ["gemini-2-5-pro"], "the last refresh's pages, of each site");
        assert_eq!(kept, ["command-r", "devstral-small-2", "gemini-2-5-pro-jun-2025"]);
        assert_eq!(
            cached_pages(&cache(FORMAT - 1)),
            [vec![], Vec::<String>::new()],
            "but none unchecked, of an older format"
        );
        epoch_listed(&mut ms, &HashSet::new());
        assert_eq!(pages(&ms), [""; 8], "none when Epoch's list of pages did not come");
        assert_eq!(epoch_slug("Tulu 3 (T\u{fc}lu 3) 70B"), "tulu-3-tlu-3-70b");
    }

    #[test]
    fn aa_folds_reasoning_settings_into_one_model() {
        let json = br#"{"status":200,"data":[
            {"slug":"claude-4-5-sonnet","name":"Claude 4.5 Sonnet (Non-reasoning)","model_creator":{"name":"Anthropic"},
             "median_output_tokens_per_second":80.5,"median_time_to_first_answer_token":1.2,
             "evaluations":{"artificial_analysis_intelligence_index":50,"terminalbench_v4_0":0.5,"scicode":0.3,"hle":0.2}},
            {"slug":"claude-4-5-sonnet-thinking","name":"Claude 4.5 Sonnet (Reasoning)","model_creator":{"name":"Anthropic"},
             "median_output_tokens_per_second":40.0,"median_time_to_first_token_seconds":0.5,
             "median_time_to_first_answer_token":9.0,
             "evaluations":{"artificial_analysis_intelligence_index":60,"terminalbench_v4_0":0.7,"scicode":0.5,"hle":0.3}},
            {"slug":"unscored","name":"Unscored","evaluations":{}}
        ]}"#;
        let sc = parse_aa(json).unwrap();
        let key = aa_words("Claude Sonnet 4.5", true).join("-");
        let (name, index, scores) = &sc.groups[&key];
        assert_eq!((name.as_str(), *index), ("Claude 4.5 Sonnet", Some(60.0)), "the best setting's index");
        assert_eq!((scores[crate::fit::AA_CODING], scores["terminalbench_v4_0"]), (0.6, 0.7), "all of that setting's");
        let e = |index, scored: usize| AaEntry {
            index: Some(index),
            scores: (0..scored).map(|i| (i.to_string(), 0.5)).collect(),
            ..Default::default()
        };
        let most = aa_fold([&e(40.0, 1), &e(39.0, 3)]).unwrap();
        assert_eq!(
            (most.index, most.scores.len()),
            (Some(39.0), 3),
            "the setting scored on the most, not another's index"
        );
        assert_eq!(scores["hle"], 0.3);
        assert_eq!((sc.page[&key].as_str(), sc.org[&key].as_str()), ("claude-4-5-sonnet", "Anthropic"));
        assert_eq!(
            sc.speed[&key],
            (Some(40.0), Some(9.0)),
            "the speed of the setting whose index it shows, to its answer"
        );
        assert_eq!(sc.groups.len(), 1, "a model with no scores is left out");
        assert!(parse_aa(br#"{"data":[]}"#).is_err());
        let w = |s| aa_words(s, true);
        assert_eq!(w("gpt-5-mini-high"), w("gpt-5-mini"), "a setting is folded");
        assert_eq!(w("grok-4-non-reasoning"), w("grok-4"));
        assert_ne!(w("magistral-medium"), w("magistral"), "a tier is not a setting");
    }

    #[test]
    fn a_row_naming_a_setting_has_that_settings_scores() {
        let api = br#"{"data":[
            {"slug":"grok-4-20","name":"Grok 4.20 (Reasoning)","median_output_tokens_per_second":40.0,
             "evaluations":{"artificial_analysis_intelligence_index":26,"hle":0.3}},
            {"slug":"grok-4-20-non-reasoning","name":"Grok 4.20 (Non-reasoning)","median_output_tokens_per_second":90.0,
             "evaluations":{"artificial_analysis_intelligence_index":14,"hle":0.1}},
            {"slug":"claude-opus-4-6-adaptive","name":"Claude Opus 4.6 (Adaptive Reasoning, Max Effort)",
             "evaluations":{"artificial_analysis_intelligence_index":32}},
            {"slug":"claude-opus-4-6","name":"Claude Opus 4.6 (Non-reasoning, High Effort)",
             "evaluations":{"artificial_analysis_intelligence_index":26}},
            {"slug":"gpt-5-3-codex","name":"GPT-5.3 Codex (Xhigh)","evaluations":{"artificial_analysis_intelligence_index":33}},
            {"slug":"minimax-m3","name":"MiniMax-M3","evaluations":{"artificial_analysis_intelligence_index":29}},
            {"slug":"phi-4","name":"Phi-4","evaluations":{"artificial_analysis_intelligence_index":6}},
            {"slug":"kimi-k2","name":"Kimi K2","release_date":"2025-07-11",
             "evaluations":{"artificial_analysis_intelligence_index":12}},
            {"slug":"kimi-k2-thinking","name":"Kimi K2 Thinking","release_date":"2025-11-06",
             "evaluations":{"artificial_analysis_intelligence_index":22}},
            {"slug":"qwen-9-instruct","name":"Qwen 9 Instruct","evaluations":{"artificial_analysis_intelligence_index":8}},
            {"slug":"qwen-9-instruct-reasoning","name":"Qwen 9 Instruct (Reasoning)",
             "evaluations":{"artificial_analysis_intelligence_index":13}},
            {"slug":"step-5","name":"Step 5 Preview","evaluations":{"artificial_analysis_intelligence_index":43}},
            {"slug":"v3-2-reasoning-0925","name":"V3.2 Exp (Reasoning)","evaluations":{"artificial_analysis_intelligence_index":16}},
            {"slug":"v3-2-0925","name":"V3.2 Exp (Non-reasoning)","evaluations":{"artificial_analysis_intelligence_index":13}},
            {"slug":"y7-reasoning-0925","name":"Y7 Exp (Reasoning)","evaluations":{"artificial_analysis_intelligence_index":16}},
            {"slug":"y7-v-0925","name":"Y7 Exp (Non-reasoning)","evaluations":{"artificial_analysis_intelligence_index":13}},
            {"slug":"mistral-medium","name":"Mistral Medium","evaluations":{"artificial_analysis_intelligence_index":5}}
        ]}"#;
        let names = [
            "Grok 4.20",
            "Grok 4.20 Reasoning",
            "Grok 4.20 Non-Reasoning",
            "Claude Opus 4.6",
            "Claude 4.6 Opus Thinking",
            "Claude 4.6 Opus Thinking Low",
            "GPT-5.3 Codex XHigh",
            "GPT-5.3 Codex Low",
            "MiniMax M3 Thinking",
            "Kimi K2",
            "Kimi K2 Thinking",
            "Step 5 Preview",
            "V3.2 Exp",
            "Y7 Exp",
            "Phi-4",
            "Phi-4-reasoning",
            "Qwen 9 Instruct",
        ];
        // The last three cannot reason.
        let models: String = names
            .iter()
            .enumerate()
            .map(|(i, n)| format!(r#""{n}": {{"name": "{n}", "reasoning": {}}},"#, i < names.len() - 3))
            .collect();
        let json = format!(
            r#"{{"p": {{"models": {{{models} "mistral-medium-latest": {{"name": "Mistral Medium", "reasoning": true}}}}}}}}"#
        );
        let d = merge(json.as_bytes(), &parse_aa(api).unwrap(), None).unwrap();
        let row = |n: &str| d.models.iter().find(|m| m.name == n).unwrap();
        let index = |n: &str| row(n).eci;
        assert_eq!(index("Grok 4.20"), Some(26.0), "a row naming no setting has the best of them");
        assert_eq!((index("Grok 4.20 Reasoning"), index("Grok 4.20 Non-Reasoning")), (Some(26.0), Some(14.0)));
        let off = row("Grok 4.20 Non-Reasoning");
        assert_eq!((off.scores["hle"], off.shown["reasoning"], off.tps), (0.1, 10.0, Some(90.0)), "all of it its own");
        assert_eq!(off.aa.as_deref(), Some("grok-4-20-non-reasoning"), "and its own page");
        assert!(off.fit["overall"] < row("Grok 4.20").fit["overall"], "ranked on its own index");
        assert_eq!((index("Claude Opus 4.6"), index("Claude 4.6 Opus Thinking")), (Some(32.0), Some(32.0)));
        let low = row("Claude 4.6 Opus Thinking Low");
        let page = Some("claude-opus-4-6-adaptive");
        assert_eq!((low.eci, low.aa.as_deref()), (None, page), "an effort not measured: no score, the reasoning page");
        assert!(low.fit.is_empty() && low.scores.is_empty());
        assert_eq!(index("GPT-5.3 Codex XHigh"), Some(33.0), "the setting is in the entry's name alone");
        assert_eq!(index("GPT-5.3 Codex Low"), None);
        assert_eq!(index("MiniMax M3 Thinking"), Some(29.0), "an entry saying no setting is the model's only one");
        let own = row("Phi-4-reasoning");
        assert_eq!((index("Phi-4"), own.eci, own.aa.as_deref()), (Some(6.0), None, None), "not one that cannot reason");
        assert_eq!(
            (index("Kimi K2"), index("Kimi K2 Thinking")),
            (Some(12.0), Some(22.0)),
            "a later release is no setting"
        );
        assert_eq!(index("Qwen 9 Instruct"), Some(8.0), "a row that cannot reason is not its reasoning setting");
        assert_eq!(index("Step 5 Preview"), Some(43.0), "found by the entry's name, where its slug says less");
        assert_eq!(index("V3.2 Exp"), Some(13.0), "of two entries so named, the shorter slug: no setting");
        assert_eq!(index("Y7 Exp"), Some(13.0), "the shorter, not the first by its letters");
        assert_eq!(index("Mistral Medium"), None, "sold as -latest: whichever release that is by now");
    }

    #[test]
    fn the_source_is_artificial_analysis_once_its_key_is_there() {
        assert_eq!(Source::preferred(), Source::Epoch);
        save_aa_key("k").unwrap();
        assert_eq!(Source::preferred(), Source::Aa);
    }

    #[test]
    fn epoch_schema_change_is_an_error() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("model_metadata.csv", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, b"model_version,renamed_group,date\nv1,g,2026-01-01\n").unwrap();
            z.finish().unwrap();
        }
        let err = parse_epoch(buf.get_ref()).unwrap_err();
        assert!(err.contains("model_group"), "{err}");
    }

    #[test]
    fn a_price_is_the_tier_an_agent_session_reaches() {
        let json = br#"{"p": {"models": {
            "m-1": {"name": "M 1", "cost": {"input": 2.5, "output": 7.5, "cache_read": 0.5, "tiers": [
                {"input": 5, "output": 15, "cache_read": 1, "tier": {"type": "context", "size": 32000}},
                {"input": 6.25, "output": 18.5, "tier": {"type": "context", "size": 128000}}]}},
            "m-2": {"name": "M 2", "cost": {"input": 1, "output": 2, "tiers": [
                {"input": 2, "output": 4, "tier": {"type": "context", "size": 200000}}]}},
            "m-3": {"name": "M 3", "cost": {"input": 1, "output": 2, "cache_read": 0.1, "tiers": [
                {"input": 2, "output": 4, "tier": {"type": "context", "size": 32000}}]}},
            "m-4": {"name": "M 4", "cost": {"input": 1, "output": 2, "tiers": [
                {"input": 2, "output": 4, "tier": "context", "size": "32k"}]}},
            "m-5": {"name": "M 5", "cost": {"input": 1, "output": 2, "tiers": [
                {"input": 2, "tier": {"type": "context", "size": 32000}}]}},
            "m-6": {"name": "M 6", "cost": {"input": 1, "output": 2, "cache_read": 0.1, "tiers": [
                {"output": 4, "tier": {"type": "context", "size": 32000}}]}}}}}"#;
        let d = merge(json, &Scores::default(), None).unwrap();
        let price = |k: &str| {
            let o = &d.models.iter().find(|m| m.key == k).unwrap().offers[0];
            (o.input, o.output, o.cache_read)
        };
        assert_eq!(price("m1"), (5.0, 15.0, Some(1.0)), "over 32k, and not yet over 128k");
        assert_eq!(price("m2"), (1.0, 2.0, None), "a tier past what a session reaches stays out");
        assert_eq!(price("m3"), (2.0, 4.0, None), "a tier's cache price is its own, not the first prices'");
        assert_eq!(price("m4"), (1.0, 2.0, None), "tiers in another shape are none, and the rest is read");
        assert_eq!(price("m5"), (2.0, 2.0, None), "a price the tier does not list stays as it was");
        assert_eq!(price("m6"), (1.0, 4.0, Some(0.1)), "the input's too, and its cache price with it");
    }

    #[test]
    fn only_a_newer_release_is_an_update() {
        let d = |latest: &str| Data { latest: latest.into(), ..Data::default() };
        assert_eq!(d("99.0.0").update(), Some("99.0.0"));
        assert!(d(env!("CARGO_PKG_VERSION")).update().is_none());
        assert!(d("0.0.1").update().is_none());
        assert!(d("").update().is_none());
        // 0.10 is newer than 0.9: compared as numbers, not text.
        let v: Vec<u64> = env!("CARGO_PKG_VERSION").split('.').map(|n| n.parse().unwrap()).collect();
        assert!(d(&format!("{}.{}.{}", v[0], v[1] + 10, 0)).update().is_some());
    }

    #[test]
    fn a_bare_name_is_the_dated_release_filed_under_it() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let mut file = |name: &str, body: &[u8]| {
                z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                std::io::Write::write_all(&mut z, body).unwrap();
            };
            file(
                "model_metadata.csv",
                b"model_version,model_group,date\ngoogle/flash-9,Flash 9 (Jun 2025),2025-06-17\n\
                  flash-9-chat,Flash 9 (Sep 2025),2025-09-25\n\
                  flash-9-preview-09,Flash 9 (Sep 2025),2025-09-25\nchat-4o-03,GPT-4o (Mar 2025),2025-03-27\n\
                  gpt-4o-11,GPT-4o (Nov 2024),2024-11-20\nfoo-2,Foo 2 (Jun 2025),2025-06-01\n\
                  foo-2-09,Foo 2 (Sep 2025),2025-09-01\nbar-3-07,Bar 3 (Jul 2025),2025-07-01\n\
                  bar-3-09,Bar 3 (Sep 2025),2025-09-01\n",
            );
            file(
                "benchmark_metadata.csv",
                b"source_file,score_column,benchmark,scale\nocr.csv,score,Some OCR,1\nswe.csv,score,DeepSWE,1\n",
            );
            file(
                "ocr.csv",
                b"Model version,score\ngoogle/flash-9,0.5\nflash-9-preview-09,0.6\nchat-4o-03,0.5\nfoo-2,0.5\n",
            );
            file(
                "swe.csv",
                b"Model version,score,Mean cost (USD),Mean output tokens\nfoo-2-09,0.5,4,4000\n\
                  foo-2-09_max,0.5,9,9000\nfoo-2-09_low,0.2,1,1000\nbar-3-09,0.5,,\nbar-3-09_low,0.3,2,2000\n",
            );
            file(
                "epoch_capabilities_index/eci_scores.csv",
                b"Model,eci\nGPT-4o (Nov 2024),128\nBar 3 (Jul 2025),140\n",
            );
            z.finish().unwrap();
        }
        let ep = parse_epoch(buf.get_ref()).unwrap();
        assert_eq!(ep.alias["flash9"], "flash9jun2025", "the release filed under the bare id, not the newest");
        assert_eq!(ep.alias["gpt4o"], "gpt4onov2024", "the one with an index, before the newest without");
        assert_eq!(ep.alias["foo2"], "foo2sep2025", "one scored on a task, before the bare id's with no score");
        assert_eq!(ep.alias["bar3"], "bar3jul2025", "and before a newer one scored on a task");
        assert_eq!(ep.ids["flash9preview09"], "flash9sep2025", "a model's id reaches its group, named otherwise");
        assert_eq!(ep.group("foo2").unwrap(), "foo2sep2025", "the name before the id, an older release's");
        assert!(!ep.ids.contains_key("flash9chat"), "an id of another kind of model is not its group's");
        assert!(ep.ids.contains_key("flash9") && !ep.ids.contains_key("googleflash9"), "without its provider");
        assert_eq!(ep.cost["foo2sep2025"].1, Some(4.0), "the cost of its best run, the cheaper of two that tie");
        assert_eq!(ep.cost["foo2sep2025"].2, Some(4000.0), "and that run's output tokens");
        assert_eq!(ep.cost["bar3sep2025"].1, None, "its best run lists no cost: not a lower run's");
    }

    #[test]
    fn epoch_links_models_benchmarked_on_no_task() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let mut file = |name: &str, body: &[u8]| {
                z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                std::io::Write::write_all(&mut z, body).unwrap();
            };
            file(
                "model_metadata.csv",
                b"model_version,model_group,date\nnew_high,New,2026-09-01\nother_high,Other,2026-08-01\nold,Old,2026-01-01\n",
            );
            file("benchmark_metadata.csv", b"source_file,score_column,benchmark,scale\nocr.csv,score,Some OCR,1\n");
            file("ocr.csv", b"Model version,score\nother_high,0.5\n");
            file("epoch_capabilities_index/eci_scores.csv", b"Model,eci\nOld,150\n");
            z.finish().unwrap();
        }
        let ep = parse_epoch(buf.get_ref()).unwrap();
        assert!(!ep.groups.contains_key("new"), "listed without results: Epoch has no page for it");
        assert_eq!(ep.groups["other"], ("Other".into(), None, BTreeMap::new()), "known, though no task scores");
        assert_eq!(ep.groups["old"].1, Some(150.0));
    }
}
