//! 合法音节之间的拼写纠错变体与基于美式 QWERTY 物理坐标的通用概率噪声信道（quanpin.md §7.9）。
//! 保留既有 `compute_typos`/`base_cost` 供双拼及兼容单测复用，同时提供统一自然对数标度的物理键盘与前后鼻音信道代价。

use std::collections::HashMap;
use std::sync::OnceLock;

use super::syllables::{intact_piece, intact_pinyin_list, is_intact, normalized_syllable};
use crate::types::autocorrect_type;

/// 按编辑代价从低到高排列；枚举顺序兼作排序键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyllableTypoKind {
    Transposition,
    Neighbor,
    MissingOrExtra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllableTypo {
    /// 用户可能想输入的规范音节。
    pub syllable: String,
    pub kind: SyllableTypoKind,
}

/// 概率纠错信道生成的合法音节变体及其负对数代价（nat，正值表示扣除的惩罚）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbabilisticTypo {
    pub syllable: &'static str,
    pub kind: SyllableTypoKind,
    /// 正值自然对数惩罚（`-log P_channel(typed | syllable)`，标定区间约 `2.4 ~ 3.8 nats`）。
    pub penalty: f64,
}

/// 旧词网格边基础罚分（个人打折前），保留供旧双拼/兼容单测使用（WLH:145-178）。
pub const TRANSPOSITION_COST: f64 = 20.0;
pub const NEIGHBOR_COST: f64 = 20.0;
pub const MISSING_OR_EXTRA_COST: f64 = 24.0;

/// 统一概率噪声信道的自然对数惩罚标定（单位：nat，正值表示 `-log P_channel`）。
pub const CHANNEL_TRANSPOSITION_PENALTY: f64 = 2.40;
/// 前后鼻音漏打尾 `g`（`in->ing`、`en->eng`、`an->ang`、`on->ong`）。
pub const CHANNEL_MISSING_NASAL_G_PENALTY: f64 = 2.40;
/// 前后鼻音多打尾 `g`（`ing->in`、`eng->en`、`ang->an`）。
pub const CHANNEL_EXTRA_NASAL_G_PENALTY: f64 = 3.10;
/// 连击重复键或单键漏击重复字母。
pub const CHANNEL_REPEATED_KEY_PENALTY: f64 = 2.50;
/// 同行相邻键（欧氏距离 `1.0`）的基础邻键误触代价。
pub const CHANNEL_NEIGHBOR_BASE_PENALTY: f64 = 2.65;
/// 邻键物理欧氏距离超出 `1.0` 键距时的二次高斯惩罚斜率。
pub const CHANNEL_NEIGHBOR_DIST_SLOPE: f64 = 2.20;
/// 漏打介母 `u`/`i`（如 `gan->guan`、`dan->dian`、`han->huan`）。
pub const CHANNEL_MISSING_MEDIAL_PENALTY: f64 = 3.20;
/// 通用单键漏击或误插的信道代价。
pub const CHANNEL_MISSING_OR_EXTRA_PENALTY: f64 = 3.60;

const KEYBOARD_ROWS: [&[u8]; 3] = [b"qwertyuiop", b"asdfghjkl", b"zxcvbnm"];
/// 美式 ANSI QWERTY 各排水平交错偏移（单位：标准键距）。
const KEYBOARD_ROW_OFFSETS: [f64; 3] = [0.0, 0.25, 0.75];

fn key_position(key: u8) -> Option<(usize, usize)> {
    KEYBOARD_ROWS.iter().enumerate().find_map(|(row, keys)| {
        keys.iter()
            .position(|&candidate| candidate == key)
            .map(|column| (row, column))
    })
}

/// 返回标准美式 ANSI QWERTY 键帽物理二维坐标 `(x, y)`（单位：键距）。
pub fn qwerty_key_coords(key: u8) -> Option<(f64, f64)> {
    let (row, col) = key_position(key.to_ascii_lowercase())?;
    Some((col as f64 + KEYBOARD_ROW_OFFSETS[row], row as f64))
}

/// 两枚字母键在标准美式 QWERTY 物理键盘上的欧氏距离（单位：键距）。
pub fn qwerty_key_distance(a: u8, b: u8) -> Option<f64> {
    let (x1, y1) = qwerty_key_coords(a)?;
    let (x2, y2) = qwerty_key_coords(b)?;
    let dx = x1 - x2;
    let dy = y1 - y2;
    Some((dx * dx + dy * dy).sqrt())
}

/// 判断是否属于前后鼻音漏打尾 `g`（`an->ang`、`en->eng`、`in->ing`、`on->ong`）。
pub fn is_missing_nasal_g(typed: &str, intended: &str) -> bool {
    intended.len() == typed.len() + 1
        && intended.ends_with('g')
        && intended.starts_with(typed)
        && (typed.ends_with("an")
            || typed.ends_with("en")
            || typed.ends_with("in")
            || typed.ends_with("on"))
}

/// 判断是否属于前后鼻音多打尾 `g`（`ang->an`、`eng->en`、`ing->in`）。
pub fn is_extra_nasal_g(typed: &str, intended: &str) -> bool {
    typed.len() == intended.len() + 1
        && typed.ends_with('g')
        && typed.starts_with(intended)
        && (intended.ends_with("an") || intended.ends_with("en") || intended.ends_with("in"))
}

/// 判断是否属于声母后漏打介母 `u`/`i`（如 `gan->guan`、`shan->shuan`、`dan->dian`）。
pub fn is_missing_medial(typed: &str, intended: &str) -> bool {
    let t = typed.as_bytes();
    let g = intended.as_bytes();
    if g.len() != t.len() + 1 || t.len() < 2 {
        return false;
    }
    let medial_pos = if matches!(g.get(..2), Some(b"zh" | b"ch" | b"sh")) {
        2
    } else {
        1
    };
    if medial_pos >= t.len() || medial_pos + 1 >= g.len() {
        return false;
    }
    matches!(g[medial_pos], b'u' | b'i')
        && g[..medial_pos] == t[..medial_pos]
        && g[medial_pos + 1..] == t[medial_pos..]
}

/// 判断是否属于单键连击重复（`typed` 比 `intended` 多一个紧邻重复字母）。
pub fn is_repeated_key_slip(typed: &str, intended: &str) -> bool {
    let t = typed.as_bytes();
    let g = intended.as_bytes();
    if t.len() != g.len() + 1 {
        return false;
    }
    (0..=g.len()).any(|pos| {
        t[..pos] == g[..pos]
            && t[pos + 1..] == g[pos..]
            && ((pos > 0 && t[pos] == g[pos - 1]) || (pos < g.len() && t[pos] == g[pos]))
    })
}

/// 按物理 QWERTY 距离与汉语拼音常见击键失误类计算统一概率信道惩罚（正值 nat）。
pub fn channel_typo_penalty(typed: &str, intended: &str, kind: SyllableTypoKind) -> f64 {
    match kind {
        SyllableTypoKind::Transposition => CHANNEL_TRANSPOSITION_PENALTY,
        SyllableTypoKind::Neighbor => {
            let t = typed.as_bytes();
            let g = intended.as_bytes();
            if t.len() == g.len() {
                let mut diff = None;
                let mut diff_count = 0usize;
                for i in 0..t.len() {
                    if t[i] != g[i] {
                        diff = Some((t[i], g[i]));
                        diff_count += 1;
                    }
                }
                if diff_count == 1 {
                    if let Some((a, b)) = diff {
                        if let Some(dist) = qwerty_key_distance(a, b) {
                            let excess = (dist - 1.0).max(0.0);
                            return CHANNEL_NEIGHBOR_BASE_PENALTY
                                + CHANNEL_NEIGHBOR_DIST_SLOPE * excess * excess;
                        }
                    }
                }
            }
            CHANNEL_NEIGHBOR_BASE_PENALTY + 0.35
        }
        SyllableTypoKind::MissingOrExtra => {
            if is_missing_nasal_g(typed, intended) {
                CHANNEL_MISSING_NASAL_G_PENALTY
            } else if is_repeated_key_slip(typed, intended) {
                CHANNEL_REPEATED_KEY_PENALTY
            } else if is_extra_nasal_g(typed, intended) {
                CHANNEL_EXTRA_NASAL_G_PENALTY
            } else if is_missing_medial(typed, intended) {
                CHANNEL_MISSING_MEDIAL_PENALTY
            } else {
                CHANNEL_MISSING_OR_EXTRA_PENALTY
            }
        }
    }
}

/// 检查某条纠错变体是否在 `autocorrect_types` 掩码中开启（区分漏键 `DELETION` 与多键 `INSERTION`）。
pub fn channel_typo_enabled(
    typed: &str,
    intended: &str,
    kind: SyllableTypoKind,
    autocorrect_types: u32,
) -> bool {
    match kind {
        SyllableTypoKind::Transposition => autocorrect_types & autocorrect_type::TRANSPOSITION != 0,
        SyllableTypoKind::Neighbor => autocorrect_types & autocorrect_type::NEIGHBOR != 0,
        SyllableTypoKind::MissingOrExtra => {
            if intended.len() > typed.len() {
                autocorrect_types & autocorrect_type::DELETION != 0
            } else if intended.len() < typed.len() {
                autocorrect_types & autocorrect_type::INSERTION != 0
            } else {
                autocorrect_types & autocorrect_type::MISSING_OR_EXTRA != 0
            }
        }
    }
}

/// 统一概率标度下的个人纠错接受折扣：随接受次数平滑收敛，最低保留 `1.0 nat` 防过度纠错。
pub fn channel_discounted(base: f64, accepted: i32) -> f64 {
    if accepted <= 0 {
        return base;
    }
    (base - 0.35 * f64::from(accepted).ln_1p().min(2.5)).max(1.0)
}

/// 返回单个合法音节在给定掩码下代价最低的前 `max_variants` 条概率纠错变体。
pub fn probabilistic_syllable_typos(
    typed: &str,
    autocorrect_types: u32,
    max_variants: usize,
) -> Vec<ProbabilisticTypo> {
    if max_variants == 0 || autocorrect_types == 0 {
        return Vec::new();
    }
    let typos = syllable_typos(typed);
    if typos.is_empty() {
        return Vec::new();
    }
    let mut ranked = Vec::with_capacity(typos.len());
    for typo in typos {
        if !channel_typo_enabled(typed, &typo.syllable, typo.kind, autocorrect_types) {
            continue;
        }
        let Some(syllable) = intact_piece(typo.syllable.as_bytes()) else {
            continue;
        };
        let penalty = channel_typo_penalty(typed, syllable, typo.kind);
        ranked.push(ProbabilisticTypo {
            syllable,
            kind: typo.kind,
            penalty,
        });
    }
    ranked.sort_by(|lhs, rhs| {
        lhs.penalty
            .total_cmp(&rhs.penalty)
            .then_with(|| lhs.syllable.cmp(rhs.syllable))
    });
    ranked.truncate(max_variants);
    ranked
}

/// 两字母非法片段（如 `meiwentj` 尾部的 `tj`、`haodr` 尾部的 `dr`）的 QWERTY 邻键单字音节恢复。
/// 仅用于长度 `>= 4` 的多音节串中的非合法两字母片段，绝不干扰独立两字母简拼（如 `wm`/`xz`/`tj`）。
pub fn two_letter_neighbor_typos(piece: &str, max_variants: usize) -> Vec<ProbabilisticTypo> {
    let bytes = piece.as_bytes();
    if max_variants == 0
        || bytes.len() != 2
        || !bytes[0].is_ascii_lowercase()
        || !bytes[1].is_ascii_lowercase()
        || is_intact(piece)
    {
        return Vec::new();
    }
    let mut ranked: Vec<ProbabilisticTypo> = Vec::new();
    for pos in 0..2 {
        for key in b'a'..=b'z' {
            if key == bytes[pos] || !keys_adjacent(key, bytes[pos]) {
                continue;
            }
            let mut edited = [bytes[0], bytes[1]];
            edited[pos] = key;
            let Some(candidate) = intact_piece(&edited) else {
                continue;
            };
            let normalized = normalized_syllable(candidate);
            let Some(syllable) = intact_piece(normalized.as_bytes()) else {
                continue;
            };
            let penalty = channel_typo_penalty(piece, candidate, SyllableTypoKind::Neighbor);
            if let Some(existing) = ranked.iter_mut().find(|item| item.syllable == syllable) {
                if penalty < existing.penalty {
                    existing.penalty = penalty;
                }
            } else {
                ranked.push(ProbabilisticTypo {
                    syllable,
                    kind: SyllableTypoKind::Neighbor,
                    penalty,
                });
            }
        }
    }
    ranked.sort_by(|lhs, rhs| {
        lhs.penalty
            .total_cmp(&rhs.penalty)
            .then_with(|| lhs.syllable.cmp(rhs.syllable))
    });
    ranked.truncate(max_variants);
    ranked
}

/// Keeps `candidate` when it is a legal syllable of two or more letters that is not the typed one; a variant reached twice keeps its cheaper kind (ST:37-54).
fn add_variant(
    variants: &mut Vec<SyllableTypo>,
    typed: &str,
    candidate: &[u8],
    kind: SyllableTypoKind,
) {
    let Ok(candidate) = std::str::from_utf8(candidate) else {
        return;
    };
    if candidate.len() < 2 || !is_intact(candidate) {
        return;
    }
    let intended = normalized_syllable(candidate);
    if intended == typed {
        return;
    }
    if let Some(existing) = variants
        .iter_mut()
        .find(|variant| variant.syllable == intended)
    {
        existing.kind = existing.kind.min(kind);
        return;
    }
    variants.push(SyllableTypo {
        syllable: intended,
        kind,
    });
}

/// ST:56-99：按原始拼写编辑，以规范拼写比较变体与输入，排除只是别名不同的同一音节。
fn compute_typos(syllable: &str) -> Vec<SyllableTypo> {
    let typed = normalized_syllable(syllable);
    if typed.len() < 2 {
        return Vec::new();
    }
    let letters = syllable.as_bytes();
    let neighbor_capacity = letters
        .iter()
        .map(|&letter| {
            (b'a'..=b'z')
                .filter(|&key| keys_adjacent(key, letter))
                .count()
        })
        .sum::<usize>();
    let capacity = letters.len().saturating_sub(1)
        + neighbor_capacity
        + letters.len()
        + letters.len().saturating_add(1).saturating_mul(26);
    let mut variants = Vec::with_capacity(capacity);
    // 所有编辑从原字节恢复；多出的一个字节容纳插入，循环中不重新申请存储。
    let mut edited = Vec::with_capacity(letters.len() + 1);
    for i in 0..letters.len() - 1 {
        if letters[i] == letters[i + 1] {
            continue;
        }
        edited.clear();
        edited.extend_from_slice(letters);
        edited.swap(i, i + 1);
        add_variant(
            &mut variants,
            &typed,
            &edited,
            SyllableTypoKind::Transposition,
        );
    }
    for i in 0..letters.len() {
        for key in b'a'..=b'z' {
            if key == letters[i] || !keys_adjacent(key, letters[i]) {
                continue;
            }
            edited.clear();
            edited.extend_from_slice(letters);
            edited[i] = key;
            add_variant(&mut variants, &typed, &edited, SyllableTypoKind::Neighbor);
        }
    }
    for i in 0..letters.len() {
        edited.clear();
        edited.extend_from_slice(letters);
        edited.remove(i);
        add_variant(
            &mut variants,
            &typed,
            &edited,
            SyllableTypoKind::MissingOrExtra,
        );
    }
    for i in 0..=letters.len() {
        for key in b'a'..=b'z' {
            edited.clear();
            edited.extend_from_slice(letters);
            edited.insert(i, key);
            add_variant(
                &mut variants,
                &typed,
                &edited,
                SyllableTypoKind::MissingOrExtra,
            );
        }
    }
    variants.sort_by_key(|variant| variant.kind);
    variants
}

/// The precomputed variants of an intact syllable, cheapest kind first; empty for unknown input (ST:56-110).
pub fn syllable_typos(typed: &str) -> &'static [SyllableTypo] {
    static TABLE: OnceLock<HashMap<&'static str, Vec<SyllableTypo>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        intact_pinyin_list()
            .iter()
            .map(|&syllable| (syllable, compute_typos(syllable)))
            .collect()
    });
    table.get(typed).map_or(&[], Vec::as_slice)
}

/// The kind of single edit that turns `typed` into `intended`, if both are intact and one edit apart.
pub fn syllable_typo_kind(typed: &str, intended: &str) -> Option<SyllableTypoKind> {
    syllable_typos(typed)
        .iter()
        .find(|typo| typo.syllable == intended)
        .map(|typo| typo.kind)
}

/// Same row neighbours, or diagonal neighbours on adjacent rows with the half-key shift (ST:145-162).
pub fn keys_adjacent(a: u8, b: u8) -> bool {
    if a == b {
        return false;
    }
    let (Some(first), Some(second)) = (key_position(a), key_position(b)) else {
        return false;
    };
    let ((upper_row, upper_column), (lower_row, lower_column)) = if first.0 <= second.0 {
        (first, second)
    } else {
        (second, first)
    };
    if upper_row == lower_row {
        return upper_column.abs_diff(lower_column) == 1;
    }
    // Each lower row is shifted half a key right, so key j sits below keys j and j + 1 of the row above.
    lower_row == upper_row + 1 && (upper_column == lower_column || upper_column == lower_column + 1)
}

/// The autocorrect type bits a kind belongs to (ST:131-143).
pub fn autocorrect_bit(kind: SyllableTypoKind) -> u32 {
    match kind {
        SyllableTypoKind::Transposition => autocorrect_type::TRANSPOSITION,
        SyllableTypoKind::Neighbor => autocorrect_type::NEIGHBOR,
        SyllableTypoKind::MissingOrExtra => autocorrect_type::MISSING_OR_EXTRA,
    }
}

pub fn base_cost(kind: SyllableTypoKind) -> f64 {
    match kind {
        SyllableTypoKind::Transposition => TRANSPOSITION_COST,
        SyllableTypoKind::Neighbor => NEIGHBOR_COST,
        SyllableTypoKind::MissingOrExtra => MISSING_OR_EXTRA_COST,
    }
}

/// `base - (base / 5) * min(ln(1 + accepted), 3)` for a positive count (TP:173-178).
pub fn discounted(base: f64, accepted: i32) -> f64 {
    if accepted <= 0 {
        return base;
    }
    base - (base / 5.0) * f64::from(accepted).ln_1p().min(3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// test_typo_correction_input_session.cpp:338-347: the discount never rises with more accepted typos and settles on its 8.0 floor.
    #[test]
    fn discount_is_monotonic_down_to_its_floor() {
        let mut previous = discounted(20.0, 0);
        assert_eq!(previous, 20.0);
        for accepted in 1..=2000 {
            let current = discounted(20.0, accepted);
            assert!(current <= previous && current >= 8.0 - 1e-9, "{accepted}");
            previous = current;
        }
        assert!(previous < 8.0 + 1e-9);
    }

    #[test]
    fn keyboard_adjacency_includes_the_staggered_diagonals() {
        assert!(keys_adjacent(b'q', b'w'));
        assert!(keys_adjacent(b'w', b'q'));
        assert!(!keys_adjacent(b'q', b'e'));
        // `a` sits below `q` and `w`.
        assert!(keys_adjacent(b'q', b'a'));
        assert!(keys_adjacent(b'w', b'a'));
        assert!(!keys_adjacent(b'e', b'a'));
        assert!(keys_adjacent(b'a', b'z'));
        assert!(keys_adjacent(b's', b'z'));
        assert!(!keys_adjacent(b'q', b'z'));
        assert!(!keys_adjacent(b'a', b'a'));
        assert!(!keys_adjacent(b'a', b'1'));
    }

    #[test]
    fn qwerty_physical_distance_and_channel_penalties_are_calibrated() {
        let same_row = qwerty_key_distance(b's', b'd').unwrap();
        let diagonal = qwerty_key_distance(b'i', b'j').unwrap();
        assert!((same_row - 1.0).abs() < 1e-9);
        assert!(diagonal > same_row && diagonal < 1.3);

        // 合法音节常见编辑落在 2.4 ~ 3.6 nats 真实概率信道区间内。
        let nasal = channel_typo_penalty("qin", "qing", SyllableTypoKind::MissingOrExtra);
        let neighbor_same = channel_typo_penalty("nai", "nao", SyllableTypoKind::Neighbor);
        let neighbor_diag = channel_typo_penalty("si", "di", SyllableTypoKind::Neighbor);
        let medial = channel_typo_penalty("gan", "guan", SyllableTypoKind::MissingOrExtra);
        assert!((nasal - CHANNEL_MISSING_NASAL_G_PENALTY).abs() < 1e-9);
        assert!((neighbor_same - CHANNEL_NEIGHBOR_BASE_PENALTY).abs() < 1e-9);
        assert!((neighbor_diag - CHANNEL_NEIGHBOR_BASE_PENALTY).abs() < 1e-9);
        assert!((medial - CHANNEL_MISSING_MEDIAL_PENALTY).abs() < 1e-9);
        assert!(nasal < medial && neighbor_same < medial);

        // 两字母非法邻键片段能稳定恢复合法音节。
        let tj = two_letter_neighbor_typos("tj", 3);
        assert!(tj.iter().any(|item| item.syllable == "ti"));
        assert!(two_letter_neighbor_typos("ti", 3).is_empty());

        // 合法音节概率变体排序稳定：前后鼻音漏 `g` 排在一般插入之前。
        let all = autocorrect_type::TRANSPOSITION
            | autocorrect_type::NEIGHBOR
            | autocorrect_type::MISSING_OR_EXTRA;
        let qin = probabilistic_syllable_typos("qin", all, 6);
        assert_eq!(qin.first().map(|item| item.syllable), Some("qing"));
        let pen = probabilistic_syllable_typos("pen", all, 6);
        assert_eq!(pen.first().map(|item| item.syllable), Some("peng"));
        let yin = probabilistic_syllable_typos("yin", all, 6);
        assert_eq!(yin.first().map(|item| item.syllable), Some("ying"));
    }

    #[test]
    fn typos_are_legal_normalised_and_cheapest_first() {
        let guan = syllable_typos("guan");
        assert!(guan.windows(2).all(|pair| pair[0].kind <= pair[1].kind));
        assert!(guan
            .iter()
            .all(|typo| is_intact(&typo.syllable) && typo.syllable != "guan"));
        assert_eq!(
            syllable_typo_kind("gan", "guan"),
            Some(SyllableTypoKind::MissingOrExtra)
        );
        assert_eq!(
            syllable_typo_kind("guan", "gan"),
            Some(SyllableTypoKind::MissingOrExtra)
        );
        assert_eq!(syllable_typo_kind("gua", "gau"), None);
        assert_eq!(syllable_typo_kind("hua", "hau"), None);
        assert_eq!(
            syllable_typo_kind("nei", "nie"),
            Some(SyllableTypoKind::Transposition)
        );
        assert_eq!(
            syllable_typo_kind("bing", "ning"),
            Some(SyllableTypoKind::Neighbor)
        );
        assert_eq!(syllable_typo_kind("ni", "hao"), None);
        // Variants carry the dictionary spelling.
        assert!(syllable_typos("le")
            .iter()
            .any(|typo| typo.syllable == "lve"));
        assert!(!syllable_typos("le")
            .iter()
            .any(|typo| typo.syllable == "lue"));
        // Both spellings are table keys; their edits differ because they start from different letters.
        assert!(!syllable_typos("lue").is_empty() && !syllable_typos("lve").is_empty());
        assert!(syllable_typos("a").is_empty());
        assert!(syllable_typos("xyz").is_empty());
    }

    #[test]
    fn a_variant_reached_twice_is_listed_once() {
        // `lue` and `lve` both normalise to `lve`, so `le` reaches it through two insertions.
        for &syllable in intact_pinyin_list() {
            let typos = syllable_typos(syllable);
            for (index, typo) in typos.iter().enumerate() {
                assert!(
                    typos[index + 1..]
                        .iter()
                        .all(|other| other.syllable != typo.syllable),
                    "{syllable}"
                );
            }
        }
        assert_eq!(
            syllable_typos("le")
                .iter()
                .filter(|typo| typo.syllable == "lve")
                .count(),
            1
        );
    }

    #[test]
    fn costs_and_bits() {
        assert_eq!(
            autocorrect_bit(SyllableTypoKind::Transposition),
            autocorrect_type::TRANSPOSITION
        );
        assert_eq!(
            autocorrect_bit(SyllableTypoKind::Neighbor),
            autocorrect_type::NEIGHBOR
        );
        assert_eq!(
            autocorrect_bit(SyllableTypoKind::MissingOrExtra),
            autocorrect_type::MISSING_OR_EXTRA
        );
        assert_eq!(base_cost(SyllableTypoKind::MissingOrExtra), 24.0);
        assert_eq!(discounted(20.0, 0), 20.0);
        assert_eq!(discounted(20.0, -3), 20.0);
        assert!((discounted(20.0, 1) - (20.0 - 4.0 * 2f64.ln())).abs() < 1e-12);
        // The discount is capped at three fifths of the base.
        assert!((discounted(20.0, 1000) - 8.0).abs() < 1e-12);
    }
}

#[cfg(test)]
#[path = "typos/edit_buffer_tests.rs"]
mod edit_buffer_tests;
