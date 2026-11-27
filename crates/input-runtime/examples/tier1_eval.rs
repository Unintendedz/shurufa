//! 端侧两阶段（Tier 0 统计词格 + Tier 1 `ShurufaNeuralService` / `Qwen3-0.6B`）真实模型评测与门禁工具。
//!
//! 在单次热启动的 `ShurufaNeuralService` 实例上连续运行多套合成评测集，同时输出：
//! - Tier 0 与 Tier 1 的 `Top1` / `Top6` / `MRR`；
//! - 干净拼音首槽被纠错词劫持比例（`t0_typo_hijack` / `t1_typo_hijack`）；
//! - Tier 1 相对 Tier 0 的净救回数（`t1_rescues`）与倒退数（`t1_regressions`）；
//! - 首页违禁幻影词命中数（`forbidden_hits`）与展开候选池覆写数（`expand_wiped`）；
//! - Tier 1 真实端到端热推理延迟分位（`p50_ms` / `p90_ms` / `max_ms`）。

#[path = "eval_resources/mod.rs"]
mod eval_resources;

use msime_engine::host::{Command, Session};
use msime_input_runtime::{
    Action, NeuralClientConfig, NeuralServiceClient, NeuralWakeupFn, Runtime,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

struct SuiteSpec {
    name: String,
    path: PathBuf,
    max_syllables: Option<usize>,
}

struct Case {
    id: String,
    input: String,
    gold: String,
    tags: Vec<String>,
    context: String,
    forbidden_ahead: Vec<String>,
}

#[derive(Default, Clone)]
struct BucketStats {
    cases: usize,
    t0_top1: usize,
    t0_top_page: usize,
    t0_rr: f64,
    t0_typo_hijack: usize,
    clean_cases: usize,
    t1_top1: usize,
    t1_top_page: usize,
    t1_rr: f64,
    t1_typo_hijack: usize,
    t1_rescues: usize,
    t1_regressions: usize,
    t0_pool_misses: usize,
    forbidden_hits: usize,
    expand_wiped: usize,
}

impl BucketStats {
    fn rate(num: usize, den: usize) -> f64 {
        if den == 0 {
            0.0
        } else {
            num as f64 / den as f64
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "cases": self.cases,
            "clean_cases": self.clean_cases,
            "t0_top1": round4(Self::rate(self.t0_top1, self.cases)),
            "t0_top6": round4(Self::rate(self.t0_top_page, self.cases)),
            "t0_mrr": round4(if self.cases == 0 { 0.0 } else { self.t0_rr / self.cases as f64 }),
            "t0_typo_hijack": self.t0_typo_hijack,
            "t0_typo_hijack_rate": round4(Self::rate(self.t0_typo_hijack, self.clean_cases)),
            "t1_top1": round4(Self::rate(self.t1_top1, self.cases)),
            "t1_top6": round4(Self::rate(self.t1_top_page, self.cases)),
            "t1_mrr": round4(if self.cases == 0 { 0.0 } else { self.t1_rr / self.cases as f64 }),
            "t1_typo_hijack": self.t1_typo_hijack,
            "t1_typo_hijack_rate": round4(Self::rate(self.t1_typo_hijack, self.clean_cases)),
            "t1_rescues": self.t1_rescues,
            "t1_regressions": self.t1_regressions,
            "t0_pool_misses": self.t0_pool_misses,
            "forbidden_hits": self.forbidden_hits,
            "expand_wiped": self.expand_wiped,
        })
    }
}

struct SuiteResult {
    name: String,
    overall: BucketStats,
    by_tag: BTreeMap<String, BucketStats>,
    latencies_ms: Vec<f64>,
    failures: Vec<Value>,
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn unescape_tsv_field(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_suite_spec(spec: &str) -> Result<SuiteSpec, String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("invalid --suite '{spec}', expected <name>=<path>[:max_syl=N]"))?;
    let mut max_syllables = None;
    let path_str = if let Some((p, suffix)) = rest.split_once(":max_syl=") {
        let parsed: usize = suffix
            .parse()
            .map_err(|e| format!("invalid max_syl in '{spec}': {e}"))?;
        max_syllables = Some(parsed);
        p
    } else {
        rest
    };
    Ok(SuiteSpec {
        name: name.to_string(),
        path: PathBuf::from(path_str),
        max_syllables,
    })
}

fn load_cases(
    path: &Path,
    max_syllables: Option<usize>,
) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut cases = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() || line.starts_with("id\t") {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 4 {
            return Err(format!("{}: malformed row: {line}", path.display()).into());
        }
        let syllables: usize = fields[3].parse()?;
        if max_syllables.is_some_and(|max_s| syllables > max_s) {
            continue;
        }
        let tags = fields
            .get(4)
            .map(|t| {
                t.split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let context = fields
            .get(5)
            .map(|s| unescape_tsv_field(s))
            .unwrap_or_default();
        let forbidden_ahead = fields
            .get(6)
            .map(|f| {
                f.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        cases.push(Case {
            id: fields[0].to_string(),
            input: fields[1].to_string(),
            gold: fields[2].to_string(),
            tags,
            context,
            forbidden_ahead,
        });
    }
    Ok(cases)
}

fn wait_for_completed_decode(
    client: &NeuralServiceClient,
    rx: &mpsc::Receiver<(u64, u64)>,
    session_id: u64,
    generation: u64,
    timeout: Duration,
) -> Option<msime_engine::neural_v2::NeuralDecodeResponse> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(done) = client.take_completed_decode(session_id, generation) {
            return done;
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let remain = deadline.saturating_duration_since(now);
        let _ = rx.recv_timeout(remain.min(Duration::from_millis(50)));
    }
}

fn rank_of(candidates: &[(String, u8)], gold: &str) -> Option<usize> {
    candidates
        .iter()
        .position(|(text, _)| text == gold)
        .map(|i| i + 1)
}

#[allow(clippy::too_many_arguments)]
fn observe_bucket(
    bucket: &mut BucketStats,
    page_size: usize,
    is_typo_case: bool,
    t0_rank: Option<usize>,
    t1_rank: Option<usize>,
    t0_hijack: bool,
    t1_hijack: bool,
    in_t0_pool: bool,
    forbidden_hit: bool,
    expand_wiped: bool,
) {
    bucket.cases += 1;
    if !is_typo_case {
        bucket.clean_cases += 1;
        if t0_hijack {
            bucket.t0_typo_hijack += 1;
        }
        if t1_hijack {
            bucket.t1_typo_hijack += 1;
        }
    }
    if let Some(r0) = t0_rank {
        if r0 == 1 {
            bucket.t0_top1 += 1;
        }
        if r0 <= page_size {
            bucket.t0_top_page += 1;
        }
        bucket.t0_rr += 1.0 / (r0 as f64);
    }
    if let Some(r1) = t1_rank {
        if r1 == 1 {
            bucket.t1_top1 += 1;
        }
        if r1 <= page_size {
            bucket.t1_top_page += 1;
        }
        bucket.t1_rr += 1.0 / (r1 as f64);
    }
    if t0_rank != Some(1) && t1_rank == Some(1) {
        bucket.t1_rescues += 1;
    }
    if t0_rank == Some(1) && t1_rank != Some(1) {
        bucket.t1_regressions += 1;
    }
    if !in_t0_pool {
        bucket.t0_pool_misses += 1;
    }
    if forbidden_hit {
        bucket.forbidden_hits += 1;
    }
    if expand_wiped {
        bucket.expand_wiped += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn run_suite(
    suite_name: &str,
    cases: &[Case],
    runtime: &mut Runtime,
    client: &Arc<NeuralServiceClient>,
    rx: &mpsc::Receiver<(u64, u64)>,
    page_size: usize,
    max_failures: usize,
) -> Result<SuiteResult, Box<dyn std::error::Error>> {
    let mut overall = BucketStats::default();
    let mut by_tag: BTreeMap<String, BucketStats> = BTreeMap::new();
    let mut latencies_ms = Vec::with_capacity(cases.len());
    let mut failures = Vec::new();

    for case in cases {
        while rx.try_recv().is_ok() {}
        runtime.dispatch(Action::Command(Command::Cancel))?;
        runtime.clear_context();
        if !case.context.is_empty() {
            runtime.seed_context(&case.context);
        }
        for byte in case.input.bytes() {
            runtime.dispatch(Action::Character {
                value: byte,
                shift: false,
            })?;
        }

        let t0_view = runtime.view();
        let t0_visible = runtime.visible_candidates_snapshot();
        let t0_rank = rank_of(&t0_visible, &case.gold);
        let is_typo_case = case.tags.iter().any(|t| t == "typo");
        let t0_slot0_corrected = t0_view
            .candidates
            .first()
            .is_some_and(|c| c.corrected && c.text != case.gold);

        let mut in_t0_pool = t0_rank.is_some_and(|r| r <= page_size);
        let mut t1_latency_ms = None;

        if let Some(req) = runtime.take_neural_request() {
            in_t0_pool = req.candidates.iter().any(|c| c.text == case.gold);
            let session_id = req.session;
            let generation = req.generation;
            let started = Instant::now();
            if client.enqueue_decode(session_id, &req, Duration::from_secs(5)) {
                if let Some(resp) = wait_for_completed_decode(
                    client,
                    rx,
                    session_id,
                    generation,
                    Duration::from_secs(3),
                ) {
                    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                    t1_latency_ms = Some(elapsed_ms);
                    latencies_ms.push(elapsed_ms);
                    in_t0_pool =
                        in_t0_pool || resp.beam_candidates.iter().any(|b| b.text == case.gold);
                    let _ = runtime.apply_neural_response(&resp, false);
                }
            }
        }

        let t1_view = runtime.view();
        let t1_visible = runtime.visible_candidates_snapshot();
        let t1_rank = rank_of(&t1_visible, &case.gold);
        let t1_slot0_corrected = t1_view
            .candidates
            .first()
            .is_some_and(|c| c.corrected && c.text != case.gold);

        let page1_texts: Vec<String> = t1_view
            .candidates
            .iter()
            .take(page_size)
            .map(|c| c.text.clone())
            .collect();
        let gold_page_pos = page1_texts.iter().position(|shown| shown == &case.gold);
        // 违禁幻影/占位候选只要抢占首槽或排在金标准之前（或金标准落出首页时违禁词闯入首页），立即判为违规。
        let forbidden_hit = case.forbidden_ahead.iter().any(|bad| {
            page1_texts
                .iter()
                .position(|shown| shown == bad)
                .is_some_and(|bad_pos| match gold_page_pos {
                    Some(gold_pos) => bad_pos == 0 || bad_pos < gold_pos,
                    None => true,
                })
        });

        // 检验展开全部候选（`all_candidates()`）是否会冲掉已结算的 Tier 1 首页排序。
        let expanded = runtime.all_candidates();
        let expanded_page1: Vec<String> = expanded
            .candidates
            .iter()
            .take(page1_texts.len())
            .map(|c| c.text.clone())
            .collect();
        let expand_wiped = expanded_page1 != page1_texts;

        observe_bucket(
            &mut overall,
            page_size,
            is_typo_case,
            t0_rank,
            t1_rank,
            t0_slot0_corrected,
            t1_slot0_corrected,
            in_t0_pool,
            forbidden_hit,
            expand_wiped,
        );
        for tag in &case.tags {
            observe_bucket(
                by_tag.entry(tag.clone()).or_default(),
                page_size,
                is_typo_case,
                t0_rank,
                t1_rank,
                t0_slot0_corrected,
                t1_slot0_corrected,
                in_t0_pool,
                forbidden_hit,
                expand_wiped,
            );
        }

        let notable_failure = t1_rank.is_none_or(|r| r > page_size)
            || (t0_rank == Some(1) && t1_rank != Some(1))
            || (!is_typo_case && (t0_slot0_corrected || t1_slot0_corrected))
            || forbidden_hit;
        if notable_failure && failures.len() < max_failures {
            let t0_top6: Vec<String> = t0_view
                .candidates
                .iter()
                .take(page_size)
                .map(|c| {
                    if c.corrected {
                        format!("{}*", c.text)
                    } else {
                        c.text.clone()
                    }
                })
                .collect();
            let t1_top6: Vec<String> = t1_view
                .candidates
                .iter()
                .take(page_size)
                .map(|c| {
                    if c.corrected {
                        format!("{}*", c.text)
                    } else {
                        c.text.clone()
                    }
                })
                .collect();
            failures.push(json!({
                "id": case.id,
                "input": case.input,
                "gold": case.gold,
                "context": case.context,
                "tags": case.tags,
                "t0_rank": t0_rank,
                "t1_rank": t1_rank,
                "t0_hijack": !is_typo_case && t0_slot0_corrected,
                "t1_hijack": !is_typo_case && t1_slot0_corrected,
                "in_t0_pool": in_t0_pool,
                "forbidden_hit": forbidden_hit,
                "latency_ms": t1_latency_ms.map(round4),
                "t0_page1": t0_top6,
                "t1_page1": t1_top6,
            }));
        }

        runtime.dispatch(Action::Command(Command::Cancel))?;
    }

    latencies_ms.sort_by(|a, b| a.total_cmp(b));
    Ok(SuiteResult {
        name: suite_name.to_string(),
        overall,
        by_tag,
        latencies_ms,
        failures,
    })
}

fn check_gates(results: &[SuiteResult]) -> Vec<String> {
    let mut violations = Vec::new();
    for suite in results {
        let o = &suite.overall;
        let t1_top1 = BucketStats::rate(o.t1_top1, o.cases);
        let t1_top6 = BucketStats::rate(o.t1_top_page, o.cases);
        let t0_hijack_rate = BucketStats::rate(o.t0_typo_hijack, o.clean_cases);
        let t1_hijack_rate = BucketStats::rate(o.t1_typo_hijack, o.clean_cases);
        let p90 = percentile(&suite.latencies_ms, 0.90);

        if o.forbidden_hits > 0 {
            violations.push(format!(
                "[{}] forbidden page-1 phantom/placeholder hits = {} (expected 0)",
                suite.name, o.forbidden_hits
            ));
        }
        if o.expand_wiped > 0 {
            violations.push(format!(
                "[{}] all_candidates()/paging wiped settled Tier 1 top1 {} time(s) (expected 0)",
                suite.name, o.expand_wiped
            ));
        }
        if p90 > 120.0 {
            violations.push(format!(
                "[{}] Tier 1 p90 latency {:.1} ms exceeds 120.0 ms ceiling",
                suite.name, p90
            ));
        }

        match suite.name.as_str() {
            "spoken-v1" => {
                if t1_top1 < 0.900 {
                    violations.push(format!("[spoken-v1] Tier 1 top1 {:.3} < 0.900", t1_top1));
                }
                if t1_top6 < 0.965 {
                    violations.push(format!("[spoken-v1] Tier 1 top6 {:.3} < 0.965", t1_top6));
                }
                if o.t1_regressions > 0 {
                    violations.push(format!(
                        "[spoken-v1] Tier 1 regressions = {} (expected 0)",
                        o.t1_regressions
                    ));
                }
            }
            "guardrails-v1" => {
                if let Some(b) = suite.by_tag.get("boundary-single") {
                    let rate = BucketStats::rate(b.t1_top_page, b.cases);
                    if rate < 0.985 {
                        violations.push(format!(
                            "[guardrails-v1/boundary-single] Tier 1 top6 {:.3} < 0.985 ({}/{})",
                            rate, b.t1_top_page, b.cases
                        ));
                    }
                }
                if let Some(b) = suite.by_tag.get("phantom-reading") {
                    let rate = BucketStats::rate(b.t1_top_page, b.cases);
                    if rate < 1.0 || b.forbidden_hits > 0 {
                        violations.push(format!(
                            "[guardrails-v1/phantom-reading] Tier 1 top6 {:.3}, forbidden_hits={}",
                            rate, b.forbidden_hits
                        ));
                    }
                }
                if let Some(b) = suite.by_tag.get("unmeasured-dict") {
                    let rate = BucketStats::rate(b.t1_top_page, b.cases);
                    if rate < 0.930 {
                        violations.push(format!(
                            "[guardrails-v1/unmeasured-dict] Tier 1 top6 {:.3} < 0.930 ({}/{})",
                            rate, b.t1_top_page, b.cases
                        ));
                    }
                }
                if let Some(b) = suite.by_tag.get("typo-genuine") {
                    let rate = BucketStats::rate(b.t1_top_page, b.cases);
                    if rate < 0.850 {
                        violations.push(format!(
                            "[guardrails-v1/typo-genuine] genuine typo top6 recall {:.3} < 0.850 ({}/{})",
                            rate, b.t1_top_page, b.cases
                        ));
                    }
                }
                if let Some(b) = suite.by_tag.get("mixed-english-ctx") {
                    let rate = BucketStats::rate(b.t1_top_page, b.cases);
                    if rate < 0.900 {
                        violations.push(format!(
                            "[guardrails-v1/mixed-english-ctx] Tier 1 top6 {:.3} < 0.900 ({}/{})",
                            rate, b.t1_top_page, b.cases
                        ));
                    }
                }
            }
            "sentences-v2-4char" => {
                if t1_top1 < 0.900 {
                    violations.push(format!(
                        "[sentences-v2-4char] Tier 1 top1 {:.3} < 0.900",
                        t1_top1
                    ));
                }
                if t0_hijack_rate > 0.035 {
                    violations.push(format!(
                        "[sentences-v2-4char] Tier 0 clean-pinyin typo hijack rate {:.3} > 0.035",
                        t0_hijack_rate
                    ));
                }
                if t1_hijack_rate > 0.025 {
                    violations.push(format!(
                        "[sentences-v2-4char] Tier 1 clean-pinyin typo hijack rate {:.3} > 0.025",
                        t1_hijack_rate
                    ));
                }
            }
            _ => {}
        }
    }
    violations
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut resources: Option<PathBuf> = None;
    let mut resource_lock: Option<PathBuf> = None;
    let mut grammar_model: Option<PathBuf> = None;
    let mut helper_path: Option<PathBuf> = None;
    let mut model_dir: Option<PathBuf> = None;
    let mut suites: Vec<SuiteSpec> = Vec::new();
    let mut report_path: Option<PathBuf> = None;
    let mut page_size: u8 = 6;
    let mut max_failures: usize = 30;
    let mut gate = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let take = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", args[*i - 1]))
        };
        match args[index].as_str() {
            "--resources" => resources = Some(PathBuf::from(take(&mut index)?)),
            "--resource-lock" => resource_lock = Some(PathBuf::from(take(&mut index)?)),
            "--grammar-model" => grammar_model = Some(PathBuf::from(take(&mut index)?)),
            "--helper" => helper_path = Some(PathBuf::from(take(&mut index)?)),
            "--model-dir" => model_dir = Some(PathBuf::from(take(&mut index)?)),
            "--suite" => suites.push(parse_suite_spec(&take(&mut index)?)?),
            "--report" => report_path = Some(PathBuf::from(take(&mut index)?)),
            "--page-size" => page_size = take(&mut index)?.parse()?,
            "--max-failures" => max_failures = take(&mut index)?.parse()?,
            "--gate" => gate = true,
            other => return Err(format!("unknown option: {other}").into()),
        }
        index += 1;
    }

    let resources = resources.ok_or(
        "usage: tier1_eval --resources <dir> [--resource-lock <lock.json>] --grammar-model <file.gram> --helper <ShurufaNeuralService> --model-dir <gguf-dir> --suite <name>=<file.tsv> [--gate]",
    )?;
    let helper_path = helper_path.ok_or("--helper <ShurufaNeuralService> is required")?;
    let model_dir = model_dir.ok_or("--model-dir <gguf-dir> is required")?;
    if suites.is_empty() {
        return Err("at least one --suite <name>=<file.tsv> is required".into());
    }

    let verified = eval_resources::verify(&resources, resource_lock.as_deref())?;
    let state = tempfile::tempdir()?;

    let mut options = msime_engine::host::prepare_options(
        resources.to_str().ok_or("non-UTF-8 resource path")?,
        state.path().join("user").to_str().ok_or("non-UTF-8 path")?,
        state
            .path()
            .join("cache")
            .to_str()
            .ok_or("non-UTF-8 path")?,
        &verified.source_commit,
    )?;
    options.scheme = 0;
    options.learning = false;
    options.frequency_mode = "disabled".into();
    options.autocorrect_transposition = true;
    options.autocorrect_neighbor = true;
    options.fuzzy_pinyin_rules = 0;
    options.mixed_english = true;
    options.mixed_emoji = false;
    options.mixed_kaomoji = false;
    options.mixed_symbols = false;
    options.local_unicode = false;
    options.local_date_time = false;
    options.local_quick_phrase = false;
    options.local_emoji = false;
    options.local_kaomoji = false;
    options.local_super_jianpin = false;
    options.local_temporary_english = false;
    options.sentence_alternatives = true;
    if let Some(gram) = &grammar_model {
        options.sentence_association.collocation_model = gram.clone();
        options.sentence_association.collocation_rerank = true;
    }

    let engine = Session::new(&options)?;
    let clamped_page = page_size.clamp(1, 9);
    let mut runtime = Runtime::new(engine, clamped_page)?;
    runtime.set_neural_v2_enabled(true);
    runtime.focus(true)?;

    let (tx, rx) = mpsc::channel::<(u64, u64)>();
    let wakeup: NeuralWakeupFn = Arc::new(move |sid, generation| {
        let _ = tx.send((sid, generation));
    });
    let socket_path = PathBuf::from(format!(
        "/private/tmp/shurufa-t1eval-{}.sock",
        std::process::id()
    ));
    let client = NeuralServiceClient::new(
        NeuralClientConfig {
            helper_path: Some(helper_path),
            model_directory: Some(model_dir),
            pinyin_db_path: Some(resources.join("msime-pinyin.db")),
            resources_directory: Some(resources.clone()),
            socket_path: Some(socket_path),
            extra_launch_arguments: Vec::new(),
            idle_unload_seconds: 120.0,
            request_timeout: Duration::from_millis(2500),
            require_model_file: true,
        },
        Some(wakeup),
    );

    let health = client
        .check_health_sync(Duration::from_secs(12))
        .ok_or("ShurufaNeuralService failed health check or model load")?;
    eprintln!("neural service ready: {health}");

    // 先跑一次热身解码，预热 Metal 管线与 BPE 前缀索引，避免首条样本掺入冷启动耗时。
    runtime.seed_context("今天");
    for b in b"women" {
        runtime.dispatch(Action::Character {
            value: *b,
            shift: false,
        })?;
    }
    if let Some(req) = runtime.take_neural_request() {
        let sid = req.session;
        let generation = req.generation;
        if client.enqueue_decode(sid, &req, Duration::from_secs(5)) {
            let _ =
                wait_for_completed_decode(&client, &rx, sid, generation, Duration::from_secs(5));
        }
    }
    runtime.dispatch(Action::Command(Command::Cancel))?;
    runtime.clear_context();

    let total_started = Instant::now();
    let mut results = Vec::with_capacity(suites.len());
    for spec in &suites {
        let cases = load_cases(&spec.path, spec.max_syllables)?;
        let res = run_suite(
            &spec.name,
            &cases,
            &mut runtime,
            &client,
            &rx,
            usize::from(clamped_page),
            max_failures,
        )?;
        let o = &res.overall;
        let p50 = percentile(&res.latencies_ms, 0.50);
        let p90 = percentile(&res.latencies_ms, 0.90);
        eprintln!(
            "[{:<18}] n={:<3} | T0 top1={:.3} top6={:.3} hijack={:>2}/{:<3} | T1 top1={:.3} top6={:.3} hijack={:>2}/{:<3} rescue=+{:<2} reg=-{:<2} forbid={} wipe={} | p50={:.1}ms p90={:.1}ms",
            res.name,
            o.cases,
            BucketStats::rate(o.t0_top1, o.cases),
            BucketStats::rate(o.t0_top_page, o.cases),
            o.t0_typo_hijack,
            o.clean_cases,
            BucketStats::rate(o.t1_top1, o.cases),
            BucketStats::rate(o.t1_top_page, o.cases),
            o.t1_typo_hijack,
            o.clean_cases,
            o.t1_rescues,
            o.t1_regressions,
            o.forbidden_hits,
            o.expand_wiped,
            p50,
            p90,
        );
        results.push(res);
    }
    let wall_secs = total_started.elapsed().as_secs_f64();
    client.stop();

    let violations = check_gates(&results);
    let suites_json: BTreeMap<String, Value> = results
        .iter()
        .map(|s| {
            let tags_json: BTreeMap<String, Value> = s
                .by_tag
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect();
            (
                s.name.clone(),
                json!({
                    "overall": s.overall.to_json(),
                    "latency_ms": {
                        "count": s.latencies_ms.len(),
                        "p50": round4(percentile(&s.latencies_ms, 0.50)),
                        "p90": round4(percentile(&s.latencies_ms, 0.90)),
                        "max": round4(s.latencies_ms.last().copied().unwrap_or(0.0)),
                    },
                    "by_tag": tags_json,
                    "failures": s.failures,
                }),
            )
        })
        .collect();

    let doc = json!({
        "wall_seconds": round4(wall_secs),
        "gate_passed": violations.is_empty(),
        "gate_violations": violations,
        "suites": suites_json,
    });
    let rendered = serde_json::to_string_pretty(&doc)?;
    if let Some(path) = &report_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{rendered}\n"))?;
    }
    println!("{rendered}");

    if gate && !violations.is_empty() {
        for v in &violations {
            eprintln!("GATE FAIL: {v}");
        }
        return Err(format!("{} Tier 1 gate violation(s)", violations.len()).into());
    }
    eprintln!("tier1_eval finished in {:.2}s (gate={gate})", wall_secs);
    Ok(())
}
