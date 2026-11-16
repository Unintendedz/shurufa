//! `C` 模式（拆字反查）与 Tab 偏旁/部件筛选的数据源：用户按部件的拼音找字，`huohuohuo` 得 焱、`nvzi` 得 好、`shouchi` 得 拆。
//!
//! 数据是 `scripts/build_chaizi_table.py` 从 kfcd/chaizi（CC BY 3.0）、Unihan 和 OpenCC 生成的两张表，编进引擎，不读磁盘：
//!
//! - `chaizi.tsv`：`键<TAB>字串`。键是各部件的去调拼音以 `'` 连接，字串按名次排好；按去掉 `'` 后的键排序。
//! - `chaizi-chars.tsv`：`字<TAB>名次<TAB>读音<TAB>繁体字形`，按码位排序。
//!
//! 第一次查询时为两张表各建一份行首偏移表，之后二分查找，不为每个键分配字符串；首次使用 Tab 部件筛选时额外从这两张表构建一次部首索引（含俗称、后置偏旁与独体部首字）。网页引擎不编入这两张表（它关掉了全部本地模式），查询总是空的。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::shuangpin::query::normalize_input;
use crate::shuangpin::ShuangpinProfile;
use crate::types::{CandidateSource, SchemeType, WordItem};

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
const KEYS: &str = include_str!("chaizi.tsv");
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
const CHARS: &str = include_str!("chaizi-chars.tsv");
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const KEYS: &str = "";
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const CHARS: &str = "";

/// 一次最多列出的行数。一个键最多 23 个字，所以整串精确命中的字总能列全。
pub const RESULT_LIMIT: usize = 36;
/// 前缀补全要求输入已经打完键的前几个部件：只打一个部件时，以它开头的键动辄几十个（`hen` 34 个、`hang` 26 个），补全出来的字没有意义，还会让 `Chen`、`Chang` 这样的英文在空格时上屏一个生僻字。
const PREFIX_COMPLETE_COMPONENTS: usize = 2;
/// 一个键最多几个部件（实测 10），用来给音节边界开定长数组。
const MAX_COMPONENTS: usize = 16;

/// 一张 TSV 的行首偏移，行不含换行符。
struct Lines {
    text: &'static str,
    starts: Vec<u32>,
}

impl Lines {
    fn new(text: &'static str) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 16);
        let mut start = 0;
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                if index > start {
                    starts.push(start as u32);
                }
                start = index + 1;
            }
        }
        if start < text.len() {
            starts.push(start as u32);
        }
        Self { text, starts }
    }

    fn len(&self) -> usize {
        self.starts.len()
    }

    fn line(&self, index: usize) -> &'static str {
        let text = self.text;
        let rest = &text[self.starts[index] as usize..];
        rest.split_once('\n').map_or(rest, |(line, _)| line)
    }

    fn field(&self, index: usize) -> &'static str {
        let line = self.line(index);
        line.split_once('\t').map_or(line, |(first, _)| first)
    }
}

fn keys() -> &'static Lines {
    static KEY_LINES: OnceLock<Lines> = OnceLock::new();
    KEY_LINES.get_or_init(|| Lines::new(KEYS))
}

fn chars() -> &'static Lines {
    static CHAR_LINES: OnceLock<Lines> = OnceLock::new();
    CHAR_LINES.get_or_init(|| Lines::new(CHARS))
}

/// `chaizi-chars.tsv` 的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CharInfo {
    rank: u32,
    reading: &'static str,
    /// OpenCC s2t 对这个字单独转换的结果；和字本身相同时为空。
    traditional: &'static str,
}

fn char_info(character: &str) -> Option<CharInfo> {
    let table = chars();
    let index = binary_search(table.len(), |index| table.field(index).cmp(character))?;
    let mut fields = table.line(index).splitn(4, '\t').skip(1);
    let rank = fields.next()?.parse().ok()?;
    let reading = fields.next().unwrap_or_default();
    let traditional = fields.next().unwrap_or_default();
    Some(CharInfo {
        rank,
        reading,
        traditional,
    })
}

fn binary_search(len: usize, compare: impl Fn(usize) -> Ordering) -> Option<usize> {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        match compare(middle) {
            Ordering::Less => low = middle + 1,
            Ordering::Greater => high = middle,
            Ordering::Equal => return Some(middle),
        }
    }
    None
}

/// 常用独体字与成字部首本身读音，用于补充拆字表没有拆法（或仅拆为纯笔画）的独体字。不泛化表外扩展区生僻字，以免筛选时误命中大量生僻字。
const STANDALONE_RADICALS: &[(char, &[&str])] = &[
    ('人', &["ren"]),
    ('入', &["ru"]),
    ('八', &["ba"]),
    ('刀', &["dao", "li"]),
    ('力', &["li"]),
    ('又', &["you"]),
    ('口', &["kou"]),
    ('土', &["tu"]),
    ('士', &["shi"]),
    ('大', &["da"]),
    ('女', &["nv", "nu"]),
    ('子', &["zi"]),
    ('寸', &["cun"]),
    ('小', &["xiao"]),
    ('尸', &["shi"]),
    ('山', &["shan"]),
    ('工', &["gong"]),
    ('己', &["ji"]),
    ('已', &["yi"]),
    ('巳', &["si"]),
    ('巾', &["jin"]),
    ('干', &["gan"]),
    ('广', &["guang"]),
    ('弓', &["gong"]),
    ('心', &["xin"]),
    ('戈', &["ge"]),
    ('户', &["hu"]),
    ('手', &["shou"]),
    ('支', &["zhi"]),
    ('文', &["wen"]),
    ('斗', &["dou"]),
    ('斤', &["jin"]),
    ('方', &["fang"]),
    ('日', &["ri"]),
    ('曰', &["yue"]),
    ('月', &["yue", "rou"]),
    ('木', &["mu"]),
    ('欠', &["qian"]),
    ('止', &["zhi"]),
    ('歹', &["dai"]),
    ('毋', &["wu"]),
    ('比', &["bi"]),
    ('毛', &["mao"]),
    ('氏', &["shi"]),
    ('气', &["qi"]),
    ('水', &["shui"]),
    ('火', &["huo"]),
    ('爪', &["zhao", "zhua"]),
    ('父', &["fu"]),
    ('片', &["pian"]),
    ('牙', &["ya"]),
    ('牛', &["niu"]),
    ('犬', &["quan", "gou"]),
    ('王', &["wang", "yu"]),
    ('玉', &["yu"]),
    ('瓜', &["gua"]),
    ('瓦', &["wa"]),
    ('甘', &["gan"]),
    ('生', &["sheng"]),
    ('用', &["yong"]),
    ('田', &["tian"]),
    ('白', &["bai"]),
    ('皮', &["pi"]),
    ('皿', &["min"]),
    ('目', &["mu"]),
    ('矛', &["mao"]),
    ('矢', &["shi"]),
    ('石', &["shi"]),
    ('示', &["shi"]),
    ('禾', &["he"]),
    ('穴', &["xue"]),
    ('立', &["li"]),
    ('竹', &["zhu"]),
    ('米', &["mi"]),
    ('糸', &["mi", "si", "jiao"]),
    ('缶', &["fou"]),
    ('羊', &["yang"]),
    ('羽', &["yu"]),
    ('老', &["lao"]),
    ('而', &["er"]),
    ('耒', &["lei"]),
    ('耳', &["er"]),
    ('肉', &["rou", "yue"]),
    ('臣', &["chen"]),
    ('自', &["zi"]),
    ('至', &["zhi"]),
    ('臼', &["jiu"]),
    ('舌', &["she"]),
    ('舟', &["zhou"]),
    ('艮', &["gen"]),
    ('色', &["se"]),
    ('虫', &["chong"]),
    ('血', &["xue"]),
    ('行', &["xing"]),
    ('衣', &["yi"]),
    ('西', &["xi"]),
    ('见', &["jian"]),
    ('角', &["jiao"]),
    ('言', &["yan"]),
    ('谷', &["gu"]),
    ('豆', &["dou"]),
    ('豕', &["shi"]),
    ('贝', &["bei"]),
    ('赤', &["chi"]),
    ('走', &["zou"]),
    ('足', &["zu"]),
    ('身', &["shen"]),
    ('车', &["che"]),
    ('辛', &["xin"]),
    ('辰', &["chen"]),
    ('邑', &["yi"]),
    ('酉', &["you"]),
    ('采', &["cai"]),
    ('里', &["li"]),
    ('金', &["jin"]),
    ('长', &["chang", "zhang"]),
    ('门', &["men"]),
    ('阜', &["fu"]),
    ('隶', &["li"]),
    ('雨', &["yu"]),
    ('青', &["qing"]),
    ('非', &["fei"]),
    ('面', &["mian"]),
    ('革', &["ge"]),
    ('韦', &["wei"]),
    ('韭', &["jiu"]),
    ('音', &["yin"]),
    ('页', &["ye"]),
    ('风', &["feng"]),
    ('飞', &["fei"]),
    ('食', &["shi"]),
    ('首', &["shou"]),
    ('香', &["xiang"]),
    ('马', &["ma"]),
    ('骨', &["gu"]),
    ('高', &["gao"]),
    ('鬼', &["gui"]),
    ('鱼', &["yu"]),
    ('鸟', &["niao"]),
    ('鹿', &["lu"]),
    ('麻', &["ma"]),
    ('黄', &["huang"]),
    ('黍', &["shu"]),
    ('黑', &["hei"]),
    ('鼎', &["ding"]),
    ('鼓', &["gu"]),
    ('鼠', &["shu"]),
    ('鼻', &["bi"]),
    ('齐', &["qi"]),
    ('齿', &["chi"]),
    ('龙', &["long"]),
    ('龟', &["gui"]),
];

/// 在右偏旁或字底位置时，即使与字音相同也明确作为部首筛选的部件音节（如 `到` 的 `dao`、`芯` 的 `xin`、`岛` 的 `shan`）。
const ALWAYS_TRAILING_RADICALS: &[&str] = &[
    "dao", "ye", "niao", "min", "qian", "xin", "jie", "cun", "bei", "kou", "yue", "mu", "tu",
    "shou", "ma", "nv", "ri", "chong", "zhui", "jian", "ge", "jin", "shan", "huo", "shui", "zhu",
];

/// 在右偏旁或字底位置时易与形声字声旁混淆的部件音节（如 `理` 的 `li`、`亿` 的 `yi`）；仅当该读音不等于整字读音时视作后置部首。
const NON_PHONETIC_TRAILING_RADICALS: &[&str] = &[
    "li", "yi", "shi", "yu", "he", "mi", "si", "tian", "che", "zhou", "yang", "er",
];

#[derive(Debug, Default)]
struct CharFilterEntry {
    standalone: &'static [&'static str],
    chains: Vec<ComponentChain>,
}

#[derive(Debug)]
struct ComponentChain {
    /// 每个部件位置展开俗称别名后的去重拼音集合。
    positions: Vec<Box<[&'static str]>>,
    /// 末位置作为右偏旁或字底可单独匹配的拼音集合；非后置部首时为空。
    trailing: Box<[&'static str]>,
}

fn push_unique(list: &mut Vec<&'static str>, item: &'static str) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// 去掉 `chaizi-chars.tsv` 读音字段里的声调符号，`ü` 写为 `v`。
fn toneless_reading(reading: &str) -> Box<str> {
    let mut plain = String::with_capacity(reading.len());
    for character in reading.chars() {
        let mapped = match character {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ē' | 'é' | 'ě' | 'è' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
            'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
            'ḿ' => 'm',
            'ǹ' | 'ń' | 'ň' => 'n',
            other if other.is_ascii_lowercase() => other,
            _ => continue,
        };
        plain.push(mapped);
    }
    plain.into_boxed_str()
}

/// 在 Unihan 与 `build_chaizi_table.py` 基础上，为偏旁筛选补充日常俗称读法（如 单人旁 `dan`、绞丝旁 `jiao`、反文/反犬 `fan`、立刀 `li` 等）。
fn expand_aliases(
    raw: &[&'static str],
    index: usize,
    next_raw: Option<&[&'static str]>,
) -> Box<[&'static str]> {
    let mut expanded = raw.to_vec();
    let has = |target: &str| raw.contains(&target);
    if has("nv") {
        push_unique(&mut expanded, "nu");
    }
    if has("ren") {
        push_unique(&mut expanded, "dan");
    }
    if has("chi") && has("ren") {
        push_unique(&mut expanded, "shuang");
    }
    // `si` 与 `wang` 共现是四字头（罒），不是绞丝旁。
    if has("si") && !has("wang") {
        push_unique(&mut expanded, "jiao");
    }
    if has("quan") && has("gou") {
        push_unique(&mut expanded, "fan");
    }
    if has("pu") && has("wen") {
        push_unique(&mut expanded, "fan");
    }
    if has("dao") {
        push_unique(&mut expanded, "li");
    }
    if has("er") && has("fu") {
        push_unique(&mut expanded, "shuang");
    }
    if has("biao") && has("huo") {
        push_unique(&mut expanded, "si");
    }
    if has("bao") && !has("mian") {
        push_unique(&mut expanded, "tu");
    }
    if has("jie") {
        push_unique(&mut expanded, "dan");
        push_unique(&mut expanded, "er");
    }
    if has("shan") {
        push_unique(&mut expanded, "san");
        push_unique(&mut expanded, "pie");
    }
    if has("yue") {
        push_unique(&mut expanded, "rou");
    }
    if has("wang") {
        push_unique(&mut expanded, "yu");
    }
    if has("kou") && has("wei") {
        push_unique(&mut expanded, "fang");
        push_unique(&mut expanded, "guo");
        push_unique(&mut expanded, "kuang");
    }
    if index == 0 {
        if has("fang") {
            push_unique(&mut expanded, "kuang");
        }
        if has("shang") {
            push_unique(&mut expanded, "xiao");
        }
        if has("shou") && next_raw.is_some_and(|next| next.contains(&"fu")) {
            push_unique(&mut expanded, "chun");
        }
    }
    expanded.into_boxed_slice()
}

/// 抽取两段及以上拆法末位置中可作为右偏旁/字底筛选的音节集合。
fn trailing_radicals(
    last_raw: &[&'static str],
    len: usize,
    char_reading: &str,
) -> Box<[&'static str]> {
    if len < 2 {
        return Box::default();
    }
    let has = |target: &str| last_raw.contains(&target);
    let has_fan_wen = has("pu") && has("wen");
    let has_shuang_er = has("er") && has("fu");
    let has_si_dian = has("biao") && has("huo");
    let has_fan_quan = has("quan") && has("gou");
    let mut allowed = Vec::new();
    for &syllable in last_raw {
        let cooccurs = (has_fan_wen && matches!(syllable, "pu" | "wen"))
            || (has_shuang_er && matches!(syllable, "er" | "fu"))
            || (has_si_dian && matches!(syllable, "biao" | "huo"))
            || (has_fan_quan && matches!(syllable, "quan" | "gou"));
        let non_phonetic = NON_PHONETIC_TRAILING_RADICALS.contains(&syllable)
            && (char_reading.is_empty() || syllable != char_reading);
        if ALWAYS_TRAILING_RADICALS.contains(&syllable) || cooccurs || non_phonetic {
            push_unique(&mut allowed, syllable);
        }
    }
    if allowed.is_empty() {
        return Box::default();
    }
    // 末位本身同时含 `wang`（罒）时不再误给 `si` 补 `jiao`。
    if has("wang") && !allowed.contains(&"wang") {
        let mut expanded = expand_aliases(&allowed, len - 1, None).into_vec();
        if !last_raw.contains(&"si") {
            return expanded.into_boxed_slice();
        }
        expanded.retain(|&item| item != "jiao");
        return expanded.into_boxed_slice();
    }
    expand_aliases(&allowed, len - 1, None)
}

fn build_chains(decompositions: &[Vec<&'static str>], char_reading: &str) -> Vec<ComponentChain> {
    let mut by_len: [Vec<&[&'static str]>; MAX_COMPONENTS + 1] = Default::default();
    for decomposition in decompositions {
        let len = decomposition.len();
        if (1..=MAX_COMPONENTS).contains(&len) {
            by_len[len].push(decomposition.as_slice());
        }
    }
    let mut chains = Vec::new();
    for (len, group) in by_len.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let mut raw_positions: Vec<Vec<&'static str>> = vec![Vec::new(); len];
        for decomposition in group {
            for (index, &syllable) in decomposition.iter().enumerate() {
                push_unique(&mut raw_positions[index], syllable);
            }
        }
        let positions: Vec<Box<[&'static str]>> = (0..len)
            .map(|index| {
                let next = raw_positions.get(index + 1).map(Vec::as_slice);
                expand_aliases(&raw_positions[index], index, next)
            })
            .collect();
        let trailing = trailing_radicals(&raw_positions[len - 1], len, char_reading);
        chains.push(ComponentChain {
            positions,
            trailing,
        });
    }
    chains
}

/// 每个字用于 Tab 偏旁/部件筛选的索引；第一次用到时从 `chaizi.tsv` 与 `chaizi-chars.tsv` 建一次。
fn component_filter_table() -> &'static HashMap<char, CharFilterEntry> {
    static FILTER_TABLE: OnceLock<HashMap<char, CharFilterEntry>> = OnceLock::new();
    FILTER_TABLE.get_or_init(|| {
        let char_table = chars();
        let mut readings: HashMap<char, Box<str>> = HashMap::with_capacity(char_table.len());
        for index in 0..char_table.len() {
            let mut fields = char_table.line(index).splitn(4, '\t');
            let Some(character) = fields.next().and_then(|text| text.chars().next()) else {
                continue;
            };
            let Some(reading) = fields.nth(1).filter(|text| !text.is_empty()) else {
                continue;
            };
            readings.insert(character, toneless_reading(reading));
        }

        let key_table = keys();
        let mut raw_decompositions: HashMap<char, Vec<Vec<&'static str>>> = HashMap::new();
        for index in 0..key_table.len() {
            let Some((key, characters)) = key_table.line(index).split_once('\t') else {
                continue;
            };
            let syllables: Vec<&'static str> =
                key.split('\'').filter(|piece| !piece.is_empty()).collect();
            if syllables.is_empty() {
                continue;
            }
            for character in characters.chars() {
                raw_decompositions
                    .entry(character)
                    .or_default()
                    .push(syllables.clone());
            }
        }

        let mut entries: HashMap<char, CharFilterEntry> =
            HashMap::with_capacity(raw_decompositions.len() + STANDALONE_RADICALS.len());
        for (character, decompositions) in raw_decompositions {
            let char_reading = readings.get(&character).map(Box::as_ref).unwrap_or("");
            entries.insert(
                character,
                CharFilterEntry {
                    standalone: &[],
                    chains: build_chains(&decompositions, char_reading),
                },
            );
        }
        for &(character, standalone) in STANDALONE_RADICALS {
            entries.entry(character).or_default().standalone = standalone;
        }
        entries
    })
}

/// 判断筛选串 `code` 是否能按顺序由 `positions` 起始各部件拼音的前缀或全拼拼出。
fn matches_chain(code: &str, positions: &[Box<[&'static str]>]) -> bool {
    if code.is_empty() {
        return true;
    }
    let Some((first, rest)) = positions.split_first() else {
        return false;
    };
    for &syllable in first.iter() {
        let max_prefix = code.len().min(syllable.len());
        for prefix_len in 1..=max_prefix {
            if syllable.starts_with(&code[..prefix_len]) && matches_chain(&code[prefix_len..], rest)
            {
                return true;
            }
        }
    }
    false
}

/// Tab 部件筛选匹配：支持各部件按顺序首字母/全拼前缀（如 `k`、`kou`、`kz`）、部首俗称（`j`/`jiao` 绞丝、`d`/`dan` 单人、`s`/`shuang` 双人/双耳、`f`/`fan` 反犬/反文、`l`/`li` 立刀等）、右/底后置部首、三部件以上首尾部件组合（如 `岛` `bs`、`baoshan`）以及常用独体部首字。
pub fn matches_component_filter(character: char, code: &str) -> bool {
    if code.is_empty() || !code.is_ascii() {
        return false;
    }
    let Some(entry) = component_filter_table().get(&character) else {
        return false;
    };
    if entry
        .standalone
        .iter()
        .any(|syllable| syllable.starts_with(code))
    {
        return true;
    }
    for chain in &entry.chains {
        if matches_chain(code, &chain.positions) {
            return true;
        }
        if !chain.trailing.is_empty() {
            if chain
                .trailing
                .iter()
                .any(|syllable| syllable.starts_with(code))
            {
                return true;
            }
            if chain.positions.len() >= 3 && code.len() >= 2 {
                for split in 1..code.len() {
                    let (first_code, last_code) = code.split_at(split);
                    if chain.positions[0]
                        .iter()
                        .any(|syllable| syllable.starts_with(first_code))
                        && chain
                            .trailing
                            .iter()
                            .any(|syllable| syllable.starts_with(last_code))
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// 拆字行的注释：这个字的读音（Unihan `kMandarin` 的第一个值，带调）。不在表里或没有读音的字为空。
pub fn reading(word: &str) -> &'static str {
    char_info(word).map_or("", |info| info.reading)
}

/// 去掉 `'` 后的键和查询串比较的结果。表按去掉 `'` 后的键排序，所以这三种结果在表里依次连成三段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Before,
    /// 键以查询串开头（含相等）。
    Extends,
    After,
}

fn relation(key: &str, query: &[u8]) -> Relation {
    let mut letters = key.bytes().filter(|&byte| byte != b'\'');
    for &wanted in query {
        match letters.next() {
            None => return Relation::Before,
            Some(letter) if letter < wanted => return Relation::Before,
            Some(letter) if letter > wanted => return Relation::After,
            Some(_) => {}
        }
    }
    Relation::Extends
}

/// 一个键的音节边界：每个部件读音结束处在去掉 `'` 后的字母串里的位置。
struct Boundaries {
    ends: [usize; MAX_COMPONENTS],
    count: usize,
}

impl Boundaries {
    fn of(key: &str) -> Option<Self> {
        let mut ends = [0; MAX_COMPONENTS];
        let mut count = 0;
        let mut length = 0;
        for syllable in key.split('\'') {
            if count == MAX_COMPONENTS {
                return None;
            }
            length += syllable.len();
            ends[count] = length;
            count += 1;
        }
        Some(Self { ends, count })
    }

    fn ends(&self) -> &[usize] {
        &self.ends[..self.count]
    }
}

/// 查询串：去掉 `'` 的字母，以及用户写下的每个 `'` 落在字母串的什么位置。
struct Query {
    letters: Vec<u8>,
    separators: Vec<usize>,
}

impl Query {
    fn new(code: &str) -> Self {
        let mut letters = Vec::with_capacity(code.len());
        let mut separators = Vec::new();
        for byte in code.bytes() {
            if byte == b'\'' {
                // 打头的 `'` 前面没有部件，不算分隔符：输入时它收不进来，但删掉 `C'huo` 中间的字母会留下它。
                if !letters.is_empty() {
                    separators.push(letters.len());
                }
            } else {
                letters.push(byte);
            }
        }
        Self {
            letters,
            separators,
        }
    }
}

/// 一个命中：精确命中排在前缀补全之前，补全按还差几个部件、再按名次排。
#[derive(Debug, Clone, Copy)]
struct Hit {
    missing: usize,
    rank: u32,
    character: &'static str,
    traditional: &'static str,
    key: &'static str,
}

/// 把 `query` 能查到的字追加到 `hits`。精确命中的 `missing` 为 0。
fn collect(query: &Query, hits: &mut Vec<Hit>) {
    if query.letters.is_empty() {
        return;
    }
    let table = keys();
    let lower = partition_point(table.len(), |index| {
        relation(table.field(index), &query.letters) == Relation::Before
    });
    let upper = partition_point(table.len(), |index| {
        relation(table.field(index), &query.letters) != Relation::After
    });
    let typed = query.letters.len();
    for index in lower..upper {
        let line = table.line(index);
        let Some((key, characters)) = line.split_once('\t') else {
            continue;
        };
        let Some(boundaries) = Boundaries::of(key) else {
            continue;
        };
        let ends = boundaries.ends();
        // 用户写下的 `'` 必须正好落在部件之间，`huo'huo` 不会匹配 `hu'ohuo` 这样的拆法。
        if !query
            .separators
            .iter()
            .all(|separator| ends.contains(separator))
        {
            continue;
        }
        let total = ends.last().copied().unwrap_or_default();
        let missing = if total == typed {
            0
        } else {
            let complete = ends.iter().take_while(|&&end| end <= typed).count();
            if ends.len() < PREFIX_COMPLETE_COMPONENTS || complete < PREFIX_COMPLETE_COMPONENTS {
                continue;
            }
            ends.len() - complete
        };
        for (offset, character) in characters.char_indices() {
            let character = &characters[offset..offset + character.len_utf8()];
            let Some(info) = char_info(character) else {
                continue;
            };
            hits.push(Hit {
                missing,
                rank: info.rank,
                character,
                traditional: info.traditional,
                key,
            });
        }
    }
}

fn partition_point(len: usize, predicate: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        if predicate(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// `C` 之后的输入 `code` 拆出的字：先列整串精确命中的字，再列前缀补全（要求已打完前两个部件），同一级按名次。双拼方案另把输入转成全拼再查一次，结果合并；微软双拼的 `;`（ing）只在这次转换里有意义。`code` 里有大写字母时不查表（`CPU`、`CEO` 照原样上屏）。`traditional_output` 为真时宿主会把行转成繁体，这里按转换后的字形去重，保留先出现的一行，免得一屏出现两个「議」。
pub fn query_chaizi(
    code: &str,
    scheme: SchemeType,
    profile: &ShuangpinProfile,
    traditional_output: bool,
) -> Vec<WordItem> {
    let shuangpin = scheme == SchemeType::Shuangpin;
    if code.is_empty()
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'\'' || (shuangpin && byte == b';'))
    {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let query = Query::new(code);
    collect(&query, &mut hits);
    if shuangpin {
        let quanpin = normalize_input(code, profile);
        if !quanpin.is_empty() && quanpin.as_bytes() != query.letters.as_slice() {
            collect(&Query::new(&quanpin), &mut hits);
        }
    }
    hits.sort_by_key(|hit| (hit.missing, hit.rank));
    let mut rows: Vec<Hit> = Vec::new();
    for hit in hits {
        if rows.len() == RESULT_LIMIT {
            break;
        }
        let shown = |hit: &Hit| {
            if traditional_output && !hit.traditional.is_empty() {
                hit.traditional
            } else {
                hit.character
            }
        };
        let duplicate = rows.iter().any(|row| {
            row.character == hit.character || (traditional_output && shown(row) == shown(&hit))
        });
        if !duplicate {
            if rows.is_empty() {
                rows.reserve_exact(RESULT_LIMIT);
            }
            rows.push(hit);
        }
    }
    let count = rows.len();
    rows.into_iter()
        .enumerate()
        .map(|(index, hit)| {
            WordItem::new(
                hit.key,
                hit.character,
                (count - index) as i64,
                CandidateSource::Generated,
                "",
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuangpin::profile::default_profile;

    fn quanpin(code: &str) -> Vec<String> {
        query_chaizi(code, SchemeType::Quanpin, xiaohe(), false)
            .into_iter()
            .map(|item| item.word.into_string())
            .collect()
    }

    fn xiaohe() -> &'static ShuangpinProfile {
        default_profile()
    }

    #[test]
    fn empty_chaizi_result_does_not_reserve_result_buffer() {
        let profile = xiaohe();
        assert!(!query_chaizi("huohuohuo", SchemeType::Quanpin, profile, false).is_empty());

        let (rows, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            query_chaizi("zzzzzzzz", SchemeType::Quanpin, profile, false)
        });

        assert!(rows.is_empty());
        assert_eq!(
            allocations, 1,
            "无命中拆字查询只应为去分隔符后的输入分配一次：{allocations}"
        );
    }

    /// Tab 偏旁筛选比部件：按顺序首字母与全拼、部首俗称、右/底部首（排除声旁）、首尾跨部件组合与独体部首字。
    #[test]
    fn matches_component_filter_covers_initials_full_pinyin_aliases_trailing_and_standalone() {
        assert!(matches_component_filter('嚏', "k"));
        assert!(matches_component_filter('嚏', "kz"));
        assert!(matches_component_filter('嚏', "kou"));
        assert!(matches_component_filter('嚏', "kouzhi"));
        assert!(!matches_component_filter('嚏', "z"));
        assert!(!matches_component_filter('嚏', "kzz"));
        assert!(!matches_component_filter('嚏', ""));

        // 河 的三点水有 san、shui 两种读法，首字母和全拼前缀都能命中。
        assert!(matches_component_filter('河', "sk"));
        assert!(matches_component_filter('河', "shui"));
        assert!(matches_component_filter('清', "shui"));
        // 僻 拆成 人、辟 时 辟 读 bi 或 pi。
        assert!(matches_component_filter('僻', "rb"));
        assert!(matches_component_filter('僻', "rp"));
        // 部件全拼筛选金字旁与竹字头。
        assert!(matches_component_filter('钱', "jin"));
        assert!(matches_component_filter('笑', "zhu"));

        // 常见部首俗称：绞丝、单人、双人、反犬、反文、四点底、立刀、双耳。
        assert!(matches_component_filter('给', "j"));
        assert!(matches_component_filter('给', "jiao"));
        assert!(matches_component_filter('续', "j"));
        assert!(matches_component_filter('续', "jiao"));
        assert!(matches_component_filter('倒', "d"));
        assert!(matches_component_filter('倒', "dan"));
        assert!(matches_component_filter('他', "d"));
        assert!(matches_component_filter('他', "dan"));
        assert!(matches_component_filter('很', "s"));
        assert!(matches_component_filter('很', "shuang"));
        assert!(matches_component_filter('得', "s"));
        assert!(matches_component_filter('狗', "f"));
        assert!(matches_component_filter('狗', "fan"));
        assert!(matches_component_filter('猫', "f"));
        assert!(matches_component_filter('教', "f"));
        assert!(matches_component_filter('教', "fan"));
        assert!(matches_component_filter('改', "f"));
        assert!(matches_component_filter('热', "s"));
        assert!(matches_component_filter('热', "si"));
        assert!(matches_component_filter('到', "l"));
        assert!(matches_component_filter('到', "li"));
        assert!(matches_component_filter('部', "e"));
        assert!(matches_component_filter('部', "er"));
        assert!(matches_component_filter('部', "shuang"));

        // 右/底偏旁与同音声旁区分：`动` 的力是部首，`理` 的里是声旁。
        assert!(matches_component_filter('利', "l"));
        assert!(matches_component_filter('刻', "li"));
        assert!(matches_component_filter('剑', "l"));
        assert!(matches_component_filter('顶', "y"));
        assert!(matches_component_filter('领', "ye"));
        assert!(matches_component_filter('鸭', "n"));
        assert!(matches_component_filter('鸭', "niao"));
        assert!(matches_component_filter('都', "e"));
        assert!(matches_component_filter('想', "x"));
        assert!(matches_component_filter('思', "xin"));
        assert!(matches_component_filter('歌', "q"));
        assert!(matches_component_filter('歌', "qian"));
        assert!(matches_component_filter('动', "l"));
        assert!(!matches_component_filter('理', "l"));
        assert!(matches_component_filter('袋', "y"));
        assert!(matches_component_filter('装', "y"));
        assert!(!matches_component_filter('亿', "y"));

        // 三部件及以上支持首尾部件跳过中间笔画：`岛`（包+点+包+山）、`照`（日+召+火）。
        assert!(matches_component_filter('岛', "bs"));
        assert!(matches_component_filter('岛', "baoshan"));
        assert!(matches_component_filter('照', "rh"));

        // 常用独体部首字可按自身读音命中，而表外生僻字不泛化。
        assert!(matches_component_filter('口', "k"));
        assert!(matches_component_filter('口', "kou"));
        assert!(matches_component_filter('马', "m"));
        assert!(matches_component_filter('马', "ma"));
        assert!(matches_component_filter('心', "x"));
        assert!(matches_component_filter('心', "xin"));
        assert!(matches_component_filter('牛', "n"));
        assert!(matches_component_filter('牛', "niu"));
        assert!(!matches_component_filter('㠀', "d"));
    }

    #[test]
    fn samples_from_the_spec_come_first() {
        assert_eq!(quanpin("huohuohuo").first().map(String::as_str), Some("焱"));
        assert_eq!(quanpin("nvzi").first().map(String::as_str), Some("好"));
        assert_eq!(quanpin("muzi").first().map(String::as_str), Some("李"));
        assert_eq!(quanpin("shouchi").first().map(String::as_str), Some("拆"));
    }

    #[test]
    fn readings_annotate_the_characters() {
        assert_eq!(reading("焱"), "yàn");
        assert_eq!(reading("好"), "hǎo");
        // 不在表里的字和非汉字没有读音。
        assert_eq!(reading("a"), "");
        assert_eq!(reading(""), "");
    }

    #[test]
    fn separators_must_fall_between_components() {
        assert_eq!(quanpin("huo'huo'huo"), quanpin("huohuohuo"));
        assert_eq!(quanpin("nv'zi").first().map(String::as_str), Some("好"));
        // `'` 切在部件中间时什么也不出。
        assert!(quanpin("hu'ohuohuo").is_empty());
        assert!(quanpin("n'vzi").is_empty());
        // 末尾的 `'` 落在最后一个部件之后，仍是精确命中。
        assert_eq!(
            quanpin("huohuohuo'").first().map(String::as_str),
            Some("焱")
        );
    }

    #[test]
    fn a_leading_separator_is_ignored() {
        // 删掉 `C'huo` 之间的字母会留下打头的 `'`，它前面没有部件，不该让整串查不到。
        assert_eq!(quanpin("'huohuohuo"), quanpin("huohuohuo"));
        assert_eq!(quanpin("'huo'huo'huo"), quanpin("huohuohuo"));
        assert!(quanpin("'").is_empty());
    }

    #[test]
    fn u_umlaut_is_spelled_v_and_nue_lue_both_work() {
        assert!(quanpin("nvzi").contains(&"好".to_owned()));
        // nüe 写作 nve 或 nue 都能查到同一个字（瘧 = 疒 虐）。
        let nve = quanpin("bingnve");
        let nue = quanpin("bingnue");
        assert!(nve.contains(&"瘧".to_owned()), "{nve:?}");
        assert_eq!(nve, nue);
        assert!(quanpin("yannve").contains(&"谑".to_owned()));
    }

    #[test]
    fn radical_aliases_find_the_everyday_names() {
        // 灬 在 Unihan 里读 biāo，日常叫火字底；扌 叫提手旁。
        assert_eq!(quanpin("nenghuo").first().map(String::as_str), Some("熊"));
        assert!(quanpin("rizhaohuo").contains(&"照".to_owned()));
        assert!(quanpin("tichi").contains(&"拆".to_owned()));
        assert!(quanpin("sangong").contains(&"江".to_owned()));
    }

    #[test]
    fn prefix_completion_needs_two_components() {
        // 只打了一个部件：以它开头的键很多，不补全。
        assert!(quanpin("hen").is_empty());
        assert!(quanpin("hang").is_empty());
        assert!(quanpin("pan").is_empty());
        // 打完两个部件后补全第三个部件（焱 = 火 火 火）。
        let rows = quanpin("huohuo");
        assert!(rows.contains(&"焱".to_owned()), "{rows:?}");
        // 精确命中（炎 = 火 火）排在补全之前。
        let exact = rows.iter().position(|word| word == "炎").expect("炎");
        let completion = rows.iter().position(|word| word == "焱").expect("焱");
        assert!(exact < completion, "{rows:?}");
    }

    #[test]
    fn rows_are_ranked_by_level_then_bounded() {
        let rows = query_chaizi("yanyi", SchemeType::Quanpin, xiaohe(), false);
        // 议 是《通用规范汉字表》一级字，排在繁体的 議 之前。
        let words: Vec<_> = rows.iter().map(|item| item.word.as_str()).collect();
        let simplified = words.iter().position(|word| *word == "议").expect("议");
        let traditional = words.iter().position(|word| *word == "議").expect("議");
        assert!(simplified < traditional, "{words:?}");
        assert!(rows.len() <= RESULT_LIMIT);
        // 行的权重递减，来源是 Generated，`pinyin` 记着命中的键。
        assert!(rows.windows(2).all(|pair| pair[0].weight > pair[1].weight));
        assert!(rows
            .iter()
            .all(|item| item.source == CandidateSource::Generated));
        assert_eq!(rows[0].pinyin, "yan'yi");
        // 很短的前缀命中大量的键，结果仍截在上限内。
        assert!(quanpin("kouk").len() <= RESULT_LIMIT);
    }

    #[test]
    fn no_match_and_uppercase_give_nothing() {
        assert!(quanpin("").is_empty());
        assert!(quanpin("xyzzy").is_empty());
        assert!(quanpin("PU").is_empty());
        assert!(quanpin("hUohuohuo").is_empty());
    }

    #[test]
    fn traditional_output_drops_rows_that_convert_alike() {
        let plain = query_chaizi("yanyi", SchemeType::Quanpin, xiaohe(), false);
        let traditional = query_chaizi("yanyi", SchemeType::Quanpin, xiaohe(), true);
        let words = |rows: &[WordItem]| {
            rows.iter()
                .map(|item| item.word.to_string())
                .collect::<Vec<_>>()
        };
        // 议 转成繁体就是 議：繁体输出时只留先出现的 议。
        assert!(words(&plain).contains(&"議".to_owned()));
        assert!(words(&traditional).contains(&"议".to_owned()));
        assert!(!words(&traditional).contains(&"議".to_owned()));
        assert!(traditional.len() < plain.len());
    }

    #[test]
    fn shuangpin_also_queries_the_quanpin_reading() {
        // 小鹤双拼：huo = hu+o → `ho`；焱 = `hohoho`。
        let rows = query_chaizi("hohoho", SchemeType::Shuangpin, xiaohe(), false);
        assert_eq!(rows.first().map(|item| item.word.as_str()), Some("焱"));
        // 全拼方案不做这次转换。
        assert!(query_chaizi("hohoho", SchemeType::Quanpin, xiaohe(), false).is_empty());
        // 微软双拼的 `;` 是 ing：清 = 氵 san（`sj`）+ 青 qing（`q;`）。全拼方案不收 `;`。
        let microsoft =
            crate::shuangpin::profile::profile(crate::types::ShuangpinProfileKind::Microsoft)
                .expect("微软双拼是内置方案");
        let rows = query_chaizi("sjq;", SchemeType::Shuangpin, microsoft, false);
        assert_eq!(rows.first().map(|item| item.word.as_str()), Some("清"));
        assert!(query_chaizi("sjq;", SchemeType::Quanpin, microsoft, false).is_empty());
    }

    #[test]
    fn tables_are_well_formed() {
        let table = keys();
        assert!(table.len() > 10_000);
        for index in 1..table.len() {
            let previous: Vec<u8> = table
                .field(index - 1)
                .bytes()
                .filter(|&b| b != b'\'')
                .collect();
            let current: Vec<u8> = table.field(index).bytes().filter(|&b| b != b'\'').collect();
            assert!(
                previous <= current,
                "{} > {}",
                table.field(index - 1),
                table.field(index)
            );
            assert!(Boundaries::of(table.field(index)).is_some());
        }
        let characters = chars();
        for index in 1..characters.len() {
            assert!(characters.field(index - 1) < characters.field(index));
        }
    }
}
