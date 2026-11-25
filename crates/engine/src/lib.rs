//! The input engine: composition state, Pinyin input schemes, dictionary queries, ranking and learning.

pub mod assets;
mod cache;
pub mod diagnostics;
mod dictionary;
mod error;
pub mod format;
pub mod host;
mod ime;
mod lattice;
mod local;
pub mod neural_v2;
pub mod ordering;
mod paths;
mod pinyin;
pub mod pipeline;
mod punctuation;
mod quanpin;
mod session;
mod shared_string;
mod shuangpin;
pub mod slot_allocator;
mod text;
pub mod time;
mod types;
mod user_dictionary;

pub use error::{EngineError, Result};
pub use lattice::octagram;
pub use paths::RuntimePaths;
pub use pipeline::{
    CandidateEntry, CandidateId, CandidateKind, CandidateOrigin, CursorSnapshot, LockedSegment,
    PurePresenter, RecallPool, UnifiedScorer,
};
pub use punctuation::{BracketMarks, PunctuationMarks, QuoteMarks};
pub use session::{Clock, Session, SessionOptions, SessionSnapshot};
pub use types::{
    autocorrect_type, fuzzy_rule, request_autocorrect_mask, CandidateEdge, CandidateSource,
    Command, CommandTranslationQuery, EnglishInputOptions, FrequencyAdjustmentMode,
    FrequencyAdjustmentOptions, FuzzyPinyinOptions, KeyResult, LocalInputMode, LocalModeOptions,
    MixedExpressiveOptions, NineKeyShuangpinKind, NineKeyShuangpinTable, OnlineInputMode,
    OnlineQuery, PersonalDictionaryEntry, PersonalDictionaryKind, PluginCommand,
    PluginCommandQuery, SchemeSet, SchemeType, SentenceAssociationOptions, ShuangpinCustomTable,
    ShuangpinHelpcodeModes, ShuangpinProfileKind, SyllableHelpcode, WordItem, WubiInputOptions,
    WubiProfileKind, DEFAULT_FIRST_PAGE_ROWS,
};
pub use user_dictionary::bundled::{
    dictionary_table_entries, edit_bundled_dictionary_entry, DictionaryTableEntry,
    DictionaryTablePage,
};
pub use user_dictionary::generation::prepare_runtime_paths;
pub use user_dictionary::journal::quarantine_corrupt_journal;
pub use user_dictionary::personal::{
    edit_personal_dictionary, personal_dictionary_entries, validate_personal_dictionary_entry,
    PersonalDictionaryPage,
};
pub use user_dictionary::replay::{replay as replay_user_dictionary, ReplayResult};
pub use user_dictionary::reset::reset_learned_data;
pub use user_dictionary::state::{
    dictionary_state_revision, stage_dictionary_state, stream_dictionary_state,
    DictionaryStateRecord, DEFAULT_MAXIMUM_RECORDS,
};

pub use local::command::{valid_plugin_argument, EXTENDED_ARGUMENT_SYMBOLS};
pub use local::date_time::LocalDateTime;
pub use local::url;

/// Create or migrate an `msime-english.db` to the schema the engine reads. Fixtures without one need it; `prepare_runtime_paths` copies both dictionaries.
pub fn ensure_english_schema(path: &std::path::Path) -> Result<()> {
    dictionary::english::ensure_english_schema(path)
}

/// Write every queued personal-context transition now. Those writes are otherwise delayed about 2 s; call this before reading the journal directly or re-preparing a generation.
pub fn flush_personal_learning() {
    user_dictionary::ngram_store::flush_all()
}

/// Drop every cached journal, personal-context and local-mode connection. Call before deleting or replacing a data directory.
pub fn close_cached_databases() {
    user_dictionary::journal::close_cached_journals()
}
