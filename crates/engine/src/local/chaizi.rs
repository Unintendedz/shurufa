//! Tab 偏旁/部件筛选数据源（自 `chaizi.tsv` 与 `chaizi-chars.tsv` 编译期构建）：
//! 支持按顺序各部件首字母或全拼前缀（`k`、`kou`、`kz`、`kouzhi`）、部首俗称（单人、双人、绞丝、反犬、反文、立刀等）、
//! 右/底后置部首（排除同音声旁）、三部件及以上首尾跨部件组合与常用独体部首字匹配。

use std::collections::HashMap;
use std::sync::OnceLock;

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
const KEYS: &str = include_str!("chaizi.tsv");
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
const CHARS: &str = include_str!("chaizi-chars.tsv");
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const KEYS: &str = "";
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const CHARS: &str = "";

/// 一个键最多拆出这么多部件（实测 10），用于固定桶分组。
const MAX_COMPONENTS: usize = 16;

/// TSV 行首偏移表（行不含换行符），首次构建后零拷贝切行。
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
        let rest = &self.text[self.starts[index] as usize..];
        rest.split_once('\n').map_or(rest, |(line, _)| line)
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

/// 在 Unihan 与拆字表基础上，为偏旁筛选补充日常俗称读法（如单人旁 `dan`、绞丝旁 `jiao`、反文/反犬 `fan`、立刀 `li` 等）。
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

/// Tab 部件筛选匹配：支持各部件按顺序首字母/全拼前缀（如 `k`、`kou`、`kz`）、部首俗称（`j`/`jiao` 绞丝、`d`/`dan` 单人、`s`/`shuang` 双人/双耳、`f`/`fan` 反犬/反文、`l`/`li` 立刀等）、右/底后置部首、三部件以上首尾部件组合以及常用独体部首字。
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

#[cfg(test)]
mod tests {
    use super::matches_component_filter;

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

        assert!(matches_component_filter('河', "sk"));
        assert!(matches_component_filter('河', "shui"));
        assert!(matches_component_filter('清', "shui"));
        assert!(matches_component_filter('僻', "rb"));
        assert!(matches_component_filter('僻', "rp"));
        assert!(matches_component_filter('钱', "jin"));
        assert!(matches_component_filter('笑', "zhu"));

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

        assert!(matches_component_filter('岛', "bs"));
        assert!(matches_component_filter('岛', "baoshan"));
        assert!(matches_component_filter('照', "rh"));

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
}
