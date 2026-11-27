//! 离线构建 `shurufa-lexicon-calibration.bin`（`SLXC` v1）。
//!
//! 用法：
//! `cargo run --release -p msime-engine --example build_lexicon_calibration -- --pinyin-db <path> [--gram <path>] --output <path>`

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use msime_engine::calibration::{
    hash_key_value, CalibrationRecord, LexiconCalibration, LEXICON_CALIBRATION_FILENAME,
};
use msime_engine::octagram::GramDb;

/// 已词汇化或高频指代/评价类的两字「`X的`」保留名单，不参与组合伪词降权。
const LEXICALIZED_X_DE: &[&str] = &[
    "我的", "你的", "他的", "她的", "它的", "您的", "谁的", "咱的", "啥的", "有的", "别的", "是的",
    "对的", "好的", "真的", "假的", "似的", "男的", "女的", "老的", "小的", "大的", "新的", "旧的",
    "错的", "目的", "标的",
];

/// 非词汇化「`X的`」组合伪词标定权重（低于常规实测词与 `100` 占位线，避免在三音节输入如 `yudedao` 中霸占前缀槽）。
const COMPOSITIONAL_X_DE_WEIGHT: i32 = 80;

fn is_han_char(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0x20000..=0x2CEAF
    )
}

fn is_all_han(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_han_char)
}

fn default_gram_path() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("SHURUFA_GRAMMAR_MODEL") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p);
        }
    }
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home)
        .join("Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/wanxiang-lts-zh-hans.gram");
    p.is_file().then_some(p)
}

fn calibrate_unmeasured_phrase(gram: &GramDb, value: &str, n_chars: usize) -> Option<i32> {
    if !(3..=5).contains(&n_chars) {
        return None;
    }
    // `.gram` 叶子存储值含 +14.0 偏移（`collocation_penalty = -14.0`，`scaled = 13.2+` 方代表显著高频共现）；
    // 仅对真实高频口语/习语做平滑对数线性升权，低频边缘词条保留 `100` 交由运行时占位护栏处理。
    let exact_ln = gram.exact_phrase_ln_count(value);
    if n_chars == 3 {
        let ln = exact_ln?;
        if ln < 13.2 {
            return None;
        }
        let w = (550.0 * ((ln - 13.2) * 0.62).exp())
            .round()
            .clamp(600.0, 14_000.0) as i32;
        return Some(w);
    }

    if let Some(ln) = exact_ln {
        if ln >= 13.5 {
            let w = (650.0 * ((ln - 13.5) * 0.62).exp())
                .round()
                .clamp(700.0, 16_000.0) as i32;
            return Some(w);
        }
    }

    // 4/5 字口语短语若未被单独收为高阶叶子（如 `恶狠狠地`），
    // 要求内部所有重叠 3-gram 窗口全部在 `.gram` 中强共现（`>= 14.5`），严防跨边界双字幻觉拼接。
    let chars: Vec<char> = value.chars().collect();
    let mut min_tri = f64::INFINITY;
    for start in 0..=(chars.len() - 3) {
        let tri: String = chars[start..start + 3].iter().collect();
        let ln = gram.exact_phrase_ln_count(&tri)?;
        if ln < min_tri {
            min_tri = ln;
        }
    }
    if min_tri >= 14.5 {
        let w = (500.0 * ((min_tri - 14.5) * 0.55).exp())
            .round()
            .clamp(600.0, 6_500.0) as i32;
        return Some(w);
    }
    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut pinyin_db: Option<PathBuf> = None;
    let mut gram_path: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;
    let mut inspect_words: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pinyin-db" => {
                pinyin_db = Some(PathBuf::from(
                    args.next().ok_or("--pinyin-db requires a path")?,
                ));
            }
            "--gram" => {
                gram_path = Some(PathBuf::from(args.next().ok_or("--gram requires a path")?));
            }
            "--output" => {
                output_path = Some(PathBuf::from(
                    args.next().ok_or("--output requires a path")?,
                ));
            }
            "--inspect" => {
                inspect_words.push(args.next().ok_or("--inspect requires a word")?);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let pinyin_db = pinyin_db.ok_or("missing --pinyin-db <msime-pinyin.db>")?;
    let output_path = output_path.unwrap_or_else(|| {
        PathBuf::from("target/language-dictionaries").join(LEXICON_CALIBRATION_FILENAME)
    });
    let gram_resolved = gram_path.or_else(default_gram_path);
    let gram = gram_resolved.as_deref().and_then(GramDb::load);

    let conn = Connection::open_with_flags(
        &pinyin_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'tbl_%' ORDER BY name",
    )?;
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut char_max_weight: HashMap<String, i64> = HashMap::new();
    let mut syl_max_weight: HashMap<String, i64> = HashMap::new();
    let mut all_pairs: Vec<(String, String, i64, String)> = Vec::with_capacity(750_000);

    for table in &tables {
        let sql = format!("SELECT \"key\", \"value\", \"weight\" FROM \"{table}\"");
        let mut q = conn.prepare(&sql)?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let is_tbl1 = table.starts_with("tbl_1_");
        for row in rows {
            let (key, value, weight) = row?;
            if is_tbl1 {
                char_max_weight
                    .entry(value.clone())
                    .and_modify(|m| *m = (*m).max(weight))
                    .or_insert(weight);
                syl_max_weight
                    .entry(key.clone())
                    .and_modify(|m| *m = (*m).max(weight))
                    .or_insert(weight);
            }
            all_pairs.push((key, value, weight, table.clone()));
        }
    }

    let lexicalized_x_de: HashSet<&str> = LEXICALIZED_X_DE.iter().copied().collect();
    // 同一 `(key, value)` 若在同表出现重复行，仅保留首次决策。
    let mut decisions: HashMap<(String, String), (i32, i32)> = HashMap::new();
    let mut suppressed_phantoms = 0usize;
    let mut demoted_x_de = 0usize;
    let mut promoted_unmeasured = 0usize;

    for (key, value, weight, table) in &all_pairs {
        let Ok(raw_i32) = i32::try_from(*weight) else {
            continue;
        };
        if table.starts_with("tbl_1_") {
            if *weight == 0
                && value.chars().count() == 1
                && char_max_weight.get(value).copied().unwrap_or(0) >= 20_000
                && syl_max_weight.get(key).copied().unwrap_or(0) >= 500
            {
                if decisions
                    .insert((key.clone(), value.clone()), (raw_i32, -1))
                    .is_none()
                {
                    suppressed_phantoms += 1;
                }
            }
            continue;
        }

        if table.starts_with("tbl_2_")
            && *weight >= 500
            && key.ends_with("'de")
            && key.split('\'').count() == 2
            && value.chars().count() == 2
            && value.ends_with('的')
            && !lexicalized_x_de.contains(value.as_str())
        {
            if decisions
                .insert(
                    (key.clone(), value.clone()),
                    (raw_i32, COMPOSITIONAL_X_DE_WEIGHT),
                )
                .is_none()
            {
                demoted_x_de += 1;
            }
            continue;
        }

        if *weight == 100 && is_all_han(value) {
            let n_chars = value.chars().count();
            if (3..=5).contains(&n_chars) && key.split('\'').count() == n_chars {
                if let Some(ref g) = gram {
                    if let Some(cal) = calibrate_unmeasured_phrase(g, value, n_chars) {
                        if cal > 100
                            && decisions
                                .insert((key.clone(), value.clone()), (raw_i32, cal))
                                .is_none()
                        {
                            promoted_unmeasured += 1;
                        }
                    }
                }
            }
        }
    }

    for target in &inspect_words {
        for (key, value, weight, table) in &all_pairs {
            if value == target {
                let cal = decisions
                    .get(&(key.clone(), value.clone()))
                    .map(|(_, c)| *c);
                let exact_ln = gram.as_ref().and_then(|g| g.exact_phrase_ln_count(value));
                eprintln!(
                    "inspect: {table} {key} -> {value}: raw={weight}, calibrated={cal:?}, exact_ln={exact_ln:?}"
                );
            }
        }
    }

    // 在整库所有 `(key, value)` 对上寻找零碰撞 `seed`：保证没有任何未校准行会与任何校准记录撞上同一个 `hash64`。
    let mut chosen_seed = None;
    let mut final_records = Vec::new();
    for seed in 0u32..1024 {
        let mut cal_hashes: HashMap<u64, (&str, &str, i32, i32)> =
            HashMap::with_capacity(decisions.len() * 2);
        let mut collision = false;
        for ((k, v), (raw_w, cal_w)) in &decisions {
            let h = hash_key_value(seed, k, v);
            if cal_hashes
                .insert(h, (k.as_str(), v.as_str(), *raw_w, *cal_w))
                .is_some()
            {
                collision = true;
                break;
            }
        }
        if collision {
            continue;
        }
        for (k, v, _, _) in &all_pairs {
            let h = hash_key_value(seed, k, v);
            if let Some(&(ck, cv, _, _)) = cal_hashes.get(&h) {
                if ck != k.as_str() || cv != v.as_str() {
                    collision = true;
                    break;
                }
            }
        }
        if !collision {
            chosen_seed = Some(seed);
            final_records = cal_hashes
                .into_iter()
                .map(
                    |(hash64, (_, _, raw_weight, calibrated_weight))| CalibrationRecord {
                        hash64,
                        raw_weight,
                        calibrated_weight,
                    },
                )
                .collect();
            break;
        }
    }

    let seed = chosen_seed.ok_or("failed to find collision-free 64-bit seed in 0..1024")?;
    let table = LexiconCalibration::from_records(seed, final_records)?;

    if let Some(parent) = Path::new(&output_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp_path = output_path.with_extension("bin.tmp");
    {
        let writer = BufWriter::new(File::create(&tmp_path)?);
        table.write_to(writer)?;
    }
    std::fs::rename(&tmp_path, &output_path)?;

    println!(
        "lexicon-calibration: seed={} total={} (suppressed_phantoms={}, demoted_x_de={}, promoted_unmeasured={}, gram={}) -> {}",
        seed,
        table.len(),
        suppressed_phantoms,
        demoted_x_de,
        promoted_unmeasured,
        gram.is_some(),
        output_path.display()
    );
    Ok(())
}
