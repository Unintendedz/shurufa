//! `Session` 公共配置与快照类型。

use std::path::PathBuf;

use crate::paths::RuntimePaths;
use crate::punctuation::PunctuationMarks;
use crate::types::{
    CandidateSource, CommandTableEntry, EnglishInputOptions, FrequencyAdjustmentOptions,
    FuzzyPinyinOptions, LocalInputMode, LocalModeOptions, MentionEntry, MixedExpressiveOptions,
    QuickPhraseEntry, SchemeSet, SchemeType, SentenceAssociationOptions, ShuangpinCustomTable,
    ShuangpinProfileKind, WordItem, DEFAULT_FIRST_PAGE_ROWS,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    pub paths: RuntimePaths,
    pub scheme: SchemeType,
    pub enabled_schemes: SchemeSet,
    pub shuangpin_profile: ShuangpinProfileKind,
    pub shuangpin_custom_profile: Option<ShuangpinCustomTable>,
    pub shuangpin_preedit_uses_raw: bool,
    pub autocorrect_types: u32,
    pub chinese_punctuation: bool,
    pub paired_punctuation: bool,
    /// 0 跟随中英状态，1 强制中文标点，2 强制英文标点。
    pub punctuation_lock: i32,
    pub punctuation_marks: PunctuationMarks,
    pub learning: bool,
    /// Shurufa 独立个人学习叠加层：在 `msime_user.db` 记录 30 天衰减事件，不改写主词库静态权重。
    pub decoupled_personal_learning: bool,
    pub learn_english_on_enter: bool,
    pub sentence_alternatives: bool,
    pub first_page_rows: usize,
    pub single_character_only: bool,
    pub caret_edit_suffix: bool,
    /// Tab 笔画与偏旁筛选（默认关）：全拼/双拼组字时按 Tab 进入筛选，`hspnzd`+`x` 按笔顺，其余小写字母按拆字部件。
    pub tab_stroke_filter: bool,
    /// `msime-stroke.db` 文件路径；空路径表示无可用笔画词库。
    pub stroke_dictionary: PathBuf,
    pub fuzzy_pinyin: FuzzyPinyinOptions,
    pub frequency: FrequencyAdjustmentOptions,
    pub local_modes: LocalModeOptions,
    pub mixed_candidates: bool,
    pub english: EnglishInputOptions,
    pub expressive: MixedExpressiveOptions,
    pub personal_context: bool,
    pub next_word_prediction: bool,
    pub sentence_association: SentenceAssociationOptions,
    pub rescoring_context: String,
    pub command_table: Vec<CommandTableEntry>,
    pub mention_entries: Vec<MentionEntry>,
    pub quick_phrase_table: Vec<QuickPhraseEntry>,
    pub quick_phrase_candidates: bool,
    pub quick_phrase_frequency: bool,
}

impl SessionOptions {
    pub fn new(paths: RuntimePaths) -> Self {
        Self {
            paths,
            scheme: SchemeType::Quanpin,
            enabled_schemes: SchemeSet::ALL,
            shuangpin_profile: ShuangpinProfileKind::Xiaohe,
            shuangpin_custom_profile: None,
            shuangpin_preedit_uses_raw: true,
            autocorrect_types: 0,
            chinese_punctuation: true,
            paired_punctuation: true,
            punctuation_lock: 0,
            punctuation_marks: PunctuationMarks::default(),
            learning: true,
            decoupled_personal_learning: false,
            learn_english_on_enter: true,
            sentence_alternatives: false,
            first_page_rows: DEFAULT_FIRST_PAGE_ROWS,
            single_character_only: false,
            caret_edit_suffix: false,
            tab_stroke_filter: false,
            stroke_dictionary: PathBuf::new(),
            fuzzy_pinyin: FuzzyPinyinOptions::default(),
            frequency: FrequencyAdjustmentOptions::default(),
            local_modes: LocalModeOptions::default(),
            mixed_candidates: true,
            english: EnglishInputOptions::default(),
            expressive: MixedExpressiveOptions::default(),
            personal_context: true,
            next_word_prediction: false,
            sentence_association: SentenceAssociationOptions::default(),
            rescoring_context: String::new(),
            command_table: Vec::new(),
            mention_entries: Vec::new(),
            quick_phrase_table: Vec::new(),
            quick_phrase_candidates: false,
            quick_phrase_frequency: true,
        }
    }
}

/// 组字状态的不可变值拷贝快照。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionSnapshot {
    pub prediction: bool,
    pub scheme: SchemeType,
    pub local_mode: LocalInputMode,
    pub spelling_symbols: String,
    pub preedit: String,
    pub raw_segmentation: String,
    pub normalized_segmentation: String,
    pub candidates: Vec<WordItem>,
    pub dedicated_english: bool,
    pub editing_text: String,
    pub caret_position: usize,
    pub shuangpin_profile: String,
    pub candidate_sources: Vec<CandidateSource>,
    pub candidate_annotations: Vec<String>,
    pub candidate_answers_key: Vec<bool>,
    pub conversion: String,
    pub conversion_focus: (usize, usize),
    pub caret_edit_suffix: bool,
    pub caret_edit_prefix: String,
}
