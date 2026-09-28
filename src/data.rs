//! Fetching, caching and merging models.dev (prices) with Epoch AI (benchmarks).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MODELS_URL: &str = "https://models.dev/api.json";
const EPOCH_URL: &str = "https://epoch.ai/data/benchmark_data.zip";
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// Bumped when the cached fields change meaning, so an older cache refreshes.
/// 2: `Offer::unpriced`, where a missing price used to read as free.
const FORMAT: u32 = 2;
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
/// How a harness tells which models it can use.
enum Probe {
    /// A command that prints the `provider/model` ids it has access to, one per line.
    List(&'static [&'static str]),
    /// No such command: being installed means access to every model of this provider.
    Provider(&'static str),
}

const HARNESSES: &[(&str, Probe)] = &[
    ("opencode", Probe::List(&["models"])),
    ("claude", Probe::Provider("anthropic")),
    ("codex", Probe::Provider("openai")),
    ("gemini", Probe::Provider("google")),
];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Offer {
    pub provider: String,
    pub provider_name: String,
    pub id: String,
    pub env: Vec<String>,
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
    /// Where you have access: the harnesses listing it, then "env" if the provider's API key is set.
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Model {
    pub key: String,
    pub name: String,
    /// Who trained it: Epoch's organization, else the vendor prefix of an OpenRouter-style id.
    #[serde(default)]
    pub developer: String,
    pub context: u64,
    pub max_output: u64,
    pub tool_call: bool,
    pub reasoning: bool,
    pub vision: bool,
    pub open_weights: bool,
    pub release: String,
    pub knowledge: String,
    pub url: String,
    pub offers: Vec<Offer>,
    /// Epoch Capabilities Index: overall capability, roughly 100..170.
    pub eci: Option<f64>,
    /// Benchmark name -> best score (0..1) across reasoning-effort settings.
    pub scores: BTreeMap<String, f64>,
    /// Task name -> 0..100 percentile (see fit.rs).
    pub fit: BTreeMap<String, f64>,
    #[serde(skip)]
    pub available: bool,
    /// Every `Offer::via` of the model, once each.
    #[serde(skip)]
    pub via: Vec<String>,
}

impl Model {
    /// The offer you'd actually use: cheapest available paid one, else a free one of yours,
    /// else one of yours with no listed price, else the most common list price. It may be
    /// `unpriced`, still naming the id to use; `priced_offer` is the one with prices to show.
    pub fn price(&self) -> Option<&Offer> {
        let paid = |o: &&Offer| !o.unpriced && o.input + o.output > 0.0;
        let avail: Vec<&Offer> = self.offers.iter().filter(|o| o.available).collect();
        if let Some(o) = avail.iter().copied().filter(paid).min_by(|a, b| a.blended().total_cmp(&b.blended())) {
            return Some(o);
        }
        if let Some(o) = avail.iter().find(|o| !o.unpriced).or(avail.first()) {
            return Some(o);
        }
        // ponytail: mode of prices ≈ list price; resellers with odd pricing are outvoted.
        // Ordered by price bits so that on a tie the cheapest wins, deterministically.
        let mut counts: BTreeMap<(u64, u64, u64), (usize, &Offer)> = BTreeMap::new();
        for o in self.offers.iter().filter(paid) {
            let cache = o.input_cached().to_bits();
            counts.entry((o.input.to_bits(), o.output.to_bits(), cache)).or_insert((0, o)).0 += 1;
        }
        counts
            .into_values()
            .rev()
            .max_by_key(|(n, _)| *n)
            .map(|(_, o)| o)
            .or_else(|| self.offers.iter().find(|o| !o.unpriced))
            .or(self.offers.first())
    }

    /// `price`, when its prices are known.
    pub fn priced_offer(&self) -> Option<&Offer> {
        self.price().filter(|o| !o.unpriced)
    }

    /// `Offer::blended` of the offer you'd pay; 0 when it is free, none when its price is unknown.
    pub fn cost(&self) -> Option<f64> {
        self.priced_offer().map(Offer::blended)
    }

    /// The model's pages, (site, url): models.dev when its developer offers it, as models.dev
    /// has pages only under the lab (`openai/gpt-5.5`, not a reseller's); Epoch AI when it has
    /// benchmarked the model, whose name is then Epoch's; OpenRouter always, `url`.
    pub fn links(&self) -> Vec<(&'static str, String)> {
        let dev = norm(&self.developer);
        let md = self.offers.iter().find(|o| !dev.is_empty() && norm(&short_org(&o.provider)) == dev);
        let epoch = (self.eci.is_some() || !self.scores.is_empty()).then(|| {
            let slug: Vec<String> = self
                .name
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(String::from)
                .collect();
            ("epoch.ai", format!("https://epoch.ai/models/{}", slug.join("-")))
        });
        md.map(|o| ("models.dev", format!("https://models.dev/models/{}/{}/", o.provider, o.id)))
            .into_iter()
            .chain(epoch)
            .chain([("openrouter.ai", self.url.clone())])
            .collect()
    }

    /// `cost`, but never 0, for dividing by.
    pub fn blended(&self) -> Option<f64> {
        self.cost().filter(|p| *p > 0.0)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Data {
    /// `FORMAT` when fetched.
    #[serde(default)]
    pub format: u32,
    pub fetched: u64,
    /// Harness -> the `provider/model` ids it reported access to at the last refresh;
    /// `provider/*` stands for all of a provider's models.
    #[serde(default)]
    pub harness: BTreeMap<String, Vec<String>>,
    /// `fit::task_benches()` when fetched; a cache made with other benchmarks is stale.
    #[serde(default)]
    pub benches: Vec<String>,
    pub models: Vec<Model>,
}

impl Data {
    pub fn age(&self) -> Duration {
        Duration::from_secs(now().saturating_sub(self.fetched))
    }

    pub fn stale(&self) -> bool {
        self.age() > MAX_AGE || self.format != FORMAT || self.benches != crate::fit::task_benches()
    }

    /// Mark offers the user can use, and where: listed by an installed harness, or the
    /// provider's API key is set.
    pub fn apply_available(&mut self) {
        let harness: Vec<(&str, HashSet<&str>)> =
            self.harness.iter().map(|(h, ids)| (h.as_str(), ids.iter().map(String::as_str).collect())).collect();
        // ponytail: "any env var set" – providers needing several vars (bedrock) may false-positive.
        let mut env: HashMap<&str, bool> = HashMap::new();
        for m in &mut self.models {
            m.via.clear();
            for o in &mut m.offers {
                let (id, all) = (format!("{}/{}", o.provider, o.id), format!("{}/*", o.provider));
                o.via = harness
                    .iter()
                    .filter(|(_, ids)| ids.contains(id.as_str()) || ids.contains(all.as_str()))
                    .map(|(h, _)| h.to_string())
                    .collect();
                let by_env = *env
                    .entry(&o.provider)
                    .or_insert_with(|| o.env.iter().any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty())));
                if by_env {
                    o.via.push("env".into());
                }
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
        // By key, or by a provider's id for it, bare or as `list --id` prints it:
        // "granite-4.0-h-micro" or "openrouter/ibm-granite/granite-4.0-h-micro" is "Granite 4.0 Micro".
        let by_id = |m: &&Model| {
            m.offers.iter().any(|o| norm(&slug(&o.id)) == q || norm(&format!("{}/{}", o.provider, o.id)) == q)
        };
        if let Some(m) = self.models.iter().find(|m| m.key == q).or_else(|| self.models.iter().find(by_id)) {
            return Ok(m);
        }
        let hits = |mine: bool| -> Vec<&Model> {
            self.models.iter().filter(|m| (!mine || m.available) && m.key.contains(&q)).collect()
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

fn cache_path() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("modelcmp").join("data.json")
}

pub fn load_cache() -> Option<Data> {
    let bytes = std::fs::read(cache_path()).ok()?;
    let mut d: Data = serde_json::from_slice(&bytes).ok()?;
    // Derived from cached fields, so a change to the formula applies without a re-download.
    crate::fit::add_value(&mut d.models);
    d.apply_available();
    Some(d)
}

/// Ask each installed harness which models it can use. A missing, failing or hung harness
/// reports nothing, so a refresh always finishes.
fn harness_models() -> BTreeMap<String, Vec<String>> {
    let on_path =
        |bin: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()));
    HARNESSES
        .iter()
        .filter_map(|(bin, probe)| {
            let ids = match probe {
                Probe::Provider(p) => on_path(bin).then(|| vec![format!("{p}/*")])?,
                Probe::List(args) => {
                    let out = run(bin, args, Duration::from_secs(30))?;
                    out.lines().map(str::trim).filter(|l| l.contains('/')).map(String::from).collect()
                }
            };
            Some((bin.to_string(), ids))
        })
        .collect()
}

/// `bin args` stdout, or `None` when it is missing, fails, or is killed at `limit`.
fn run(bin: &str, args: &[&str], limit: Duration) -> Option<String> {
    use std::process::{Command, Stdio};
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
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // A process it left running may hold the pipe open: wait for the rest no longer than the limit.
    rx.recv_timeout(limit.saturating_sub(start.elapsed())).ok()?.ok()
}

/// Download the sources and ask the harnesses in parallel, merge, write cache.
pub fn refresh() -> Result<Data, String> {
    let (models, epoch, harness) = std::thread::scope(|s| {
        let a = s.spawn(|| fetch(MODELS_URL));
        let b = s.spawn(|| fetch(EPOCH_URL));
        let d = s.spawn(harness_models);
        (a.join().unwrap(), b.join().unwrap(), d.join().unwrap())
    });
    let mut data = merge(&models?, &epoch?)?;
    data.harness = harness;
    let json = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&cache_path(), &json).map_err(|e| format!("cache: {e}"))?;
    data.apply_available();
    Ok(data)
}

/// Cached data, refreshing if missing or stale. Falls back to stale cache when offline.
pub fn load(force: bool) -> Result<(Data, Option<String>), String> {
    match load_cache() {
        Some(d) if !force && !d.stale() => Ok((d, None)),
        cached => match refresh() {
            Ok(d) => Ok((d, None)),
            Err(e) => match cached {
                Some(d) => {
                    let w = format!("refresh failed ({e}); using data {} old", crate::view::age(d.age()));
                    Ok((d, Some(w)))
                }
                None => Err(format!("could not download model data: {e}")),
            },
        },
    }
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
    agent
        .get(url)
        .call()
        .and_then(|mut r| r.body_mut().with_config().limit(200 << 20).read_to_vec())
        .map_err(|e| format!("{url}: {e}"))
}

/// Lowercase alphanumerics only: "Claude Opus 4.5" == "claude-opus-4-5" == "claude_opus_4.5".
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
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

/// Rows whose offers mostly use the same model id are one model under several names:
/// Cloudflare's "Granite 4.0 H Micro" and OpenRouter's "granite-4.0-micro" are both
/// `granite-4.0-h-micro`. Only each row's most common id counts, as providers mislabel models
/// (Vercel calls `gpt-5.2-pro` "GPT 5.2") and one such offer must not merge two models.
/// The row OpenRouter or Epoch knows (`known`) absorbs the others, else the first by key.
/// Returns each absorbed key and the key it went into.
// ponytail: ids without a digit ("auto", "deepseek-chat") are too generic to trust and never merge.
fn merge_same_ids(by_key: &mut HashMap<String, Model>, known: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let mut order: Vec<String> = by_key.keys().cloned().collect();
    order.sort_by_cached_key(|k| (!known(k), k.clone()));
    let mut owner: HashMap<String, String> = HashMap::new(); // main id -> key of the row absorbing it
    let mut moved = Vec::new();
    for k in order {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for o in &by_key[&k].offers {
            *counts.entry(slug(&o.id)).or_default() += 1;
        }
        // A clear winner only: a row of several ids, one offer each, has no main id.
        let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
        ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
        let main = match ranked.as_slice() {
            [(m, _)] => m.clone(),
            [(m, a), (_, b), ..] if a > b => m.clone(),
            _ => continue,
        };
        if !main.bytes().any(|b| b.is_ascii_digit()) {
            continue;
        }
        let to = owner.entry(main).or_insert_with(|| k.clone()).clone();
        if to != k {
            let m = by_key.remove(&k).unwrap();
            let t = by_key.get_mut(&to).unwrap();
            t.context = t.context.max(m.context);
            t.max_output = t.max_output.max(m.max_output);
            t.tool_call |= m.tool_call;
            t.reasoning |= m.reasoning;
            t.open_weights |= m.open_weights;
            t.vision |= m.vision;
            if !m.release.is_empty() && (t.release.is_empty() || m.release < t.release) {
                t.release = m.release;
            }
            if t.knowledge.is_empty() {
                t.knowledge = m.knowledge;
            }
            t.offers.extend(m.offers);
            moved.push((k, to));
        }
    }
    moved
}

/// "meta-llama/Llama-4-Maverick" -> "llama-4-maverick": a model id without its vendor.
fn slug(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_lowercase()
}

/// "openaigpt41" -> "gpt41" when "gpt41" exists: providers that prefix the vendor
/// ("OpenAI: GPT-4.1", "Anthropic Claude Opus 4.7") fold into the plain entry. A stealth
/// "Space Bunny Alpha" folds the same way into "Space Bunny" when a provider lists that.
fn fold_vendor(key: &str, keys: &HashSet<String>) -> String {
    let mut k = key;
    const VENDORS: &[&str] = &["openai", "anthropic", "google", "xai", "meta", "mistral"];
    // ponytail: suffix must be ≥5 chars with a digit (or follow a known vendor, for "o3"),
    // a cheap guard against false merges.
    while let Some(r) = (2..k.len()).map(|i| &k[i..]).find(|r| {
        let long = r.len() >= 5 && r.bytes().any(|b| b.is_ascii_digit());
        let vendor = VENDORS.contains(&&k[..k.len() - r.len()]);
        (long || vendor) && r.len() >= 2 && keys.contains(*r)
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
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    doc: String,
    #[serde(default)]
    models: HashMap<String, MdModel>,
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
}

// ---------- Epoch ----------

/// (display name, eci, bench -> score)
type Group = (String, Option<f64>, BTreeMap<String, f64>);
type CsvRow = HashMap<String, String>;

#[derive(Default, Debug)]
struct Epoch {
    /// Keyed by norm(name). Ordered so that merge results are deterministic.
    groups: BTreeMap<String, Group>,
    /// "Gemini 2.5 Pro (Jun 2025)" is reachable as "gemini25pro"; newest dated version wins.
    alias: HashMap<String, String>,
    /// norm(group) -> organization.
    org: HashMap<String, String>,
}

/// Short developer names, the same whichever source named them.
fn short_org(s: &str) -> String {
    // "Z.ai (Zhipu AI),Tsinghua University" -> "Z.ai"; "~anthropic" -> "anthropic".
    let s = s.split(',').next().unwrap_or_default();
    let s = s.split('(').next().unwrap_or_default().trim().trim_start_matches('~');
    match s.to_lowercase().as_str() {
        "" => String::new(),
        "openai" | "~openai" => "OpenAI".into(),
        "google" | "google deepmind" => "Google".into(),
        "qwen" | "alibaba" => "Alibaba".into(),
        "meta" | "meta-llama" | "meta ai" => "Meta".into(),
        "mistral" | "mistralai" | "mistral ai" => "Mistral".into(),
        "deepseek" => "DeepSeek".into(),
        "xai" | "x-ai" => "xAI".into(),
        "z-ai" | "zhipuai" | "thudm" => "Z.ai".into(),
        "bytedance" | "bytedance-seed" => "ByteDance".into(),
        "ibm" | "ibm-granite" => "IBM".into(),
        "xiaomimimo" => "Xiaomi".into(),
        "inclusionai" => "inclusionAI".into(),
        "moonshot" | "moonshotai" => "Moonshot".into(),
        "minimax" => "MiniMax".into(),
        "nvidia" => "NVIDIA".into(),
        _ => {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// One spelling per developer: names equal once case, punctuation and a trailing "AI", "org",
/// "Labs", "Corp", "Research" or "Inc" are dropped ("Zai-org" and "Z.ai", "Deepseek-ai" and
/// "DeepSeek") all take the most used one, capitalised ones winning ties.
fn unify_developers(models: &mut [Model]) {
    fn key(d: &str) -> String {
        let mut k = norm(d);
        while let Some(s) = ["ai", "org", "labs", "corp", "research", "inc"]
            .iter()
            .find_map(|x| k.strip_suffix(x).filter(|s| !s.is_empty()))
        {
            k = s.to_string();
        }
        k
    }
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
    zip.by_name(name).ok()?.read_to_end(&mut buf).ok()?;
    let mut rdr = csv::Reader::from_reader(buf.as_slice());
    let headers = rdr.headers().ok()?.clone();
    let rows = rdr
        .records()
        .flatten()
        .map(|r| headers.iter().map(String::from).zip(r.iter().map(String::from)).collect())
        .collect();
    Some(rows)
}

fn parse_epoch(bytes: &[u8]) -> Result<Epoch, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("epoch zip: {e}"))?;
    let meta = csv_rows(&mut zip, "model_metadata.csv").ok_or("epoch zip: missing model_metadata.csv")?;
    let mut ep = Epoch::default();
    let mut dates: HashMap<String, String> = HashMap::new();
    let mut version_group: HashMap<String, String> = HashMap::new();
    for r in &meta {
        let (version, group) = (col(r, "model_version")?, col(r, "model_group")?);
        if version.is_empty() || group.is_empty() {
            continue;
        }
        dates.insert(norm(group), col(r, "date")?.to_string());
        version_group.insert(version.to_string(), group.to_string());
        if let Some(o) = r.get("organization").filter(|o| !o.is_empty()) {
            ep.org.entry(norm(group)).or_insert_with(|| o.clone());
        }
    }
    let group_of = |version: &str| {
        version_group.get(version).cloned().unwrap_or_else(|| version.split('_').next().unwrap_or(version).to_string())
    };

    let task_benches = crate::fit::task_benches();
    let benches = csv_rows(&mut zip, "benchmark_metadata.csv").ok_or("epoch zip: missing benchmark_metadata.csv")?;
    for b in &benches {
        let (file, score_col, bench) = (col(b, "source_file")?, col(b, "score_column")?, col(b, "benchmark")?);
        if file.is_empty() || score_col.is_empty() || !task_benches.contains(&bench) {
            continue;
        }
        let scale: f64 = col(b, "scale")?.parse().unwrap_or(1.0);
        let Some(rows) = csv_rows(&mut zip, file) else { continue };
        for r in &rows {
            let (Some(v), Some(s)) = (r.get("Model version"), r.get(score_col)) else { continue };
            let Ok(s) = s.trim_end_matches('%').parse::<f64>() else { continue };
            let g = group_of(v);
            let e = ep.groups.entry(norm(&g)).or_insert_with(|| (g, None, BTreeMap::new()));
            let best = e.2.entry(bench.to_string()).or_insert(0.0);
            *best = best.max(s * scale);
        }
    }
    for r in csv_rows(&mut zip, "epoch_capabilities_index/eci_scores.csv").unwrap_or_default() {
        let Ok(eci) = col(&r, "eci")?.parse::<f64>() else { continue };
        let g = col(&r, "Model")?.to_string();
        dates.entry(norm(&g)).or_insert_with(|| r.get("date").cloned().unwrap_or_default());
        if let Some(o) = r.get("Organization").filter(|o| !o.is_empty()) {
            ep.org.entry(norm(&g)).or_insert_with(|| o.clone());
        }
        ep.groups.entry(norm(&g)).or_insert_with(|| (g, None, BTreeMap::new())).1 = Some(eci);
    }
    let mut newest: HashMap<String, &str> = HashMap::new();
    for (k, (name, _, _)) in &ep.groups {
        let short = norm(&clean_name(name));
        let date = dates.get(k).map_or("", String::as_str);
        // Strictly newer wins; on equal dates the first group in key order keeps the alias.
        if short != *k && newest.get(&short).is_none_or(|d| date > *d) {
            newest.insert(short.clone(), date);
            ep.alias.insert(short, k.clone());
        }
    }
    if ep.groups.is_empty() {
        return Err("epoch zip: no benchmark data found".into());
    }
    Ok(ep)
}

// ---------- merge ----------

fn merge(models_json: &[u8], epoch_zip: &[u8]) -> Result<Data, String> {
    let providers: HashMap<String, MdProvider> =
        serde_json::from_slice(models_json).map_err(|e| format!("models.dev: {e}"))?;
    let ep = parse_epoch(epoch_zip)?;
    let pct = crate::fit::percentiles(ep.groups.iter().map(|(k, (_, eci, s))| (k.as_str(), *eci, s)));
    let mut by_key: HashMap<String, Model> = HashMap::new();
    let mut openrouter: HashMap<String, String> = HashMap::new();
    // OpenRouter ids by their last part: "llama-4-maverick" -> "meta-llama/llama-4-maverick".
    let mut or_slug: HashMap<String, String> = HashMap::new();
    // Developer votes per model, one per offer: resellers prefix ids with their own names
    // ("@cf/meta/...", "novita/..."), so the first offer alone is not to be trusted.
    let mut dev_votes: HashMap<String, HashMap<String, usize>> = HashMap::new();

    let mut pids: Vec<&String> = providers.keys().collect();
    pids.sort(); // deterministic merge order
    let entries: Vec<(&String, &String, &MdModel, String)> = pids
        .into_iter()
        .flat_map(|pid| providers[pid].models.iter().map(move |(mid, md)| (pid, mid, md)))
        // Text generation models only: skip image/video/embedding endpoints.
        .filter(|(_, _, md)| md.modalities.output.is_empty() || md.modalities.output.iter().any(|o| o == "text"))
        .map(|(pid, mid, md)| (pid, mid, md, clean_name(if md.name.is_empty() { mid } else { &md.name })))
        .filter(|e| !norm(&e.3).is_empty())
        .collect();
    let keys: HashSet<String> = entries.iter().map(|e| norm(&e.3)).collect();

    for (pid, mid, md, name) in entries {
        let p = &providers[pid];
        let raw = norm(&name);
        let key = fold_vendor(&raw, &keys);
        if pid == "openrouter" {
            openrouter.entry(key.clone()).or_insert_with(|| mid.clone());
            or_slug.entry(slug(mid)).or_insert_with(|| mid.clone());
        }
        let m = by_key.entry(key.clone()).or_insert_with(|| Model {
            key: key.clone(),
            name: name.clone(),
            url: p.doc.clone(),
            ..Default::default()
        });
        if raw == key {
            m.name = name.clone();
        }
        let hint = developer_hint(pid, mid);
        if !hint.is_empty() {
            *dev_votes.entry(key.clone()).or_default().entry(hint).or_default() += 1;
        }
        m.context = m.context.max(md.limit.context);
        m.max_output = m.max_output.max(md.limit.output);
        m.tool_call |= md.tool_call;
        m.reasoning |= md.reasoning;
        m.open_weights |= md.open_weights;
        m.vision |= md.modalities.input.iter().any(|i| i == "image");
        if !md.release_date.is_empty() && (m.release.is_empty() || md.release_date < m.release) {
            m.release = md.release_date.clone();
        }
        if m.knowledge.is_empty() {
            m.knowledge = md.knowledge.clone();
        }
        let cost = md.cost.as_ref();
        m.offers.push(Offer {
            provider: pid.clone(),
            provider_name: p.name.clone(),
            id: if md.id.is_empty() { mid.clone() } else { md.id.clone() },
            env: p.env.clone(),
            input: cost.map_or(0.0, |c| c.input),
            output: cost.map_or(0.0, |c| c.output),
            // A few list a paid model's cache as 0, a placeholder; a cache never costs more than input.
            cache_read: cost.and_then(|c| c.cache_read.filter(|&r| r > 0.0).map(|r| r.min(c.input))),
            unpriced: cost.is_none(),
            ..Default::default()
        });
    }

    let moved = merge_same_ids(&mut by_key, |k| {
        openrouter.contains_key(k) || ep.groups.contains_key(k) || ep.alias.contains_key(k)
    });
    for (from, to) in moved {
        if let Some(id) = openrouter.remove(&from) {
            openrouter.entry(to.clone()).or_insert(id);
        }
        for (d, n) in dev_votes.remove(&from).unwrap_or_default() {
            *dev_votes.entry(to.clone()).or_default().entry(d).or_default() += n;
        }
    }

    let mut models: Vec<Model> = by_key.into_values().collect();
    for m in &mut models {
        // The family in the name is surest; else what most offers' ids say.
        m.developer = match developer_from_name(&m.name) {
            "" => dev_votes.remove(&m.key).and_then(|v| Some(v.into_iter().max_by_key(|(d, n)| (*n, d.clone()))?.0)),
            d => Some(d.into()),
        }
        .unwrap_or_default();
        // The model's OpenRouter page, or a search there when OpenRouter does not list it.
        // Else an offer with the same id as an OpenRouter model: Helicone's "llama-4-maverick"
        // is OpenRouter's, under whatever name Helicone gives it.
        // ponytail: ids without a digit ("auto", "free") are routers, not models, and never match.
        let or_id = openrouter.get(&m.key).or_else(|| {
            m.offers
                .iter()
                .map(|o| slug(&o.id))
                .filter(|s| s.bytes().any(|b| b.is_ascii_digit()))
                .find_map(|s| or_slug.get(&s))
        });
        m.url = or_id.map_or_else(
            || format!("https://openrouter.ai/models?q={}", m.name.replace(' ', "+")),
            |id| format!("https://openrouter.ai/{id}"),
        );
        let gk = if ep.groups.contains_key(&m.key) {
            Some(m.key.as_str())
        } else {
            ep.alias.get(&m.key).map(String::as_str)
        };
        if let Some(k) = gk
            && let Some((gname, eci, scores)) = ep.groups.get(k)
        {
            m.name = gname.clone();
            if let Some(o) = ep.org.get(k) {
                m.developer = short_org(o);
            }
            m.eci = *eci;
            m.scores = scores.clone();
            if let Some(f) = pct.get(k) {
                m.fit = f.clone();
            }
        }
    }
    unify_developers(&mut models);
    crate::fit::add_value(&mut models);
    models.sort_by(|a, b| b.eci.unwrap_or(0.0).total_cmp(&a.eci.unwrap_or(0.0)).then(b.release.cmp(&a.release)));
    let benches = crate::fit::task_benches().into_iter().map(String::from).collect();
    Ok(Data { format: FORMAT, fetched: now(), harness: BTreeMap::new(), benches, models })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_output_and_gives_up_on_hangs() {
        assert_eq!(run("sh", &["-c", "echo a/b"], Duration::from_secs(5)).as_deref(), Some("a/b\n"));
        assert_eq!(run("sh", &["-c", "exit 1"], Duration::from_secs(5)), None, "a failure reports nothing");
        let big = run("sh", &["-c", "head -c 200000 /dev/zero | tr '\\0' a"], Duration::from_secs(5));
        assert_eq!(big.map(|s| s.len()), Some(200_000), "more than a pipe holds");
        assert_eq!(run("no-such-binary-xyz", &[], Duration::from_secs(5)), None, "so does a missing harness");
        let t = Instant::now();
        assert_eq!(run("sleep", &["10"], Duration::from_millis(200)), None, "a hang is killed");
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
    fn folds_vendor_prefix() {
        let keys: HashSet<String> =
            ["gpt41", "openaigpt41", "prozaiorgglm5", "zaiorgglm5", "glm5x", "o3", "openaio3", "spacebunny"]
                .map(String::from)
                .into();
        assert_eq!(fold_vendor("openaigpt41", &keys), "gpt41");
        assert_eq!(fold_vendor("prozaiorgglm5", &keys), "zaiorgglm5");
        assert_eq!(fold_vendor("gpt41", &keys), "gpt41");
        assert_eq!(fold_vendor("openaio3", &keys), "o3");
        assert_eq!(fold_vendor("spacebunnyalpha", &keys), "spacebunny");
        assert_eq!(fold_vendor("oxalpha", &keys), "oxalpha", "no plain entry, no fold");
    }

    #[test]
    fn links_to_each_site_that_has_the_model() {
        let offer = |p: &str, id: &str| Offer { provider: p.into(), id: id.into(), ..Default::default() };
        let mut m = Model {
            name: "Claude Opus 5.5".into(),
            developer: "Anthropic".into(),
            url: "https://openrouter.ai/anthropic/claude-opus-5.5".into(),
            offers: vec![offer("openrouter", "anthropic/claude-opus-5.5"), offer("anthropic", "claude-opus-5-5")],
            ..Default::default()
        };
        let sites = |m: &Model| m.links().into_iter().map(|(s, _)| s).collect::<Vec<_>>();
        m.offers.push(offer("302ai", "claude-opus-5-5"));
        assert_eq!(sites(&m), ["models.dev", "openrouter.ai"], "no Epoch page without its benchmarks");
        m.eci = Some(160.0);
        assert_eq!(
            m.links(),
            [
                ("models.dev", "https://models.dev/models/anthropic/claude-opus-5-5/".into()),
                ("epoch.ai", "https://epoch.ai/models/claude-opus-5-5".into()),
                ("openrouter.ai", m.url.clone()),
            ]
        );
        m.offers.remove(1);
        assert_eq!(sites(&m), ["epoch.ai", "openrouter.ai"], "models.dev has pages only under the developer");
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
            ]),
            models: vec![
                m(vec![o("anthropic", "claude-opus-5"), o("openrouter", "claude-opus-5")]),
                m(vec![o("anthropic", "claude-haiku-4-5")]),
                m(vec![o("openai", "gpt-5")]),
            ],
            ..Default::default()
        };
        d.apply_available();
        assert!(d.models[0].available && d.models[0].offers[0].available && !d.models[0].offers[1].available);
        assert_eq!(d.models[0].via, ["claude", "opencode"]);
        assert_eq!(d.models[1].via, ["claude"], "a provider-wide harness covers every model of it");
        assert!(!d.models[2].available && d.models[2].via.is_empty());
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
            crate::app::col_about(crate::app::PRICE)
                .contains(&format!("{:.0}% of the input cached", crate::data::cached() * 100.0))
        );
        // A one-off prompt caches nothing: the cheaper list price wins again.
        set_cached(0.0);
        assert_eq!(m.price().unwrap().provider, "p");
        assert!(crate::app::col_about(crate::app::PRICE).contains(" 0% of the input cached"));
        set_cached(2.0);
        assert_eq!(crate::data::cached(), 1.0, "clamped");
        // A price nobody lists is unknown, not free: yours still names the id, but costs nothing known.
        let unknown = Offer { provider: "u".into(), unpriced: true, available: true, ..Default::default() };
        let free = Offer { provider: "f".into(), available: true, ..Default::default() };
        let m = Model { offers: vec![unknown.clone()], ..Default::default() };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost()), ("u", None));
        let m = Model { offers: vec![unknown, free], ..Default::default() };
        assert_eq!((m.price().unwrap().provider.as_str(), m.cost()), ("f", Some(0.0)), "a free offer is priced");
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
}
