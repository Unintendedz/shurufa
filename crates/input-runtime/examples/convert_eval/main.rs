//! Measures pinyin-to-text conversion quality against the locked dictionary.

#[path = "../eval_resources/mod.rs"]
mod eval_resources;
mod metrics;

use metrics::{Bucket, Observation, Report};
use msime_engine::host::{Command, Session};
use msime_input_runtime::{Action, Runtime};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

struct Case {
    id: String,
    input: String,
    gold: String,
    syllables: usize,
    tags: Vec<String>,
    context: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut resources: Option<PathBuf> = None;
    let mut resource_lock: Option<PathBuf> = None;
    let mut sets: Vec<PathBuf> = Vec::new();
    let mut limit: Option<usize> = None;
    let mut report_path: Option<PathBuf> = None;
    let mut baseline: Option<PathBuf> = None;
    let mut update = false;
    let mut dump: Option<PathBuf> = None;
    let mut autocorrect = false;
    let mut attribution = false;
    let mut grammar_model: Option<PathBuf> = None;
    let mut collocation_weight: Option<f64> = None;
    let mut page_size: u8 = 9;

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
            "--set" => sets.push(PathBuf::from(take(&mut index)?)),
            "--limit" => limit = Some(take(&mut index)?.parse()?),
            "--report" => report_path = Some(PathBuf::from(take(&mut index)?)),
            "--baseline" => baseline = Some(PathBuf::from(take(&mut index)?)),
            "--update-baseline" => update = true,
            "--dump" => dump = Some(PathBuf::from(take(&mut index)?)),
            "--autocorrect" => autocorrect = true,
            "--attribution" => attribution = true,
            "--no-settled" | "--no-neural" | "--settled-lock-leader" | "--keyboard-neural" => {}
            "--grammar-model" => grammar_model = Some(PathBuf::from(take(&mut index)?)),
            "--collocation-weight" => collocation_weight = Some(take(&mut index)?.parse()?),
            "--page-size" => page_size = take(&mut index)?.parse()?,
            other => return Err(format!("unknown option: {other}").into()),
        }
        index += 1;
    }
    let resources =
        resources.ok_or("usage: convert_eval --resources <verified-dir> --set <file.tsv> [--resource-lock <file.json>] [...]")?;
    if sets.is_empty() {
        return Err("at least one --set is required".into());
    }

    let verified = eval_resources::verify(&resources, resource_lock.as_deref())?;

    let mut cases = Vec::new();
    for set in &sets {
        cases.extend(load(set)?);
    }
    if let Some(limit) = limit {
        let stride = cases.len().div_ceil(limit.max(1));
        cases = cases.into_iter().step_by(stride.max(1)).collect();
    }
    eprintln!("{} cases from {} set(s)", cases.len(), sets.len());

    let state = tempfile::tempdir()?;
    let report = run(
        &EvalConfig {
            resources: &resources,
            state: state.path(),
            dump: dump.as_deref(),
            autocorrect,
            attribution,
            grammar_model: grammar_model.as_deref(),
            collocation_weight,
            page_size,
            source_commit: &verified.source_commit,
        },
        &cases,
    )?;
    let json = render(&report, &cases, attribution);

    if let Some(path) = &report_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{json}\n"))?;
    }
    println!("{json}");

    if let Some(path) = baseline {
        if update {
            std::fs::write(&path, format!("{json}\n"))?;
            eprintln!("baseline written: {}", path.display());
        } else if path.exists() {
            let previous = std::fs::read_to_string(&path)?;
            if previous.trim() != json.trim() {
                eprintln!(
                    "eval differs from {}; run with --update-baseline to accept",
                    path.display()
                );
                return Err("eval baseline mismatch".into());
            }
            eprintln!("eval: at baseline");
        }
    }
    Ok(())
}

fn load(path: &Path) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
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
        cases.push(Case {
            id: fields[0].to_string(),
            input: fields[1].to_string(),
            gold: fields[2].to_string(),
            syllables: fields[3].parse()?,
            context: fields.get(5).unwrap_or(&"").to_string(),
            tags: fields
                .get(4)
                .map(|t| {
                    t.split(',')
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(cases)
}

struct EvalConfig<'a> {
    resources: &'a Path,
    state: &'a Path,
    dump: Option<&'a Path>,
    autocorrect: bool,
    attribution: bool,
    grammar_model: Option<&'a Path>,
    collocation_weight: Option<f64>,
    page_size: u8,
    source_commit: &'a str,
}

/// 一次性读取 `msime-pinyin.db` 所有单字表 `tbl_1_*` 中的汉字集合，供归因区分「字表缺失 OOV」与「可拼出但搜索被剪」。
fn load_single_chars(resources: &Path) -> Result<HashSet<char>, Box<dyn std::error::Error>> {
    let path = resources.join("msime-pinyin.db");
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut table_stmt =
        conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'tbl_1_%'")?;
    let tables: Vec<String> = table_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    let mut chars = HashSet::new();
    for table in tables {
        let mut stmt = conn.prepare(&format!("SELECT value FROM {table}"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for value in rows {
            let value = value?;
            let mut iter = value.chars();
            if let (Some(ch), None) = (iter.next(), iter.next()) {
                chars.insert(ch);
            }
        }
    }
    Ok(chars)
}

fn run(config: &EvalConfig<'_>, cases: &[Case]) -> Result<Report, Box<dyn std::error::Error>> {
    let EvalConfig {
        resources,
        state,
        dump,
        autocorrect,
        attribution,
        grammar_model,
        collocation_weight,
        page_size,
        source_commit,
    } = *config;
    let single_chars = if attribution {
        Some(load_single_chars(resources)?)
    } else {
        None
    };
    let mut dumped = String::new();
    let mut options = msime_engine::host::prepare_options(
        resources.to_str().ok_or("non-UTF-8 resource path")?,
        state.join("user").to_str().ok_or("non-UTF-8 state path")?,
        state.join("cache").to_str().ok_or("non-UTF-8 cache path")?,
        source_commit,
    )?;
    options.scheme = 0;
    options.learning = false;
    options.frequency_mode = "disabled".into();
    options.autocorrect_transposition = autocorrect;
    options.autocorrect_neighbor = autocorrect;
    options.fuzzy_pinyin_rules = 0;
    options.mixed_english = false;
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
    if let Some(grammar_path) = grammar_model {
        options.sentence_association.collocation_model = grammar_path.to_path_buf();
        options.sentence_association.collocation_rerank = true;
        if let Some(w) = collocation_weight {
            options.sentence_association.collocation_weight = w;
        }
        eprintln!(
            "collocation grammar: {} (weight={})",
            grammar_path.display(),
            options.sentence_association.collocation_weight
        );
    }

    let engine = Session::new(&options)?;
    let mut runtime = Runtime::new(engine, page_size.clamp(1, 9))?;
    runtime.focus(true)?;
    let mut report = Report::default();

    for case in cases {
        runtime.dispatch(Action::Command(Command::Cancel))?;
        runtime.clear_context();
        runtime.seed_context(&case.context);
        for byte in case.input.bytes() {
            runtime.dispatch(Action::Character {
                value: byte,
                shift: false,
            })?;
        }
        let immediate_candidates = if attribution {
            Some(runtime.visible_candidates_snapshot())
        } else {
            None
        };
        let snapshot = runtime.all_candidates();
        let candidates: Vec<(String, u8)> = snapshot
            .candidates
            .iter()
            .map(|c| (c.text.clone(), c.source))
            .collect();
        if let (Some(immediate), Some(chars)) =
            (immediate_candidates.as_deref(), single_chars.as_ref())
        {
            let raw_engine = runtime.raw_engine_candidates();
            let spellable = case.gold.chars().all(|ch| chars.contains(&ch));
            report
                .attribution
                .observe(&case.gold, immediate, &candidates, &raw_engine, spellable);
        }
        if dump.is_some() {
            let rows: Vec<serde_json::Value> = candidates
                .iter()
                .map(|(text, source)| serde_json::json!({"text": text, "source": source}))
                .collect();
            dumped.push_str(
                &serde_json::json!({
                    "id": case.id, "input": case.input, "gold": case.gold,
                    "context": case.context, "candidates": rows,
                })
                .to_string(),
            );
            dumped.push('\n');
        }
        report.observe(Observation {
            id: &case.id,
            input: &case.input,
            gold: &case.gold,
            tags: &case.tags,
            syllables: case.syllables,
            candidates: &candidates,
            keep_failure: report.failures.len() < 40,
        });
        runtime.dispatch(Action::Command(Command::Cancel))?;
    }
    if let Some(path) = dump {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, dumped)?;
    }
    Ok(report)
}

fn render(report: &Report, cases: &[Case], attribution: bool) -> String {
    let bucket = |b: &Bucket| {
        serde_json::json!({
            "cases": b.cases,
            "top1": round(Bucket::rate(b.top1, b.cases)),
            "top5": round(Bucket::rate(b.top5, b.cases)),
            "top9": round(Bucket::rate(b.top9, b.cases)),
            "found": round(Bucket::rate(b.found, b.cases)),
            "mrr": round(if b.cases == 0 { 0.0 } else { b.reciprocal_rank / b.cases as f64 }),
            "top1_char_prefix": round(if b.cases == 0 { 0.0 } else { b.top1_char_prefix / b.cases as f64 }),
        })
    };
    let by_syllables: BTreeMap<String, _> = report
        .by_syllables
        .iter()
        .map(|(k, v)| (k.to_string(), bucket(v)))
        .collect();
    let by_tag: BTreeMap<String, _> = report
        .by_tag
        .iter()
        .map(|(k, v)| (k.clone(), bucket(v)))
        .collect();

    let mut value = serde_json::json!({
        "cases": cases.len(),
        "overall": bucket(&report.overall),
        "by_syllables": by_syllables,
        "by_tag": by_tag,
        "gold_source": report.gold_source,
        "top1_source": report.top1_source,
        "sample_failures": report.failures.iter().take(40).map(|f| serde_json::json!({
            "id": f.id, "input": f.input, "gold": f.gold, "top1": f.got, "rank": f.rank,
        })).collect::<Vec<_>>(),
    });
    if attribution {
        let a = &report.attribution;
        let pct = a.percentiles().map(|p| {
            serde_json::json!({
                "p50": p.p50,
                "p90": p.p90,
                "p95": p.p95,
                "max": p.max,
            })
        });
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "attribution".to_string(),
                serde_json::json!({
                    "cases": a.cases,
                    "top1": a.top1,
                    "page1_not_top1": a.page1_not_top1,
                    "presentation_demoted_off_page1": a.presentation_demoted_off_page1,
                    "engine_rank_gt6": a.engine_rank_gt6,
                    "search_pruned": a.search_pruned,
                    "dict_oov": a.dict_oov,
                    "immediate_top1": round(Bucket::rate(a.immediate_top1, a.cases)),
                    "immediate_top6": round(Bucket::rate(a.immediate_top6, a.cases)),
                    "settled_top6": round(Bucket::rate(a.settled_top6, a.cases)),
                    "settled_jitter": {
                        "flips": a.settled_top1_flips,
                        "flip_rate": round(Bucket::rate(a.settled_top1_flips, a.cases)),
                        "rescues": a.settled_top1_rescues,
                        "regressions": a.settled_top1_regressions,
                        "lateral": a.settled_top1_lateral,
                    },
                    "rank_percentiles": pct,
                }),
            );
        }
    }
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
