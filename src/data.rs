//! Fetching, caching and merging models.dev (prices) with Epoch AI (benchmarks).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MODELS_URL: &str = "https://models.dev/api.json";
/// Epoch's scores.
const EPOCH_URL: &str = "https://epoch.ai/data/benchmark_data.zip";
/// Epoch's model pages, as it has none for some models it scored and some for models it did not.
/// The index of Epoch's sitemaps, which names those of its model pages (`epoch_pages`).
const EPOCH_PAGES_URL: &str = "https://epoch.ai/sitemap-index.xml";
/// Artificial Analysis's page names, to link a model there (`aa_pages`): its public sitemap, no key.
const AA_URL: &str = "https://artificialanalysis.ai/sitemap.xml";
/// OpenRouter's models, for the Hugging Face repo it names for each (`hf_listed`).
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
/// OpenRouter's benchmark leaderboard, its GPQA diamond runs on the models it serves over the
/// last 90 days (CC BY 4.0): the speed of the runs (`openrouter_speeds`).
const OPENROUTER_GPQA_URL: &str = "https://openrouter.ai/api/v1/datasets/exports/benchmark-leaderboard/latest.csv";
/// Arena's Agent leaderboard, the newest one, from the dataset it publishes on Hugging Face
/// (CC-BY): no key needed (`Arena`).
// ponytail: one page of 100 rows, twice what the leaderboard has, and a warning past that
// (`Arena::cut`). Page with `offset` then.
macro_rules! arena_url {
    ($config:literal) => {
        arena_url!($config, 0)
    };
    ($config:literal, $offset:literal) => {
        concat!(
            "https://datasets-server.huggingface.co/rows?dataset=lmarena-ai/leaderboard-dataset&config=",
            $config,
            "&split=latest&offset=",
            $offset,
            "&length=100"
        )
    };
}
const ARENA_URL: &str = arena_url!("agent");
/// Arena's agent score (`Model::arena`) as the agentic task and the Agentic column's dropdown name it.
pub const ARENA_AGENT: &str = "Agent";
/// The signals Arena's agent score is made of, each a leaderboard of its own with the same rows:
/// its name in the Agentic column's dropdown, and where it is (`Model::arena_more`).
pub const ARENA_MORE: [(&str, &str); 5] = [
    ("Task outcome", arena_url!("agent_task_outcome_explicit")),
    ("Tool hallucination", arena_url!("agent_tool_hallucination")),
    ("Steerability", arena_url!("agent_steerability")),
    ("Bash reliability", arena_url!("agent_bash_reliability")),
    ("Praise", arena_url!("agent_praise_complaint")),
];
/// Arena's leaderboards of chat and of web development, in the ECI and Coding columns' dropdowns, rated
/// as Arena rates them, around 1500 and not in percent: the name there, the leaderboard on arena.ai,
/// which a click on a rating opens, and the pages the overall rows are on, as they come first
/// (`Arena::parse`).
pub const ARENA_BOARDS: [(&str, &str, &[&str]); 2] = [
    (
        "Chat",
        "https://arena.ai/leaderboard/text",
        &[
            arena_url!("text"),
            arena_url!("text", 100),
            arena_url!("text", 200),
            arena_url!("text", 300),
            arena_url!("text", 400),
        ],
    ),
    ("WebDev", "https://arena.ai/leaderboard/code/webdev", &[arena_url!("webdev"), arena_url!("webdev", 100)]),
];
/// The leaderboard itself, which `o` and a click on a model's Arena score open (`arena_page`).
const ARENA_PAGE: &str = "https://arena.ai/leaderboard/agent";
/// modelcmp's newest release, fetched with the data, and alone by a start of the TUI that
/// fetches none (`newest_release`), so a new version is told at the next start.
const RELEASE_URL: &str = "https://api.github.com/repos/danctorres/modelcmp/releases/latest";
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// How old a cache without Arena's scores, or some of Epoch's, may be when they did not come
/// (`Data::arena_missed`, `Data::missed`): tried again long before `MAX_AGE`.
const RETRY: Duration = Duration::from_secs(3600);
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
/// 23: `Model::arena_more`. 24: `Model::aka`, and rows merged by a vendor word, a sold id and a size.
/// 25: an offer that names no models.dev page votes for the one its id is.
/// 26: `Model::arena_more` has `ARENA_BOARDS` too. 27: `Data::missed`. 28: Artificial Analysis's has
/// Epoch's, and no `Model::task_cost` nor `task_tokens`. 29: Epoch's coding score is WeirdML v3, where
/// it was WeirdML, which stopped at 2026-09-03, and its tasks have more benchmarks. 30: `SHORT_NAMES`.
/// 31: Epoch's coding score is FrontierCode, which scores more models than WeirdML v3.
/// 32: a row keeps the vendor most offers name it with. 33: an Arena row dated in brackets is of
/// that release. 34: Artificial Analysis's scores too are of the release a row's offers are, an
/// Arena name with its vendor first is of its model, a merged row takes its dated release's Arena
/// rating, and volcengine's ids are ByteDance's. 35: an offer's output is at its reasoning price
/// where that is higher. 36: no Artificial Analysis scores, nor `Model::tps` and `ttft`. 37: the
/// task scores are on the ECI scale, with what they are from (`fit::Basis`), `Model::scores` has
/// every benchmark of Epoch's index, and no `Model::shown`. 38: OpenRouter's GPQA diamond in
/// `Model::scores`. 39: `Model::tps`. 40: not 38, `UNFITTED` benchmarks in `Model::scores`, coding
/// is CursorBench's score, no task falls back to overall, and an estimate needs a score away from
/// a benchmark's floor and ceiling. 41: no agentic estimate in `Model::fit`, as agentic is Arena's.
const FORMAT: u32 = 41;
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

/// Set the cache share, clamped to 0..1. Prices derived from it (`App::vals`) must be recomputed
/// by the caller.
pub fn set_cached(share: f64) {
    let bits = share.clamp(0.0, 1.0).to_bits();
    #[cfg(test)]
    CACHED.with(|c| c.store(bits, Relaxed));
    #[cfg(not(test))]
    CACHED.store(bits, Relaxed);
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
    /// No such command: being installed means access to every model of this provider, unless
    /// the shell line given exits 0, which it does only when sure that it is logged out.
    Provider(&'static str, &'static str),
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
    ("claude", Probe::Provider("anthropic", r#"claude auth status | grep -q '"loggedIn": false'"#)),
    ("codex", Probe::Provider("openai", CODEX_OUT)),
    ("gemini", Probe::Provider("google", GEMINI_OUT)),
    ("copilot", Probe::Copilot),
    (OLLAMA, Probe::Names(&["list"])),
    (LLAMA, Probe::Cache(&["--cache-list"])),
];

/// Exits 0 when codex is logged out: `login status` says so, and there is no key in
/// CODEX_API_KEY, which `codex exec` takes, nor a provider of its own, which needs no login.
const CODEX_OUT: &str = r#"[ -z "$CODEX_API_KEY" ] && ! grep -qs model_provider "${CODEX_HOME:-$HOME/.codex}/config.toml" && codex login status 2>&1 | grep -q 'Not logged in'"#;

/// Exits 0 when gemini, which has no status command, is logged out: no Google login saved,
/// which signing out deletes, no other auth type in its settings, and no variable it takes one
/// from, set or in its `.env` at home. A project's own `.env`, in a folder unknown here, is not seen.
const GEMINI_OUT: &str = r#"h=${GEMINI_CLI_HOME:-$HOME} && [ ! -f "$h/.gemini/oauth_creds.json" ] && ! grep -sE '"selected(Auth)?Type"' "$h/.gemini/settings.json" | grep -qv oauth-personal && [ -z "$GEMINI_API_KEY$GOOGLE_GEMINI_BASE_URL$GOOGLE_GENAI_USE_GCA$GOOGLE_GENAI_USE_VERTEXAI$CLOUD_SHELL$GEMINI_CLI_USE_COMPUTE_ADC$GEMINI_FORCE_ENCRYPTED_FILE_STORAGE" ] && ! grep -qs -e GEMINI_API_KEY -e GOOGLE_GEMINI_BASE_URL -e GOOGLE_GENAI_USE -e CLOUD_SHELL -e COMPUTE_ADC -e FORCE_ENCRYPTED "$h/.gemini/.env" "$h/.env""#;

/// Ollama, and the provider of the offer made for a model it runs here (`Offer::local`).
pub const OLLAMA: &str = "ollama";
/// llama.cpp's CLI, and the provider of the offer made for a model it runs here.
pub const LLAMA: &str = "llama-cli";
/// models.dev's provider for the models Hugging Face serves, by their repo there.
const HF: &str = "huggingface";
/// The harnesses that run models on this machine, and what each is called.
const LOCAL: &[(&str, &str)] = &[(OLLAMA, "Ollama"), (LLAMA, "llama.cpp")];

/// llama.cpp's CLI as a machine may have it: `llama-cli`, else the one binary of newer builds, else
/// its server, which lists and loads the same models.
const LLAMA_CMDS: [&[&str]; 3] = [&[LLAMA], &["llama", "cli"], &["llama-server"]];

/// `LLAMA_CMDS`' first this machine has.
pub fn llama_cmd() -> Option<&'static [&'static str]> {
    if cfg!(test) {
        return Some(&[LLAMA]);
    }
    LLAMA_CMDS.into_iter().find(|c| installed(c[0]))
}

/// The harness a command starting with `bin` runs: llama.cpp's starts with the binary this machine
/// has, which is not its name.
pub fn harness_of(bin: &str) -> &str {
    if LLAMA_CMDS.iter().any(|c| c[0] == bin) { LLAMA } else { bin }
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
    let list = fetch_within(&url, HF_WAIT)?;
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
    // Tests keep a cache of their own, never reading or writing yours.
    #[cfg(test)]
    return std::env::temp_dir().join("modelcmp-test").join(name);
    #[cfg(not(test))]
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

/// The bytes the GGUF `repo` takes to download: the file Hugging Face gives llama.cpp for it
/// (`gguf_file`) and its vision projector, every part of a split one as its files list them.
/// None when it lists no `.gguf` one, so there is nothing to download.
pub fn gguf_size(repo: &str) -> Result<Option<u64>, String> {
    let manifest = format!("https://huggingface.co/v2/{repo}/manifests/latest");
    // Asked as llama.cpp is, or the answer is ollama's, which has no file named. A repo with
    // none, refused, has `gguf_bytes` guess the file from the list. No answer is an error: a
    // guess is the wrong file at times, and a size is kept for good.
    let chosen = match request(&manifest, Some(("User-Agent", "llama-cpp")), HF_WAIT) {
        Ok(m) => gguf_file(&m),
        Err(e) if passing(&e) => return Err(format!("{manifest}: {e}")),
        Err(_) => None,
    };
    if let Some((file, size, proj)) = &chosen
        && split_stem(file).is_none()
    {
        return Ok(Some(size + proj));
    }
    let url = format!("https://huggingface.co/api/models/{repo}/tree/main?recursive=true");
    let tree = fetch_within(&url, HF_WAIT)?;
    let (file, proj) = chosen.map_or((None, 0), |(f, _, p)| (Some(f), p));
    Ok(gguf_bytes(&tree, file.as_deref()).map(|b| b + proj))
}

/// `gguf_size` of the GGUF repo `gguf_repo` finds for the model `key`, whose weights are in the
/// Hugging Face repo `base`: None when Hugging Face has no such copy.
pub fn download_size(base: &str, key: &str) -> Result<Option<u64>, String> {
    gguf_repo(base, key)?.map_or(Ok(None), |r| gguf_size(&r))
}

/// The memory a model runs in on this machine, in bytes: the NVIDIA GPUs' together
/// (`nvidia-smi`), else the RAM, which on a Mac the GPU shares. Kept for `MEMORY_FOR`, as
/// `nvidia-smi` takes half a second.
// ponytail: other GPUs (AMD, Intel) count as RAM, and a model split between GPU and RAM runs
// slower than one that fits: `modelcmp memory` sets it by hand.
pub fn memory() -> Option<u64> {
    static MEMORY: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEMORY.get_or_init(|| {
        if cfg!(test) {
            return None;
        }
        let kept: HashMap<String, u64> = kept(MEMORY_FILE);
        if let (Some(&bytes), Some(&at)) = (kept.get("bytes"), kept.get("at"))
            && now().saturating_sub(at) < MEMORY_FOR
        {
            return Some(bytes);
        }
        let (found, sure) = find_memory()?;
        if sure {
            keep(MEMORY_FILE, &HashMap::from([("bytes".to_string(), found), ("at".to_string(), now())]));
        }
        Some(found)
    })
}

/// Where `memory` keeps what it found, and for how long, in seconds.
const MEMORY_FILE: &str = "memory.json";
const MEMORY_FOR: u64 = 7 * 24 * 3600;

/// `memory`, looked up now, and whether to keep it. A wedged GPU driver can hang `nvidia-smi`,
/// which the TUI's first frame waits on, so it is given up on at `LIMIT` and the RAM counted, not
/// kept: a driver slow to start after a boot answers the next time.
fn find_memory() -> Option<(u64, bool)> {
    const LIMIT: Duration = Duration::from_secs(5);
    let out = |bin: &str, args: &[&str]| run(bin, args, LIMIT, &AtomicBool::new(false));
    let nvidia = || {
        let mib = out("nvidia-smi", &["--query-gpu=memory.total", "--format=csv,noheader,nounits"])?;
        let all: u64 = mib.lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum();
        (all > 0).then_some(all << 20)
    };
    let mac = || out("sysctl", &["-n", "hw.memsize"])?.trim().parse().ok();
    let linux = || {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb = info.lines().find_map(|l| l.strip_prefix("MemTotal:"))?;
        Some(kb.trim().trim_end_matches("kB").trim().parse::<u64>().ok()? << 10)
    };
    if cfg!(target_os = "macos") {
        return Some((mac()?, true));
    }
    let asked = Instant::now();
    match nvidia() {
        Some(gpus) => Some((gpus, true)),
        None => Some((linux()?, asked.elapsed() < LIMIT)),
    }
}

/// The file Hugging Face's manifest for llama.cpp names, its size and its projector's (0 for
/// none), out of `{"ggufFile": {"rfilename": "x.gguf", "size": 1}, "mmprojFile": {...}}`.
fn gguf_file(manifest: &[u8]) -> Option<(String, u64, u64)> {
    #[derive(Deserialize)]
    struct File {
        rfilename: String,
        size: u64,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Manifest {
        gguf_file: File,
        mmproj_file: Option<File>,
    }
    let m: Manifest = serde_json::from_slice(manifest).ok()?;
    Some((m.gguf_file.rfilename, m.gguf_file.size, m.mmproj_file.map_or(0, |p| p.size)))
}

/// What a split file's parts share, in lower case: its path up to `-00001-of-00002.gguf`.
fn split_stem(path: &str) -> Option<String> {
    let p = path.to_ascii_lowercase();
    let mut ends = p.trim_end_matches(".gguf").rsplitn(4, '-');
    let num = |s: Option<&str>| s.is_some_and(|s| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit()));
    let split = num(ends.next()) && ends.next() == Some("of") && num(ends.next());
    ends.next().filter(|_| split).map(str::to_string)
}

/// The size of the weights ollama and llama.cpp take from a repo named with no quantization,
/// out of Hugging Face's `[{"path": "x-Q4_K_M.gguf", "size": 1}]`: the `file` named, else the
/// first Q4_K_M one, else the first `.gguf`, with every part of it when it is split
/// (`x-Q4_K_M-00001-of-00002.gguf`).
fn gguf_bytes(tree: &[u8], file: Option<&str>) -> Option<u64> {
    #[derive(Deserialize)]
    struct File {
        path: String,
        #[serde(default)]
        size: u64,
    }
    let files: Vec<File> = serde_json::from_slice(tree).ok()?;
    let lower = |f: &File| f.path.to_ascii_lowercase();
    let weights = || files.iter().filter(|f| lower(f).ends_with(".gguf") && !lower(f).contains("mmproj"));
    let whole = |f: &File| split_stem(&f.path);
    let named = || weights().find(|f| file.is_some_and(|n| f.path == n));
    let first = named().or_else(|| weights().find(|f| lower(f).contains("q4_k_m"))).or_else(|| weights().next())?;
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
    kept(SIZES)
}

/// Keeps `sizes` for the next run.
pub fn save_sizes(sizes: &HashMap<String, u64>) {
    keep(SIZES, sizes);
}

/// The file of `load_sizes`. Not `sizes.json`, whose sizes were of a file guessed from the
/// repo's list, a draft model's at times (`gguf_size`).
const SIZES: &str = "gguf-sizes.json";

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

/// Hugging Face said just now that it has no GGUF copy of the models `keys`: kept with `gone`, in
/// one write.
pub fn save_gone(gone: &mut HashMap<String, u64>, keys: impl IntoIterator<Item = String>) {
    gone.extend(keys.into_iter().map(|k| (k, now())));
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
    // unsloth's dynamic quantizations start with `ud`, `UD-Q4_K_XL`.
    let extra = |p: &str| {
        ["qat", "ud"].contains(&p)
            || (hub && p == "latest")
            || ["q", "iq", "tq", "mxfp", "fp", "bf", "f"].iter().any(|q| p.strip_prefix(q).is_some_and(digit))
            || (digit(p) && p.ends_with('k'))
    };
    // Whoever it was pulled from, `hf.co/bartowski/Qwen3.5-4B-GGUF:Q4_K_M` or `someone/qwen3.5:4b`.
    let tag = tag.rsplit('/').next().unwrap_or(tag).to_ascii_lowercase();
    // A file split in parts, `x-Q4_K_M-00001-of-00002.gguf`, by the name they share.
    let tag = split_stem(&tag).map_or(tag, |stem| stem + ".gguf");
    let (name, variant) = match tag.strip_suffix(".gguf") {
        // A file downloaded by hand, `gemma-3-4b-it-Q4_K_M.gguf`: its name, then its quantization.
        Some(file) => file.rsplit_once(['-', '.']).unwrap_or((file, "")),
        None => tag.split_once(':').unwrap_or((&tag, "")),
    };
    let name = name.trim_end_matches("-gguf").trim_end_matches("-ud");
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

/// The harness running modelcmp, as an agent's `pick` does, told by a variable it sets for
/// its commands. Copilot sets none, and omp's goes before pi's as it is pi's fork.
pub fn orchestrator() -> Option<&'static str> {
    static OURS: std::sync::LazyLock<Option<&str>> = std::sync::LazyLock::new(|| {
        let vars = [
            ("CLAUDECODE", "claude"),
            ("OPENCODE", "opencode"),
            ("OMPCODE", "omp"),
            ("PI_CODING_AGENT", "pi"),
            ("CODEX_THREAD_ID", "codex"),
            ("GEMINI_CLI", "gemini"),
        ];
        vars.into_iter().find(|(v, _)| std::env::var_os(v).is_some_and(|x| !x.is_empty())).map(|(_, h)| h)
    });
    *OURS
}

/// The harnesses that have only their developer's models (claude, codex, gemini): the one of a
/// model's own developer runs it, before a harness of anyone's models.
pub fn first_party() -> impl Iterator<Item = &'static str> {
    HARNESSES.iter().filter(|h| matches!(h.1, Probe::Provider(..))).map(|h| h.0)
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

/// Of offers you can use, the one you'd pay: the cheapest, a free one first, as OpenCode's
/// `beam-free` beside OpenRouter's paid Beam, else one with no listed price.
fn cheapest<'a>(offers: impl Iterator<Item = &'a Offer>) -> Option<&'a Offer> {
    let offers: Vec<&Offer> = offers.collect();
    let by_price = offers.iter().copied().filter(|o| !o.unpriced).min_by(|a, b| a.blended().total_cmp(&b.blended()));
    by_price.or(offers.first().copied())
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
    /// Epoch Capabilities Index: overall capability, roughly 100..175.
    pub eci: Option<f64>,
    /// Benchmark name -> best score across reasoning-effort settings, as Epoch scales it,
    /// 0..1 or as `score_ceiling` says, of every benchmark in Epoch's index.
    pub scores: BTreeMap<String, f64>,
    /// Task name -> its score on the ECI scale and what it is from (`fit::scored`), which
    /// ranks it.
    pub fit: crate::fit::Fit,
    /// The model's page name on artificialanalysis.ai, when it has one (`aa_pages`).
    #[serde(default)]
    pub aa: Option<String>,
    /// The model's page name on epoch.ai, when it has one (`epoch_listed`), though no task uses
    /// the benchmarks Epoch ran it on.
    #[serde(default)]
    pub epoch: Option<String>,
    /// Output tokens per second in OpenRouter's GPQA diamond runs, the median over the providers
    /// that ran it: end to end, the thinking and the time to the first token included
    /// (`openrouter_gpqa`).
    #[serde(default)]
    pub tps: Option<f64>,
    /// Its score on Arena's Agent leaderboard, in percent: the net improvement arena.ai measures
    /// in real agent sessions, the best of its reasoning settings (`Arena`).
    #[serde(default)]
    pub arena: Option<f64>,
    /// Its score on each of `ARENA_MORE` that ranks it, by name, in percent as `arena`, and its
    /// rating on each of `ARENA_BOARDS`.
    #[serde(default)]
    pub arena_more: BTreeMap<String, f64>,
    /// The keys of the rows merged into it, which Arena may rank it under: "nemotron3ultra" of
    /// "Nemotron 3 Ultra 550B A55B".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aka: Vec<String>,
    #[serde(skip)]
    pub available: bool,
    /// Every `Offer::via` of the model, once each.
    #[serde(skip)]
    pub via: Vec<String>,
}

impl Model {
    /// What a column that picked `b` in its dropdown shows: its score on the benchmark `b` in
    /// percent, or Arena's agent score or one of `arena_more` as it is.
    pub fn picked(&self, b: &str) -> Option<f64> {
        if b == ARENA_AGENT {
            return self.arena;
        }
        self.arena_more.get(b).copied().or_else(|| self.scores.get(b).map(|s| s * 100.0))
    }

    /// The offer you'd actually use: the cheapest available one, a free one first, else one of
    /// yours with no listed price, else the `list` one. It may be `unpriced`,
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

    /// The offer `harness` runs the model on: of the ones it reaches, the one you'd pay, your
    /// machine's copy only when it reaches no other, as with `price`: a free quant is not the
    /// model the scores are of.
    pub fn offer_via(&self, harness: &str) -> Option<&Offer> {
        let via = || self.offers.iter().filter(|o| o.via.iter().any(|v| v == harness));
        cheapest(via().filter(|o| !o.local)).or_else(|| cheapest(via()))
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

    /// The model's page on epoch.ai, when it has one there (`epoch`).
    pub fn epoch_page(&self) -> Option<String> {
        self.epoch.as_ref().map(|id| format!("https://epoch.ai/models/{id}"))
    }

    /// The model's page on openrouter.ai, when OpenRouter lists it (`openrouter`).
    pub fn openrouter_page(&self) -> Option<String> {
        self.openrouter.as_ref().map(|id| format!("https://openrouter.ai/{id}"))
    }

    /// The model's page on artificialanalysis.ai, when it has one there (`aa`).
    fn aa_page(&self) -> Option<String> {
        self.aa.as_ref().map(|id| format!("https://artificialanalysis.ai/models/{id}"))
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

    /// Arena's Agent leaderboard, for a model it ranks (`arena`), else the first of
    /// `ARENA_BOARDS` that rates it: Arena has no page per model.
    pub fn arena_page(&self) -> Option<String> {
        let board = ARENA_BOARDS.into_iter().find(|b| self.arena_more.contains_key(b.0)).map(|b| b.1);
        self.arena.map(|_| ARENA_PAGE).or(board).map(String::from)
    }

    /// The model's pages, (site, url), of the sites that have one: models.dev, Epoch, Artificial
    /// Analysis, OpenRouter, Hugging Face (`hf_repo`), then Arena's leaderboard (`arena_page`).
    /// None is a guess: a model no site has a page for has no link.
    pub fn links(&self) -> Vec<(&'static str, String)> {
        let md = self.md_page().map(|url| ("models.dev", url));
        let epoch = self.epoch_page().map(|url| ("epoch.ai", url));
        let aa = self.aa_page().map(|url| ("artificialanalysis.ai", url));
        let or = self.openrouter_page().map(|url| ("openrouter.ai", url));
        let hf = self.hf_repo().map(|repo| ("huggingface.co", format!("https://huggingface.co/{repo}")));
        let arena = self.arena_page().map(|url| ("arena.ai", url));
        md.into_iter().chain(epoch).chain(aa).chain(or).chain(hf).chain(arena).collect()
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
    /// The day Arena published each of `ARENA_BOARDS` that `Model::arena_more` has, by name.
    #[serde(default)]
    pub arena_dates: BTreeMap<String, String>,
    /// huggingface.co did not send the leaderboard and no kept scores stood in: the Agentic column
    /// is empty, and the cache is stale after `RETRY`.
    #[serde(default)]
    pub arena_missed: bool,
    /// The task benchmarks Epoch sent no scores of, as when its index names no file for one:
    /// their scores are empty, and the cache is stale after `RETRY`.
    #[serde(default)]
    pub missed: Vec<String>,
    pub models: Vec<Model>,
    /// What went wrong in a refresh that still gave data, for the caller to show. Cached, so the
    /// frame says the last refresh had issues until the next one, after a restart too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// A harness still asked for its models has no listing, not even the cache's: a refresh's
    /// early data, too soon to say that there is access to no model.
    #[serde(skip)]
    pub listing: bool,
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
        (version(&self.latest) > version(env!("CARGO_PKG_VERSION"))).then_some(self.latest.as_str())
    }

    /// Whether it was fetched on another day than today, by the local clock. The TUI's first
    /// start of a day refreshes behind the table, where a command would wait for the download.
    pub fn of_another_day(&self) -> bool {
        local_day(self.fetched) != local_day(now())
    }

    pub fn stale(&self) -> bool {
        // Dated ahead of the clock, it would never grow old.
        // What Epoch missed is said at every load, so it is asked for again.
        let unranked = (self.arena_missed || !self.missed.is_empty()) && self.age() > RETRY;
        self.age() > MAX_AGE
            || unranked
            || self.fetched > now()
            || self.format != FORMAT
            || self.benches != crate::fit::task_benches()
    }

    /// What a load of the cache says: what its refresh said, as that one's, else what `missed`
    /// leaves empty.
    pub fn said(&self) -> Option<String> {
        self.warning.as_ref().map(|w| format!("{LAST_REFRESH}{w}")).or_else(|| self.unscored())
    }

    /// What `missed` leaves empty, said at every load until a refresh brings it.
    pub fn unscored(&self) -> Option<String> {
        (!self.missed.is_empty()).then(|| format!("epoch.ai sent no {} scores", self.missed.join(", ")))
    }

    /// Mark offers the user can use, and where: listed by an installed harness. A model ollama
    /// or llama.cpp runs here gets an offer of its own from each, free, for the tag that names it
    /// (`local_key`).
    pub fn apply_available(&mut self) {
        let keys: HashSet<&str> = self.models.iter().map(|m| m.key.as_str()).collect();
        let repos = REPOS.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_default();
        let tags = |h: &str| Some(local_tags(self.harness.get(h)?, &keys, &repos));
        let local: Vec<_> = LOCAL.iter().filter_map(|(h, name)| Some((*h, *name, tags(h)?))).collect();
        // Each id listed, `provider/*` too, with the harnesses that list it: one search per offer,
        // not one per harness.
        let mut listing: HashMap<String, Vec<&str>> = HashMap::new();
        for (h, ids) in &self.harness {
            for i in ids {
                let on = listing.entry(canonical(h, i)).or_default();
                if on.last() != Some(&h.as_str()) {
                    on.push(h);
                }
            }
        }
        let on = |id: &str| listing.get(id).map_or(&[][..], Vec::as_slice);
        let mut id = String::new();
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
                id.clear();
                id.extend([o.provider.as_str(), "/", o.id.as_str()]);
                let named = on(&id);
                id.truncate(o.provider.len() + 1);
                id.push('*');
                let all = on(&id);
                // In the harnesses' order, as one listing it both ways counts once.
                o.via = self
                    .harness
                    .keys()
                    .filter(|h| named.contains(&h.as_str()) || all.contains(&h.as_str()))
                    .cloned()
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
        // Or as pi and omp name the provider, which is the id `pick` prints for them, or whole
        // as llama-cli's `-hf` takes it: "unsloth/Qwen3.8-GGUF:Q8_0".
        let pi = norm(&canonical("pi", query));
        // Some(false) by a whole `provider/id`, Some(true) by a bare one only. Read as norm makes
        // them, with no String for each of the thousands of offers: that took 8 ms.
        let by_id = |m: &Model| {
            let hit = |o: &Offer| {
                let full = || normed(&o.provider).chain(normed(&o.id));
                let whole = full().eq(q.bytes()) || full().eq(pi.bytes());
                let bare = o.id.rsplit('/').next().unwrap_or(&o.id);
                (whole || normed(bare).eq(q.bytes()) || normed(&o.id).eq(q.bytes())).then_some(!whole)
            };
            m.offers.iter().filter_map(hit).min()
        };
        // Or by the name shown, which says more than the key when it is Epoch's: "Kimi K2 (Sep 2025)".
        let by_name = |m: &&Model| normed(&m.name).eq(q.bytes());
        let all = || self.models.iter();
        // A whole `provider/id` names its model before another provider's bare id does: NanoGPT
        // sells `google/gemini-3.1-pro-preview-customtools` under Gemini 3.1 Pro. Two models can go
        // by one bare id at different providers: yours first.
        let id = || all().filter_map(|m| Some(((by_id(m)?, !m.available), m))).min_by_key(|r| r.0).map(|r| r.1);
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

/// The day `secs` since 1970 falls in by the local clock, as its year and its day of the year.
#[cfg(unix)]
#[allow(unsafe_code)]
fn local_day(secs: u64) -> (i32, i32) {
    let t = libc::time_t::try_from(secs).unwrap_or(0);
    // SAFETY: a `tm` of zeroes is a valid one, and `localtime_r` keeps neither pointer.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    (tm.tm_year, tm.tm_yday)
}

// ponytail: UTC's day where there is no `localtime_r`; ask Windows for its zone if it matters.
#[cfg(not(unix))]
fn local_day(secs: u64) -> (i32, i32) {
    (0, (secs / 86400) as i32)
}

fn cache_path() -> PathBuf {
    cache_file("data.json")
}

/// When the file at `p` was last written, which a refresh of another run changes.
fn written(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub fn load_cache() -> Option<Data> {
    load_repos();
    let bytes = std::fs::read(cache_path()).ok()?;
    // One written in another format reads wrong here (a missing price as free), so it is no fallback.
    let mut d = serde_json::from_slice::<Data>(&bytes).ok().filter(|d| d.format == FORMAT)?;
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

/// Whether Windows Terminal, which opens the new tab under WSL, would split its command line at
/// `text`: it does on `;` wherever it stands.
pub fn splits(text: &str) -> bool {
    text.contains(';') && std::env::var_os("WSL_DISTRO_NAME").is_some()
}

/// `path` with a leading `~` as your home folder, as a shell reads it.
pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => path.into(),
    }
}

/// `path` with its last part completed as a shell completes a folder: to the one folder that
/// starts with it, a `/` after it, else as far as the ones that do agree, which come with it.
/// A hidden folder is one only for a part starting with `.`.
pub fn complete(path: &str) -> (String, Vec<String>) {
    if path == "~" {
        return ("~/".into(), vec![]);
    }
    let (parent, start) = path.split_at(path.rfind('/').map_or(0, |i| i + 1));
    let dir = if parent.is_empty() { ".".into() } else { expand(parent) };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        // By name first: asking what each entry is can be slow, as on a Windows drive under WSL.
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e)))
        .filter(|(n, _)| n.starts_with(start) && (start.starts_with('.') || !n.starts_with('.')))
        .filter(|(_, e)| e.path().is_dir())
        .map(|(n, _)| n)
        .collect();
    names.sort();
    let (Some(first), Some(last)) = (names.first(), names.last()) else { return (path.into(), names) };
    // Sorted, so what the first and the last share they all do.
    let same: usize = first.chars().zip(last.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
    let slash = if names.len() == 1 { "/" } else { "" };
    let done = format!("{parent}{}{slash}", &first[..same]);
    (done, if names.len() > 1 { names } else { vec![] })
}

/// `cmd` as a new WSL session starts it in `dir`, else here, under WSL with `wsl.exe` on `PATH`: bare, so a login
/// shell sets up PATH and keys as for a typed command. A harness is asked for its models the way
/// it is launched, as a key exported by hand in this shell, which opencode would list a provider
/// for, is not in the new tab.
pub fn wsl_session(cmd: &[String], dir: Option<&str>) -> Option<Vec<String>> {
    let distro = std::env::var("WSL_DISTRO_NAME").ok().filter(|_| installed("wsl.exe"))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let cwd = match dir {
        Some(d) => d.to_string(),
        None => std::env::current_dir().ok()?.to_string_lossy().into_owned(),
    };
    Some(["wsl.exe", "-d", &distro, "--cd", &cwd, "-e", &shell, "-lic", &exec_line(cmd)].map(String::from).to_vec())
}

/// What `bin` prints for `args`, asked in a new session under WSL (`wsl_session`), else here.
fn listing(bin: &str, args: &[&str], stop: &AtomicBool) -> Option<String> {
    let cmd: Vec<String> = std::iter::once(bin).chain(args.iter().copied()).map(String::from).collect();
    let argv = wsl_session(&cmd, None).unwrap_or(cmd);
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    // opencode can take longer than the others to list its models.
    let secs = if bin == "opencode" { 60 } else { 30 };
    run(&argv[0], &args, Duration::from_secs(secs), stop)
}

/// What a harness answered: its ids, or none when its listing failed, hung or was cut short.
type Listing = (String, Option<Vec<String>>);

/// What each harness `asked` says it can use, sent to `answers` as it answers: its ids, or
/// none when its listing failed, hung or was cut short by `stop`, so a refresh always finishes.
/// All are asked at once, so the slowest is the wait.
fn harness_models(asked: &[&(&'static str, Probe)], stop: &AtomicBool, steps: &Steps, answers: &Sender<Listing>) {
    let ids = |bin: &str, probe: &Probe| match probe {
        // Logged out, it runs none. A check that fails or hangs cannot tell, so it may run them all.
        Probe::Provider(p, out) => Some(match listing("sh", &["-c", out], stop) {
            Some(_) => vec![],
            None => vec![format!("{p}/*")],
        }),
        Probe::List(args) => {
            let out = listing(bin, args, stop)?;
            Some(out.lines().map(str::trim).filter(|l| l.contains('/')).map(String::from).collect())
        }
        Probe::Table(args) => table_ids(&listing(bin, args, stop)?),
        // A server that is down says so at once and runs none: no model is kept for it. One that
        // hangs cannot tell, so it keeps its last listing as the others do.
        Probe::Names(args) => {
            let out = listing("sh", &["-c", &format!("{bin} {} || true", args.join(" "))], stop)?;
            Some(name_ids(&out).unwrap_or_default())
        }
        Probe::Cache(args) => {
            let cmd = [llama_cmd()?, args].concat();
            let out = listing(cmd[0], &cmd[1..], stop)?;
            Some(cache_ids(&out).unwrap_or_default().into_iter().chain(gguf_ids()).collect())
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

/// The pages linked at the last refresh, Artificial Analysis's then Epoch's, from the cache: the
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
    let session = answer("copilot", &["--acp"], &dir, &asks, 2, Duration::from_secs(30), stop)?;
    // Copilot keeps every session for you to resume, this one too, which is none of yours.
    let home =
        std::env::var_os("COPILOT_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".copilot")));
    if let Some(kept) = copilot_session(&session, home) {
        let _ = std::fs::remove_dir_all(kept);
    }
    copilot_enabled(&session)
}

/// The folder Copilot's CLI keeps the session of a new session's answer in, under its `home`:
/// none for an id that is no plain name, which could name another folder.
fn copilot_session(answer: &serde_json::Value, home: Option<PathBuf>) -> Option<PathBuf> {
    let plain = |id: &&str| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    Some(home?.join("session-state").join(answer["result"]["sessionId"].as_str().filter(plain)?))
}

/// The ids of a new session's answer that Copilot's CLI takes, under models.dev's provider
/// for them: the ones no policy keeps off, switched off or not yet on. None for an error. A
/// session without models is of a CLI not logged in, which opens one all the same: it takes none.
fn copilot_enabled(answer: &serde_json::Value) -> Option<Vec<String>> {
    let taken = |m: &&serde_json::Value| {
        let state = &m["_meta"]["copilotEnablement"];
        state.is_null() || state == "enabled"
    };
    let session = &answer["result"];
    session["sessionId"].as_str()?;
    let ids = session["models"]["availableModels"].as_array().into_iter().flatten().filter(taken);
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
    use std::process::Stdio;
    use std::sync::mpsc::RecvTimeoutError::Timeout;
    if stop.load(Relaxed) {
        return None;
    }
    let (io, null) = (Stdio::piped, Stdio::null());
    let mut child = grouped(bin).args(args).current_dir(cwd).stdin(io()).stdout(io()).stderr(null).spawn().ok()?;
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
    kill(&mut child);
    let _ = child.wait();
    answer
}

/// `bin args` stdout, or `None` when it is missing, fails, or is killed at `limit` or on `stop`,
/// which also keeps it from starting.
fn run(bin: &str, args: &[&str], limit: Duration, stop: &AtomicBool) -> Option<String> {
    use std::process::Stdio;
    use std::sync::mpsc::RecvTimeoutError::Timeout;
    if stop.load(Relaxed) {
        return None;
    }
    let mut child =
        grouped(bin).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
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
                kill(&mut child);
                // Reaped off the caller: one stuck in the kernel, as `nvidia-smi` in a wedged
                // driver, takes the kill only once it is out, which may be never.
                std::thread::spawn(move || drop(child.wait()));
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

/// `bin` in a process group of its own, which `kill` ends whole.
fn grouped(bin: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(bin);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd
}

/// Kills `child` with what it started, as the rest of a pipeline `sh -c` runs, which would run
/// on and hold its output open.
fn kill(child: &mut std::process::Child) {
    // Not once reaped, as its id may then be another's.
    #[cfg(unix)]
    if let (Ok(None), Ok(id)) = (child.try_wait(), libc::pid_t::try_from(child.id())) {
        #[allow(unsafe_code)]
        // SAFETY: `kill` takes no pointer, and the group is the child's own (`grouped`).
        unsafe {
            libc::kill(-id, libc::SIGKILL);
        }
    }
    let _ = child.kill();
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

/// `refresh_held` under the cache's lock: the TUI's refresh and a command's (`load`) run one at
/// a time, and a command that comes second reads what the first wrote.
pub fn refresh(steps: &Steps, early: Option<impl FnOnce(Data)>) -> Result<Data, String> {
    let _held = crate::store::lock(&cache_path());
    refresh_held(steps, early)
}

/// Download the sources and ask the harnesses in parallel, merge, write cache. `steps` counts
/// them for `progress`. `early` gets the data as soon as it is downloaded, when harnesses are
/// still listing their models, which have the ones the cache had until the whole answer: they
/// are not waited for to show the rest. Nor is Arena's leaderboard, whose column has the scores
/// the cache had until it is in.
fn refresh_held(steps: &Steps, early: Option<impl FnOnce(Data)>) -> Result<Data, String> {
    load_repos();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, answers) = std::sync::mpsc::channel();
    // The installed ones: one not installed is left out.
    let asked: Vec<_> = HARNESSES.iter().filter(|h| has(h.0)).collect();
    let harness = {
        let (asked, stop, steps) = (asked.clone(), Arc::clone(&stop), Arc::clone(steps));
        std::thread::spawn(move || harness_models(&asked, &stop, &steps, &tx))
    };
    let res = download(steps);
    // A refresh that cannot finish kills the harnesses rather than wait for them.
    stop.store(res.is_err(), Relaxed);
    let (mut data, [aa, epoch, openrouter], release, late) = match res {
        Ok(d) => d,
        Err(e) => {
            let _ = harness.join();
            return Err(e);
        }
    };
    // Only links hang on them, so without one the refresh still succeeds, and says so.
    let text = |xml: Result<Vec<u8>, String>| String::from_utf8_lossy(&xml.unwrap_or_default()).into_owned();
    let (aa, epoch) = (text(aa), text(epoch));
    let (mut aa, mut epoch) = (sitemap(&aa, "artificialanalysis.ai"), sitemap(&epoch, "epoch.ai"));
    // Read once, and only by a refresh that a list of pages or a harness's models did not reach,
    // or not yet.
    let cache = std::cell::LazyCell::new(|| std::fs::read(cache_path()).unwrap_or_default());
    // Only a page a site lists is linked; without its list, only one linked at the last refresh.
    let before = if aa.is_empty() || epoch.is_empty() { cached_pages(&cache) } else { Default::default() };
    let was = std::cell::LazyCell::new(|| Listed::of(&cache));
    let mut unpaged = [None, None];
    for (i, (pages, site)) in [(&mut aa, "artificialanalysis.ai"), (&mut epoch, "epoch.ai")].into_iter().enumerate() {
        if pages.is_empty() {
            let so = if before[i].is_empty() { "may be missing" } else { "are kept from the last refresh" };
            unpaged[i] = Some(format!("{site} did not list its pages: links to it {so}"));
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
    // Only the update notice hangs on it, so without it the refresh still succeeds.
    data.latest = release.ok().and_then(|b| release_tag(&b)).unwrap_or_default();
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
        // Arena's leaderboard, the slowest download, is not waited for either.
        if let Some(mut arena) = Arena::cached(&cache, now()) {
            if !arena.current(now()) {
                arena = Arena { boards: arena.boards, ..Default::default() };
            }
            arena.score(&mut first.models);
            (first.arena_date, first.arena_dates) = (arena.date.clone(), arena.dates());
        }
        first.apply_available();
        early(first);
    }
    // After the early data, which does not wait for it, and while the harnesses answer.
    step(steps, "huggingface.co");
    let (mut found, mut asks): (Asked, _) = (kept("hf.json"), HF_ASKS);
    hf_found(&mut data.models, &mut found, &mut asks, hf_spelled);
    keep("hf.json", &found);
    answered(steps, "huggingface.co");
    // Only Agentic hangs on it: without it, the scores of the last refresh are kept,
    // and none show once Arena has published no leaderboard for `ARENA_MAX_AGE`.
    let [arena, more @ ..] = late.all(steps);
    let ranked = Arena::parse(&arena.unwrap_or_default());
    let unranked = ranked.is_none().then_some("could not get Arena's agent leaderboard from huggingface.co");
    let mut kept = Arena::cached(&cache, now()).unwrap_or_default();
    let mut kept_boards = std::mem::take(&mut kept.boards);
    let arena = ranked.or(Some(kept).filter(|k| !k.scores.is_empty())).unwrap_or_default();
    let unranked = match (unranked, arena.current(now())) {
        (Some(e), true) => Some(format!("{e}, so its scores are kept from the last refresh")),
        (Some(e), false) => {
            data.arena_missed = true;
            Some(format!("{e}, so the Agentic column is empty"))
        }
        (None, false) => Some(format!(
            "Arena's agent leaderboard is from {}, too old to show, so the Agentic column is empty",
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
    let (signals, mut pages) = more.split_at(ARENA_MORE.len());
    for ((name, _), rows) in ARENA_MORE.into_iter().zip(signals) {
        let signal = rows.as_ref().ok().and_then(|r| Arena::parse(r)).filter(|s| s.date == arena.date);
        arena.more.extend(signal.map(|s| (name, s.scores)));
    }
    // Of its own day, and current, else the one of the last refresh while it is: one bad page is
    // not a day without it.
    let mut uncut = vec![];
    for (name, _, urls) in ARENA_BOARDS {
        let got;
        (got, pages) = pages.split_at(urls.len());
        let board = Arena::board(got.iter().map(|r| r.as_deref().ok())).filter(|b| b.current(now()));
        let Some(board) = board.or_else(|| kept_boards.remove(name)) else { continue };
        let said =
            || format!("Arena's {name} leaderboard has more rows than were read: some models have no {name} rating");
        uncut.extend(board.cut.then(said));
        arena.boards.insert(name, board);
    }
    arena.score(&mut data.models);
    data.arena_date.clone_from(&arena.date);
    data.arena_dates = arena.dates();
    // Until the last harness has answered. One that never does, its thread dead, is silent.
    listed.extend(answers.iter());
    let _ = harness.join();
    for h in awaited {
        listed.entry(h.to_string()).or_insert(None);
    }
    // Of this refresh's answers: a harness of its developer's models lists none only when it is
    // sure to be logged out, and copilot none when it is.
    let logged_out: Vec<String> = HARNESSES
        .iter()
        .filter(|h| matches!(h.1, Probe::Provider(..) | Probe::Copilot))
        .filter(|h| matches!(listed.get(h.0), Some(Some(ids)) if ids.is_empty()))
        .map(|h| format!("{} is installed but not logged in, so its models are left out", h.0))
        .collect();
    let silent;
    (data.harness, silent) = keep_listed(listed, || was.harness(false));
    let lost;
    (data.kept, lost) = silent.into_iter().partition(|h| data.harness.contains_key(h));
    let unlisted = |hs: &[String], kept: &str| {
        let its = if hs.len() > 1 { "their" } else { "its" };
        (!hs.is_empty()).then(|| format!("{} did not list {its} models{kept}", hs.join(", ")))
    };
    let common: Vec<String> =
        [unlisted(&data.kept, ", so the ones from the last refresh are kept"), unlisted(&lost, "")]
            .into_iter()
            .chain(unpaged)
            .chain([unnamed, unranked])
            .flatten()
            .chain(logged_out)
            .chain(uncut)
            .collect();
    // Epoch's own, then the ones of the whole refresh.
    let all: Vec<_> = data.warning.take().into_iter().chain(data.unscored()).chain(common).collect();
    data.warning = Some(all.join(SEP)).filter(|w| !w.is_empty());
    let json = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    // An unwritable cache costs the next start a download, not this one its data.
    let uncached = crate::store::write_atomic(&cache_path(), &json)
        .err()
        .map(|e| format!("could not cache the data in {}: {e}", cache_path().display()));
    // What no cache holds, said only now.
    let all: Vec<_> = data.warning.take().into_iter().chain(uncached).collect();
    data.warning = Some(all.join(SEP)).filter(|w| !w.is_empty());
    data.apply_available();
    Ok(data)
}

/// What `download` gives: the merged data, then Artificial Analysis's and Epoch's sitemaps,
/// OpenRouter's models and modelcmp's newest release, which a refresh can do without, and
/// Arena's downloads, still under way.
type Downloaded = (Data, [Result<Vec<u8>, String>; 3], Result<Vec<u8>, String>, Late);

/// Where Arena's downloads start in `download`'s: its agent leaderboard, then `ARENA_MORE`, then
/// the pages of `ARENA_BOARDS`.
const ARENA_AT: usize = 7;

/// How many of `download`'s are Arena's.
const ARENA_URLS: usize = 1 + ARENA_MORE.len() + {
    let (mut n, mut b) = (0, 0);
    while b < ARENA_BOARDS.len() {
        (n, b) = (n + ARENA_BOARDS[b].2.len(), b + 1);
    }
    n
};

/// Arena's downloads, which `download` does not wait for: the ones it `got`, and the rest as
/// they come, until `end`.
struct Late {
    got: [Result<Vec<u8>, String>; ARENA_URLS],
    rx: std::sync::mpsc::Receiver<(usize, Result<Vec<u8>, String>)>,
    end: Instant,
    sites: Vec<&'static str>,
}

impl Late {
    /// All of them, waited for until `end`: one that did not come is a failure.
    fn all(mut self, steps: &Steps) -> [Result<Vec<u8>, String>; ARENA_URLS] {
        let timed =
            std::iter::from_fn(|| self.rx.recv_timeout(self.end.saturating_duration_since(Instant::now())).ok());
        // The ones that came before `end`, when it is past.
        for (i, res) in timed.chain(self.rx.try_iter()) {
            if let Some(at) = i.checked_sub(ARENA_AT) {
                self.got[at] = res;
            }
        }
        self.sites.into_iter().for_each(|site| answered(steps, site));
        self.got
    }
}

/// How long the downloads a refresh can do without are waited for once it has the others.
const GRACE: Duration = Duration::from_secs(10);

/// The sources, downloaded in parallel and merged. It returns at the first failure of a download
/// it cannot do without, models.dev's and Epoch's scores, leaving the others to end on their
/// own: none is waited for once the refresh cannot finish, and the links and the newest release
/// no longer than `GRACE` once it can. Arena's are not waited for at all, but by `Late`.
fn download(steps: &Steps) -> Result<Downloaded, String> {
    const URLS: [&str; ARENA_AT + ARENA_URLS] = {
        let head = [
            MODELS_URL,
            EPOCH_URL,
            AA_URL,
            EPOCH_PAGES_URL,
            OPENROUTER_URL,
            OPENROUTER_GPQA_URL,
            RELEASE_URL,
            ARENA_URL,
        ];
        let (mut urls, mut n, mut i) = ([""; ARENA_AT + ARENA_URLS], 0, 0);
        while n < head.len() {
            urls[n] = head[n];
            n += 1;
        }
        while i < ARENA_MORE.len() {
            (urls[n], n, i) = (ARENA_MORE[i].1, n + 1, i + 1);
        }
        let mut b = 0;
        while b < ARENA_BOARDS.len() {
            i = 0;
            while i < ARENA_BOARDS[b].2.len() {
                (urls[n], n, i) = (ARENA_BOARDS[b].2[i], n + 1, i + 1);
            }
            b += 1;
        }
        assert!(n == urls.len());
        urls
    };
    let needed = [0, 1];
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut sites, mut left) = (vec![], 0);
    for (i, url) in URLS.into_iter().enumerate() {
        let (tx, steps) = (tx.clone(), Arc::clone(steps));
        let pages = url == EPOCH_PAGES_URL;
        // Counted under its site's name, as the warnings name it.
        let site =
            url.split('/').nth(2).unwrap_or(url).trim_start_matches("api.").trim_start_matches("datasets-server.");
        step(&steps, site);
        sites.push((i, site));
        left += usize::from(i < ARENA_AT);
        std::thread::spawn(move || {
            let res = if pages { epoch_pages(url) } else { fetch(url) };
            answered(&steps, site);
            tx.send((i, res))
        });
    }
    drop(tx);
    let mut got: [Result<Vec<u8>, String>; ARENA_AT + ARENA_URLS] = URLS.map(|url| Err(format!("{url}: no reply")));
    // Once the two that are needed are in, the rest get `GRACE` and no more.
    let (mut missing, mut end) = (needed.len(), None::<Instant>);
    while left > 0 {
        let next = match end {
            None => rx.recv().ok(),
            Some(end) => rx.recv_timeout(end.saturating_duration_since(Instant::now())).ok(),
        };
        let Some((i, res)) = next else { break };
        if needed.contains(&i) {
            res.as_ref().map_err(String::clone)?;
            missing -= 1;
            if missing == 0 {
                end = Some(Instant::now() + GRACE);
            }
        }
        left -= usize::from(i < ARENA_AT);
        got[i] = res;
    }
    // A download not waited for is awaited no more, before the merge and not after it.
    let (sites, late): (Vec<_>, Vec<_>) = sites.into_iter().partition(|(i, _)| *i < ARENA_AT);
    sites.into_iter().for_each(|(_, site)| answered(steps, site));
    let [models, epoch, aa, epoch_pages, openrouter, gpqa, release, arena @ ..] = got;
    let sites = late.into_iter().map(|(_, site)| site).collect();
    let late = Late { got: arena, rx, end: end.unwrap_or_else(Instant::now), sites };
    let ep = parse_epoch(&epoch?)?;
    let mut data = merge(&models?, &ep)?;
    epoch_named(&mut data.models, &ep);
    data.missed.clone_from(&ep.missed);
    // Only Tok/s hangs on OpenRouter's runs: without them the refresh still succeeds, and says so.
    match gpqa.map(|csv| openrouter_speeds(&csv)) {
        Ok(speeds) => {
            for m in &mut data.models {
                m.tps = m.openrouter.as_ref().and_then(|id| speeds.get(id)).copied();
            }
        }
        Err(e) => {
            data.warning = Some(format!("could not get OpenRouter's GPQA diamond runs ({e}), so no model has a speed"))
        }
    }
    for m in &mut data.models {
        m.fit = crate::fit::scored(m.eci, &m.scores, &ep.benches);
    }
    Ok((data, [aa, epoch_pages, openrouter], release, late))
}

/// The median of `xs`, none of an empty list.
fn median(xs: &mut [f64]) -> Option<f64> {
    xs.sort_by(f64::total_cmp);
    match xs.len() {
        0 => None,
        n if n % 2 == 1 => Some(xs[n / 2]),
        n => Some((xs[n / 2 - 1] + xs[n / 2]) / 2.0),
    }
}

/// The output tokens a second of OpenRouter's GPQA diamond runs by model slug, from its benchmark
/// leaderboard `csv`: the tokens a task took over its duration, end to end, the median over the
/// providers that ran it, or the auto-routing row's when no provider has one. Not its accuracy:
/// GPQA is too easy for today's best models to tell them apart. Nor its tau-bench rows.
fn openrouter_speeds(csv: &[u8]) -> HashMap<String, f64> {
    let mut rdr = csv::Reader::from_reader(csv);
    let headers = rdr.headers().cloned().unwrap_or_default();
    let at = |name: &str| headers.iter().position(|c| c == name);
    let (Some(bench), Some(slug), Some(provider), Some(accuracy)) =
        (at("benchmark"), at("model_slug"), at("provider_name"), at("accuracy"))
    else {
        return HashMap::new();
    };
    let (tokens, ms) = (at("avg_output_tokens_per_task"), at("avg_duration_per_task_ms"));
    // slug -> the providers' speeds, and the auto-routing row's
    let mut by_slug: HashMap<String, (Vec<f64>, Option<f64>)> = HashMap::new();
    for r in rdr.records().flatten() {
        let get = |i: usize| r.get(i).unwrap_or_default();
        let num = |i: Option<usize>| get(i?).parse::<f64>().ok().filter(|v| v.is_finite());
        // A row with no accuracy is no run.
        if get(bench) != "gpqa_diamond" || get(slug).is_empty() || num(Some(accuracy)).is_none() {
            continue;
        }
        let tps = num(tokens).zip(num(ms).filter(|ms| *ms > 0.0)).map(|(tokens, ms)| tokens / (ms / 1000.0));
        let e = by_slug.entry(get(slug).to_string()).or_default();
        if get(provider) == "auto-routing" { e.1 = tps } else { e.0.extend(tps) }
    }
    by_slug
        .into_iter()
        .filter_map(|(slug, (mut providers, auto))| {
            let tps = if providers.is_empty() { auto } else { median(&mut providers) };
            Some((slug, tps?))
        })
        .collect()
}

/// What a warning from the cached data's refresh starts with (`Data::said`).
pub const LAST_REFRESH: &str = "last refresh: ";
/// Between the warnings of a refresh, which `!` and `modelcmp` split on to list them.
pub const SEP: &str = " · ";

/// When a command's refresh last failed to download, and why, under `FAILED_KEY`: offline, every
/// command would otherwise wait on a refresh of its own to fail.
const FAILED: &str = "failed.json";
const FAILED_KEY: &str = "epoch";

/// Cached data, refreshing if missing or stale. Falls back to stale cache when offline.
pub fn load(force: bool) -> Result<(Data, Option<String>), String> {
    // Tried again once `RETRY` has passed, or when you ask for it.
    let failed = || {
        let (at, e) = kept::<(u64, String)>(FAILED).remove(FAILED_KEY)?;
        let ago = now().saturating_sub(at);
        (ago < RETRY.as_secs()).then(|| {
            let (ago, next) = (Duration::from_secs(ago), RETRY - Duration::from_secs(ago));
            let age = crate::view::age;
            format!("{e}, {} ago, not tried again until {} from now or --refresh", age(ago), age(next))
        })
    };
    let kept_from = |d: Data, e: &str| {
        let w = format!("refresh failed ({e}), so this is the cached data from {} ago", crate::view::age(d.age()));
        Ok((d, Some(w)))
    };
    // Before the read, so that a refresh another run makes after it is read below.
    let read = written(&cache_path());
    match load_cache() {
        Some(d) if !force && !d.stale() => {
            let w = d.said();
            Ok((d, w))
        }
        cached => {
            // One refresh at a time: agents that find the cache stale together wait for the
            // first and read what it wrote. Unwritten, it is the same cache, not read twice.
            let _held = crate::store::lock(&cache_path());
            let wrote = written(&cache_path()) != read;
            if !force
                && wrote
                && let Some(d) = load_cache().filter(|d| !d.stale())
            {
                let w = d.said();
                return Ok((d, w));
            }
            let cached = match (cached, (!force).then(failed).flatten()) {
                (Some(d), Some(e)) => return kept_from(d, &e),
                (cached, _) => cached,
            };
            // A refresh can take up to a minute, which with nothing said looks like a hang.
            eprintln!("downloading model data, cached for 24h...");
            match refresh_held(&Steps::default(), None::<fn(Data)>) {
                Ok(mut d) => {
                    let w = d.warning.take();
                    Ok((d, w))
                }
                Err(e) => match cached {
                    Some(d) => {
                        let mut failed = kept::<(u64, String)>(FAILED);
                        failed.insert(FAILED_KEY.into(), (now(), e.clone()));
                        keep(FAILED, &failed);
                        kept_from(d, &e)
                    }
                    None => Err(format!("could not download model data: {e}")),
                },
            }
        }
    }
}

/// What every download goes through, given up at `limit`.
fn agent(limit: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(limit)).build().into()
}

/// "0.10.1" as its numbers, which compare as versions do.
fn version(s: &str) -> Vec<u64> {
    s.split('.').map(|n| n.parse().unwrap_or(0)).collect()
}

/// The version GitHub's answer names as the newest release, "0.2.0".
fn release_tag(body: &[u8]) -> Option<String> {
    let release = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    Some(release["tag_name"].as_str()?.trim_start_matches('v').to_string())
}

/// modelcmp's newest release, asked for alone: `None` when GitHub does not say.
pub fn newest_release() -> Option<String> {
    release_tag(&fetch_within(RELEASE_URL, Duration::from_secs(10)).ok()?)
}

/// `url`'s body.
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    fetch_within(url, Duration::from_secs(60))
}

/// `fetch`, given up on after `limit`.
fn fetch_within(url: &str, limit: Duration) -> Result<Vec<u8>, String> {
    request(url, None, limit).map_err(|e| format!("{url}: {e}"))
}

/// How many times a download is asked for: a site that fails once often answers the next time.
const TRIES: u32 = 3;

/// Whether asking again can help: not for an answer that says the request itself is wrong.
fn passing(e: &ureq::Error) -> bool {
    !matches!(e, ureq::Error::StatusCode(c) if (400..500).contains(c) && !matches!(c, 408 | 429))
}

/// `fetch_within`, with the site's own error for one that reads its status. A failure that can
/// pass is tried again, up to `TRIES` times and all within `limit`. `header` is sent with it.
fn request(url: &str, header: Option<(&str, &str)>, limit: Duration) -> Result<Vec<u8>, ureq::Error> {
    // One for every download, so a second request to a site goes over the first's connection.
    static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| agent(Duration::from_secs(60)));
    let end = Instant::now() + limit;
    let mut tried = 0;
    loop {
        tried += 1;
        let left = end.saturating_duration_since(Instant::now());
        let mut req = AGENT.get(url).config().timeout_global(Some(left)).build();
        if let Some((name, value)) = header {
            req = req.header(name, value);
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
fn epoch_pages(index: &str) -> Result<Vec<u8>, String> {
    let index = fetch(index)?;
    let mut all = Vec::new();
    for url in model_sitemaps(&String::from_utf8_lossy(&index)) {
        all.extend(fetch(url)?);
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

/// A release date, "2026-09-18" or "2026-09", in months since 1970.
// ponytail: months of the mean length, so a day or two off; nothing here turns on a day.
fn months(date: &str) -> Option<f64> {
    let mut parts = date.split('-').map(|p| p.parse::<f64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(15.0));
    Some((year - 1970.0) * 12.0 + (month - 1.0) + (day - 1.0) / 30.44)
}

/// Whether a leaderboard of `date` was published within `ARENA_MAX_AGE` of `now`, in seconds
/// since 1970.
fn fresh(date: &str, now: u64) -> bool {
    let age = months(date).map(|then| now as f64 / (30.44 * 86400.0) - then);
    age.is_some_and(|a| a <= ARENA_MAX_AGE)
}

/// Arena's Agent leaderboard: the day it was published, each model's score by the words of its
/// name, "gpt 6 high", in percent to a decimal, and whether it has rows that were not read.
#[derive(Default, PartialEq, Debug)]
struct Arena {
    date: String,
    scores: HashMap<String, f64>,
    cut: bool,
    /// Each of `ARENA_MORE` that came with it, by name: its scores, as `scores`.
    more: BTreeMap<&'static str, HashMap<String, f64>>,
    /// Each of `ARENA_BOARDS`, by name, of its own day: it shows whatever the agent leaderboard's.
    boards: BTreeMap<&'static str, Arena>,
}

impl Arena {
    /// `page` with a score at least.
    fn parse(rows: &[u8]) -> Option<Arena> {
        Arena::page(rows).filter(|a| !a.scores.is_empty())
    }

    /// From a page of the rows of its dataset, none when they are not that: `{"num_rows_total":
    /// 50, "rows": [{"row_idx": 0, "row": {"model_name": "Claude Opus 5 (High)", "score": 0.0867,
    /// "category": "overall", "leaderboard_publish_date": "2026-10-02"}}]}`, or a "rating" for a
    /// score in `ARENA_BOARDS`. Arena has a row per reasoning setting, named in brackets or at the
    /// end, "gpt-5.5-high": the model gets its best, as with Epoch's benchmarks (`score`). The
    /// overall rows come first: it is cut when a page ends in one with more rows after it.
    // ponytail: an order Arena keeps, not one it says. Ask its filter endpoint for the overall
    // rows if that changes, which was not up at the time.
    fn page(rows: &[u8]) -> Option<Arena> {
        #[derive(Deserialize)]
        struct Rows {
            #[serde(default)]
            num_rows_total: usize,
            rows: Vec<Wrapped>,
        }
        #[derive(Deserialize)]
        struct Wrapped {
            /// Its place in the dataset, from 0: a page with none is the first.
            row_idx: Option<usize>,
            row: Row,
        }
        // Any of them may be null: the row is then left out, not the leaderboard.
        #[derive(Deserialize)]
        struct Row {
            model_name: Option<String>,
            score: Option<f64>,
            rating: Option<f64>,
            category: Option<String>,
            leaderboard_publish_date: Option<String>,
        }
        let all = serde_json::from_slice::<Rows>(rows).ok()?;
        let overall = |r: &Row| r.category.as_deref().is_none_or(|c| c == "overall");
        let ends_overall = all.rows.last().is_none_or(|w| overall(&w.row));
        let read = all.rows.last().and_then(|w| w.row_idx).map_or(all.rows.len(), |i| i + 1);
        let mut arena = Arena { cut: all.num_rows_total > read && ends_overall, ..Default::default() };
        for r in all.rows.into_iter().map(|w| w.row).filter(overall) {
            // No -0.0, which would show apart from 0.0.
            let pct = r.score.map(|s| (s * 1000.0).round() / 10.0 + 0.0);
            let (Some(name), Some(score)) = (r.model_name, pct.or(r.rating.map(f64::round)).filter(|s| s.is_finite()))
            else {
                continue;
            };
            let w = arena_words(&name).join(" ");
            arena.scores.entry(w).and_modify(|b| *b = b.max(score)).or_insert(score);
            arena.date = arena.date.max(r.leaderboard_publish_date.unwrap_or_default());
        }
        Some(arena)
    }

    /// One of `ARENA_BOARDS`, from its pages in order: those with its overall rows, as one past
    /// them may have other rows or none. None when one of those did not come.
    fn board<'a>(pages: impl IntoIterator<Item = Option<&'a [u8]>>) -> Option<Arena> {
        let mut pages = pages.into_iter().map(|p| Arena::page(p?));
        let mut board = pages.next()??;
        for page in pages {
            if !board.cut {
                break;
            }
            board = board.and(page?);
        }
        Some(board).filter(|b| !b.scores.is_empty())
    }

    /// The leaderboard of its pages `self` and `next`, its scores the best of either.
    fn and(mut self, next: Arena) -> Arena {
        for (k, v) in next.scores {
            self.scores.entry(k).and_modify(|b| *b = b.max(v)).or_insert(v);
        }
        Arena { date: self.date.max(next.date), cut: next.cut, ..self }
    }

    /// The one of the last refresh, from its cache, when this refresh got none: one bad download
    /// is not a day without the column. Its boards are those still current at `now`, and its
    /// scores may be none.
    fn cached(cache: &[u8], now: u64) -> Option<Arena> {
        #[derive(Deserialize)]
        struct Kept {
            format: u32,
            #[serde(default)]
            arena_date: String,
            #[serde(default)]
            arena_dates: BTreeMap<String, String>,
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
        let of = |name| kept.models.iter().filter_map(|m| Some((m.key.clone(), *m.arena_more.get(name)?))).collect();
        let more = ARENA_MORE.into_iter().map(|(name, _)| (name, of(name))).collect();
        let boards = ARENA_BOARDS.into_iter().filter_map(|(name, ..)| {
            let date = kept.arena_dates.get(name)?.clone();
            Some((name, Arena { date, scores: of(name), ..Default::default() })).filter(|b| b.1.current(now))
        });
        let boards = boards.collect();
        let scores = kept.models.into_iter().filter_map(|m| Some((m.key, m.arena?))).collect();
        Some(Arena { date: kept.arena_date, scores, more, boards, cut: false })
    }

    /// Its signals and boards, by name.
    fn lists(&self) -> impl Iterator<Item = (&&'static str, &HashMap<String, f64>)> {
        self.more.iter().chain(self.boards.iter().map(|(n, b)| (n, &b.scores)))
    }

    /// The day each of its boards was published, as `Data::arena_dates`.
    fn dates(&self) -> BTreeMap<String, String> {
        self.boards.iter().map(|(n, b)| (n.to_string(), b.date.clone())).collect()
    }

    /// Whether it was published within `ARENA_MAX_AGE` of `now`, in seconds since 1970.
    fn current(&self, now: u64) -> bool {
        fresh(&self.date, now)
    }

    /// Gives each model its score, none to one Arena does not rank. A name that is no model's,
    /// ending in a reasoning setting or a "max", is that setting of the model it names before
    /// it: Arena's chat leaderboard has "gpt-6.1-sol-max", and "Qwen3.8-Max" is a model. Not when
    /// that one cannot reason: "Phi-4-reasoning" is not Phi-4 set to reason (`own_reasoner`).
    // ponytail: a Max model models.dev does not list gets its rating given to the one before it,
    // which can reason. Only a list of Arena's own models would tell them apart.
    fn score(&self, models: &mut [Model]) {
        let known: HashSet<&str> =
            models.iter().flat_map(|m| std::iter::once(&m.key).chain(&m.aka)).map(String::as_str).collect();
        let plain = plain(models);
        let settled = |s: &HashMap<String, f64>| {
            let mut releases: HashMap<&str, usize> = HashMap::new();
            for name in s.keys() {
                *releases.entry(undated(name).unwrap_or(name)).or_default() += 1;
            }
            // The key the words `w` of `name` give, and whether it is the spelling of a release Arena dates.
            let key = |name: &str, mut w: Vec<String>| {
                let mut dated = false;
                // A dated release is its own row, which models.dev's ids may date as "0813" of
                // "20260813", else the undated one when Arena lists no other release of it.
                // ponytail: that may be an older release when models.dev has no row of that date.
                if let Some(plain) = undated(name) {
                    let date = w.pop().unwrap_or_default();
                    let spelled = |d: &str| Some(w.concat() + d).filter(|k| known.contains(k.as_str()));
                    match spelled(&date).or_else(|| spelled(&date[4..]).filter(|_| date.len() == 8)) {
                        Some(own) => (w, dated) = (vec![own], true),
                        None if releases[plain] == 1 => {}
                        None => w.push(date),
                    }
                }
                let n = match setting_at(&w) {
                    n if n == w.len() && n > 1 && w[n - 1] == "max" => n - 1,
                    n => n,
                };
                let (whole, base) = (w.concat(), w[..n].concat());
                let set = n < w.len() && !known.contains(whole.as_str()) && known.contains(base.as_str());
                // As `own_reasoner`, with "max" a setting too.
                let own = setting(&w[n..]).0 == Some(true) && plain.contains(&base);
                (if set && !own { base } else { whole }, dated)
            };
            // Each key's score, and whether it is the spelling of a release Arena dates.
            let mut out: HashMap<String, (bool, f64)> = HashMap::new();
            for (name, &v) in s {
                let w: Vec<String> = name.split(' ').map(String::from).collect();
                let (mut k, mut dated) = key(name, w.clone());
                // A vendor before the name is no part of it, as in `merge_same_ids`, unless a model
                // is named with it: Arena's "NVIDIA-Nemotron-3-Super-120B-A12B" is Nemotron 3 Super
                // 120B A12B, and "Mistral Large 4" is Mistral Large 4.
                if !known.contains(k.as_str())
                    && (VENDORS.contains(&w[0].as_str()) || w[0] == "amazon")
                    && let Some(b) = Some(key(name, w[1..].to_vec())).filter(|b| known.contains(b.0.as_str()))
                {
                    (k, dated) = b;
                }
                out.entry(k).and_modify(|b| *b = (b.0 || dated, b.1.max(v))).or_insert((dated, v));
            }
            out
        };
        let (scores, more) = (settled(&self.scores), self.lists().map(|(n, s)| (n, settled(s))).collect::<Vec<_>>());
        for m in models {
            // A model that absorbed the key of a dated release is that release, whatever its name.
            let of = |s: &HashMap<String, (bool, f64)>| {
                let rows = || std::iter::once(&m.key).chain(&m.aka).filter_map(|k| s.get(k));
                rows().find(|r| r.0).or_else(|| rows().next()).map(|r| r.1)
            };
            m.arena = of(&scores);
            m.arena_more = more.iter().filter_map(|(n, s)| Some((n.to_string(), of(s)?))).collect();
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

/// A name's words as Arena's rows are kept: those `clean_name` leaves, then the numbers of each
/// bracket as one, which date a release: "DeepSeek-V4-Pro (High, 20260813)" is "deepseek v4 pro
/// 20260813", not DeepSeek-V4-Pro. The rest is a setting ("High", "32K") or a quantization
/// ("NVFP4"), and the model's.
fn arena_words(name: &str) -> Vec<String> {
    let brackets = name.split(['(', '[']).skip(1).filter_map(|b| b.split([')', ']']).next());
    let date = |b: &str| words(b).into_iter().filter(|w| w.bytes().all(|b| b.is_ascii_digit())).collect::<String>();
    words(&clean_name(name)).into_iter().chain(brackets.map(date).filter(|d| !d.is_empty())).collect()
}

/// A name of Arena's without the release it ends in (`arena_words`), "qwen3 8 max" of "qwen3 8 max
/// 0902": none when it ends in none.
fn undated(name: &str) -> Option<&str> {
    let date = |d: &str| d.len() >= 4 && d.bytes().all(|b| b.is_ascii_digit());
    name.rsplit_once(' ').filter(|(_, d)| date(d)).map(|(name, _)| name)
}

/// The model pages a site's sitemap lists, by name: `claude-opus-5-5`. None when it is not one.
fn sitemap<'a>(xml: &'a str, site: &str) -> HashSet<&'a str> {
    xml.split(&format!("<loc>https://{site}/models/"))
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

/// Lowercase alphanumerics only: "Claude Opus 4.5" == "claude-opus-4-5" == "claude_opus_4.5".
/// A "+" is "plus": "Command R+" == "command-r-plus", and is not Command R.
pub fn norm(s: &str) -> String {
    s.replace('+', "plus").chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// The bytes of `norm(s)` one by one, to compare with no String made.
fn normed(s: &str) -> impl Iterator<Item = u8> + '_ {
    s.bytes().flat_map(|b| {
        let plus = (b == b'+').then_some(*b"plus").into_iter().flatten();
        plus.chain(b.is_ascii_alphanumeric().then(|| b.to_ascii_lowercase()))
    })
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
        let mut w = words(&m.name);
        let num = |w: &String| w.starts_with(|c: char| c.is_ascii_digit());
        // A vendor before the name is no part of it: DigitalOcean's "Anthropic Claude 5.5
        // Haiku" is "Claude Haiku 5.5". Not when only numbers follow, "Mistral 7B" being a name.
        if w.first().is_some_and(|v| VENDORS.contains(&v.as_str())) && w[1..].iter().any(|w| !num(w)) {
            w.remove(0);
        }
        let mut k: Vec<String> = w.iter().filter(|w| !num(w)).cloned().collect();
        k.sort();
        k.extend(w.chunk_by(|a, b| num(a) && num(b)).filter(|r| num(&r[0])).map(|r| r.join(".")));
        let k = k.join("-");
        k.bytes().any(|b| b.is_ascii_digit()).then_some(k)
    };
    let mut moved = merge_by(by_key, &known, main_id);
    moved.extend(merge_by(by_key, &known, any_order));
    moved.extend(merge_into_seller(by_key));
    moved
}

/// A row goes into the one that sells its main id more: Jiekou's "claude-haiku-4-5-20251001",
/// named by its id, is the snapshot that Anthropic and others list under "Claude Haiku 4.5".
/// More, not any: Vercel's one `gpt-5.2-pro` sold as "GPT 5.2" must not take GPT-5.2 Pro in,
/// and a tie between two sellers is no merge. And only when it has the seller's name, adding
/// numbers alone, a date: "Claude Opus 5 Fast" is a price of its own, though two resellers sell
/// `claude-opus-5-fast` as "Claude Opus 5", and "GPT-5.3" is not "GPT-5.2".
// ponytail: a row whose seller moves too stays, chains are left for when one shows up.
fn merge_into_seller(by_key: &mut HashMap<String, Model>) -> Vec<(String, String)> {
    // Each id's sellers: the rows whose offers go by it, and how many of them.
    let mut sellers: HashMap<String, HashMap<&String, usize>> = HashMap::new();
    for (k, m) in by_key.iter() {
        for o in &m.offers {
            *sellers.entry(slug(&o.id)).or_default().entry(k).or_default() += 1;
        }
    }
    let num = |w: &String| w.bytes().all(|b| b.is_ascii_digit());
    let mut keys: Vec<&String> = by_key.keys().collect();
    keys.sort();
    let pairs: Vec<(String, String)> = keys
        .iter()
        .filter_map(|&k| {
            let id = main_id(&by_key[k])?;
            let rows = &sellers[&id];
            let to = winner(rows.iter().filter(|r| *r.0 != k).map(|(t, n)| (*t, *n)))?;
            let (mine, theirs) = (words(&by_key[k].name), words(&by_key[to].name));
            let dated = theirs.iter().all(|w| mine.contains(w)) && mine.iter().all(|w| theirs.contains(w) || num(w));
            (rows[to] > rows[k] && dated).then(|| (k.clone(), to.clone()))
        })
        .collect();
    let from: HashSet<&String> = pairs.iter().map(|p| &p.0).collect();
    let pairs: Vec<(String, String)> = pairs.iter().filter(|p| !from.contains(&p.1)).cloned().collect();
    for (k, to) in &pairs {
        let m = by_key.remove(k).unwrap();
        absorb(by_key.get_mut(to).unwrap(), m);
    }
    pairs
}

/// Rows on one models.dev page go together when one name adds to another numbers alone and
/// a size at least: OpenCode's "Beam" is "Beam 1.0 501B A23B". So "GLM", which moves on to
/// each release, stays out of "GLM-5.3", and "Kimi K3-256K" or "Claude Opus 5 Fast", priced
/// on their own, out of theirs. The row OpenRouter or a source knows (`known`) takes the
/// others in, else the longest name: "Nemotron 3 Ultra" and "Nemotron 3 Ultra 550B" go into
/// "Nemotron 3 Ultra 550B A55B". A source names the model either way (`joined`).
fn merge_sized(
    by_key: &mut HashMap<String, Model>,
    pages: &HashMap<String, String>,
    known: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let size = |w: &String| {
        let n = w.strip_prefix('a').unwrap_or(w).strip_suffix('b');
        n.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let num = |w: &String| w.bytes().all(|b| b.is_ascii_digit());
    let names: HashMap<&String, Vec<String>> = by_key.iter().map(|(k, m)| (k, words(&m.name))).collect();
    let adds_size = |short: &[String], long: &[String]| {
        let extra: Vec<&String> = long.iter().filter(|w| !short.contains(w)).collect();
        short.iter().all(|w| long.contains(w))
            && extra.iter().any(|w| size(w))
            && extra.iter().all(|w| size(w) || num(w))
    };
    let mut rows: Vec<(usize, &String)> = names.iter().map(|(k, w)| (w.len(), *k)).collect();
    rows.sort();
    // The rows of each page, the longest names first: a row is compared with its page's alone.
    let mut on_page: HashMap<&String, Vec<&String>> = HashMap::new();
    for &(_, k) in rows.iter().rev() {
        if let Some(page) = pages.get(k) {
            on_page.entry(page).or_default().push(k);
        }
    }
    // Each row's group, by the longest name that adds a size to it: that one adds a size to
    // all the shorter ones too.
    let mut groups: BTreeMap<&String, Vec<&String>> = BTreeMap::new();
    for &(_, k) in &rows {
        let Some(page) = pages.get(k) else { continue };
        let mut longer = on_page[page].iter().copied().filter(|&t| t != k && adds_size(&names[k], &names[t]));
        let Some(t) = longer.next() else { continue };
        // Two sizes of one name are two models: "Llama 3.1" is neither 8B nor 70B.
        if longer.all(|c| adds_size(&names[c], &names[t])) {
            groups.entry(t).or_insert_with(|| vec![t]).push(k);
        }
    }
    let mut moved = Vec::new();
    for keys in groups.into_values() {
        let to = keys.iter().max_by_key(|k| (known(k), names[*k].len(), std::cmp::Reverse(*k))).unwrap();
        moved.extend(keys.iter().filter(|k| *k != to).map(|k| ((*k).clone(), (*to).clone())));
    }
    drop(names);
    for (k, to) in &moved {
        let m = by_key.remove(k).unwrap();
        absorb(by_key.get_mut(to).unwrap(), m);
    }
    moved
}

/// Moves row `m` into `t`: its offers, and the features any of them has.
fn absorb(t: &mut Model, m: Model) {
    t.tool_call |= m.tool_call;
    t.reasoning |= m.reasoning;
    t.open_weights |= m.open_weights;
    t.vision |= m.vision;
    if t.knowledge.is_empty() {
        t.knowledge = m.knowledge;
    }
    t.offers.extend(m.offers);
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
    absorbed: &HashMap<String, Vec<(String, String)>>,
    ep: &'a Scores,
) -> Vec<Option<(&'a String, bool)>> {
    let gone = |m: &Model| absorbed.get(&m.key).into_iter().flatten();
    let keys = |m: &Model| std::iter::once(m.key.clone()).chain(gone(m).map(|r| r.0.clone()));
    // Its name and the ones of the rows it absorbed: a source may name it as one of those.
    let names = |m: &Model| std::iter::once(m.name.clone()).chain(gone(m).map(|r| r.1.clone()));
    let main: Vec<Option<String>> = models.iter().map(sold_as).collect();
    let fits = |id: &Option<String>, g: &String| !id.as_ref().is_some_and(|id| other_release(id, ep.versions.get(g)));
    let named = |m: &Model, id: &Option<String>| keys(m).filter_map(|k| ep.group(&k)).find(|g| fits(id, g));
    let named: Vec<Option<&String>> = models.iter().zip(&main).map(|(m, id)| named(m, id)).collect();
    let owned: HashSet<&String> = named.iter().flatten().copied().collect();
    let rows: HashSet<&str> = models.iter().map(|m| m.key.as_str()).collect();
    let by_id = |m: &Model, id: &Option<String>| {
        let group = |g: &String| Some(words(&clean_name(&ep.groups.get(g)?.0)));
        let sold = id.as_ref().and_then(|id| {
            let g = ep.ids.get(&norm(id))?;
            let said = [words(id), group(g)?].concat();
            names(m).any(|n| words(&n).iter().all(|w| said.contains(w))).then_some(g)
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
        (fits(id, g) && (free || names(m).any(|n| adds(&n)) || id.as_deref().is_some_and(adds))).then_some((g, rename))
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
            absorb(by_key.get_mut(&to).unwrap(), m);
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

/// Vendors whose name prefixes their models' in a reseller's listing: "Anthropic Claude Opus 4.7".
const VENDORS: &[&str] = &["openai", "anthropic", "google", "xai", "meta", "mistral", "nvidia"];

/// "openaigpt41" -> "gpt41" when "gpt41" exists: providers that prefix the vendor
/// ("OpenAI: GPT-4.1", "Anthropic Claude Opus 4.7") fold into the plain entry. A stealth
/// "Space Bunny Alpha" folds the same way into "Space Bunny" when a provider lists that.
/// Only a prefix in `orgs` folds, so a fine-tune ("Shisa v2 Llama 3.3 70B", "GrayLine Qwen3
/// 8B") keeps its own entry and prices. A name without a digit folds when another provider
/// sells the same id by the plain name (`sold`): NanoGPT's "Elorian Glimpse" is `elorian/glimpse`,
/// which OpenRouter calls "Glimpse". Ambient's "Ambient Large" is not Mistral's "Large".
/// Nor does a name fold into one fewer offers give (`keys`, each name's count): "Mistral Medium
/// 3.5" is the model's, and aimlapi's "Medium 3.5" goes into it (`merge_same_ids`), not it there.
fn fold_vendor(
    key: &str,
    sold: impl Fn(&str) -> bool,
    keys: &HashMap<String, usize>,
    orgs: &HashSet<String>,
) -> String {
    let mut k = key;
    // ponytail: suffix must be ≥5 chars with a digit, or be sold by that name under the same
    // id (or follow a known vendor, for "o3"), a cheap guard against false merges.
    while let Some(r) = (2..k.len()).map(|i| &k[i..]).find(|r| {
        let prefix = &k[..k.len() - r.len()];
        let long = r.len() >= 5 && r.bytes().any(|b| b.is_ascii_digit());
        let said = sold(r);
        let vendor = VENDORS.contains(&prefix);
        ((long || said) && orgs.contains(prefix) || vendor) && keys.get(*r).is_some_and(|n| Some(n) >= keys.get(k))
    }) {
        k = r;
    }
    k.strip_suffix("alpha").filter(|b| keys.contains_key(*b)).unwrap_or(k).to_string()
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
    /// The output's price while the model thinks, when it differs.
    reasoning: Option<f64>,
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
    reasoning: Option<f64>,
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
    /// the input's price stays too. The output is at the reasoning price where that is higher, as
    /// an agent runs the model thinking: Alibaba bills all of such a reply at it. A lower one,
    /// Perplexity's for the thinking apart from the answer, or a 0 placeholder, is none.
    fn agent(&self) -> (f64, f64, Option<f64>) {
        let thinking = |output: f64, reasoning: Option<f64>| output.max(reasoning.unwrap_or(0.0));
        let output = thinking(self.output, self.reasoning);
        let prices = |t: &MdTier| {
            (t.input.unwrap_or(self.input), thinking(t.output.unwrap_or(self.output), t.reasoning.or(self.reasoning)))
        };
        let reached = |t: &&MdTier| {
            let (input, out) = prices(t);
            t.tier.kind == "context" && t.tier.size <= AGENT_CONTEXT && input >= self.input && out >= output
        };
        match self.tiers.iter().filter(reached).max_by(|a, b| a.tier.size.total_cmp(&b.tier.size)) {
            Some(t) => (prices(t).0, prices(t).1, t.cache_read.or(self.cache_read.filter(|_| t.input.is_none()))),
            None => (self.input, output, self.cache_read),
        }
    }
}

// ---------- Epoch ----------

/// (display name, overall index, bench -> score)
type Group = (String, Option<f64>, BTreeMap<String, f64>);
type CsvRow = HashMap<String, String>;

/// Epoch's models, as `merge` joins them to models.dev's.
#[derive(Default, Debug)]
struct Scores {
    /// Keyed by norm(name). Ordered so that merge results are deterministic.
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
    /// Developer (`dev_key`) -> its country.
    country: HashMap<String, String>,
    /// The benchmarks of Epoch's index, by the name `groups` score them under, with their
    /// curves in Epoch's model of scores (`fit::scored`).
    benches: HashMap<String, crate::fit::Bench>,
    /// The task benchmarks no row scored (`Data::missed`).
    missed: Vec<String>,
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
        "bytedance" | "bytedance-seed" | "volcengine" => "ByteDance".into(),
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

/// Epoch's benchmark names too long for a column's dropdown, and what modelcmp calls them.
const SHORT_NAMES: [(&str, &str); 2] =
    [("FrontierMath-Tiers-1-3-v2-Private", "FrontierMath T1-3"), ("FrontierMath-Tier-4-v2-Private", "FrontierMath T4")];

/// Benchmarks outside Epoch's index, whose rows its zip has though its metadata names no file:
/// (name, file, score column). With no curve they are in no estimate, but in coding's dropdown.
const UNFITTED: [(&str, &str, &str); 2] =
    [("CursorBench", "cursorbench_external.csv", "Score"), ("DeepSWE", "deepswe_external.csv", "Pass@1")];

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

    // The curves of Epoch's model of scores, by the benchmark's long name.
    let mut curves: HashMap<String, (f64, f64)> = HashMap::new();
    for r in csv_rows(&mut zip, "epoch_capabilities_index/edi_scores.csv").unwrap_or_default() {
        let num = |c: &str| col(&r, c)?.parse::<f64>().map_err(|e| format!("epoch csv: {c}: {e}"));
        if let (Ok(edi), Ok(slope)) = (num("edi"), num("estimated_slope_scaled")) {
            curves.insert(col(&r, "benchmark_name")?.to_string(), (edi, slope));
        }
    }
    let mut unscored: HashMap<String, BTreeSet<String>> = HashMap::new();
    let benches = csv_rows(&mut zip, "benchmark_metadata.csv").ok_or("epoch zip: missing benchmark_metadata.csv")?;
    for b in &benches {
        let (mut file, mut score_col, bench) = (col(b, "source_file")?, col(b, "score_column")?, col(b, "benchmark")?);
        let unfitted = UNFITTED.iter().find(|u| u.0 == bench);
        if let Some(&(_, f, c)) = unfitted {
            (file, score_col) = (f, c);
        } else if file.is_empty() || score_col.is_empty() || col(b, "in_eci")? != "True" {
            continue;
        }
        // "nan" and "inf" parse as numbers, and are none.
        let num =
            |c: &str, or: f64| b.get(c).and_then(|v| v.parse().ok()).filter(|v: &f64| v.is_finite()).unwrap_or(or);
        col(b, "scale")?;
        let scale = num("scale", 1.0);
        // One in the index but with no curve, as SuperGLUE, scores nothing.
        let curve = curves.get(bench).map(|&(edi, slope)| crate::fit::Bench {
            edi,
            slope,
            baseline: num("random_baseline", 0.0),
            ceiling: num("score_ceiling", 1.0),
        });
        let bench = SHORT_NAMES.iter().find(|(long, _)| *long == bench).map_or(bench, |(_, short)| short);
        if let Some(curve) = curve {
            ep.benches.insert(bench.to_string(), curve);
        }
        let Some(rows) = csv_rows(&mut zip, file) else { continue };
        for r in &rows {
            // A row without a version is no model's: together they would be one that outscores most.
            let (Some(v), Some(s)) = (r.get("Model version").filter(|v| !v.is_empty()), r.get(score_col)) else {
                continue;
            };
            let Some(s) = s.trim_end_matches('%').parse::<f64>().ok().filter(|s| s.is_finite()) else { continue };
            let g = group_of(v);
            // The ids its scores are of, else those of any result.
            let scores = curve.is_some() || unfitted.is_some();
            let scored = if scores { &mut ep.versions } else { &mut unscored };
            scored.entry(norm(&g)).or_default().insert(v.clone());
            // Any benchmark result may give the model a page on Epoch, only one with a curve or
            // `UNFITTED` scores it.
            let e = ep.groups.entry(norm(&g)).or_insert_with(|| (g, None, BTreeMap::new()));
            if !scores {
                continue;
            }
            let best = e.2.entry(bench.to_string()).or_insert(0.0);
            *best = best.max(s * scale);
        }
    }
    for (g, versions) in unscored {
        ep.versions.entry(g).or_insert(versions);
    }
    let scored = |b: &str| ep.groups.values().any(|g| g.2.contains_key(b));
    ep.missed = crate::fit::task_benches().iter().filter(|b| !scored(b)).map(ToString::to_string).collect();
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
    Ok(ep)
}

// ---------- merge ----------

fn merge(models_json: &[u8], ep: &Scores) -> Result<Data, String> {
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
    // Each name with how many offers go by it.
    let mut keys: HashMap<String, usize> = HashMap::new();
    for e in &entries {
        *keys.entry(norm(&e.3)).or_default() += 1;
    }
    let offer_id = |mid: &String, md: &MdModel| if md.id.is_empty() { mid.clone() } else { md.id.clone() };
    // Each name with the ids it is sold as.
    let sold: HashSet<(String, String)> =
        entries.iter().map(|e| (norm(&e.3), offer_id(e.1, e.2).to_lowercase())).collect();
    // models.dev's pages by their lowercase id: an offer that names none but goes by one is that model.
    let all_pages: HashMap<String, &str> =
        entries.iter().map(|e| (e.2.canonical_model_id.to_lowercase(), e.2.canonical_model_id.as_str())).collect();
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
        let id = offer_id(mid, md);
        let key = fold_vendor(&raw, |r| sold.contains(&(r.to_string(), id.to_lowercase())), &keys, &orgs);
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
        // W&B sells "Beam 1.0" as `reflection/Beam-1.0-501B-A23B`, naming no page: the id is one.
        let page = Some(md.canonical_model_id.as_str())
            .filter(|p| !p.is_empty())
            .or_else(|| all_pages.get(&mid.to_lowercase()).copied());
        if let Some(page) = page {
            *page_votes.entry(key.clone()).or_default().entry(page).or_default() += 1;
        }
        if m.knowledge.is_empty() {
            m.knowledge = md.knowledge.clone();
        }
        let cost = md.cost.as_ref().map(MdCost::agent);
        m.offers.push(Offer {
            provider: pid.clone(),
            provider_name: p.name.clone(),
            id,
            input: cost.map_or(0.0, |c| c.0),
            output: cost.map_or(0.0, |c| c.1),
            // A few list a paid model's cache as 0, a placeholder; a cache never costs more than input.
            cache_read: cost.and_then(|c| c.2.filter(|&r| r > 0.0).map(|r| r.min(c.0))),
            unpriced: cost.is_none(),
            ..Default::default()
        });
    }

    let row_names: HashMap<String, String> = by_key.iter().map(|(k, m)| (k.clone(), m.name.clone())).collect();
    let known = |k: &str| openrouter.contains_key(k) || ep.groups.contains_key(k) || ep.alias.contains_key(k);
    let mut moved = merge_same_ids(&mut by_key, known);
    // A row's page counts the votes of the rows it absorbed.
    for (from, to) in &moved {
        for (page, n) in page_votes.remove(from).unwrap_or_default() {
            *page_votes.entry(to.clone()).or_default().entry(page).or_default() += n;
        }
    }
    let pages: HashMap<String, String> = page_votes
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), winner(v.iter().map(|(p, n)| (p.to_string(), *n)))?)))
        .collect();
    moved.extend(merge_sized(&mut by_key, &pages, known));
    // The keys and names each row absorbed: the source may score the model under one of them.
    let mut absorbed: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (from, to) in moved {
        // A row merged into one that is merged in turn takes what it absorbed along.
        let mut rows = absorbed.remove(&from).unwrap_or_default();
        rows.push((from.clone(), row_names[&from].clone()));
        absorbed.entry(to.clone()).or_default().extend(rows);
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
    for m in &mut models {
        m.aka = absorbed.get(&m.key).into_iter().flatten().map(|r| r.0.clone()).collect();
    }
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
        if let Some((k, _)) = group
            && let Some((gname, eci, scores)) = ep.groups.get(k)
        {
            // Scored on no task, a page is all Epoch adds; its name for the model may be a bare id.
            if eci.is_none() && scores.is_empty() {
                continue;
            }
            // Epoch's names are the cleaner.
            if group.is_some_and(|g| g.1 && named[g.0].1 == m.key) {
                m.name = gname.clone();
            }
            if let Some(o) = ep.org.get(k) {
                m.developer = short_org(o);
            }
            (m.eci, m.scores) = (*eci, scores.clone());
        }
    }
    unify_developers(&mut models);
    for m in &mut models {
        m.country = ep.country.get(&dev_key(&m.developer)).cloned().unwrap_or_default();
    }
    models.sort_by(|a, b| {
        b.eci
            .unwrap_or(0.0)
            .total_cmp(&a.eci.unwrap_or(0.0))
            .then(b.release.cmp(&a.release))
            .then_with(|| a.key.cmp(&b.key))
    });
    let benches = crate::fit::task_benches().into_iter().map(String::from).collect();
    Ok(Data { format: FORMAT, fetched: now(), benches, models, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_completes_a_folder_as_a_shell_does() {
        let dir = std::env::temp_dir().join(format!("modelcmp-complete-{}", std::process::id()));
        for d in ["alpha", "alps", "beta", ".hid"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("bfile"), "").unwrap();
        let at = |part: &str| format!("{}/{part}", dir.display());
        assert_eq!(complete(&at("b")), (at("beta/"), vec![]), "the one folder, no file");
        assert_eq!(complete(&at("a")), (at("alp"), vec!["alpha".to_string(), "alps".into()]), "as far as they agree");
        assert_eq!(complete(&at("z")), (at("z"), vec![]), "none: as it was");
        assert_eq!(complete(&at(".")).0, at(".hid/"), "a hidden one when asked for");
        assert_eq!(complete(&at("")).1.len(), 3, "and not otherwise");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Arena's downloads are had when asked for after their time is up, and the others left out.
    #[test]
    fn arena_downloads_that_came_are_had_past_their_end() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send((ARENA_AT + 1, Ok(b"signal".to_vec()))).unwrap();
        tx.send((0, Ok(b"models".to_vec()))).unwrap();
        let steps = Steps::default();
        step(&steps, "huggingface.co");
        let got = std::array::from_fn(|i| if i == 0 { Ok(b"agent".to_vec()) } else { Err("no reply".into()) });
        let late = Late { got, rx, end: Instant::now(), sites: vec!["huggingface.co"] };
        let all = late.all(&steps).map(Result::ok);
        assert_eq!(all[..3], [Some(b"agent".to_vec()), Some(b"signal".to_vec()), None]);
        assert_eq!(progress(&steps), "1/1", "and are awaited no more");
    }

    #[test]
    fn a_download_that_fails_once_is_asked_for_again() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", server.local_addr().unwrap());
        // It answers by the path asked for, not by the connection: a signal, as when a process
        // another test started ends, can cut a read short, and the download is then asked for
        // again on a new connection.
        let (tx, gone) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut busy = true;
            for stream in server.incoming() {
                let (mut stream, mut head) = (stream.unwrap(), [0; 1024]);
                let n = stream.read(&mut head).unwrap_or(0);
                let reply = if head[..n].starts_with(b"GET /gone ") {
                    let _ = tx.send(());
                    "404 Not Found"
                } else if std::mem::take(&mut busy) {
                    "503 Busy"
                } else {
                    "200 OK"
                };
                let _ = write!(stream, "HTTP/1.1 {reply}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
        });
        let s5 = Duration::from_secs(5);
        assert_eq!(request(&url, None, s5).unwrap(), b"ok", "a 503 is tried again");
        let missing = request(&format!("{url}gone"), None, s5);
        // Asked for again only after a read cut short, not all the tries.
        let asked = gone.try_iter().count();
        assert!(matches!(missing, Err(ureq::Error::StatusCode(404))) && asked < TRIES as usize, "a 404 is not");
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
        let mut merged = [Model { key: "opus5".into(), aka: vec!["claudeopus5".into()], ..Default::default() }];
        arena.score(&mut merged);
        assert_eq!(merged[0].arena, Some(8.7), "or by the key of a row merged into it");
        assert!(Arena::parse(rows.replace("7,", "8,").as_bytes()).unwrap().cut, "more rows than were read");
        assert_eq!(Arena::parse(b"<html>"), None);
        // A board's ratings, over the pages with its overall rows: cut while a page ends in one
        // that is not the dataset's last.
        let page = |total: usize, rows: &[(usize, &str, f64, &str)]| {
            let rows = rows.iter().map(|(i, name, rating, cat)| {
                format!(
                    r#"{{"row_idx": {i}, "row": {{"model_name": "{name}", "rating": {rating}, "category": "{cat}"}}}}"#
                )
            });
            format!(r#"{{"num_rows_total": {total}, "rows": [{}]}}"#, rows.collect::<Vec<_>>().join(","))
        };
        let first = page(9, &[(0, "gpt-6-high", 1520.4, "overall"), (1, "gpt-6-max", 1510.0, "overall")]);
        let tail = page(9, &[(7, "qwen3.8-max", 1490.0, "overall"), (8, "kimi-k2-thinking", 1480.0, "overall")]);
        let ended = page(9, &[(2, "phi-4-reasoning", 1400.0, "overall"), (3, "gpt-6", 1600.0, "coding")]);
        let (first, tail) = (first.as_bytes(), tail.as_bytes());
        assert!(Arena::page(first).unwrap().cut && !Arena::page(tail).unwrap().cut, "the dataset's last row");
        assert!(!Arena::page(ended.as_bytes()).unwrap().cut, "its last overall row");
        let past = page(9, &[(4, "gpt-6", 1600.0, "coding")]);
        let board = Arena::board([Some(first), Some(ended.as_bytes()), None, Some(past.as_bytes())]).unwrap();
        assert!(!board.cut, "no page read past the overall rows, nor needed");
        assert_eq!(Arena::board([Some(first), None, Some(tail)]), None, "one with overall rows did not come");
        assert_eq!(Arena::board([Some(past.as_bytes())]), None, "no rating");
        let uncategorized = br#"{"num_rows_total": 1, "rows": [{"row": {"model_name": "x-1", "rating": 1000}}]}"#;
        let one = Arena::page(uncategorized).unwrap();
        assert_eq!((one.scores.get("x 1"), one.cut), (Some(&1000.0), false), "a row of no category is overall");
        let board = Arena::board([Some(first), Some(tail)]).unwrap();
        // A setting or a "max" at the end of a name that is no model's goes, but not from one that
        // cannot reason, nor a "max" that is part of a word.
        let more = [("phi 4 reasoning", 1400.0), ("foo 3dsmax", 1300.0), ("foo 2 max", 1250.0)];
        let scores = board.scores.into_iter().chain(more.map(|(k, v)| (k.to_string(), v))).collect();
        let reasoner = |key: &str| Model { reasoning: true, ..row(key) };
        let mut ms = [reasoner("gpt6"), row("qwen38max"), row("qwen38"), row("kimik2thinking"), reasoner("kimik2")];
        let mut ms2 = [row("phi4"), row("foo3ds"), row("foo2")];
        let board = Arena { scores, ..Default::default() };
        let rated = Arena { boards: [("Chat", board)].into(), ..Default::default() };
        rated.score(&mut ms);
        rated.score(&mut ms2);
        let got = ms.each_ref().map(|m| m.arena_more.get("Chat").copied());
        assert_eq!(got, [Some(1520.0), Some(1490.0), None, Some(1480.0), None], "the best setting, or its own");
        assert_eq!(ms2.each_ref().map(|m| m.arena_more.get("Chat").copied()), [None, None, None]);
        // Kept from the cache when it did not come, until Arena has published none for too long.
        let cache = format!(
            r#"{{"format": {FORMAT}, "fetched": 0, "arena_date": "2026-10-02",
            "arena_dates": {{"Chat": "2026-09-30", "WebDev": "2026-07-01"}},
            "models": [{{"key": "claudeopus5", "arena": 8.7, "arena_more": {{"Praise": 3.1}}}},
                {{"key": "gpt6", "arena_more": {{"Chat": 1500, "WebDev": 1400}}}}]}}"#
        );
        let day = |d: u64| (56 * 365 + 14 + 273 + d) * 86400; // 2026-10-01 and d days
        let kept = Arena::cached(cache.as_bytes(), day(7)).unwrap();
        assert_eq!(kept.boards.get("Chat").map(|c| c.scores["gpt6"]), Some(1500.0), "a board of its own day");
        assert!(kept.boards.keys().eq(&["Chat"]), "while that is not too old");
        let unranked = cache.replace(r#""arena": 8.7, "#, "");
        let unranked = Arena::cached(unranked.as_bytes(), day(7)).unwrap();
        assert!(unranked.scores.is_empty() && unranked.boards.contains_key("Chat"), "with no agent score kept");
        assert_eq!((kept.more["Praise"]["claudeopus5"], kept.more["Steerability"].len()), (3.1, 0), "its signals too");
        assert_eq!((kept.date.as_str(), kept.scores.len(), kept.scores["claudeopus5"]), ("2026-10-02", 1, 8.7));
        let other = cache.replace(&FORMAT.to_string(), "1");
        assert_eq!(Arena::cached(other.as_bytes(), day(7)), None, "not of another format");
        assert!(kept.current(day(7)) && !kept.current(day(120)) && !Arena::default().current(day(7)));
        // A cache that missed them is tried again within the hour, not the day.
        let aged = |mins: u64, arena_missed| Data {
            format: FORMAT,
            fetched: now() - mins * 60,
            benches: crate::fit::task_benches().into_iter().map(String::from).collect(),
            arena_missed,
            ..Default::default()
        };
        assert!(aged(120, true).stale() && !aged(10, true).stale() && !aged(120, false).stale());
        let unscored = Data { missed: vec!["HLE".into()], ..aged(120, false) };
        assert!(unscored.stale() && !Data { fetched: now() - 600, ..unscored.clone() }.stale());
        // A load says what the refresh did, as its own, cached with the data.
        assert_eq!(unscored.said().as_deref(), Some("epoch.ai sent no HLE scores"));
        let warned = Data { warning: Some("copilot did not list its models".into()), ..unscored };
        let back: Data = serde_json::from_slice(&serde_json::to_vec(&warned).unwrap()).unwrap();
        assert_eq!(back.said().as_deref(), Some("last refresh: copilot did not list its models"));
        // Not stale yet, yesterday's is still refreshed by the TUI's first start of today.
        assert!(aged(48 * 60, false).of_another_day() && !aged(0, false).of_another_day());
        // And `?` says so.
        assert!(crate::app::HELP[0].1.iter().any(|&(k, about)| k == "r" && about.contains("a day's first start")));
    }

    /// A stale cache is read again under the lock only when another run wrote it meanwhile, not
    /// at every command while a refresh that failed is not tried again.
    #[test]
    fn a_stale_cache_is_read_again_only_once_written() {
        let p = std::env::temp_dir().join(format!("modelcmp-written-{}.json", std::process::id()));
        crate::store::write_atomic(&p, b"{}").unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 86400);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        let read = written(&p);
        assert!(read.is_some() && written(&p) == read, "unwritten, it is the same");
        crate::store::write_atomic(&p, b"{}").unwrap();
        assert_ne!(written(&p), read, "written again, though with the same bytes");
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn a_dated_arena_row_is_of_its_release() {
        let row = |key: &str| Model { key: key.into(), ..Default::default() };
        let keys = ["deepseekv4pro", "deepseekv4pro0813", "qwen38max", "qwen38max0902", "kimi9", "grok9"];
        let mut ms = keys.map(row);
        let rated =
            |(name, rating): (&str, f64)| format!(r#"{{"row": {{"model_name": "{name}", "rating": {rating}}}}}"#);
        let rows = [
            ("DeepSeek-V4-Pro (High, 20260813)", 1582.0),
            ("DeepSeek-V4-Pro", 1447.0),
            ("Qwen3.8-Max (0902)", 1673.0),
            ("Qwen3.8-Max", 1672.0),
            ("Kimi 9 (0711)", 1370.0),
            ("Kimi 9 (0905)", 1380.0),
            ("Grok 9 (High, 20260709)", 1500.0),
        ];
        let rows = format!(r#"{{"rows": [{}]}}"#, rows.map(rated).join(","));
        Arena::parse(rows.as_bytes()).unwrap().score(&mut ms);
        let got = ms.each_ref().map(|m| m.arena);
        let want = [Some(1447.0), Some(1582.0), Some(1672.0), Some(1673.0), None, Some(1500.0)];
        assert_eq!(got, want, "each release its row, a lone one the model's, and one of two others none");
    }

    #[test]
    fn an_arena_row_named_with_its_vendor_first_is_of_its_model() {
        let row = |key: &str| Model { key: key.into(), ..Default::default() };
        let vl = Model { aka: vec!["nemotron35supervl120ba12b".into()], ..row("nemotron35supervl") };
        let mut ms = [vl, row("nova2lite"), row("mistrallarge4")];
        let rated =
            |(name, rating): (&str, f64)| format!(r#"{{"row": {{"model_name": "{name}", "rating": {rating}}}}}"#);
        let rows = [
            ("NVIDIA-Nemotron-3.5-Super-VL-120B-A12B (NVFP4)", 1405.0),
            ("Amazon Nova 2 Lite", 1362.0),
            ("Mistral Large 4", 1432.0),
        ];
        let rows = format!(r#"{{"rows": [{}]}}"#, rows.map(rated).join(","));
        Arena::parse(rows.as_bytes()).unwrap().score(&mut ms);
        let got = ms.each_ref().map(|m| m.arena);
        assert_eq!(got, [Some(1405.0), Some(1362.0), Some(1432.0)], "the vendor left out, unless the model has it");
    }

    #[test]
    fn a_merged_model_is_the_dated_release_it_absorbed() {
        // Providers name command-r-plus-08-2024 "Command R+" and "Command R+ 08-2024": one row, of
        // that release, which Arena dates in brackets beside its undated original.
        let mut ms =
            [Model { key: "commandrplus".into(), aka: vec!["commandrplus082024".into()], ..Default::default() }];
        let rows = br#"{"rows": [{"row": {"model_name": "Command R+ (08-2024)", "rating": 1228.87}},
                                 {"row": {"model_name": "Command R+", "rating": 1203.8}}]}"#;
        Arena::parse(rows).unwrap().score(&mut ms);
        assert_eq!(ms[0].arena, Some(1229.0), "the release it is, not the one its name is");
    }

    #[test]
    fn a_harness_is_logged_out_only_when_sure() {
        let home = std::env::temp_dir().join(format!("modelcmp-login-{}", std::process::id()));
        for dir in [&home.join(".codex"), &home.join(".gemini")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // As the real ones answer, exiting 1: claude on stdout, codex on stderr. Functions, not
        // files: a file just written cannot run while a process another test starts still holds
        // it open, and sh then runs the real one further on `PATH`.
        let fakes = r#"claude() { echo "{\"loggedIn\": $IN}"; return 1; }; codex() { echo "$SAYS" >&2; return 1; }"#;
        let path = std::env::var("PATH").unwrap_or_default();
        let out = |harness: &str, env: &[(&str, &str)]| {
            let Some((_, Probe::Provider(_, line))) = HARNESSES.iter().find(|h| h.0 == harness) else { panic!() };
            let mut sh = std::process::Command::new("sh");
            sh.args(["-c", &format!("{fakes}\n{line}")]);
            sh.env_clear().env("HOME", &home).env("PATH", &path).envs(env.iter().copied());
            sh.status().unwrap().success()
        };
        let write = |file: &str, text: &str| std::fs::write(home.join(file), text).unwrap();
        assert!(out("claude", &[("IN", "false")]));
        assert!(!out("claude", &[("IN", "true")]));
        assert!(!out("claude", &[]), "an answer it does not know");
        assert!(out("codex", &[("SAYS", "Not logged in")]));
        assert!(!out("codex", &[("SAYS", "Logged in using ChatGPT")]));
        assert!(!out("codex", &[("SAYS", "Not logged in"), ("CODEX_API_KEY", "k")]), "a key `codex exec` takes");
        write(".codex/config.toml", "model_provider = \"azure\"\n");
        assert!(!out("codex", &[("SAYS", "Not logged in")]), "a provider of its own");
        assert!(out("gemini", &[]), "nothing to log in with");
        assert!(!out("gemini", &[("GEMINI_API_KEY", "k")]));
        write(".env", "GEMINI_API_KEY=k\n");
        assert!(!out("gemini", &[]), "a key in its .env");
        std::fs::remove_file(home.join(".env")).unwrap();
        write(".gemini/settings.json", r#"{"security": {"auth": {"selectedType": "oauth-personal"}}}"#);
        assert!(out("gemini", &[]), "a Google login picked, then signed out of");
        write(".gemini/oauth_creds.json", "{}");
        assert!(!out("gemini", &[]), "a Google login");
        std::fs::remove_file(home.join(".gemini/oauth_creds.json")).unwrap();
        write(".gemini/settings.json", r#"{"security": {"auth": {"selectedType": "vertex-ai"}}}"#);
        assert!(!out("gemini", &[]), "another auth type, which it may have all it needs for");
        let _ = std::fs::remove_dir_all(&home);
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
    fn a_llama_cache_that_does_not_answer_keeps_its_last_listing() {
        let (tx, rx) = std::sync::mpsc::channel();
        let llama = HARNESSES.iter().find(|h| h.0 == LLAMA).unwrap();
        // Stopped, it is not asked: none, as a listing that fails or hangs gives, not an empty cache.
        harness_models(&[llama], &AtomicBool::new(true), &Steps::default(), &tx);
        drop(tx);
        let now: BTreeMap<_, _> = rx.iter().collect();
        assert_eq!(now[LLAMA], None);
        let before = || BTreeMap::from([(LLAMA.to_string(), vec![format!("{LLAMA}/unsloth/Qwen3.5-4B-GGUF:Q4_K_M")])]);
        let (kept, silent) = keep_listed(now, before);
        assert_eq!((kept[LLAMA].len(), silent), (1, vec![LLAMA.to_string()]), "what it listed before, and said");
    }

    #[test]
    fn an_ollama_that_does_not_answer_keeps_its_last_listing() {
        let (tx, rx) = std::sync::mpsc::channel();
        let ollama = HARNESSES.iter().find(|h| h.0 == OLLAMA).unwrap();
        // Stopped, it is not asked: none, as a listing that hangs gives, not a server that is down.
        harness_models(&[ollama], &AtomicBool::new(true), &Steps::default(), &tx);
        drop(tx);
        let now: BTreeMap<_, _> = rx.iter().collect();
        assert_eq!(now[OLLAMA], None);
        let before = || BTreeMap::from([(OLLAMA.to_string(), vec![format!("{OLLAMA}/qwen3.5:4b")])]);
        let (kept, silent) = keep_listed(now, before);
        assert_eq!((kept[OLLAMA].len(), silent), (1, vec![OLLAMA.to_string()]), "what it listed before, and said");
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
        let out = serde_json::json!({"jsonrpc": "2.0", "id": 2,
            "error": {"code": -32000, "message": "Authentication required"}});
        assert_eq!(copilot_enabled(&out), None, "an error is not a listing");
        // Copilot 1.0.96's when not logged in, trimmed.
        let auto = serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"configOptions": [{"type": "select",
            "id": "mode", "category": "mode",
            "currentValue": "https://agentclientprotocol.com/protocol/session-modes#agent"}],
            "sessionId": "e8c80308-7952-4ad2-96e8-4f323b297c22"}});
        assert_eq!(copilot_enabled(&auto), Some(vec![]), "a session without models takes none");
    }

    #[test]
    fn copilot_forgets_its_own_session_alone() {
        let session = |id: &str| serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": id}});
        let home = || Some(PathBuf::from("/h"));
        let own = copilot_session(&session("e8c80308-7952-4ad2"), home());
        assert_eq!(own, Some(PathBuf::from("/h/session-state/e8c80308-7952-4ad2")));
        for id in ["", "..", "../x", "a/b", "/"] {
            assert_eq!(copilot_session(&session(id), home()), None, "{id:?} names no session of its own");
        }
        let out = serde_json::json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32000, "message": "no"}});
        assert_eq!(copilot_session(&out, home()), None, "an error has no session");
        assert_eq!(copilot_session(&session("s"), None), None, "nor is there one with no home");
    }

    #[test]
    fn answer_returns_the_response_and_gives_up_on_hangs() {
        let (go, dir, s5) = (&AtomicBool::new(false), std::env::temp_dir(), Duration::from_secs(5));
        let ask =
            |bin: &str, args: &[&str], limit, stop| answer(bin, args, &dir, "{\"id\":1}\n{\"id\":2}\n", 2, limit, stop);
        // It answers once it has read both lines, after a notification and a request of its own,
        // and once its input closes makes the file it is given.
        let cli = r#"read a; read b; echo '{"id":1,"result":1}'; echo '{"method":"session/update"}'
            echo '{"id":2,"method":"ask"}'; echo "not json"; echo '{"id":2,"result":{"cwd":"'$PWD'"}}'
            cat >/dev/null; : >"$1""#;
        let ended = dir.join(format!("modelcmp-{}-ended", std::process::id()));
        let t = Instant::now();
        let cwd = ask("sh", &["-c", cli, "sh", ended.to_str().unwrap()], s5, go);
        let cwd = cwd.map(|v| v["result"]["cwd"].as_str().map(PathBuf::from));
        assert_eq!(cwd.flatten().and_then(|p| p.canonicalize().ok()), dir.canonicalize().ok(), "run in its cwd");
        // By the file, not the time it took, which a busy machine stretches past any bound.
        assert!(std::fs::remove_file(&ended).is_ok(), "it ends when its input closes, with no wait to kill it");
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
    fn a_hang_is_killed_with_the_pipeline_it_runs() {
        // As a Provider probe, `sh -c 'a | b'`: a part left running would mark its file later.
        let (go, dir, limit) = (&AtomicBool::new(false), std::env::temp_dir(), Duration::from_millis(200));
        let mark = |name: &str| dir.join(format!("modelcmp-{}-{name}", std::process::id()));
        let (ran, asked) = (mark("run"), mark("answer"));
        let line = |f: &PathBuf| format!("(sleep 1; touch '{}') | cat", f.display());
        assert_eq!(run("sh", &["-c", &line(&ran)], limit, go), None);
        assert_eq!(answer("sh", &["-c", &line(&asked)], &dir, "", 1, limit, go), None);
        std::thread::sleep(Duration::from_millis(1500));
        let left = [&ran, &asked].map(|f| std::fs::remove_file(f).is_ok());
        assert_eq!(left, [false, false], "nothing of it is left to run on");
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
    fn a_name_most_offers_give_keeps_its_vendor() {
        // aimlapi alone leaves Mistral out: its "Medium 3.5" goes into the model, not the model
        // into it, which no source names so.
        let json = br#"{"mistral": {"models": {"mistral-medium-2604": {"name": "Mistral Medium 3.5"}}},
                        "openrouter": {"models": {"mistralai/mistral-medium-3-5": {"name": "Mistral Medium 3.5"}}},
                        "aimlapi": {"models": {"mistralai/mistral-medium-3-5": {"name": "Medium 3.5"}}}}"#;
        let mut ep = Scores::default();
        ep.groups.insert("mistralmedium35".into(), ("Mistral Medium 3.5".into(), Some(141.0), BTreeMap::new()));
        let d = merge(json, &ep).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.name.as_str(), m.eci, m.offers.len())).collect();
        assert_eq!(rows, [("mistralmedium35", "Mistral Medium 3.5", Some(141.0), 3)]);
        // Nor one vendor's word apart from another's: NVIDIA's name and the plain one are one row.
        let json = br#"{"a": {"models": {"nvidia/nemotron-9-nano-30b": {"name": "NVIDIA Nemotron 9 Nano 30B"}}},
                        "b": {"models": {"nvidia/nemotron-9-nano-30b": {"name": "NVIDIA Nemotron 9 Nano 30B"}}},
                        "c": {"models": {"nemotron-9-nano-30b": {"name": "Nemotron 9 Nano 30B"}}}}"#;
        let d = merge(json, &ep).unwrap();
        assert_eq!(d.models.iter().map(|m| m.offers.len()).collect::<Vec<_>>(), [3]);
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
    fn a_row_goes_into_the_one_naming_its_size() {
        let mut by_key: HashMap<String, Model> = HashMap::new();
        let mut pages = HashMap::new();
        for (key, name, page) in [
            ("beam", "Beam", "r/beam"),
            ("beam10501ba23b", "Beam 1.0 501B A23B", "r/beam"),
            ("ultra", "Nemotron 3 Ultra", "n/ultra"),
            ("ultra550b", "Nemotron 3 Ultra 550B", "n/ultra"),
            ("ultra550ba55b", "Nemotron 3 Ultra 550B A55B", "n/ultra"),
            ("glm", "GLM", "z/glm-5.3"),
            ("glm53", "GLM-5.3", "z/glm-5.3"),
            ("kimik3", "Kimi K3", "m/k3"),
            ("kimik3256k", "Kimi K3-256K", "m/k3"),
            ("devstral2", "Devstral 2", "m/devstral"),
            ("devstral2123b", "Devstral 2 123B", "m/devstral"),
            ("devstral2124b", "Devstral 2 124B", "m/devstral-other"),
            ("llama", "Llama 9", "l/9"),
            ("llama8b", "Llama 9 8B", "l/9"),
            ("llama70b", "Llama 9 70B", "l/9"),
        ] {
            by_key.insert(key.into(), Model { key: key.into(), name: name.into(), ..Default::default() });
            pages.insert(key.to_string(), page.to_string());
        }
        let mut moved = merge_sized(&mut by_key, &pages, |k| k == "devstral2" || k == "ultra550ba55b");
        moved.sort();
        let to = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            moved,
            [
                to("beam", "beam10501ba23b"),
                to("devstral2123b", "devstral2"),
                to("ultra", "ultra550ba55b"),
                to("ultra550b", "ultra550ba55b")
            ]
        );
        assert_eq!(by_key.len(), 11, "no version alone, no word, no other page, no two sizes");
        assert!(by_key.contains_key("devstral2"), "the known row takes the longer name in");
    }

    #[test]
    fn a_row_goes_into_the_one_selling_its_id_more() {
        let row = |key: &str, name: &str, ids: &[&str]| {
            let offers = ids.iter().map(|i| Offer { id: i.to_string(), ..Default::default() }).collect();
            (key.to_string(), Model { key: key.into(), name: name.into(), offers, ..Default::default() })
        };
        let dated = "claude-haiku-4-5-20251001";
        let mut by_key: HashMap<String, Model> = [
            row(
                "claudehaiku45",
                "Claude Haiku 4.5",
                &["claude-haiku-4-5", dated, dated, "x/claude-haiku-4-5-20251001"],
            ),
            row("claudehaiku4520251001", dated, &[dated, dated]),
            row("gpt52", "GPT-5.2", &["gpt-5.2", "gpt-5.2", "gpt-5.2-pro"]),
            row("gpt52pro", "GPT-5.2 Pro", &["gpt-5.2-pro", "gpt-5.2-pro"]),
            row("claudeopus5", "Claude Opus 5", &["claude-opus-5", "claude-opus-5-fast", "claude-opus-5-fast"]),
            row("claudeopus5fast", "Claude Opus 5 Fast", &["claude-opus-5-fast"]),
            row("claudeopus6", "Claude Opus 6", &["claude-opus-6", "claude-opus-6-fast", "claude-opus-6-fast"]),
            row("claudeopus6fast", "Claude Opus 6 Fast", &["claude-opus-6-fast"; 3]),
            row("gpt53", "GPT-5.3", &["gpt-5.3"]),
            row("gpt52b", "GPT-5.2", &["gpt-5.2", "gpt-5.3", "gpt-5.3"]),
            row("glm9", "GLM 9", &["glm-9"]),
            row("glm9a", "GLM 9 20250101", &["glm-9", "glm-9"]),
            row("glm9b", "GLM 9 20250202", &["glm-9", "glm-9"]),
        ]
        .into();
        let moved = merge_into_seller(&mut by_key);
        assert_eq!(moved, [("claudehaiku4520251001".to_string(), "claudehaiku45".to_string())]);
        assert_eq!(by_key["claudehaiku45"].offers.len(), 6);
        assert!(by_key.contains_key("gpt52pro"), "one mislabelled offer takes no row in");
        assert!(by_key.contains_key("claudeopus5fast"), "nor do two, for a name that adds a word");
        assert!(by_key.contains_key("claudeopus6"), "nor goes a name into one that adds a word");
        assert!(by_key.contains_key("gpt53"), "nor one into another version");
        assert!(by_key.contains_key("glm9"), "nor one into either of two that sell it as much");
    }

    #[test]
    fn a_model_scored_under_an_absorbed_key_keeps_its_scores() {
        // One id, so one row, under the shorter key as OpenRouter knows it; Epoch names the longer.
        let json = br#"{"openrouter": {"models": {"google/gemma-9-it": {"name": "Gemma 9"}}},
                        "b": {"models": {"gemma-9-it": {"name": "Gemma 9 IT"}}}}"#;
        let mut ep = Scores::default();
        ep.groups.insert("gemma9it".into(), ("Gemma 9 IT".into(), Some(140.0), BTreeMap::new()));
        let d = merge(json, &ep).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.eci)).collect();
        assert_eq!(rows, [("gemma9", Some(140.0))]);
        assert!(merge(b"{}", &ep).is_err(), "no models is a reply in another shape, not data");
        // The same words in another order, under ids of its own: still one row, and scored.
        let json = br#"{"openrouter": {"models": {"x/opus-9": {"name": "Claude Opus 9"}}},
                        "b": {"models": {"opus9-a": {"name": "Claude 9 Opus"}, "opus9-b": {"name": "Claude 9 Opus"}}}}"#;
        ep.groups.insert("claudeopus9".into(), ("Claude Opus 9".into(), Some(150.0), BTreeMap::new()));
        let d = merge(json, &ep).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.eci, m.offers.len())).collect();
        assert_eq!(rows, [("claudeopus9", Some(150.0), 3)]);
        // And with the vendor before them, as DigitalOcean writes "Anthropic Claude 5.5 Haiku".
        let json = br#"{"openrouter": {"models": {"x/opus-9": {"name": "Claude Opus 9"}}},
                        "b": {"models": {"opus9-a": {"name": "Anthropic Claude 9 Opus"}}}}"#;
        let d = merge(json, &ep).unwrap();
        let rows: Vec<_> = d.models.iter().map(|m| (m.key.as_str(), m.offers.len())).collect();
        assert_eq!(rows, [("claudeopus9", 2)]);
        // Numbers apart are not numbers side by side, and a name without one merges with none.
        let json = br#"{"b": {"models": {"a": {"name": "Grok 4.1 Fast"}, "b": {"name": "Grok 4 Fast 1"},
                        "c": {"name": "Sonar Pro"}, "d": {"name": "Pro Sonar"}}}}"#;
        assert_eq!(merge(json, &ep).unwrap().models.len(), 4);
        // An id reaches a group no row has by name, and only then.
        ep.groups.insert("grok9beta".into(), ("Grok 9 Beta".into(), Some(160.0), BTreeMap::new()));
        ep.ids.insert("grok9".into(), "grok9beta".into());
        let eci = |json: &[u8]| {
            let d = merge(json, &ep).unwrap();
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
        let d = merge(json, &ep).unwrap();
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
        let d = merge(json, &ep).unwrap();
        let mut rows: Vec<_> = d.models.iter().map(|m| (m.name.as_str(), m.eci)).collect();
        rows.sort_by_key(|r| r.0);
        assert_eq!(rows, [("Kimi 9 (Sep 2025)", Some(150.0)), ("Kimi-9-Instruct-0905", Some(150.0))]);

        // A row named as another release than it sells is the group of a key it absorbed.
        ep.groups.insert("mistralmedium9".into(), ("Mistral Medium 9".into(), Some(150.0), BTreeMap::new()));
        ep.versions.insert("mistralmedium9".into(), vs(&["mistral-medium-2609"]));
        let offers = vec![Offer { id: "mistral-medium-2609".into(), ..Default::default() }];
        let row = Model { key: "mistralmedium".into(), name: "Mistral Medium".into(), offers, ..Default::default() };
        let absorbed =
            HashMap::from([("mistralmedium".to_string(), vec![("mistralmedium9".into(), "Mistral Medium 9".into())])]);
        assert_eq!(joined(&[row], &absorbed, &ep), [Some((&"mistralmedium9".to_string(), true))]);
    }

    #[test]
    fn retired_and_fine_tuned_models_keep_out_of_the_base() {
        let json = br#"{"p": {"models": {
            "gpt-4.1": {"name": "GPT-4.1", "cost": {"input": 2, "output": 8}},
            "old/gpt-4.1": {"name": "GPT-4.1", "status": "deprecated", "cost": {"input": 1, "output": 1}},
            "shisa/shisa-v2-gpt-4.1": {"name": "Shisa v2 GPT-4.1", "cost": {"input": 0.5, "output": 0.5}}
        }}}"#;
        let d = merge(json, &Scores::default()).unwrap();
        let offers = |k: &str| d.models.iter().find(|m| m.key == k).map(|m| m.offers.len());
        assert_eq!((offers("gpt41"), offers("shisav2gpt41")), (Some(1), Some(1)));
    }

    #[test]
    fn an_offer_naming_no_page_is_on_the_one_its_id_is() {
        let json = br#"{
            "a": {"models": {"r/beam-1.0-501b": {"name": "Beam 1.0 501B A23B", "canonical_model_id": "r/Beam-1.0-501B-A23B"}}},
            "b": {"models": {"r/beam": {"name": "Beam 1.0 501B A23B", "canonical_model_id": "r/Beam-1.0-501B-A23B"}}},
            "wandb": {"models": {"r/beam-1.0-501b-a23b": {"name": "Beam 1.0"}}},
            "kilo": {"models": {"r/beam-free": {"name": "Beam 1.0"}}},
            "vercel": {"models": {"r/beam-free": {"name": "Beam 1.0"}}}
        }"#;
        let d = merge(json, &Scores::default()).unwrap();
        assert_eq!(d.models.len(), 1, "W&B's \"Beam 1.0\" goes by the 501B's page, so is that model");
    }

    #[test]
    fn the_release_date_is_the_one_most_offers_give() {
        let json = br#"{
            "a": {"models": {"x-1": {"name": "X 1", "release_date": "2026-04-16"}, "y-1": {"release_date": "2026-02"}}},
            "b": {"models": {"x-1": {"name": "X 1", "release_date": "2026-04-16"}, "y-1": {"release_date": "2026-01"}}},
            "stealth": {"models": {"x-1": {"name": "X 1", "release_date": "2025-08-26"}, "z-1": {}}}
        }"#;
        let d = merge(json, &Scores::default()).unwrap();
        let date = |k: &str| d.models.iter().find(|m| m.key == k).unwrap().release.as_str();
        assert_eq!(date("x1"), "2026-04-16", "not the earliest, one reseller's own");
        assert_eq!((date("y1"), date("z1")), ("2026-01", ""), "the earliest on a tie, none when no offer has one");
        let json = br#"{
            "openai": {"models": {"gpt-9": {"release_date": "2026-04-16"}}},
            "stealth": {"models": {"gpt-9": {"release_date": "2025-08-26"}, "w-1": {"release_date": "2026-01-05"}}},
            "a": {"models": {"gpt-9": {"release_date": "2025-08-26"}, "w-1": {"release_date": "2026-02"}}},
            "b": {"models": {"w-1": {"release_date": "2026-02-10"}}}
        }"#;
        let d = merge(json, &Scores::default()).unwrap();
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
        let d = merge(json, &Scores::default()).unwrap();
        let of = |k: &str| d.models.iter().find(|m| m.key == k).map(|m| (m.context, m.max_output)).unwrap();
        assert_eq!(of("x1"), (400_000, 128_000), "not the most one reseller claims");
        assert_eq!(of("y1"), (200_000, 0), "the smaller on a tie, none when no offer has one");
        assert_eq!(of("gpt9").0, 400_000, "its developer's own, whatever the resellers say");
    }

    #[test]
    fn folds_vendor_prefix() {
        let keys: HashMap<String, usize> =
            ["gpt41", "openaigpt41", "prozaiorgglm5", "zaiorgglm5", "glm5x", "o3", "openaio3", "spacebunny"]
                .map(|k| (k.to_string(), 1))
                .into();
        let orgs: HashSet<String> = ["pro", "zaiorg"].map(String::from).into();
        let fold = |k| fold_vendor(k, |_| false, &keys, &orgs);
        assert_eq!(fold("openaigpt41"), "gpt41");
        assert_eq!(fold("prozaiorgglm5"), "zaiorgglm5");
        assert_eq!(fold("gpt41"), "gpt41");
        assert_eq!(fold("openaio3"), "o3");
        assert_eq!(fold("spacebunnyalpha"), "spacebunny");
        assert_eq!(fold("oxalpha"), "oxalpha", "no plain entry, no fold");
        assert_eq!(fold("shisav2gpt41"), "shisav2gpt41", "a fine-tune is not its base");
        let keys: HashMap<String, usize> =
            ["glimpse", "elorianglimpse", "large", "ambientlarge"].map(|k| (k.to_string(), 1)).into();
        let orgs: HashSet<String> = ["elorian", "ambient"].map(String::from).into();
        let by_glimpse = |r: &str| r == "glimpse";
        assert_eq!(fold_vendor("elorianglimpse", by_glimpse, &keys, &orgs), "glimpse", "sold by the plain name too");
        assert_eq!(
            fold_vendor("ambientlarge", by_glimpse, &keys, &orgs),
            "ambientlarge",
            "no digit, its id not sold so: no fold"
        );
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
        let d = merge(json, &Scores::default()).unwrap();
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
        let pages = sitemap(&xml, "artificialanalysis.ai");
        assert_eq!(pages.len(), 17, "the model pages alone");
        assert!(sitemap("<html>not found</html>", "artificialanalysis.ai").is_empty());
        let w = |s| aa_words(s, true);
        assert_eq!(w("gpt-5-mini-high"), w("gpt-5-mini"), "a setting is folded");
        assert_eq!(w("grok-4-non-reasoning"), w("grok-4"));
        assert_ne!(w("magistral-medium"), w("magistral"), "a tier is not a setting");
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
        assert_eq!(d.find("ibm-granite/granite-4.0-h-micro").unwrap().key, "granite40micro", "as llama-cli -hf takes");
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
    fn a_whole_provider_id_names_its_model_before_another_providers_bare_id() {
        // NanoGPT sells the custom tools release under Gemini 3.1 Pro by the id opencode takes
        // for Google's own: `list --via opencode --id` printed it for the custom tools one.
        let offer = |p: &str, id: &str| Offer { provider: p.into(), id: id.into(), ..Default::default() };
        let pro = Model {
            key: "gemini31pro".into(),
            available: true,
            offers: vec![offer("nano-gpt", "google/gemini-3.1-pro-preview-customtools")],
            ..Default::default()
        };
        let tools = Model {
            key: "gemini31propreviewcustomtools".into(),
            offers: vec![offer("google", "gemini-3.1-pro-preview-customtools")],
            ..Default::default()
        };
        let d = Data { models: vec![pro, tools], ..Default::default() };
        assert_eq!(
            d.find("google/gemini-3.1-pro-preview-customtools").unwrap().key,
            "gemini31propreviewcustomtools",
            "Google's, though only the other is yours"
        );
        assert_eq!(d.find("nano-gpt/google/gemini-3.1-pro-preview-customtools").unwrap().key, "gemini31pro");
    }

    #[test]
    fn find_reads_ids_as_norm_does() {
        // An id or a name is compared byte by byte as norm makes them, with no String for each.
        for s in ["Command R+", "c++", "IBM-Granite/granite-4.0", "Flash – Thinking", "日本", "--", ""] {
            assert_eq!(normed(s).collect::<Vec<_>>(), norm(s).into_bytes(), "{s}");
        }
        let offer = Offer { provider: "cohere".into(), id: "cohere/Command-R+".into(), ..Default::default() };
        let m = Model { key: "commandr".into(), name: "Cmd R+".into(), offers: vec![offer], ..Default::default() };
        let d = Data { models: vec![m], ..Default::default() };
        assert_eq!(d.find("command-r+").unwrap().key, "commandr", "bare, a + as plus");
        assert_eq!(d.find("COHERE/cohere/command-r+").unwrap().key, "commandr", "as list --id prints");
        assert_eq!(d.find("cmd r+").unwrap().key, "commandr", "by the name shown");
        assert!(d.find("cohere/command").is_err(), "the whole id, not its start");
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
    fn volcengine_ids_are_bytedance_models() {
        // ofox sells ByteDance's Seed as "volcengine/doubao-seed-character", Volcengine being
        // ByteDance's cloud, and Volcengine's own id names no vendor.
        let json = br#"{
            "ofox": {"models": {"volcengine/doubao-seed-character": {"name": "Seed Character"}}},
            "volcengine": {"models": {"doubao-seed-character-260628": {"name": "Seed Character"}}}
        }"#;
        let d = merge(json, &Scores::default()).unwrap();
        let devs: Vec<_> = d.models.iter().map(|m| (m.name.as_str(), m.developer.as_str())).collect();
        assert_eq!(devs, [("Seed Character", "ByteDance")], "its developer, not the cloud it is sold on");
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
        assert_eq!(gguf_bytes(tree, None).map(gb).as_deref(), Some("2.5 GB"), "both parts of the Q4_K_M one");
        let other =
            br#"[{"type":"directory","path":"x"},{"path":"mmproj.gguf","size":8},{"path":"m-Q8_0.gguf","size":7}]"#;
        assert_eq!(
            (gguf_bytes(other, None), gguf_bytes(b"[]", None)),
            (Some(7), None),
            "else the first, the projector aside"
        );
        // gpt-oss-120b's repo: the file Hugging Face names, not a draft model listed first.
        let draft = br#"[{"path":"eagle3-BF16.gguf","size":2},{"path":"m-MXFP4.gguf","size":63}]"#;
        assert_eq!((gguf_bytes(draft, Some("m-MXFP4.gguf")), gguf_bytes(draft, None)), (Some(63), Some(2)));
        let split = Some("Q4_K_M/m-Q4_K_M-00001-of-00002.gguf");
        assert_eq!(gguf_bytes(tree, split).map(gb).as_deref(), Some("2.5 GB"), "every part of the one named");
        let manifest =
            br#"{"ggufFile":{"rfilename":"m-Q4_K_M.gguf","size":5},"mmprojFile":{"rfilename":"mmproj.gguf","size":2}}"#;
        assert_eq!(gguf_file(manifest), Some(("m-Q4_K_M.gguf".into(), 5, 2)));
        assert_eq!(gguf_file(br#"{"layers":[]}"#), None, "ollama's answer, which names no file");
        let two = br#"[{"path":"imatrix/m-Q4_K_M.gguf","size":3},{"path":"m-Q4_K_M.gguf","size":5},{"path":"m-ablit-Q4_K_M.gguf","size":9}]"#;
        assert_eq!(gguf_bytes(two, None), Some(3), "one file, not every one so quantized");
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
        let fit = BTreeMap::from([("coding".to_string(), (50.0, crate::fit::Basis::Benches(1)))]);
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
    fn local_names_leave_out_every_quantization() {
        for (tag, key) in [
            ("hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:IQ3_M", "llama323binstruct"),
            ("/models/Llama-3.2-3B-Instruct-IQ4_XS.gguf", "llama323binstruct"),
            ("hf.co/unsloth/Qwen3-8B-GGUF:UD-Q4_K_XL", "qwen38b"),
            ("/models/Qwen3-8B-UD-Q4_K_XL.gguf", "qwen38b"),
            ("/models/gpt-oss-20b-mxfp4.gguf", "gptoss20b"),
            ("/models/Qwen3-235B-A22B-Q4_K_M-00001-of-00003.gguf", "qwen3235ba22b"),
        ] {
            assert_eq!(local_key(tag), key, "{tag} without its quantization");
        }
        // A file split in parts is loaded from its first.
        let parts = ["00002", "00001"].map(|n| format!("{LLAMA}//models/Qwen3-235B-A22B-Q4_K_M-{n}-of-00002.gguf"));
        let tags = local_tags(&parts, &HashSet::from(["qwen3235ba22b"]), &HashMap::new());
        assert_eq!(tags, HashMap::from([("qwen3235ba22b".to_string(), &parts[1][LLAMA.len() + 1..])]));
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
        let m = Model { offers: vec![unknown.clone(), free.clone()], ..Default::default() };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost()), ("f", Some(0.0)), "a free offer is priced");
        assert!(!m.listed(), "and it is yours");
        let paid = Offer { provider: "p".into(), input: 1.0, output: 3.0, available: true, ..Default::default() };
        let m = Model { offers: vec![paid, free], ..Default::default() };
        assert_eq!(m.price().unwrap().provider, "f", "a free one of yours before a paid one");
        let via =
            |p: &str, local| Offer { provider: p.into(), via: vec!["opencode".into()], local, ..Default::default() };
        let cloud = Offer { input: 1.0, available: true, ..via("cloud", false) };
        let m = Model { offers: vec![via("ollama", true), cloud], ..Default::default() };
        assert_eq!(m.offer_via("opencode").unwrap().provider, "cloud", "not your machine's free copy");
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
        assert_eq!(marked, ["price", "in", "cache", "out"], "the columns a ~ goes on");
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
    fn openrouter_speed_is_the_median_over_providers() {
        let csv = b"benchmark,model_slug,provider_name,accuracy,avg_duration_per_task_ms,avg_output_tokens_per_task\n\
                    gpqa_diamond,a/one,P,0.9,10000,900\ngpqa_diamond,a/one,Q,0.5,,\ngpqa_diamond,a/one,R,0.02,2000,600\n\
                    gpqa_diamond,a/one,auto-routing,0.88,1000,10\n\
                    gpqa_diamond,a/two,P,0.8,2000,100\ngpqa_diamond,a/two,Q,0.6,0,100\n\
                    gpqa_diamond,a/auto,auto-routing,0.7,1000,50\n\
                    tau_bench_verified_airline,a/tau,P,0.75,1,1\ngpqa_diamond,,P,0.75,1,1\ngpqa_diamond,a/bad,P,nan,1,1\n";
        let g = openrouter_speeds(csv);
        assert_eq!(g["a/one"], 195.0, "the median of the runs that say how long they took, not the auto-routing one");
        assert_eq!(g["a/two"], 50.0, "no speed from no time");
        assert_eq!(g["a/auto"], 50.0, "the auto-routing run when no provider has one");
        assert_eq!(g.len(), 3, "not tau-bench, a row with no slug, nor one with no accuracy");
        assert!(openrouter_speeds(b"no,such,columns\n1,2,3\n").is_empty());
        let none = openrouter_speeds(b"benchmark,model_slug,provider_name,accuracy\ngpqa_diamond,a/one,P,0.9\n");
        assert!(none.is_empty(), "no speed without the run columns");
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
        let d = merge(json, &Scores::default()).unwrap();
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
    fn a_reply_costs_its_thinking_price() {
        // Alibaba bills all of a thinking reply at its reasoning price, as an agent runs the model
        // its scores are of. Perplexity's lower one is for the thinking apart from the answer, and
        // a 0 is a placeholder.
        let json = br#"{"p": {"models": {
            "q-1": {"name": "Q 1", "cost": {"input": 0.4, "output": 1.2, "reasoning": 4}},
            "q-2": {"name": "Q 2", "cost": {"input": 0.4, "output": 1.2, "reasoning": 4, "tiers": [
                {"input": 0.8, "output": 2.4, "reasoning": 8, "tier": {"type": "context", "size": 32000}}]}},
            "s-1": {"name": "S 1", "cost": {"input": 2, "output": 8, "reasoning": 3}},
            "o-1": {"name": "O 1", "cost": {"input": 4, "output": 20, "reasoning": 0}}}}}"#;
        let d = merge(json, &Scores::default()).unwrap();
        let output = |k: &str| d.models.iter().find(|m| m.key == k).unwrap().offers[0].output;
        assert_eq!(output("q1"), 4.0);
        assert_eq!(output("q2"), 8.0, "a tier's own, as the session reaches it");
        assert_eq!(output("s1"), 8.0, "the answer still costs its output price");
        assert_eq!(output("o1"), 20.0);
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
                b"source_file,score_column,benchmark,scale,in_eci,random_baseline,score_ceiling\n\
                  ocr.csv,score,Some OCR,1,True,0,1\nswe.csv,score,FrontierSWE,1,True,0,1\n\
                  hle.csv,Accuracy,HLE,1,False,0,1\nfm.csv,score,FrontierMath-Tier-4-v2-Private,1,True,0,1\n",
            );
            file(
                "epoch_capabilities_index/edi_scores.csv",
                b"benchmark_name,edi,estimated_slope_scaled\nFrontierSWE,164.5,0.15\n\
                  FrontierMath-Tier-4-v2-Private,158.3,0.25\n",
            );
            file(
                "ocr.csv",
                b"Model version,score\ngoogle/flash-9,0.5\nflash-9-preview-09,0.6\nchat-4o-03,0.5\nfoo-2,0.5\n",
            );
            file("swe.csv", b"Model version,score\nfoo-2-09,0.5\nfoo-2-09_low,0.2\nbar-3-09,0.5\nbar-3-09_low,0.3\n");
            file("hle.csv", b"Model version,Accuracy\nfoo-2-09,0.9\n");
            file("fm.csv", b"Model version,score\nfoo-2-09,0.8\n");
            file(
                "epoch_capabilities_index/eci_scores.csv",
                b"Model,eci\nGPT-4o (Nov 2024),128\nBar 3 (Jul 2025),140\n",
            );
            z.finish().unwrap();
        }
        let ep = parse_epoch(buf.get_ref()).unwrap();
        assert_eq!(ep.alias["flash9"], "flash9jun2025", "the release filed under the bare id, not the newest");
        let missed = |b: &str| ep.missed.iter().any(|m| m == b);
        assert!(!missed("FrontierSWE") && missed("MirrorCode"), "a task benchmark no row scored, as one with no file");
        assert!(!missed("HLE") && !ep.groups["foo2sep2025"].2.contains_key("HLE"), "not in Epoch's index");
        assert_eq!(ep.groups["foo2sep2025"].2.get("FrontierMath T4"), Some(&0.8), "under its short name");
        assert_eq!(ep.benches["FrontierMath T4"].edi, 158.3, "with its curve, under that name too");
        assert!(!ep.benches.contains_key("Some OCR"), "in the index but with no curve: pages alone");
        assert_eq!(ep.alias["gpt4o"], "gpt4onov2024", "the one with an index, before the newest without");
        assert_eq!(ep.alias["foo2"], "foo2sep2025", "one scored on a task, before the bare id's with no score");
        assert_eq!(ep.alias["bar3"], "bar3jul2025", "and before a newer one scored on a task");
        assert_eq!(ep.ids["flash9preview09"], "flash9sep2025", "a model's id reaches its group, named otherwise");
        assert_eq!(ep.group("foo2").unwrap(), "foo2sep2025", "the name before the id, an older release's");
        assert!(!ep.ids.contains_key("flash9chat"), "an id of another kind of model is not its group's");
        assert!(ep.ids.contains_key("flash9") && !ep.ids.contains_key("googleflash9"), "without its provider");
    }

    #[test]
    fn release_tag_is_the_version_without_its_v() {
        assert_eq!(release_tag(br#"{"tag_name": "v0.2.0"}"#).as_deref(), Some("0.2.0"));
        assert_eq!(release_tag(br#"{"message": "API rate limit exceeded"}"#), None);
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
            file(
                "benchmark_metadata.csv",
                b"source_file,score_column,benchmark,scale,in_eci\nocr.csv,score,Some OCR,1,True\n",
            );
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
