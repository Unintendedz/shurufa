//! 运行时资源文件名契约。所有模块统一经此引用文件名。

pub const CONTRACT_VERSION: i32 = 1;

/// 主词库（拼音各表与快捷短语）。每个代次里有一份工作副本。
pub const MAIN_DICTIONARY: &str = "msime-pinyin.db";
/// 英文词库；每个代次里有一份工作副本。
pub const ENGLISH_DICTIONARY: &str = "msime-english.db";
/// 词网格二元/三元转移表，随代次复制。
pub const BIGRAM_TABLE: &str = "msime-bigram.bin";
pub const TRIGRAM_TABLE: &str = "msime-trigram.bin";
/// Emoji、颜文字与符号只读资源库。
pub const OTHER_DICTIONARY: &str = "msime-others.db";
pub const DICTIONARY_MANIFEST: &str = "msime-dictionary-manifest.json";
/// 统一 `msime-` 前缀之前的文件名，按（现名，旧名）成对列出，供旧代次平滑回退。
pub const LEGACY_NAMES: [(&str, &str); 6] = [
    (MAIN_DICTIONARY, "msime.db"),
    (ENGLISH_DICTIONARY, "english.db"),
    (BIGRAM_TABLE, "bigram.bin"),
    (TRIGRAM_TABLE, "trigram.bin"),
    (OTHER_DICTIONARY, "others.db"),
    (DICTIONARY_MANIFEST, "dictionary-manifest.json"),
];
/// 自定义翻译词表，作为附加资源复制到代次目录。
pub const TRANSLATIONS: &str = "custom_translations.txt";
/// 用户学习日志库。
pub const USER_JOURNAL: &str = "msime_user.db";
/// 释义缓存库名。
pub const GLOSS_CACHE: &str = "gloss_cache.db";
/// 宿主写入的已学习释义库。
pub const LEARNED_GLOSSES: &str = "translation-glosses.db";
/// 暂存代次完成落盘的就绪标记。
pub const GENERATION_READY: &str = ".ready";
