//! 宿主门面接口：供 `input-runtime` 与 `host-api` 调用的会话、词典、释义与文本工具集。

pub mod dictionary;
pub mod glosses;
pub mod options;
pub mod session;
#[cfg(test)]
mod tests;
pub mod text;

pub use crate::local::catalog::{EmojiCatalogItem, EmojiCatalogSlice, EmojiSymbolGroup};
pub use crate::local::command::{
    builtin_titles as builtin_command_titles, builtin_triggers as builtin_command_triggers,
    usable_command_table,
};
pub use crate::shuangpin::custom::{
    builtin_table as shuangpin_builtin_table,
    check_custom_profile as check_shuangpin_custom_profile,
    validate_custom_profile as validate_shuangpin_custom_profile,
    CustomProfileError as ShuangpinCustomProfileError, CustomTablePart as ShuangpinCustomTablePart,
};
pub use crate::shuangpin::hints::ShuangpinKeyHint;
pub use crate::types::{
    CandidateEdge, CommandTableEntry, CommandTranslationQuery, MentionEntry, QuickPhraseEntry,
    SentenceAssociationOptions, ShuangpinCustomTable,
};
pub use crate::{close_cached_databases, flush_personal_learning};
pub use dictionary::{
    count_learning_habits, dictionary_edit, dictionary_edit_batch, dictionary_edit_bundled,
    dictionary_edit_collection_batch, dictionary_entries, dictionary_entries_for_kind,
    dictionary_export_entries, dictionary_export_entries_for_kind, dictionary_state_revision,
    dictionary_table_entries, dictionary_validate, dictionary_validate_previous, habit_is_storable,
    merge_dictionary_state, merge_learning_habits, replay_user_dictionary, reset_learned_data,
    stage_dictionary_state, stream_dictionary_state, stream_learning_habits, DictionaryApplied,
    DictionaryEdit, DictionaryEntry, DictionaryKind, DictionaryPage, DictionaryStateMerge,
    DictionaryStateRecord, DictionaryTableEntry, DictionaryTablePage, LearningHabit,
    LearningHabitsMerge, PinyinWeightCalibrator, SnapshotReadError,
};
pub use glosses::{
    candidate_glosses, candidate_glosses_with_user, candidate_target_glosses, english_completions,
    english_corrections, english_phonetics, save_candidate_gloss,
};
pub use options::{prepare_options, prepare_options_for, EngineOptions};
pub use session::{
    local_mode_counts_as_typing, Command, EngineResult, EngineSnapshot, OnlineQuerySnapshot,
    Session,
};
pub use text::{
    emoji_catalog_filtered_page, emoji_catalog_groups, emoji_catalog_slice, emoji_symbol_groups,
    hanzi_to_pinyin, hanzi_to_pinyin_batch, normalize_full_pinyin, shuangpin_key_hints,
    shuangpin_zero_initials,
};
