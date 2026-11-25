//! Versioned, thread-confined C interface for native IME hosts.
//! A handle registry rejects stale and wrong-thread handles without dereferencing them.

// The workspace denies unsafe code; this crate is the C ABI native hosts link against, so every handle,
// pointer and string it accepts crosses a boundary the compiler cannot check.
// The exemption is stated here rather than left implicit by opting out of
// the workspace lint table, which would also silently drop every other lint
// the workspace adds later.
#![allow(unsafe_code)]

use msime_client_core::edition::Edition;
use msime_client_core::host_surface::{
    offered_input_schemes, HostCapabilities, HostPlatform, SurfaceRoute,
};
use msime_client_providers::ai::AiSuggestionRequest;
use msime_client_services::dictionary::access::DictionaryAccess;
/// 按用户的设置打一串键看候选，给 `msime-mcp candidates` 用。
pub mod candidate_preview;
pub mod clipboard_history;
pub mod settings_dictionary;
pub mod system_fonts;
use msime_client_core::preferences::{
    InputScheme, Preferences, PreferencesSnapshot, PreferencesStore, ShuangpinCustomProfile,
    ShuangpinProfile,
};
use msime_client_core::punctuation::{
    route as punctuation_route, PunctuationContext, PunctuationRoute,
};
use msime_client_core::resources::{ResourceSet, ResourceStore, VerifiedMarker};
use msime_client_providers::resource_packs::{self, ResourcePack};
use msime_client_providers::voice::doubao_frame::{
    audio_frame, decode_error_code, decode_json_frame, start_frame,
};
use msime_client_providers::voice::VoiceSessionState;
use msime_client_services::typing_statistics::{
    CommitEfficiency, TypingSource, TypingStatisticsStore, RANKS,
};
use msime_engine::host::{
    check_shuangpin_custom_profile, shuangpin_builtin_table, validate_shuangpin_custom_profile,
    CandidateEdge, Command, EngineOptions, Session, ShuangpinCustomProfileError,
    ShuangpinCustomTable, ShuangpinCustomTablePart,
};
use msime_engine::{SchemeSet, SchemeType, ShuangpinProfileKind};
use msime_input_runtime::EmojiPanelQuery;
use msime_input_runtime::{
    Action, AiAssistantProviderConfig, CandidateId, CharacterWidth, OnlineCandidate, OnlineQuery,
    Runtime, Transition, TranslationService,
};
#[cfg(unix)]
use msime_input_runtime::{
    NeuralServiceClient, TranslationQuery, UnixSocketProvider, NEURAL_PREFILL_DEBOUNCE,
    NEURAL_PROVISIONAL_WINDOW,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
// Only the Unix socket streaming entry point takes raw callback context.
#[cfg(unix)]
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
mod bounded_file;
mod chinese_english_spacing;
mod dictionary;
// The exports live in `ffi`, but Rust consumers - this crate's own tests and
// examples, and anything linking the rlib - have always reached them at the
// crate root. Re-exported so moving the file changes no caller.
/// Largest preference document a host may hand to the C ABI, in bytes.
///
/// A valid document carries the custom touch-keyboard skin, whose photo alone may be 682,668 base64 characters; at the 16 KiB every other buffer uses, a document holding a photo could never be written back, which failed every later preference write from that host.
pub(crate) const PREFERENCES_DOCUMENT_LIMIT: usize = 1 << 20;
/// Largest serialized HostOptions document accepted by native entry points.
/// Preferences may carry a base64 custom keyboard photo, so the old 16 KiB
/// ABI limit rejected valid saved preferences before a session could start.
pub(crate) const HOST_OPTIONS_DOCUMENT_LIMIT: usize = PREFERENCES_DOCUMENT_LIMIT;
/// Largest dictionary-management request: one HostOptions document plus the
/// bounded personal-dictionary import payload and a small amount of framing.
pub(crate) const DICTIONARY_REQUEST_LIMIT: usize = HOST_OPTIONS_DOCUMENT_LIMIT + 1_200_000;

mod ffi;
pub use ffi::*;
pub mod local_session;

/// 会话实际用的每页候选数：偏好或宿主覆盖的值，按本库编译到的平台能排的上限截断（`HostPlatform::max_candidate_page_size`）。共享文档在别的设备上存了 10 时，只排得下九个的宿主仍拿到 9。
fn host_page_size(requested: u8) -> u8 {
    requested.min(ffi::host::compiled_platform().max_candidate_page_size())
}

/// 会话的候选窗是否竖排：桌面宿主按 `candidate_layout` 画候选窗，触屏宿主的候选栏总是横排，不读这一项（它的缺省是竖排，照读会让外接键盘的左右键不再选词）。
fn host_vertical_candidates(layout: msime_client_core::preferences::CandidateLayout) -> bool {
    ffi::host::compiled_platform().is_desktop()
        && layout == msime_client_core::preferences::CandidateLayout::Vertical
}

/// 会话实际用的中英混输触发字符数：本库编译到的平台不读这一项时（macOS、Windows 和 Linux 三个桌面宿主，`HostPlatform::reads_mixed_input_minimum_prefix`）固定为默认值，文档里存的数不起作用，设置页也不再给出这一行。
fn host_english_minimum_prefix(stored: u8) -> u8 {
    english_minimum_prefix_for(ffi::host::compiled_platform(), stored)
}

fn english_minimum_prefix_for(
    platform: msime_client_core::host_surface::HostPlatform,
    stored: u8,
) -> u8 {
    if platform.reads_mixed_input_minimum_prefix() {
        stored
    } else {
        msime_client_core::preferences::DEFAULT_MIXED_INPUT_MINIMUM_PREFIX
    }
}

/// 会话实际用的 AI 联想候选数量：本库编译到的平台固定了这个数（`HostPlatform::fixed_ai_candidate_limit`，macOS、Windows、Linux、鸿蒙）时用固定值，不读偏好里存的值；否则用偏好。座位配置、在线查询和请求描述都从这里取，`chat_completion_http_request` 要求请求和配置里的数量一致。
fn host_ai_candidate_limit(stored: u8) -> u8 {
    ffi::host::compiled_platform()
        .fixed_ai_candidate_limit()
        .unwrap_or(stored)
}
mod doubao_auth;
pub use doubao_auth::msime_client_doubao_auth_headers;
mod learned_translation;
mod niutrans_translation;
mod pronunciation;
mod supplementary_glosses;
mod tencent_translation;
mod word_breakdown;
pub use dictionary::{
    dictionary_request_json, dictionary_words, edit_dictionary_word, edit_user_quick_phrase,
    import_dictionary_words, lookup_candidates, msime_client_dictionary,
    msime_client_dictionary_import_entries, msime_client_dictionary_scel_text,
    msime_client_dictionary_validate, msime_client_personal_dictionary_request,
    msime_client_personal_dictionary_sync, personal_dictionary_request_json, scel_text_json,
    user_quick_phrases, CandidateOrigin, DictionaryOptions, LookupCandidate, LookupScheme, NewWord,
    QuickPhrase, QuickPhraseEdit, QuickPhrasePage, Word, WordEdit, WordImport, WordKind, WordPage,
};
mod diagnostics;
pub use diagnostics::{msime_client_set_diagnostic_sink, DiagnosticSink};
mod dictionary_updates;
mod key_sound;
// 生命周期核心；`pub` 是为了让集成测试（零分配断言）直接调用纯推导函数。
pub mod lifecycle;
mod plugin_tables;
mod voice_capture;

/// Capture endpoint identities paired with labels. Neither belongs in logs.
pub fn voice_capture_devices() -> Vec<(String, String)> {
    voice_capture::capture_devices()
}

/// A device listing as the shared settings page's 「可用录音设备」 reads it: `[{backend, id, label}]` in listing order, `backend` the value `voice_input.capture_backend` takes when the user picks the device and `id` the one `voice_input.capture_device` takes. The Tauri settings shell (Windows and macOS) and `msime_client_voice_capture_devices` both shape their listing here.
pub fn voice_capture_device_values(backend: &str, devices: Vec<(String, String)>) -> Vec<Value> {
    devices
        .into_iter()
        .map(|(id, label)| json!({"backend": backend, "id": id, "label": label}))
        .collect()
}

/// Capture bounded mono 16 kHz PCM for a platform host.
pub fn voice_capture_pcm(milliseconds: u32) -> Result<Vec<f32>, &'static str> {
    if !(1..=60_000).contains(&milliseconds) {
        return Err("invalid voice capture duration");
    }
    let samples = voice_capture::capture_audio(milliseconds);
    if samples.is_empty() {
        return Err("voice capture unavailable");
    }
    Ok(samples)
}

thread_local! {
    static SESSIONS: RefCell<HashMap<u64, HostSession>> = RefCell::new(HashMap::new());
}

struct HostSession {
    runtime: Runtime,
    options: EngineOptions,
    /// HostOptions 里宿主声明的「快捷短语混进普通候选」能力（macOS 请求）：偏好重建 `options` 时和 `mixed_input.quick_phrase` 相与。
    quick_phrase_candidates_requested: bool,
    /// HostOptions 记录的版本：偏好里的方案只在本版本提供的方案里取，回退到本版本的默认方案。
    edition: &'static Edition,
    applied: Preferences,
    requested: Option<PreferencesSnapshot>,
    /// Whether the requested preferences still need an Engine replacement. This decision is made
    /// when the document arrives so every keystroke does not compare the full preference tree.
    preferences_pending: bool,
    punctuation_override: Option<bool>,
    paired_punctuation_override: Option<bool>,
    punctuation_lock_override: Option<u8>,
    english_mode: bool,
    /// 宿主经 `msime_client_set_caps_lock` 报告的大写锁定状态。不报告的宿主一直是 `false`，`caps_lock_ascii_punctuation` 对它不起作用。
    caps_lock: bool,
    page_size_override: Option<u8>,
    statistics_private: bool,
    preview: bool,
    /// An AI provider credential the host keeps outside the preferences, handed over for this session only and never written back.
    ai_credential: Option<SessionAiCredential>,
    /// Cached copy used by every online query until preferences change.
    ai_provider_cache: Option<AiAssistantProviderConfig>,
    voice: VoiceSessionState,
    /// Committing candidate selections counted but not yet written to typing statistics, indexed by one-based position minus one, with every position past a page in the last slot. See `SELECTION_BATCH`.
    pending_selections: [u64; RANKS + 1],
    /// 还没写进打字统计的上屏，和 `pending_selections` 一起写。只在 [`COUNTS_COMMIT_EFFICIENCY`] 时计；读音在写入时于后台线程查，不在输入线程上查。
    pending_efficiency: Vec<EfficiencyCandidate>,
    /// 打字统计的开关，第一次要计效率时从统计文件读一次，每次写入后作废重读；`None` 是还没读。统计关闭时效率一项都不查。
    statistics_enabled: Option<bool>,
    /// Where this session's plugin packs, command tables and name list are read from.
    plugin_roots: key_sound::PluginRoots,
    /// The key sound, melody, commit, achievement and music settings of the newest preferences, which take effect at once rather than waiting for the composition to end: none of them is Engine state.
    sound: key_sound::SessionSound,
    /// The command-table and name-list files `options` was filled from.
    plugin_tables: plugin_tables::PluginTables,
    /// 按需下载资源包所在的状态目录（HostOptions 的 `preferences_directory`），不是绝对路径时为空。
    state_root: Option<PathBuf>,
    /// 会话打开后有资源包新装好（或被移除），`options` 里的路径已经更新，Engine 还要在输入空闲时重建。
    resources_pending: bool,
    /// 生命周期核心的状态，只由 `msime_client_set_lifecycle` 设上。为 `None` 时不推导任何事件，transition JSON 逐字节不变。
    lifecycle: Option<Box<lifecycle::LifecycleState>>,
    #[cfg(unix)]
    neural_client: Option<Arc<NeuralServiceClient>>,
    #[cfg(unix)]
    last_neural_prefilled_context: Option<String>,
    pub(crate) view_buffer: Box<ffi::snapshot::ViewBuffer>,
    // Declared after runtime so the Engine is dropped before releasing access.
    _dictionary_access: DictionaryAccess,
}

#[derive(PartialEq, Eq)]
struct SessionAiCredential {
    endpoint: String,
    token: String,
}

/// 「移动光标后编辑光标后的部分」只在三个桌面宿主上交给 Engine：偏好随同步到达手机时，触屏宿主没有设置入口、也不画模式提示，所以那里一律按关。
const CARET_EDIT_SUFFIX_HOST: bool = cfg!(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
));

fn caret_edit_suffix_applies(preferences: &Preferences) -> bool {
    CARET_EDIT_SUFFIX_HOST && preferences.caret_edit_suffix
}

/// 只约束运行时副本，旧配置保留在偏好存储中，不能恢复独立版已移除的能力。
fn apply_shurufa_input_policy(preferences: &mut Preferences, edition: &Edition) {
    if edition.id != "shurufa" {
        return;
    }
    preferences.paired_punctuation = false;
    preferences.smart_punctuation = false;
    preferences.smart_punctuation_direct_digit = false;
    preferences.smart_punctuation_direct_letter = false;
    preferences.smart_punctuation_repeat = false;
    preferences.smart_punctuation_space_convert = false;
    preferences.english_online_candidates = false;
    preferences.enter_learns_english_word = false;
    preferences.multilingual_input.enabled = false;
    // Shurufa 放开英文单词自动补全（首页保留 3 席）与经 SlotAllocator 投影的精选内联 Emoji（尊重用户开关，默认开）；
    // 引擎旧 queries.rs 路径的 mixed_emoji 固定关闭，其余混输分成员全部关闭。
    preferences.mixed_input.enabled = true;
    preferences.mixed_input.english = true;
    preferences.mixed_input.first_page_count = 3;
    preferences.mixed_input.kaomoji = false;
    preferences.mixed_input.symbols = false;
    preferences.mixed_input.japanese = false;
    preferences.mixed_input.quick_phrase = false;
    preferences.local_modes = msime_client_core::preferences::LocalModePreferences {
        unicode: false,
        date_time: false,
        date_time_inline: false,
        date_time_menu: false,
        quick_phrase: false,
        emoji: false,
        kaomoji: false,
        super_jianpin: false,
        temporary_english: false,
        temporary_japanese: false,
        // 计算与数字固定开着，只从全拼的小写 `v` 进入（`local_expression_lowercase_entry`）；Shift+V 随总开关关掉。
        expression: true,
        command: false,
        mention: false,
        mention_places: false,
        chaizi: Some(false),
        symbol: false,
        shift_letter_entry: false,
    };
    preferences.word_character.enabled = false;
    preferences.navigation.brackets = true;
    preferences.navigation.minus_equal = true;
    preferences.navigation.comma_period = false;
    preferences.navigation.arrows = true;
    preferences.navigation.left_right = true;
    preferences.navigation.mouse_wheel = true;
}

fn apply_local_mode_resource_gates(options: &mut EngineOptions, _edition: &Edition) {
    let resources = std::path::Path::new(&options.resources);
    let has_emoji_catalog = resources.join("msime-others.db").is_file();
    let has_english_dictionary = resources.join("msime-english.db").is_file();
    options.local_emoji &= has_emoji_catalog;
    options.local_kaomoji &= has_emoji_catalog;
    options.local_temporary_english &= has_english_dictionary;
    options.sentence_association.neural_keyboard = false;
}

fn punctuation_lock_code(lock: msime_client_core::preferences::PunctuationLock) -> u8 {
    match lock {
        msime_client_core::preferences::PunctuationLock::Follow => 0,
        msime_client_core::preferences::PunctuationLock::Chinese => 1,
        msime_client_core::preferences::PunctuationLock::English => 2,
    }
}

/// The Engine's sentence association switches. The desktop model switch is not among them: it gates the runtime's settled reranker (`Runtime::set_settled_rerank_enabled`), the only place that model runs.
fn engine_sentence_association(
    preferences: &msime_client_core::preferences::SentenceAssociationPreferences,
    state_root: Option<&std::path::Path>,
) -> msime_engine::host::SentenceAssociationOptions {
    // 选中的语法模型装好后才有文件路径；选中但没装（或包装在别处的 state_root
    // 下没发布）时留空，引擎逐字节退回无模型基线。
    let pack = match preferences.grammar_model {
        msime_client_core::preferences::GrammarModel::None => None,
        msime_client_core::preferences::GrammarModel::Wanxiang => {
            Some(ResourcePack::GrammarWanxiang)
        }
        msime_client_core::preferences::GrammarModel::BaguwenWord => {
            Some(ResourcePack::GrammarBaguwen)
        }
        msime_client_core::preferences::GrammarModel::BaguwenCompact => {
            Some(ResourcePack::GrammarBaguwenCompact)
        }
        msime_client_core::preferences::GrammarModel::Baishuang => {
            Some(ResourcePack::GrammarBaishuang)
        }
    };
    let collocation_model = state_root
        .zip(pack)
        .and_then(|(root, pack)| {
            pack.set()
                .artifacts
                .first()
                .and_then(|artifact| resource_packs::installed_file(root, pack, &artifact.name))
        })
        .unwrap_or_default();
    msime_engine::host::SentenceAssociationOptions {
        word_lattice: preferences.word_lattice,
        neural_keyboard: preferences.neural_keyboard,
        show_next_on_duplicate: preferences.show_next_on_duplicate,
        collocation_model,
        collocation_rerank: pack.is_some(),
        // 搭配调参系数逐字段透传；偏好层已把非法值回退到出厂值。
        collocation_weight: preferences.sentence_collocation_weight,
        collocation_grammar: msime_engine::octagram::GramConfig {
            collocation_max_length: preferences.sentence_collocation_max_length,
            collocation_min_length: preferences.sentence_collocation_min_length,
            collocation_penalty: preferences.sentence_collocation_penalty,
            non_collocation_penalty: preferences.sentence_collocation_non_penalty,
            weak_collocation_penalty: preferences.sentence_collocation_weak_penalty,
            rear_penalty: preferences.sentence_collocation_rear_penalty,
        },
    }
}

/// 资源包卸载前摘掉引擎进程内的 `.gram` 映射，避免 Windows 删除目录时仍被文件句柄占用。
pub fn evict_grammar_model(path: &std::path::Path) {
    msime_engine::octagram::GramDb::evict_shared(path);
}

/// 引擎的引号与方头括号样式，由 client-core 从偏好解析（`corner_when_traditional` 在那里按繁体开关定下）。View 报告的是同一个解析结果。
fn apply_punctuation_marks(
    options: &mut EngineOptions,
    preferences: &msime_client_core::preferences::Preferences,
) {
    let marks = msime_client_core::punctuation::punctuation_marks(preferences);
    options.corner_quotes = marks.quotes == msime_client_core::punctuation::QuoteMarks::Corner;
    options.hollow_brackets =
        marks.brackets == msime_client_core::preferences::BracketStyle::Hollow;
    options.enumeration_comma_backslash = marks.enumeration_comma_backslash;
    options.enumeration_comma_slash = marks.enumeration_comma_slash;
}

/// The switch the Engine is handed. Windows turns the punctuation switch on whenever punctuation is locked to Chinese, while the Engine drops Chinese punctuation whenever its switch is off, so a lock to Chinese has to carry the switch with it.
fn engine_chinese_punctuation(enabled: bool, lock: u8) -> bool {
    enabled || lock == 1
}

impl HostSession {
    fn is_private_or_secure(&self) -> bool {
        self.statistics_private
            || self.lifecycle.as_deref().is_some_and(|state| {
                state.context().traits & (lifecycle::TRAIT_SECURE | lifecycle::TRAIT_PRIVATE) != 0
            })
    }

    #[cfg(unix)]
    fn neural_v2_active(&self) -> bool {
        self.edition.id == "shurufa"
            && self.runtime.neural_v2_enabled()
            && self.runtime.is_focused()
            && !self.english_mode
            && !self.is_private_or_secure()
            && self
                .neural_client
                .as_ref()
                .is_some_and(|c| c.is_available())
    }

    #[cfg(not(unix))]
    fn neural_v2_active(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn schedule_neural_prefill_if_idle(&mut self) {
        if !self.neural_v2_active() || !self.runtime.is_idle() {
            return;
        }
        let ctx = self.runtime.neural_prefill_context().to_owned();
        if self.last_neural_prefilled_context.as_deref() == Some(ctx.as_str()) {
            return;
        }
        if let Some(client) = self.neural_client.as_ref() {
            self.last_neural_prefilled_context = Some(ctx.clone());
            client.schedule_prefill(ctx, NEURAL_PREFILL_DEBOUNCE);
        }
    }

    #[cfg(unix)]
    fn prefill_neural_now_if_changed(&mut self) {
        if !self.neural_v2_active() {
            return;
        }
        let ctx = self.runtime.neural_prefill_context().to_owned();
        if self.last_neural_prefilled_context.as_deref() == Some(ctx.as_str()) {
            return;
        }
        if let Some(client) = self.neural_client.as_ref() {
            self.last_neural_prefilled_context = Some(ctx.clone());
            client.prefill_context(&ctx);
        }
    }

    #[cfg(not(unix))]
    fn prefill_neural_now_if_changed(&mut self) {}

    /// 判断一个标点键去向所用的上下文。`msime_client_punctuation_with_context` 和大写锁定改道（[`Self::caps_lock_punctuation`]）用的是同一份。
    fn punctuation_context(&self, character: u8, preceding: Option<char>) -> PunctuationContext {
        let lock = match self.punctuation_lock_override {
            Some(1) => msime_client_core::preferences::PunctuationLock::Chinese,
            Some(2) => msime_client_core::preferences::PunctuationLock::English,
            Some(_) => msime_client_core::preferences::PunctuationLock::Follow,
            None => self.applied.punctuation_lock,
        };
        PunctuationContext {
            character,
            preceding,
            host_context_available: self
                .runtime
                .punctuation_host_context_available(self.english_mode),
            has_composition: !self.runtime.is_idle(),
            chinese_punctuation: self
                .punctuation_override
                .unwrap_or(self.applied.chinese_punctuation),
            smart_punctuation: self.applied.smart_punctuation,
            direct_digit: self.applied.smart_punctuation_direct_digit,
            direct_letter: self.applied.smart_punctuation_direct_letter,
            lock,
            caps_lock_ascii: self.caps_lock && self.applied.caps_lock_ascii_punctuation,
        }
    }

    /// 大写锁定时，把经字符或标点入口送来的标点键改走字面 ASCII 路线（#6370）。宿主大多把标点键当普通字符交给 `msime_client_character`，这条路线上没有标点判断，所以在分发前统一改道，每个报告了大写锁定的宿主都不用再写一份。只看大写锁定这一条：智能标点要读前文，不在这里判断。不改道时原样返回。
    fn caps_lock_punctuation(&self, action: Action) -> Action {
        if !(self.caps_lock && self.applied.caps_lock_ascii_punctuation) {
            return action;
        }
        let ascii = match action {
            Action::Character { value, .. } | Action::Punctuation(value)
                if value.is_ascii_punctuation() =>
            {
                value
            }
            _ => return action,
        };
        let mut context = self.punctuation_context(ascii, None);
        context.smart_punctuation = false;
        match punctuation_route(context) {
            PunctuationRoute::Ascii => Action::PunctuationAscii(ascii),
            PunctuationRoute::Engine => action,
        }
    }

    /// `engine_chinese_punctuation` for the live overrides over the applied preferences.
    fn live_engine_chinese_punctuation(&self) -> bool {
        engine_chinese_punctuation(
            self.punctuation_override
                .unwrap_or(self.applied.chinese_punctuation),
            self.punctuation_lock_override
                .unwrap_or_else(|| punctuation_lock_code(self.applied.punctuation_lock)),
        )
    }

    fn ai_provider_config(&self) -> Option<&AiAssistantProviderConfig> {
        self.ai_provider_cache.as_ref()
    }
    fn ai_query_is_current(&self, query: &OnlineQuery) -> bool {
        self.english_query_allowed(query)
            && self
                .ai_provider_config()
                .is_some_and(|config| query.ai_assistant.as_ref() == Some(config))
    }
    fn english_query_allowed(&self, query: &OnlineQuery) -> bool {
        if query.mode != msime_engine::OnlineInputMode::English {
            return true;
        }
        let enabled = self
            .requested
            .as_ref()
            .map_or(self.applied.english_online_candidates, |snapshot| {
                snapshot.preferences.english_online_candidates
            });
        enabled
            && self.runtime.online_query().is_ok_and(|live| {
                live.is_some_and(|live| {
                    live.mode == query.mode
                        && live.scheme == query.scheme
                        && live.session_id == query.session_id
                        && live.generation == query.generation
                        && live.identity == query.identity
                        && live.query_text == query.query_text
                        && live.cache_key == query.cache_key
                        && live.pinyin_segments == query.pinyin_segments
                        && live.ai_context == query.ai_context
                        && live.cloud_eligible == query.cloud_eligible
                        && live.ai_eligible == query.ai_eligible
                })
            })
    }
    fn set_ai_provider_cache(&mut self, preferences: &Preferences) {
        let ai = &preferences.ai_assistant;
        self.ai_provider_cache = ai.enabled.then(|| AiAssistantProviderConfig {
            enabled: true,
            provider: ai.provider.clone(),
            model: ai.model.clone(),
            endpoint: ai.endpoint.clone(),
            candidate_limit: host_ai_candidate_limit(ai.candidate_limit),
            thinking: ai.thinking.clone(),
            prompt_id: ai.prompt_id.clone(),
            prompt_custom_1: ai.prompt_custom_1.clone(),
            prompt_custom_2: ai.prompt_custom_2.clone(),
            prompt_custom_3: ai.prompt_custom_3.clone(),
        });
    }
    /// 以最新请求的开关与来源配置判断回包；组字结束才应用的旧偏好不能让已关闭或已切换的服务继续回答。
    fn cloud_query_is_current(&self, query: &OnlineQuery) -> bool {
        let preferences = self
            .requested
            .as_ref()
            .map_or(&self.applied, |snapshot| &snapshot.preferences);
        self.english_query_allowed(query)
            && self.cloud_candidates_enabled()
            && query.cloud_request.as_ref().map_or_else(
                || {
                    query.mode != msime_engine::OnlineInputMode::English
                        && preferences.cloud_candidate_provider == Default::default()
                },
                |request| request.provider_id == preferences.cloud_candidate_provider.identity(),
            )
    }

    fn cloud_candidates_enabled(&self) -> bool {
        self.requested
            .as_ref()
            .map_or(self.applied.cloud_candidates, |snapshot| {
                snapshot.preferences.cloud_candidates
            })
    }
    /// Android、鸿蒙与 iOS 会话是否不记统计：只看宿主经 `msime_client_set_private_session` 标出的隐私会话。用户在设置里关掉学习不算，统计开着就照常计数。只在 [`PRIVATE_SESSIONS_SKIP_STATISTICS`] 时成立，其他宿主照旧计数。只看候选的会话（`preview`）在任何平台上都不计。
    fn private_session(&self) -> bool {
        self.preview || (PRIVATE_SESSIONS_SKIP_STATISTICS && self.statistics_private)
    }

    /// Count a committing selection in memory, and hand the batch to the store once it is `SELECTION_BATCH` long.
    fn count_selection(&mut self, position: usize) {
        // Android 的隐私模式与不学习输入框、鸿蒙和 iOS 的隐私模式不统计选词位置。
        if self.private_session() {
            return;
        }
        // Positions are one-based; zero is not a position, and the store has always refused it.
        let Some(slot) = position.checked_sub(1) else {
            return;
        };
        let slot = &mut self.pending_selections[slot.min(RANKS)];
        *slot = slot.saturating_add(1);
        if self.pending_selections.iter().sum::<u64>() >= SELECTION_BATCH {
            self.flush_selections();
        }
    }

    /// 选中的候选在派发前的样子，供上屏后计效率：不记统计的会话、统计关闭、非 Android 宿主或找不到这个候选时为 `None`。
    fn efficiency_candidate(&mut self, action: &Action) -> Option<EfficiencyCandidate> {
        if !COUNTS_COMMIT_EFFICIENCY || self.private_session() {
            return None;
        }
        let id = match action {
            Action::Select(id) | Action::SelectAnyCandidate(id) => *id,
            _ => return None,
        };
        if !self.statistics_enabled()? {
            return None;
        }
        let view = self.runtime.view();
        let candidate = view
            .candidates
            .into_iter()
            .find(|candidate| candidate.id == id)?;
        Some(EfficiencyCandidate {
            prediction: candidate.code.is_empty() && view.preedit.is_empty(),
            sentence: msime_engine::CandidateSource::from_u8(candidate.source)
                .is_some_and(msime_engine::CandidateSource::is_sentence_learning),
            typed_keys: candidate
                .code
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .count() as u64,
            text: candidate.text,
        })
    }

    /// 统计开关；读不到统计目录或文件时当作关闭。
    fn statistics_enabled(&mut self) -> Option<bool> {
        if self.edition.id == "shurufa" {
            return Some(false);
        }
        if let Some(enabled) = self.statistics_enabled {
            return Some(enabled);
        }
        let directory = std::path::Path::new(&self.options.user_data);
        let enabled = directory.is_absolute()
            && TypingStatisticsStore::new(directory)
                .load()
                .is_ok_and(|statistics| statistics.enabled);
        self.statistics_enabled = Some(enabled);
        Some(enabled)
    }

    /// 记下一次上屏，等下一次写统计时一起算。上限 [`EFFICIENCY_BATCH_LIMIT`]，超出的不计。
    fn count_efficiency(&mut self, candidate: EfficiencyCandidate) {
        if self.pending_efficiency.len() < EFFICIENCY_BATCH_LIMIT {
            self.pending_efficiency.push(candidate);
        }
    }

    /// Write the selections counted since the last flush, in the store the host already keeps.
    ///
    /// Best effort on purpose: statistics must never be the reason a keystroke fails, so a locked or unwritable store is dropped rather than surfaced, and the batch goes with it rather than being retried on every later key. The store honours the user's switch itself, so there is no second check here to fall out of step with it. Nothing pending means nothing touches the disk.
    fn flush_selections(&mut self) {
        let pending = std::mem::take(&mut self.pending_selections);
        let directory = std::path::Path::new(&self.options.user_data);
        if !directory.is_absolute() {
            return;
        }
        let mut batch = Vec::with_capacity(RANKS);
        batch.extend(
            pending
                .iter()
                .enumerate()
                .filter(|(_, count)| **count > 0)
                .map(|(slot, count)| (slot + 1, *count)),
        );
        let store = TypingStatisticsStore::new(directory);
        let enabled = store.record_selections(&batch).ok().flatten();
        let commits = std::mem::take(&mut self.pending_efficiency);
        if !commits.is_empty() {
            // 查读音要打开词库，放到后台线程，不占输入线程；测试里同步写，好断言结果。
            if cfg!(test) {
                record_efficiency(&self.options, &commits);
            } else {
                record_efficiency_in_background(self.options.clone(), commits);
            }
        }
        // 用户可能在设置里关掉或打开了统计：写这一批时刚读过文件，就用读到的开关；没读（空批次或出错）时下一批再读。不在输入线程上为一个开关再读一遍整个文件。
        self.statistics_enabled = enabled;
    }

    /// Rebuild the Engine for the requested preferences once the composition is idle. The `Ok` value is why the preferred scheme was not the one applied, when it was not.
    fn apply_pending(&mut self) -> Result<Option<String>, String> {
        if self.runtime.is_idle() {
            if let Some(size) = self.page_size_override {
                self.runtime
                    .set_page_size(host_page_size(size))
                    .map_err(|e| e.to_string())?;
            }
        }
        if !(self.preferences_pending || self.resources_pending) || !self.runtime.is_idle() {
            return Ok(None);
        }
        // 只因资源包变化而重建时，没有新请求的偏好，按已应用的那份重建。
        let preferences = self
            .requested
            .as_ref()
            .map(|snapshot| &snapshot.preferences)
            .unwrap_or(&self.applied)
            .clone();
        let mut options = self.options.clone();
        let (scheme, scheme_fallback) = effective_scheme(
            &preferences,
            &offered_input_schemes(self.edition),
            self.edition.default_scheme,
        );
        options.scheme = scheme_code(scheme);
        let profile_fallback = apply_shuangpin_profile(&mut options, &preferences);
        // 自定义双拼表不能用的原因只在真的跑双拼时才值得报告。
        let fallback =
            scheme_fallback.or(profile_fallback.filter(|_| scheme == InputScheme::Shuangpin));
        options.shuangpin_preedit_uses_raw = preferences.shuangpin_preedit_uses_raw;
        options.single_character_only = preferences.single_character_only;
        options.caret_edit_suffix = caret_edit_suffix_applies(&preferences);
        options.learning = preferences.learning;
        options.decoupled_personal_learning = self.edition.id == "shurufa";
        options.next_word_prediction =
            preferences.next_word_prediction && cfg!(target_os = "android");
        options.learn_english_on_enter = preferences.enter_learns_english_word;
        options.autocorrect_transposition = preferences.quanpin.autocorrect_transposition;
        options.autocorrect_neighbor = preferences.quanpin.autocorrect_neighbor;
        options.fuzzy_pinyin_rules = preferences.fuzzy_pinyin.active_rules();
        options.frequency_mode = preferences.frequency.mode.as_str().into();
        options.frequency_trigger_count = preferences.frequency.trigger_count;
        options.frequency_linear_step = preferences.frequency.linear_step;
        options.mixed_candidates = preferences.mixed_input.enabled;
        options.mixed_english = preferences.mixed_input.english;
        options.english_minimum_prefix =
            host_english_minimum_prefix(preferences.mixed_input.minimum_prefix);
        options.english_first_page_count = preferences.mixed_input.first_page_count;
        // Shurufa 内联 Emoji 改由重排后纯函数 SlotAllocator 投影，绕开旧 queries.rs 混排。
        options.mixed_emoji = self.edition.id != "shurufa" && preferences.mixed_input.emoji;
        options.mixed_kaomoji = preferences.mixed_input.kaomoji;
        options.mixed_symbols = preferences.mixed_input.symbols;
        options.quick_phrase_candidates =
            self.quick_phrase_candidates_requested && preferences.mixed_input.quick_phrase;
        options.quick_phrase_frequency = preferences.mixed_input.quick_phrase_frequency;
        options.local_unicode = preferences.local_modes.unicode;
        options.local_date_time = preferences.local_modes.date_time;
        options.local_date_time_inline = preferences.local_modes.date_time_inline;
        options.local_date_time_menu = preferences.local_modes.date_time_menu;
        options.local_quick_phrase = preferences.local_modes.quick_phrase;
        options.local_emoji = preferences.local_modes.emoji;
        options.local_kaomoji = preferences.local_modes.kaomoji;
        options.local_super_jianpin = preferences.local_modes.super_jianpin;
        options.local_temporary_english = preferences.local_modes.temporary_english;
        options.local_expression = preferences.local_modes.expression;
        options.local_command = preferences.local_modes.command;
        options.local_mention = preferences.local_modes.mention;
        options.local_symbol = preferences.local_modes.symbol;
        options.local_shift_letter_entry = preferences.local_modes.shift_letter_entry;
        let plugin_root = self.plugin_roots.installed.as_deref();
        let plugin_tables = plugin_tables::PluginTables::stamp(
            plugin_root,
            self.plugin_roots.builtin_packs.as_deref(),
            &options,
            &preferences.plugins,
        );
        plugin_tables.fill(&self.plugin_tables, plugin_root, &mut options);
        options.sentence_association = engine_sentence_association(
            &preferences.sentence_association,
            self.state_root.as_deref(),
        );
        let page_size = host_page_size(
            self.page_size_override
                .unwrap_or(preferences.candidate_page_size),
        );
        options.first_page_rows = usize::from(page_size);
        options.sentence_alternatives = true;
        apply_local_mode_resource_gates(&mut options, self.edition);
        options.paired_punctuation = preferences.paired_punctuation;
        options.punctuation_lock = punctuation_lock_code(preferences.punctuation_lock);
        options.chinese_punctuation =
            engine_chinese_punctuation(preferences.chinese_punctuation, options.punctuation_lock);
        apply_punctuation_marks(&mut options, &preferences);
        // 先构建并校验，失败时原会话仍然可用。
        let mut engine = Session::new(&options).map_err(|e| e.to_string())?;
        engine.set_prediction_allowed(!self.statistics_private);
        engine.set_cloud_candidates_enabled(preferences.cloud_candidates);
        engine.set_english_online_candidates_enabled(preferences.english_online_candidates);
        if self.punctuation_override.is_some() || self.punctuation_lock_override.is_some() {
            engine
                .set_chinese_punctuation_enabled(engine_chinese_punctuation(
                    self.punctuation_override
                        .unwrap_or(preferences.chinese_punctuation),
                    self.punctuation_lock_override
                        .unwrap_or(options.punctuation_lock),
                ))
                .map_err(|e| e.to_string())?;
        }
        if let Some(enabled) = self.paired_punctuation_override {
            engine
                .set_paired_punctuation_enabled(enabled)
                .map_err(|e| e.to_string())?;
        }
        if let Some(lock) = self.punctuation_lock_override {
            engine
                .set_punctuation_lock(lock)
                .map_err(|e| e.to_string())?;
        }
        engine
            .set_dedicated_english(self.english_mode)
            .map_err(|e| e.to_string())?;
        engine
            .set_mention_places(preferences.local_modes.mention_places)
            .map_err(|e| e.to_string())?;
        self.runtime
            .replace_engine_with_touch_layout(engine, page_size, preferences.touch_keyboard_layout)
            .map_err(|e| e.to_string())?;
        let emoji_mru = (!options.user_data.is_empty())
            .then(|| std::path::PathBuf::from(&options.user_data).join("emoji_mru.json"));
        self.runtime.set_inline_emoji_projection(
            self.edition.id == "shurufa" && preferences.mixed_input.emoji,
            emoji_mru,
        );
        self.runtime
            .set_punctuation_marks(msime_client_core::punctuation::punctuation_marks(
                &preferences,
            ));
        self.options = options;
        self.plugin_tables = plugin_tables;
        self.applied = preferences;
        self.preferences_pending = false;
        self.resources_pending = false;
        Ok(fallback)
    }

    /// 输入框获得焦点时，让 `/` 指令表、K 模式短语表和 `@` 名单跟上插件目录：设置页可能刚导入了表、改了名单。没有文件变动时什么都不读。换表失败只报告，不让聚焦失败。
    fn refresh_plugin_tables(&mut self) {
        let root = self.plugin_roots.installed.as_deref();
        let tables = plugin_tables::PluginTables::stamp(
            root,
            self.plugin_roots.builtin_packs.as_deref(),
            &self.options,
            &self.applied.plugins,
        );
        if tables.commands_differ(&self.plugin_tables) {
            let table = tables.command_table(root);
            if let Err(error) = self.runtime.set_command_table(&table) {
                diagnostics::report("command table view not refreshed", &error.to_string());
            }
            self.options.command_table = table;
        }
        if tables.mentions_differ(&self.plugin_tables) {
            let entries = tables.mention_entries(root, &self.options);
            if let Err(error) = self.runtime.set_mention_entries(&entries) {
                diagnostics::report("mention list view not refreshed", &error.to_string());
            }
            self.options.mention_entries = entries;
        }
        if tables.phrases_differ(&self.plugin_tables) {
            let table = tables.quick_phrase_table(root);
            if let Err(error) = self.runtime.set_quick_phrase_table(&table) {
                diagnostics::report("quick phrase view not refreshed", &error.to_string());
            }
            self.options.quick_phrase_table = table;
        }
        self.plugin_tables = tables;
    }

    /// 输入框获得焦点时，看设置页是否在会话打开后装好（或移除）了语法模型资源包。
    fn refresh_resource_packs(&mut self) {
        let state_root = self.state_root.as_deref();
        let grammar_model =
            engine_sentence_association(&self.applied.sentence_association, state_root)
                .collocation_model;
        if grammar_model == self.options.sentence_association.collocation_model {
            return;
        }
        self.options.sentence_association.collocation_model = grammar_model;
        self.resources_pending = true;
    }

    /// 「中英文之间加空格」开关，跟着最新请求的偏好走：它只是上屏后处理，不是 Engine 状态，不必等组字结束后重建。
    fn chinese_english_spacing(&self) -> bool {
        self.requested
            .as_ref()
            .map_or(&self.applied, |snapshot| &snapshot.preferences)
            .chinese_english_spacing
    }

    fn complete_transition(&mut self, mut result: Transition) -> Transition {
        let generation = self.runtime.generation();
        let note = match self.apply_pending() {
            Ok(fallback) => fallback,
            Err(error) => Some(format!("Preferences update deferred: {error}")),
        };
        if let Some(note) = note {
            let prior = result.diagnostic.take().unwrap_or_default();
            result.diagnostic = Some(format!("{prior} {note}").trim().to_owned());
        }
        // 替换只改变 view 的代次，不改变已完成的上屏。不转全角的方案（韩文）无论宽度开关如何都写半角 ASCII 标点和数字；专用英文模式在任何方案里都按自己的规则走，所以它的上屏和中文方案下一样转全角。网址模式的网址部分始终是半角：上屏后 view 已回到 `none`，所以看上屏前记下的 `commit_context`；结束网址的那个标点不是网址能收的字符，照常转全角。
        let half_width_text = SchemeType::from_u8(result.view.scheme)
            .is_some_and(|scheme| !scheme.widens_full_width())
            && !result.view.dedicated_english
            && result.view.local_mode == "none";
        let url_commit = result
            .commit_context
            .as_ref()
            .is_some_and(|context| context.local_mode == "url");
        if result.view.character_width == CharacterWidth::Fullwidth && !half_width_text {
            if let Some(c) = result.commit.as_mut() {
                *c = c
                    .chars()
                    .map(|x| {
                        if url_commit && x.is_ascii() && msime_engine::url::accepts(x as u8) {
                            x
                        } else if x == ' ' {
                            '\u{3000}'
                        } else if ('!'..='~').contains(&x) {
                            char::from_u32(x as u32 + 0xfee0).unwrap()
                        } else {
                            x
                        }
                    })
                    .collect();
            }
        }
        // 中英文之间加空格排在全角转换之后：先加的半角空格会被转成 U+3000；全角下字母已是全角，本来就不会触发。宿主的简繁转换在拿到上屏文本之后才做，OpenCC 只换汉字、不动空格。之后的 `before_commit` 看到的是加过空格的文本。
        if let Some(commit) = result.commit.as_mut() {
            if self.chinese_english_spacing()
                && chinese_english_spacing::applies(result.commit_context.as_ref())
            {
                if let Some(spaced) = chinese_english_spacing::space(commit) {
                    *commit = spaced;
                }
            }
        }
        // Dispatch already built the view for this transition. Rebuild it only
        // when applying a deferred page-size or preference change advanced the
        // runtime generation; otherwise cloning the page again costs every key.
        if self.runtime.generation() != generation {
            result.view = self.runtime.view();
        }
        result
    }

    fn update(&mut self, mut snapshot: PreferencesSnapshot) -> Result<Value, String> {
        apply_shurufa_input_policy(&mut snapshot.preferences, self.edition);
        if snapshot.format_version != 1 {
            return Err("unsupported preferences format".into());
        }
        snapshot
            .preferences
            .validate_for_edition(self.edition)
            .map_err(|e| e.to_string())?;
        if let Some(previous) = &self.requested {
            if snapshot.revision < previous.revision
                || (snapshot.revision == previous.revision && snapshot != *previous)
            {
                return Err("stale or conflicting preferences revision".into());
            }
        }
        let ai_changed = self
            .requested
            .as_ref()
            .map_or(&self.applied.ai_assistant, |previous| {
                &previous.preferences.ai_assistant
            })
            != &snapshot.preferences.ai_assistant;
        let cloud_disabled = self
            .requested
            .as_ref()
            .map_or(self.applied.cloud_candidates, |previous| {
                previous.preferences.cloud_candidates
            })
            && !snapshot.preferences.cloud_candidates;
        let cloud_changed = self
            .requested
            .as_ref()
            .map_or(&self.applied.cloud_candidate_provider, |previous| {
                &previous.preferences.cloud_candidate_provider
            })
            != &snapshot.preferences.cloud_candidate_provider;
        if (cloud_disabled || cloud_changed) && !self.runtime.is_idle() {
            // 关闭云候选或切换来源配置时移除旧云行，使迟到回包失效。
            self.runtime
                .clear_online_candidates(0)
                .map_err(|e| e.to_string())?;
        }
        let previous_preferences = self
            .requested
            .as_ref()
            .map_or(&self.applied, |previous| &previous.preferences);
        let translation_changed =
            translation_query_preferences_changed(previous_preferences, &snapshot.preferences);
        if translation_changed && !self.runtime.is_idle() {
            // 释义配置与服务凭据变化时清掉旧释义，使旧代次的回包不能进入新配置。
            self.runtime
                .invalidate_translations()
                .map_err(|e| e.to_string())?;
        }
        if ai_changed && !self.runtime.is_idle() {
            // AI 配置或令牌变化时先清理旧候选并使旧查询失效，再发布新偏好。
            self.runtime
                .clear_online_candidates(1)
                .map_err(|e| e.to_string())?;
        }
        self.preferences_pending = snapshot.preferences != self.applied;
        self.requested = Some(snapshot);
        let requested_preferences = self
            .requested
            .as_ref()
            .expect("requested snapshot exists")
            .preferences
            .clone();
        self.set_ai_provider_cache(&requested_preferences);
        self.runtime
            .set_english_online_candidates_enabled(requested_preferences.english_online_candidates)
            .map_err(|e| e.to_string())?;
        self.runtime
            .set_cloud_candidates_enabled(requested_preferences.cloud_candidates)
            .map_err(|e| e.to_string())?;
        // 候选词翻译的范围只改查询和画法，跟着最新请求的偏好走；换范围时的旧释义已经在上面随释义偏好的变化清掉。
        self.runtime
            .set_translation_scope_highlighted(translation_scope_highlighted(
                &requested_preferences,
            ));
        // 宿主状态，不是 Engine 状态：跟着最新请求的偏好走，不必等这次偏好重建 Engine，也不必等组字结束。
        self.runtime
            .set_backspace_reverts_selection(requested_preferences.backspace_reverts_selection);
        // Esc 保留已选词同样是宿主状态：改完下一次 Esc 就按新开关解释。
        self.runtime
            .set_escape_keeps_selected_word(requested_preferences.escape_keeps_selected_word);
        // 左右键的候选回退（`navigation.left_right`）同样是宿主状态：改完立刻生效，左右键不必等 Engine 重建。
        self.runtime
            .set_left_right_navigation(requested_preferences.navigation.left_right);
        self.runtime
            .set_vertical_candidates(host_vertical_candidates(
                requested_preferences.candidate_layout,
            ));
        // 首页英文候选数同样是宿主状态：在线座位整理按它给英文组留位，改完下一次刷新就生效。
        self.runtime
            .set_english_first_page_count(requested_preferences.mixed_input.first_page_count);
        // 「二三候选」键也是宿主编排状态：跟着最新请求的偏好走，下一键生效。
        self.runtime
            .set_second_third_candidate(requested_preferences.second_third_candidate);
        self.sound.update(key_sound::SoundSettings::new(
            &requested_preferences.plugins,
            &self.plugin_roots,
        ));
        let fallback = self.apply_pending()?;
        let snapshot = self.requested.as_ref().expect("requested snapshot exists");
        let mut response = json!({ "revision": snapshot.revision, "deferred": snapshot.preferences != self.applied, "view": self.runtime.view(), "floating_toolbar": { "enabled": snapshot.preferences.floating_toolbar.enabled, "english_mode": snapshot.preferences.floating_toolbar.english_mode, "scale_percent": snapshot.preferences.floating_toolbar.scale_percent, "font_size": snapshot.preferences.floating_toolbar.font_size, "fullwidth": snapshot.preferences.floating_toolbar.fullwidth, "punctuation": snapshot.preferences.floating_toolbar.punctuation, "character_set": snapshot.preferences.floating_toolbar.character_set, "emoji": snapshot.preferences.floating_toolbar.emoji, "screen_keyboard": snapshot.preferences.floating_toolbar.screen_keyboard, "settings": snapshot.preferences.floating_toolbar.settings } });
        // Applied at once, so no transition will carry the reason the preferred scheme was replaced; a deferred change reports it from `complete_transition` instead.
        if let Some(fallback) = fallback {
            response["diagnostic"] = fallback.into();
        }
        Ok(response)
    }
}

fn profile_code(profile: ShuangpinProfile) -> u8 {
    match profile {
        ShuangpinProfile::Xiaohe => 0,
        ShuangpinProfile::Ziranma => 1,
        ShuangpinProfile::Shoudao => 2,
        ShuangpinProfile::Microsoft => 3,
        ShuangpinProfile::SmartAbc => 5,
        ShuangpinProfile::Custom => 4,
    }
}

/// 偏好里的自定义双拼表换成 Engine 的表。
fn engine_shuangpin_custom_table(profile: &ShuangpinCustomProfile) -> ShuangpinCustomTable {
    let pairs = |part: &std::collections::BTreeMap<String, String>| {
        part.iter()
            .map(|(unit, key)| (unit.clone(), key.clone()))
            .collect()
    };
    ShuangpinCustomTable {
        initials: pairs(&profile.initials),
        finals: pairs(&profile.finals),
        zero_initials: pairs(&profile.zero_initials),
    }
}

/// 偏好里的自定义双拼表能不能用，由 Engine 判定；不能用时是给日志和代理看的英文原因。设置页、MCP 等写偏好的一方保存之前用它校验。
pub fn validate_custom_shuangpin_profile(profile: &ShuangpinCustomProfile) -> Result<(), String> {
    validate_shuangpin_custom_profile(&engine_shuangpin_custom_table(profile))
        .map_err(|error| error.to_string())
}

/// 自定义双拼表为什么不能用，按 Engine 的 `ShuangpinCustomProfileError` 逐项摊开，给设置页按种类写自己的提示、标出出错的那一格。Engine 在第一个错误处就停，所以一次只有一条。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ShuangpinCustomProblem {
    pub kind: ShuangpinCustomProblemKind,
    /// 出错的是表的哪一部分：`initials`、`finals` 或 `zero_initials`。编码冲突不属于某一部分，没有这一项；多字母声母占了别的键时是 `initials`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<&'static str>,
    /// 出错的单位，例如 `zh`、`iong`、`er`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// 不合法或已被占用的键（已转成小写）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 两个音节撞上的那个两键编码，以及这两个音节。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShuangpinCustomProblemKind {
    UnknownUnit,
    DuplicateUnit,
    MissingUnit,
    InvalidKey,
    InitialKeyTaken,
    AmbiguousCode,
}

impl ShuangpinCustomProblem {
    fn of_kind(kind: ShuangpinCustomProblemKind) -> Self {
        Self {
            kind,
            part: None,
            unit: None,
            key: None,
            code: None,
            first: None,
            second: None,
        }
    }
}

impl From<ShuangpinCustomProfileError> for ShuangpinCustomProblem {
    fn from(error: ShuangpinCustomProfileError) -> Self {
        use ShuangpinCustomProblemKind as Kind;
        match error {
            ShuangpinCustomProfileError::UnknownUnit { part, unit } => Self {
                part: Some(part.name()),
                unit: Some(unit),
                ..Self::of_kind(Kind::UnknownUnit)
            },
            ShuangpinCustomProfileError::DuplicateUnit { part, unit } => Self {
                part: Some(part.name()),
                unit: Some(unit),
                ..Self::of_kind(Kind::DuplicateUnit)
            },
            ShuangpinCustomProfileError::MissingUnit { part, unit } => Self {
                part: Some(part.name()),
                unit: Some(unit.to_owned()),
                ..Self::of_kind(Kind::MissingUnit)
            },
            ShuangpinCustomProfileError::InvalidKey { part, unit, key } => Self {
                part: Some(part.name()),
                unit: Some(unit),
                key: Some(key),
                ..Self::of_kind(Kind::InvalidKey)
            },
            ShuangpinCustomProfileError::InitialKeyTaken { unit, key } => Self {
                part: Some(ShuangpinCustomTablePart::Initials.name()),
                unit: Some(unit.to_owned()),
                key: Some(key),
                ..Self::of_kind(Kind::InitialKeyTaken)
            },
            ShuangpinCustomProfileError::AmbiguousCode {
                code,
                first,
                second,
            } => Self {
                code: Some(code),
                first: Some(first),
                second: Some(second),
                ..Self::of_kind(Kind::AmbiguousCode)
            },
        }
    }
}

/// 编辑中的自定义双拼表有什么问题，能用时是 `None`。结论与 [`validate_custom_shuangpin_profile`] 相同，但用 Engine 不留存的检查：设置页每改一个键就问一次，每张合法的中间表都留下来就是永久泄漏。
pub fn custom_shuangpin_profile_problem(
    profile: &ShuangpinCustomProfile,
) -> Option<ShuangpinCustomProblem> {
    check_shuangpin_custom_profile(&engine_shuangpin_custom_table(profile))
        .err()
        .map(ShuangpinCustomProblem::from)
}

/// 内置双拼方案写成自定义表的样子，作为编辑自定义方案的起点；`Custom` 本身没有内置的表，返回 `None`。
pub fn builtin_shuangpin_custom_profile(
    profile: ShuangpinProfile,
) -> Option<ShuangpinCustomProfile> {
    let table =
        ShuangpinProfileKind::from_u8(profile_code(profile)).and_then(shuangpin_builtin_table)?;
    let map = |pairs: Vec<(String, String)>| pairs.into_iter().collect();
    Some(ShuangpinCustomProfile {
        initials: map(table.initials),
        finals: map(table.finals),
        zero_initials: map(table.zero_initials),
    })
}

/// 把双拼方案写进 Engine 选项。选的是自定义方案时先让 Engine 校验用户的表：表是空的或不合法时按小鹤运行，返回原因，免得一张写坏的表让 Engine 建不起来、整个输入法停在旧会话上。
fn apply_shuangpin_profile(
    options: &mut EngineOptions,
    preferences: &Preferences,
) -> Option<String> {
    options.shuangpin_custom_profile = None;
    options.shuangpin_profile = profile_code(preferences.shuangpin_profile);
    if preferences.shuangpin_profile != ShuangpinProfile::Custom {
        return None;
    }
    match validate_custom_shuangpin_profile(&preferences.shuangpin_custom_profile) {
        Ok(()) => {
            options.shuangpin_custom_profile = Some(engine_shuangpin_custom_table(
                &preferences.shuangpin_custom_profile,
            ));
            None
        }
        Err(error) => {
            options.shuangpin_profile = profile_code(ShuangpinProfile::Xiaohe);
            Some(format!(
                "Custom shuangpin profile unusable because {error}; using Xiaohe."
            ))
        }
    }
}

/// 交给 Engine 的 `enabled_schemes`：本版本提供的方案。
fn engine_schemes(edition: &Edition) -> SchemeSet {
    offered_input_schemes(edition)
        .into_iter()
        .filter_map(|scheme| SchemeType::from_u8(scheme_code(scheme)))
        .fold(SchemeSet::EMPTY, SchemeSet::with)
}

fn scheme_code(scheme: InputScheme) -> u8 {
    match scheme {
        InputScheme::Quanpin => 0,
        InputScheme::Shuangpin => 1,
        InputScheme::Wubi => 2,
        InputScheme::Japanese => 3,
        InputScheme::Korean => 4,
        InputScheme::Cantonese => 5,
        InputScheme::Zhuyin => 6,
        InputScheme::Vietnamese => 7,
        InputScheme::Tibetan => 8,
        InputScheme::Stroke => 9,
    }
}

/// HostOptions 的 `preferences_directory`，只认绝对路径：按需下载的资源包装在它下面。
fn absolute_state_root(preferences_directory: Option<&str>) -> Option<PathBuf> {
    preferences_directory
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// 按 `preferences` 交给 Engine 的方案，以及没用偏好里那个方案时的原因。本版本不提供的方案回退到上一次的中文方案（可用时），否则回退到 `default`。
pub(crate) fn effective_scheme(
    preferences: &Preferences,
    supported: &[InputScheme],
    default: InputScheme,
) -> (InputScheme, Option<String>) {
    let usable = |scheme: InputScheme| supported.contains(&scheme);
    let preferred = preferences.scheme;
    if usable(preferred) {
        return (preferred, None);
    }
    let fallback = preferences
        .last_chinese_scheme
        .map(InputScheme::from)
        .filter(|scheme| usable(*scheme))
        .unwrap_or(default);
    (
        fallback,
        Some(format!(
            "Input scheme {preferred:?} unavailable because this host does not offer it; using {fallback:?}."
        )),
    )
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HostOptions {
    api_version: u32,
    resources: String,
    user_data: String,
    cache: String,
    dictionaries: String,
    preferences: Preferences,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferences_directory: Option<String>,
    /// The host draws a half-composed phrase itself: picking a candidate that covers only part of
    /// the input leaves the chosen piece in `view.phrase_prefix` instead of committing it, and the
    /// whole phrase commits at once when the composition ends. Absent means the previous behaviour,
    /// where each piece went to the document as it was picked, because a host that does not draw
    /// the field would otherwise show nothing for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phrase_preedit: Option<bool>,
    /// macOS 和 Linux 的两个宿主把普通组字里的快捷短语也作为候选显示；缺省关闭以保持其他宿主和旧文档的入口契约。建会话时写进 `EngineOptions`，偏好变更重建 Engine 时沿用同一份选项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_phrase_candidates: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clipboard_history_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    online_provider_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    translation_provider_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voice_provider_socket: Option<String>,
    /// Absolute path to the bundle's built-in sound packs (`resources/sound-packs` in the repository), for a host whose bundle does not put them in `sound-packs` beside `resources`, the directory used when this is absent. Installed packs, command tables and the `@` name list are read from `plugins` under `preferences_directory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sound_packs: Option<String>,
    /// 桌面安装包随带的官方插件包目录（绝对路径，`<kind>/<id>/`，构建产物是 `target/plugin-packs`）。没有时取内置音效包目录旁边的 `plugin-packs`，那里也没有就没有官方包。macOS 的 bundle 和 Linux 的安装把它放在音效包旁边，不用写；Windows 的 Server 写它，因为那里的音效包在普通用户可写的数据目录里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_packs: Option<String>,
    /// 产品版本（`Edition::HOST_OPTIONS_KEY`）。
    #[serde(default, skip_serializing_if = "Option::is_none", with = "edition_id")]
    edition: Option<&'static Edition>,
}

/// HostOptions 的 `edition` 键与版本表条目之间的转换。
mod edition_id {
    use msime_client_core::edition::Edition;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        edition: &Option<&'static Edition>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match edition {
            Some(edition) => serializer.serialize_str(&edition.id),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<&'static Edition>, D::Error> {
        match Option::<String>::deserialize(deserializer)? {
            None => Ok(None),
            Some(id) => Edition::by_id(&id)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown edition {id:?}"))),
        }
    }
}

impl HostOptions {
    /// 文档记录的版本，缺省是 full。
    fn edition(&self) -> &'static Edition {
        self.edition.unwrap_or_else(Edition::full)
    }

    /// 解析 HostOptions 文档。其中的 `preferences` 是准备运行时配置那一刻的副本，升级时被原样带下去（`refresh_options_file`）。#2830 退役 `autocorrect` 时，带着它的副本曾让会话、快照和词库请求全部被拒，macOS 每个按键都像在英文模式下一样直接交给应用；偏好结构现在不拒绝不认识的键，新增或退役一个键不会再让副本读不了，但取值的类型或格式变化仍会。副本被拒时，改用 `preferences_directory` 下实时的 config.json（或尚未迁移的 preferences.json）——设置界面会保持它最新并负责修复，宿主在会话打开后本来也会应用它。副本能解析时照原样使用。
    pub(crate) fn from_document(mut document: Value) -> Option<Self> {
        if let Ok(options) = Self::deserialize(&document) {
            return Some(options);
        }
        let directory = std::path::PathBuf::from(document.get("preferences_directory")?.as_str()?);
        // load() 会创建传入的目录；从没在那里写过偏好的宿主没有可替代的内容。
        if !directory.is_absolute()
            || (!directory.join("config.json").is_file()
                && !directory.join("preferences.json").is_file())
        {
            return None;
        }
        let snapshot = PreferencesStore::new(&directory).load().ok()?;
        document["preferences"] = serde_json::to_value(snapshot.preferences).ok()?;
        Self::deserialize(&document).ok()
    }

    fn into_engine_options(mut self) -> EngineOptions {
        let edition = self.edition();
        apply_shurufa_input_policy(&mut self.preferences, edition);
        let state_root = absolute_state_root(self.preferences_directory.as_deref());
        let (scheme, _) = effective_scheme(
            &self.preferences,
            &offered_input_schemes(edition),
            edition.default_scheme,
        );
        let mut options = EngineOptions {
            resources: self.resources,
            user_data: self.user_data,
            cache: self.cache,
            dictionaries: self.dictionaries,
            // 词库维护类操作也走这里：`HostOptions` 没有 `page_size_override`，只能用偏好值。
            first_page_rows: usize::from(host_page_size(self.preferences.candidate_page_size)),
            scheme: scheme_code(scheme),
            enabled_schemes: engine_schemes(edition),
            shuangpin_profile: profile_code(self.preferences.shuangpin_profile),
            shuangpin_custom_profile: None,
            shuangpin_preedit_uses_raw: self.preferences.shuangpin_preedit_uses_raw,
            single_character_only: self.preferences.single_character_only,
            caret_edit_suffix: caret_edit_suffix_applies(&self.preferences),
            learning: self.preferences.learning,
            decoupled_personal_learning: edition.id == "shurufa",
            learn_english_on_enter: self.preferences.enter_learns_english_word,
            next_word_prediction: self.preferences.next_word_prediction
                && cfg!(target_os = "android"),
            autocorrect_transposition: self.preferences.quanpin.autocorrect_transposition,
            autocorrect_neighbor: self.preferences.quanpin.autocorrect_neighbor,
            fuzzy_pinyin_rules: self.preferences.fuzzy_pinyin.active_rules(),
            frequency_mode: self.preferences.frequency.mode.as_str().into(),
            frequency_trigger_count: self.preferences.frequency.trigger_count,
            frequency_linear_step: self.preferences.frequency.linear_step,
            mixed_candidates: self.preferences.mixed_input.enabled,
            mixed_english: self.preferences.mixed_input.english,
            english_minimum_prefix: host_english_minimum_prefix(
                self.preferences.mixed_input.minimum_prefix,
            ),
            english_first_page_count: self.preferences.mixed_input.first_page_count,
            english_short_prefix_completion: edition.id == "shurufa",
            mixed_emoji: edition.id != "shurufa" && self.preferences.mixed_input.emoji,
            mixed_kaomoji: self.preferences.mixed_input.kaomoji,
            mixed_symbols: self.preferences.mixed_input.symbols,
            local_unicode: self.preferences.local_modes.unicode,
            local_date_time: self.preferences.local_modes.date_time,
            local_date_time_inline: self.preferences.local_modes.date_time_inline,
            local_date_time_menu: self.preferences.local_modes.date_time_menu,
            local_quick_phrase: self.preferences.local_modes.quick_phrase,
            local_emoji: self.preferences.local_modes.emoji,
            local_kaomoji: self.preferences.local_modes.kaomoji,
            local_super_jianpin: self.preferences.local_modes.super_jianpin,
            local_temporary_english: self.preferences.local_modes.temporary_english,
            local_url: edition.id != "shurufa",
            local_expression: self.preferences.local_modes.expression,
            local_expression_lowercase_entry: edition.id == "shurufa",
            local_command: self.preferences.local_modes.command,
            local_mention: self.preferences.local_modes.mention,
            local_symbol: self.preferences.local_modes.symbol,
            local_shift_letter_entry: self.preferences.local_modes.shift_letter_entry,
            command_table: Vec::new(),
            mention_entries: Vec::new(),
            quick_phrase_table: Vec::new(),
            // 宿主声明能力、用户在设置里打开，两个条件都满足才把快捷短语混进普通候选；`mixed_input.quick_phrase` 缺键的旧文档读作开，保住 macOS/Linux 升级前的行为。
            quick_phrase_candidates: self.quick_phrase_candidates.unwrap_or(false)
                && self.preferences.mixed_input.quick_phrase,
            quick_phrase_frequency: self.preferences.mixed_input.quick_phrase_frequency,
            sentence_association: engine_sentence_association(
                &self.preferences.sentence_association,
                state_root.as_deref(),
            ),
            rescoring_context: String::new(),
            sentence_alternatives: true,
            chinese_punctuation: engine_chinese_punctuation(
                self.preferences.chinese_punctuation,
                punctuation_lock_code(self.preferences.punctuation_lock),
            ),
            paired_punctuation: self.preferences.paired_punctuation,
            punctuation_lock: punctuation_lock_code(self.preferences.punctuation_lock),
            corner_quotes: false,
            hollow_brackets: false,
            enumeration_comma_backslash: true,
            enumeration_comma_slash: false,
        };
        apply_punctuation_marks(&mut options, &self.preferences);
        // 建会话没有地方带原因，表不能用时按小鹤建起来就够了；之后的偏好变更会在诊断里报告原因。
        let _ = apply_shuangpin_profile(&mut options, &self.preferences);
        apply_local_mode_resource_gates(&mut options, edition);
        options
    }
}

/// The offline gloss dictionary for one non-English target language installed beside a resource bundle, when one is there: `offline-glosses/zh-<language>.db`, built by `scripts/build_offline_glosses.py` and pinned by `resources/offline-glosses.lock.json`. Absence is the normal case.
pub(crate) fn offline_glosses_beside(
    resources: &std::path::Path,
    language: &str,
) -> Option<std::path::PathBuf> {
    if !OFFLINE_GLOSS_LANGUAGES.contains(&language) {
        return None;
    }
    let path = resources
        .parent()?
        .join("offline-glosses")
        .join(format!("zh-{language}.db"));
    path.is_file().then_some(path)
}

/// 一种非英文目标语言的离线释义词典：资源目录旁随包的那份优先，没有时用 `state_root` 下已下载的 `offline-glosses` 资源包里的那份（Android 的发布包不再内置它们，用户打开离线释义时下载），都没有时为 `None`。
pub(crate) fn offline_glosses_file(
    resources: &Path,
    state_root: Option<&Path>,
    language: &str,
) -> Option<PathBuf> {
    offline_glosses_beside(resources, language)
        .or_else(|| {
            let state = state_root?;
            let edition = Edition::recorded_in(state).unwrap_or_else(Edition::full);
            let original = msime_client_providers::dictionary_updates::bundled_resources(
                state,
                &edition.resource_set().ok()?,
            )?;
            offline_glosses_beside(&original, language)
        })
        .or_else(|| {
            if !OFFLINE_GLOSS_LANGUAGES.contains(&language) {
                return None;
            }
            resource_packs::installed_file(
                state_root?,
                ResourcePack::OfflineGlosses,
                &format!("zh-{language}.db"),
            )
        })
}

/// Bootstrap a new host using the reviewed desktop data and Engine-owned replay.
/// Call only while all sessions using state_root are stopped. Does not activate it.
pub fn bundled_only_host_configuration_placeholder() {}

/// The target languages an offline gloss dictionary can exist for; English is glossed from the packaged msime-english.db instead.
pub(crate) const OFFLINE_GLOSS_LANGUAGES: [&str; 6] = ["fr", "ja", "es", "ru", "de", "ko"];

/// Drop the `\\?\` prefix Windows canonicalisation adds.
///
/// The resource path is recorded in every HostOptions document this writes, and those documents have always held the plain drive form. Keeping it means an existing document and a freshly prepared one name the same directory the same way, and a native host reading the document gets the spelling it always got.
///
/// Only the drive form is unwrapped. `\\?\UNC\server\share` means something different from
/// `\\server\share` to the filesystem, so it is left alone rather than rewritten into a path that
/// happens to parse.
fn without_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    let text = match path.to_str() {
        Some(text) => text,
        None => return path,
    };
    let stripped = match text.strip_prefix(r"\\?\") {
        Some(stripped) => stripped,
        None => return path,
    };
    let drive = stripped.as_bytes();
    if drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\' {
        return std::path::PathBuf::from(stripped);
    }
    path
}

/// Hash the resource set unless the last successful verification still describes what is on disk.
///
/// This runs at every Server start, and the desktop set is 169 MB: about half a second of SHA-256
/// before the first keystroke can be served, repeated at every login. `VerifiedMarker` records what
/// was verified so the repeat is skipped while the files are untouched.
///
/// The marker lives under the state root rather than beside the resources: on Windows the
/// resources are installed under Program Files, which the Server does not get to write to.
///
/// Failing to write the marker is not failing to start. The next launch hashes again, which is the
/// behaviour this function replaces, so the cost of that miss is the cost of doing nothing here.
fn reject_symlinked_state_root(path: &Path) -> Result<(), std::io::Error> {
    msime_path_trust::reject_symlinked_components(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidInput {
            std::io::Error::new(error.kind(), "state root contains a symbolic link")
        } else {
            error
        }
    })
}

/// 当前平台发布包可以不内置、改为按需下载的资源文件组：macOS 是日文词典与它的两份 Mozc 许可文本、以及五笔码表，Android 和 iOS 是日文词典那一组，其余平台照旧全部内置。规则本身见 [`msime_client_core::resources::on_demand_artifacts`]，测试在任何主机上都能检查每个目标的取值。
pub(crate) const ON_DEMAND_ARTIFACTS: &[&[&str]] =
    msime_client_core::resources::on_demand_artifacts(std::env::consts::OS);

/// `resources` 实际按哪一份清单发货：`on_demand` 里每一组文件全部缺席时去掉这一组，否则这一组照锁文件要求。见 [`ResourceSet::as_shipped_in_groups`]。
pub(crate) fn shipped_specification(
    specification: &ResourceSet,
    resources: &Path,
    on_demand: &[&[&str]],
) -> ResourceSet {
    specification.as_shipped_in_groups(resources, on_demand)
}

fn verify_resources_once(
    resources: &std::path::Path,
    specification: &ResourceSet,
    state_root: &std::path::Path,
    on_demand: &[&[&str]],
) -> Result<(), Box<dyn std::error::Error>> {
    // 从内置日文词典的旧版本升级后，第一次启动时校验标记描述的清单变了，会重新哈希一次，这是有意的。
    let shipped = shipped_specification(specification, resources, on_demand);
    reject_symlinked_state_root(state_root)?;
    match std::fs::symlink_metadata(state_root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "state root is a symbolic link",
            )
            .into());
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "state root is not a directory",
            )
            .into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let marker_path = state_root.join("verified-resources.json");
    let current = VerifiedMarker::describe(resources, &shipped)?;
    if let (Some(current), Some(recorded)) = (&current, VerifiedMarker::read(&marker_path)) {
        if *current == recorded {
            return Ok(());
        }
    }
    ResourceStore::new(resources).verify(resources, &shipped)?;
    if let Some(current) = current {
        let _ = current.write(&marker_path);
    }
    Ok(())
}

/// 只读检查已准备配置所指向的资源；沿用宿主的版本锁、按需资源分组和校验标记，不创建输入会话。
pub fn runtime_resources_ready(document: &Value) -> bool {
    let check = || -> Option<bool> {
        let resources = Path::new(document.get("resources")?.as_str()?);
        let state = Path::new(document.get("preferences_directory")?.as_str()?);
        if !resources.is_absolute() || !state.is_absolute() {
            return Some(false);
        }
        let baseline = Edition::of_host_options(document)?.resource_set().ok()?;
        let selected = dictionary_updates::selected_resources(state, &baseline);
        let specification = selected
            .as_ref()
            .filter(|(path, _)| path == resources)
            .map(|(_, set)| set)
            .unwrap_or(&baseline);
        Some(runtime_resources_ready_with(
            resources,
            state,
            specification,
            ON_DEMAND_ARTIFACTS,
        ))
    };
    check().unwrap_or(false)
}

fn runtime_resources_ready_with(
    resources: &Path,
    state: &Path,
    specification: &ResourceSet,
    on_demand: &[&[&str]],
) -> bool {
    if reject_symlinked_state_root(state).is_err() {
        return false;
    }
    let shipped = shipped_specification(specification, resources, on_demand);
    if let (Ok(Some(current)), Some(recorded)) = (
        VerifiedMarker::describe(resources, &shipped),
        VerifiedMarker::read(&state.join("verified-resources.json")),
    ) {
        if current == recorded {
            return true;
        }
    }
    ResourceStore::new(resources)
        .verify(resources, &shipped)
        .is_ok()
}

pub fn prepare_host_configuration(
    resources: &std::path::Path,
    state_root: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error>> {
    prepare_host_configuration_for_edition(resources, state_root, Edition::full())
}

/// 为 `edition` 准备宿主：[`prepare_host_configuration`] 就是 full 的这一个。
///
/// 不是 full 的版本在文档里记下 `edition`，之后的会话、[`refresh_host_options`] 和 `msime-mcp` 都从文档里读它；full 的文档不写这个键，与以前完全相同。状态目录也记下它属于哪个版本（[`Edition::record_in`]，full 不写）：各平台宿主经 C 接口、设置应用经自己的存储都只按目录读写偏好，并不知道版本，偏好文件不见了或被修复时靠这份记录回到本版本的默认偏好。状态目录里还没有偏好文件时，每个版本都会把此刻读到的偏好写成第一份偏好文件：不经偏好存储直接读文件的一方第一次读到的也是本版本的默认值；新装和从以前的版本升级也只在这一刻分得清（触屏键盘的默认方案不同，见 `PreferencesStore::write_first_document`），写下文件后结论就固定了。
pub fn prepare_host_configuration_for_edition(
    resources: &std::path::Path,
    state_root: &std::path::Path,
    edition: &'static Edition,
) -> Result<String, Box<dyn std::error::Error>> {
    let specification = edition.resource_set()?;
    prepare_host_with_updates(resources, state_root, edition, &specification)
}

/// 安装选择与随包基线共用准备流程，测试用小型真实词库验证代次切换与日志回放。
fn prepare_host_with_updates(
    resources: &Path,
    state_root: &Path,
    edition: &'static Edition,
    specification: &ResourceSet,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some((updated, selected)) =
        dictionary_updates::selected_resources(state_root, specification)
    {
        if verify_resources_once(&updated, &selected, state_root, &[]).is_ok() {
            let mut document: Value = serde_json::from_str(&prepare_shipped_host_configuration(
                &updated,
                state_root,
                &selected,
                &[],
                edition,
            )?)?;
            // 随包的独立模型、音效和插件不属于主词库代次，保留原有入口。
            let original = msime_client_providers::dictionary_updates::bundled_resources(
                state_root,
                specification,
            )
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| resources.to_path_buf());
            for (key, name) in [
                ("sound_packs", "sound-packs"),
                ("plugin_packs", "plugin-packs"),
            ] {
                if let Some(path) = original
                    .parent()
                    .map(|p| p.join(name))
                    .filter(|p| p.is_dir())
                {
                    document[key] = json!(path);
                }
            }
            return Ok(serde_json::to_string_pretty(&document)?);
        }
    }
    // 记录的 resources 已属于下载代次时，回退仍需回到安装包目录。
    let bundled =
        msime_client_providers::dictionary_updates::bundled_resources(state_root, specification)
            .filter(|p| p.is_dir());
    prepare_shipped_host_configuration(
        bundled.as_deref().unwrap_or(resources),
        state_root,
        specification,
        ON_DEMAND_ARTIFACTS,
        edition,
    )
}

/// 状态目录里 `edition` 的偏好。还没有偏好文件时，先把此刻读到的偏好写成第一份文件（[`PreferencesStore::write_first_document`]），理由见 [`prepare_host_configuration_for_edition`]。必须在建用户词库代次之前调用：没有偏好文件时，状态目录有没有被准备过决定触屏键盘按新装还是按升级前的默认方案。
fn edition_preferences(
    state_root: &Path,
    edition: &'static Edition,
) -> Result<Preferences, Box<dyn std::error::Error>> {
    Ok(PreferencesStore::for_edition(state_root, edition)
        .write_first_document()?
        .preferences)
}

/// [`prepare_host_configuration_for_edition`] 按给定的锁文件和按需下载清单准备；测试借它在各平台上检查 macOS 的发货规则。
fn prepare_shipped_host_configuration(
    resources: &std::path::Path,
    state_root: &std::path::Path,
    specification: &ResourceSet,
    on_demand: &[&[&str]],
    edition: &'static Edition,
) -> Result<String, Box<dyn std::error::Error>> {
    let resources = without_verbatim_prefix(std::fs::canonicalize(resources)?);
    let state_root = std::path::absolute(state_root)?;
    verify_resources_once(&resources, specification, &state_root, on_demand)?;
    // 偏好要在建 `user/dictionaries` 之前读：没有偏好文件时，以前的版本准备过的状态目录按升级前的默认方案，新装按现在的默认方案，建了代次目录就分不清了。
    let preferences = edition_preferences(&state_root, edition)?;
    // 用户词库日志读不出是数据库时，代次准备会把它改名挪开、从空日志开始，否则回放每次都失败、宿主永远起不来。在这里先挪，好经诊断出口报出来；准备代次时再查一遍只是空操作。
    let journal = state_root
        .join("user")
        .join(msime_engine::assets::USER_JOURNAL);
    if let Some(moved) = msime_engine::quarantine_corrupt_journal(&journal)? {
        crate::diagnostics::report(
            "user dictionary journal moved aside",
            &format!(
                "{} is not a database; learning starts again from an empty journal",
                moved.display()
            ),
        );
    }
    // 代次按本版本的方案准备：不含全拼、双拼、五笔的版本（日文、越南文、藏文）随包不带 msime-pinyin.db，代次里也没有它；full 的方案是全部，与以前相同。
    let prepared = msime_engine::host::prepare_options_for(
        resources.to_str().ok_or("non-UTF-8 resource path")?,
        state_root
            .join("user")
            .to_str()
            .ok_or("non-UTF-8 state path")?,
        state_root
            .join("cache")
            .to_str()
            .ok_or("non-UTF-8 cache path")?,
        // 代次按完整锁文件计算，不随发布包是否内置日文词典而变：user/dictionaries/<generation> 和 refreshed_host_options 都保持原样，升级后不会重新准备，仍在运行的旧版输入法也不会。
        &specification.generation()?,
        engine_schemes(edition),
    )?;
    // 先记下状态目录属于哪个版本：之后只拿到这个目录的偏好存储（各平台的 C ABI、设置应用）靠它取本版本的默认偏好。full 什么也不写。
    edition.record_in(&state_root)?;
    Ok(serde_json::to_string_pretty(&HostOptions {
        api_version: 1,
        resources: prepared.resources,
        user_data: prepared.user_data,
        cache: prepared.cache,
        dictionaries: prepared.dictionaries,
        preferences,
        preferences_directory: Some(
            state_root
                .to_str()
                .ok_or("non-UTF-8 state path")?
                .to_owned(),
        ),
        phrase_preedit: None,
        quick_phrase_candidates: None,
        clipboard_history_path: None,
        online_provider_socket: None,
        translation_provider_socket: None,
        voice_provider_socket: None,
        sound_packs: None,
        plugin_packs: None,
        edition: (!edition.is_full()).then_some(edition),
    })?)
}

/// A runtime options refresh found that the recorded resource directory does not hold the resource set this build pins.
///
/// This is the state a user who downloaded the dictionaries (rather than getting them from the package) is left in after an upgrade that raised the dictionary version: the package replaced the lock but nothing replaced the files. It is told apart from every other refresh failure because only this one has a fix the user can run, `msime-linux-setup --update --download`. The C ABI passes errors through as their `Display` text, so the stable part of the contract is the `dictionary_outdated:` prefix; what follows it is diagnostic and may name private paths.
#[derive(Debug)]
pub struct DictionaryOutdated(msime_client_core::resources::ResourceError);

impl std::fmt::Display for DictionaryOutdated {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{DICTIONARY_OUTDATED_PREFIX} {}", self.0)
    }
}

impl std::error::Error for DictionaryOutdated {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// How a [`DictionaryOutdated`] error begins once it has crossed the C ABI as text.
pub const DICTIONARY_OUTDATED_PREFIX: &str = "dictionary_outdated:";

/// Turn a verification failure out of `prepare_host_configuration` into [`DictionaryOutdated`], leaving every other error as it was. Only a length, digest or directory-content mismatch counts: an unreadable file or an invalid compiled lock is not something a download fixes.
fn outdated_resources(error: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
    use msime_client_core::resources::ResourceError;
    match error.downcast::<ResourceError>() {
        Ok(resource) => match *resource {
            outdated @ (ResourceError::Integrity | ResourceError::ExistingGeneration(_)) => {
                Box::new(DictionaryOutdated(outdated))
            }
            other => Box::new(other),
        },
        Err(error) => error,
    }
}

fn reject_symlinked_options_parent(path: &Path) -> std::io::Result<()> {
    let mut current = path.parent();
    while let Some(candidate) = current {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if msime_path_trust::is_trusted_system_alias(candidate) {
                    current = candidate.parent();
                    continue;
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "options path has a symbolic-link parent",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        current = candidate.parent();
    }
    Ok(())
}

/// Bring a published HostOptions file up to the installed dictionary generation.
///
/// A package upgrade replaces the resource bundle in place but leaves each user's options pointing at working dictionaries copied from the previous bundle, so the new dictionary never reaches the Engine and the user-dictionary replay the Windows installer runs after an upgrade never happens. When the recorded dictionaries directory is not the generation the installed lock describes, this prepares that generation (the Engine copies the new dictionaries and replays the user journal into them) and rewrites only `resources` and `dictionaries`, keeping every other key a setup or the settings app wrote. A current file is only read.
///
/// Returns whether the file was rewritten. Run it before the caller's own sessions exist. The previous generation is never modified, so a host still using it keeps working until it restarts. A symlink, or a document whose paths do not follow the layout `prepare_host_configuration` produces, is left alone rather than guessed at. When the recorded resources do not match the compiled lock the error is [`DictionaryOutdated`] and the file is left as it was.
pub fn refresh_host_options(path: &std::path::Path) -> Result<bool, Box<dyn std::error::Error>> {
    refresh_options_file(path, None)
}

/// 给自带一份已校验资源的宿主用的 [`refresh_host_options`]（macOS 设置应用 bundle 里的 `EngineResources` 就是这样一份）：记录的资源目录与编译进来的词库锁不符（[`DictionaryOutdated`]）时，改用 `bundled` 准备新代次，此后 `resources` 指向它。
pub fn refresh_host_options_from(
    path: &std::path::Path,
    bundled: &std::path::Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    refresh_options_file(path, Some(bundled))
}

fn refresh_options_file(
    path: &std::path::Path,
    bundled: Option<&std::path::Path>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("runtime options have no directory")?;
    let name = path
        .file_name()
        .ok_or("runtime options have no file name")?;
    let directory = msime_client_core::file_lock::open_private_directory(parent)?;
    reject_symlinked_options_parent(path)?;
    let locator = std::fs::symlink_metadata(path)?;
    if locator.file_type().is_symlink() {
        return Ok(false);
    }
    let file = msime_client_core::file_lock::open_private_file_at(&directory, name)?;
    let metadata = file.metadata()?;
    // The file can be replaced or grow after symlink_metadata returns. Read through a
    // limit-aware handle so that the size check remains effective across that race.
    let bytes =
        crate::bounded_file::read(file, HOST_OPTIONS_DOCUMENT_LIMIT as u64).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                Box::<dyn std::error::Error>::from("runtime options exceed 1 MiB")
            } else {
                Box::<dyn std::error::Error>::from(error)
            }
        })?;
    let document: Value = serde_json::from_slice(&bytes)?;
    // 文档记录的版本，缺省是 full。新代次按同一个版本准备；`refreshed_layout` 只替换 `resources` 和 `dictionaries`，`edition` 键原样保留。
    let edition =
        Edition::of_host_options(&document).ok_or("runtime options name an unknown edition")?;
    // 已安装的代次要和 `prepare_host_configuration_for_edition` 用同一份锁比较，否则不是 full 的版本每次刷新都会被当成过期。
    let baseline = edition.resource_set()?;
    let state = document["preferences_directory"].as_str().map(Path::new);
    let updated = state
        .and_then(|state| dictionary_updates::selected_resources(state, &baseline))
        .filter(|(path, set)| {
            state.is_some_and(|state| verify_resources_once(path, set, state, &[]).is_ok())
        });
    let specification = updated.as_ref().map(|(_, set)| set).unwrap_or(&baseline);
    let fallback = state.and_then(|state| {
        msime_client_providers::dictionary_updates::bundled_resources(state, &baseline)
    });
    let Some(refreshed) = refreshed_host_options(
        &document,
        &specification.generation()?,
        updated
            .as_ref()
            .map(|(path, _)| path.as_path())
            .or(bundled)
            .or(fallback.as_deref()),
        Path::is_dir,
        |resources, state| {
            Ok(serde_json::from_str(
                &prepare_host_configuration_for_edition(resources, state, edition)
                    .map_err(outdated_resources)?,
            )?)
        },
    )?
    else {
        return Ok(false);
    };
    replace_options_file(&directory, name, &metadata, &refreshed)?;
    Ok(true)
}

/// 用已打开的父目录句柄原子替换配置文件，保留原有权限。
fn replace_options_file(
    directory: &msime_client_core::file_lock::PrivateDirectory,
    name: &std::ffi::OsStr,
    metadata: &std::fs::Metadata,
    document: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut serialized = serde_json::to_vec_pretty(document)?;
    serialized.push(b'\n');
    msime_client_core::file_lock::replace_private_file_at(
        directory,
        name,
        &serialized,
        &metadata.permissions(),
    )?;
    Ok(())
}

/// 把输入法进程这次启动准备好的词库代次写进已发布的 `runtime-options.json`，只改 `dictionaries` 一个键；文件已经是这个代次时只读不写。返回是否改写了文件。
///
/// Windows 的 Server 每次启动都重新准备宿主（`msime_client_prepare_host`），自己的会话用准备出的文档；`runtime-options.json` 却只在首次运行时写下（`FirstRun.h`）。升级换了词库锁、或者装上了在线词库更新之后，Server 的会话读新代次，而照这份文件工作的设置窗口、`msime-mcp` 和各应用进程里的 TIP 还在旧代次上：设置里新增的快捷短语和五笔词条写进旧代次，Server 要等下次启动回放日志才看得到，用户看到的就是「加了没用」（#9434）。
///
/// 只改 `dictionaries`，不动 `resources`：在线词库更新的资源目录在状态目录下，而 Server 启动时从这份文件读 `resources` 并要求它与状态目录不相交，改了会让 Server 下次起不来。词库新增、编辑和查找都只经过代次里的工作副本，`resources` 留在安装包目录不影响它们。两份文档都必须是准备布局（[`prepared_layout`]）且属于同一个状态目录，否则原样不动；写入的路径沿用文件自己的拼写，只换代次目录名。
pub fn publish_prepared_dictionaries(
    path: &Path,
    prepared: &Value,
) -> Result<bool, Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("runtime options have no directory")?;
    let name = path
        .file_name()
        .ok_or("runtime options have no file name")?;
    let directory = msime_client_core::file_lock::open_private_directory(parent)?;
    reject_symlinked_options_parent(path)?;
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Ok(false);
    }
    let file = msime_client_core::file_lock::open_private_file_at(&directory, name)?;
    let metadata = file.metadata()?;
    let bytes = crate::bounded_file::read(file, HOST_OPTIONS_DOCUMENT_LIMIT as u64)?;
    let mut document: Value = serde_json::from_slice(&bytes)?;
    let (Some((_, published, state)), Some((_, generation, prepared_state))) =
        (prepared_layout(&document), prepared_layout(prepared))
    else {
        return Ok(false);
    };
    // 同一个状态目录可以写成不同的字面路径（Windows 的盘符大小写、短文件名），按规范化后的路径比较；文件记的状态目录已经不在，就不是这次准备的宿主的文件。
    let same_state = std::fs::canonicalize(state).is_ok_and(|state| {
        std::fs::canonicalize(prepared_state).is_ok_and(|prepared| state == prepared)
    });
    if !same_state {
        return Ok(false);
    }
    // 两份文档的拼写可能不同，只取代次目录名，接在文件自己的 `user_data/dictionaries` 下：照搬准备出的整条路径会让 `dictionaries` 与文件里的 `user_data` 拼写不一致，文件从此不再是准备布局，之后的发布和刷新都会跳过它。
    let (Some(current), Some(generation_name)) = (published.parent(), generation.file_name())
    else {
        return Ok(false);
    };
    if published.file_name() == Some(generation_name) {
        return Ok(false);
    }
    document["dictionaries"] = Value::String(
        current
            .join(generation_name)
            .to_str()
            .ok_or("dictionary generation path is not UTF-8")?
            .to_owned(),
    );
    replace_options_file(&directory, name, &metadata, &document)?;
    Ok(true)
}

/// 随包资源所在的目录：安装包里 Engine 资源目录的上一级，设置应用在这里找背单词的内置词书（`wordbooks/`）。
///
/// 词库在线更新后，准备出的文档里 `resources` 指向状态目录下的下载代次，那里没有随包资源；这时取更新记录里的原安装包目录，与 [`prepare_host_with_updates`] 给音效包和插件包定位、[`with_installed_language_dictionaries`] 找语言词库的规则相同。文档不是准备布局、或没有可用的更新记录时用 `resources` 本身；`resources` 不是绝对路径时为 `None`。
pub fn bundled_resource_root(document: &Value) -> Option<PathBuf> {
    let resources = document
        .get("resources")
        .and_then(Value::as_str)
        .map(Path::new)
        .filter(|resources| resources.is_absolute())?;
    let bundled = prepared_layout(document)
        .and_then(|(_, _, state)| {
            let baseline = Edition::of_host_options(document)?.resource_set().ok()?;
            msime_client_providers::dictionary_updates::bundled_resources(state, &baseline)
        })
        .filter(|bundled| bundled.is_dir());
    bundled
        .as_deref()
        .unwrap_or(resources)
        .parent()
        .map(Path::to_path_buf)
}

/// The `resources`, `dictionaries` and state directory of a document in the layout `prepare_host_configuration` produces, or `None` for any other document.
fn prepared_layout(document: &Value) -> Option<(&Path, &Path, &Path)> {
    let path = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(Path::new)
            .filter(|path| path.is_absolute())
    };
    let (resources, user_data, dictionaries, state) = (
        path("resources")?,
        path("user_data")?,
        path("dictionaries")?,
        path("preferences_directory")?,
    );
    (user_data == state.join("user")
        && dictionaries.parent() == Some(user_data.join("dictionaries").as_path()))
    .then_some((resources, dictionaries, state))
}

/// `refresh_host_options` 会发布的配置；文件已是当前代次或不符合准备布局时为 `None`。记录的资源目录是 [`DictionaryOutdated`]、而宿主自带 `bundled` 资源时，改用它准备代次。
fn refreshed_host_options(
    document: &Value,
    generation: &str,
    bundled: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
    mut prepare: impl FnMut(&Path, &Path) -> Result<Value, Box<dyn std::error::Error>>,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let Some((resources, dictionaries, state)) = prepared_layout(document) else {
        return Ok(None);
    };
    // 记录的资源目录已经不存在，最常见的是用户把设置应用挪了位置（例如从 /Applications 挪到 ~/Applications）：代次没变，只看代次就会让 `resources` 一直指向不存在的旧路径，下一次开会话就找不到词库。带着 bundle 刷新时改从 bundle 准备，不论代次是否变化。
    if let Some(bundled) = bundled.filter(|bundled| *bundled != resources && !exists(resources)) {
        return refreshed_layout(document, prepare(bundled, state)?).map(Some);
    }
    if dictionaries.file_name().and_then(|name| name.to_str()) == Some(generation) {
        return Ok(None);
    }
    let prepared = match (prepare(resources, state), bundled) {
        (Err(error), Some(bundled)) if error.is::<DictionaryOutdated>() && bundled != resources => {
            prepare(bundled, state)?
        }
        (prepared, _) => prepared?,
    };
    refreshed_layout(document, prepared).map(Some)
}

/// `document` with `resources` and `dictionaries` taken from what `prepare_host_configuration` returned.
fn refreshed_layout(
    document: &Value,
    prepared: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut refreshed = document.clone();
    for key in ["resources", "dictionaries"] {
        refreshed[key] = prepared
            .get(key)
            .filter(|value| value.is_string())
            .cloned()
            .ok_or("prepared options are incomplete")?;
    }
    for key in ["sound_packs", "plugin_packs"] {
        if refreshed.get(key).is_none() {
            if let Some(value) = prepared.get(key) {
                refreshed[key] = value.clone();
            }
        }
    }
    Ok(refreshed)
}

/// The reason prefix for an entry this layer or the Engine refuses on its own terms. Callers map it to one error code, so it stays stable while the part after the colon says which rule failed.
pub const INVALID_DICTIONARY_ENTRY: &str = "invalid dictionary entry";

pub(crate) fn invalid_dictionary_entry(reason: &str) -> String {
    format!("{INVALID_DICTIONARY_ENTRY}: {reason}")
}

/// 本版本没有中文词库时编辑拼音、五笔或快捷短语的拒绝原因。
pub(crate) const NO_CHINESE_DICTIONARY: &str =
    "this edition of the input method has no Chinese dictionary; only English words can be edited";

/// `kind` 的词能不能在 `options` 的版本里编辑。不含全拼、双拼、五笔的版本（日文、越南文、藏文）随包不带 msime-pinyin.db，拼音、五笔和快捷短语都无处可存，只有英文词能编辑（`SchemeSet::reads_main_dictionary`）。在加锁之前就拒绝，把原因告诉调用方，而不是让 Engine 拒绝后报成笼统的「dictionary edit rejected」。
pub(crate) fn require_dictionary_kind(
    options: &EngineOptions,
    kind: msime_engine::host::DictionaryKind,
) -> Result<(), String> {
    if kind == msime_engine::host::DictionaryKind::English
        || options.enabled_schemes.reads_main_dictionary()
    {
        Ok(())
    } else {
        Err(NO_CHINESE_DICTIONARY.into())
    }
}

/// Edit only after every participating host has destroyed its sessions.
/// Busy is retryable without cancelling any composition. The host must recreate
/// sessions after success; no native/Tauri management command is exposed yet.
///
/// An entry the Engine refuses is reported as `invalid dictionary entry: <reason>` with the Engine's reason, checked before anything is locked. Those reasons are fixed sentences in `validate_personal_dictionary_entry` that never repeat the submitted entry; collapsing them into one generic "rejected" left the settings page telling the user to retry an entry that could never be saved. Every other Engine diagnostic is still withheld, since it may include user text.
pub fn edit_personal_dictionary(
    options: &EngineOptions,
    previous: Option<&msime_engine::host::DictionaryEntry>,
    replacement: Option<&msime_engine::host::DictionaryEntry>,
    request_id: &str,
) -> Result<(), String> {
    validate_personal_dictionary_edit(options, previous, replacement, request_id)?;
    let access = DictionaryAccess::try_maintenance(
        std::path::Path::new(&options.user_data),
        std::path::Path::new(&options.dictionaries),
    )
    .map_err(|_| "dictionary access unavailable")?
    .ok_or("dictionary maintenance busy")?;
    edit_personal_dictionary_with_access(&access, options, previous, replacement, request_id)
}

/// 在调用方持有维护租约时校验并应用一个词条改动。
/// 租约按借用传入，让有界批次共用一个独占窗口，不必为每个待处理词条重新取锁。
pub(crate) fn edit_personal_dictionary_with_access(
    _access: &DictionaryAccess,
    options: &EngineOptions,
    previous: Option<&msime_engine::host::DictionaryEntry>,
    replacement: Option<&msime_engine::host::DictionaryEntry>,
    request_id: &str,
) -> Result<(), String> {
    validate_personal_dictionary_edit(options, previous, replacement, request_id)?;
    msime_engine::host::dictionary_edit(options, previous, replacement, request_id)
        .map_err(|_| "dictionary edit rejected".into())
}

fn validate_personal_dictionary_edit(
    options: &EngineOptions,
    previous: Option<&msime_engine::host::DictionaryEntry>,
    replacement: Option<&msime_engine::host::DictionaryEntry>,
    request_id: &str,
) -> Result<(), String> {
    for entry in previous.iter().chain(replacement.iter()) {
        require_dictionary_kind(options, entry.kind)?;
    }
    // The previous row is one the list returned, whose weight learning may have lifted past the ceiling a new entry is held to.
    for entry in previous.iter() {
        msime_engine::host::dictionary_validate_previous(entry)
            .map_err(|error| invalid_dictionary_entry(&error.to_string()))?;
    }
    for entry in replacement.iter() {
        msime_engine::host::dictionary_validate(entry)
            .map_err(|error| invalid_dictionary_entry(&error.to_string()))?;
    }
    if request_id.is_empty() {
        return Err("dictionary request id required".into());
    }
    Ok(())
}

/// A host-owned view of one entry in the verified local Emoji catalog.
#[derive(Clone, Debug, Serialize)]
pub struct LocalEmojiCatalogItem {
    pub text: String,
    pub annotation: String,
    pub group: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalEmojiCatalogSlice {
    pub items: Vec<LocalEmojiCatalogItem>,
    pub next_offset: usize,
    pub complete: bool,
}

/// Read catalog rows without collapsing equal text from distinct categories.
// Not unix-gated: the bodies only call the engine, which builds on Windows too. The gate was a porting gap, and it left the Windows desktop falling back to the compact built-in catalog - 97 emoji against the several thousand rows in msime-others.db - behind a permanent "catalog failed to load" banner.
pub fn local_emoji_catalog_slice(
    resources: &str,
    category: &str,
    offset: usize,
    limit: u16,
) -> Result<LocalEmojiCatalogSlice, &'static str> {
    if !std::path::Path::new(resources).is_absolute() {
        return Err("resources path must be absolute");
    }
    if limit == 0 || limit > 4096 {
        return Err("invalid local emoji page size");
    }
    msime_engine::host::emoji_catalog_slice(resources, "", category, "", offset, limit, "")
        .map(|page| LocalEmojiCatalogSlice {
            items: page
                .items
                .into_iter()
                .map(|item| LocalEmojiCatalogItem {
                    text: item.text,
                    annotation: item.annotation,
                    group: item.group,
                })
                .collect(),
            next_offset: page.next_offset,
            complete: page.complete,
        })
        .map_err(|_| "local emoji catalog unavailable")
}

pub use msime_client_services::plugins::symbol_set::PluginSymbolGroup;

/// 插件目录 `root` 下已安装的符号集的全部组，供宿主追加到内置符号目录之后：`symbols` 组放在以 `pack_name` 为上级分类的分组下，`kaomoji` 组放在颜文字的 All 之后。读几个小清单：不要在按键路径上调用。
pub fn plugin_symbol_groups(root: &std::path::Path) -> Vec<PluginSymbolGroup> {
    msime_client_services::plugins::symbol_set::plugin_symbol_groups(root)
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalSymbolCatalogGroup {
    pub parent: String,
    pub title: String,
    pub items: Vec<LocalEmojiCatalogItem>,
}

/// Preserve Engine-owned symbol parent categories and subgroup order.
pub fn local_symbol_catalog(resources: &str) -> Result<Vec<LocalSymbolCatalogGroup>, &'static str> {
    if !std::path::Path::new(resources).is_absolute() {
        return Err("resources path must be absolute");
    }
    let groups = msime_engine::host::emoji_symbol_groups(resources)
        .map_err(|_| "local symbol catalog unavailable")?;
    let mut result = Vec::with_capacity(groups.len());
    let mut remaining_pages = 256usize;
    for group in groups {
        let mut items = Vec::new();
        let mut first_page = true;
        let mut offset = 0usize;
        loop {
            if remaining_pages == 0 {
                return Err("local symbol catalog exceeds limit");
            }
            remaining_pages -= 1;
            let page = msime_engine::host::emoji_catalog_slice(
                resources,
                "",
                "symbols",
                &group.title,
                offset,
                512,
                &group.parent,
            )
            .map_err(|_| "local symbol catalog unavailable")?;
            if first_page {
                // The engine exposes pages rather than a total count. Use the first
                // page's actual item count as the only reliable initial capacity.
                items = Vec::with_capacity(page.items.len());
                first_page = false;
            }
            let complete = page.complete;
            let next_offset = page.next_offset;
            items.extend(page.items.into_iter().map(|item| LocalEmojiCatalogItem {
                text: item.text,
                annotation: item.annotation,
                group: item.group,
            }));
            if complete {
                break;
            }
            if next_offset <= offset {
                return Err("local symbol catalog cursor did not advance");
            }
            offset = next_offset;
        }
        result.push(LocalSymbolCatalogGroup {
            parent: group.parent,
            title: group.title,
            items,
        });
    }
    Ok(result)
}

fn response(operation: impl FnOnce() -> Result<Value, String>) -> *mut c_char {
    let value = match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => json!({ "ok": true, "value": value }),
        Ok(Err(error)) => json!({ "ok": false, "error": error }),
        Err(_) => json!({ "ok": false, "error": "internal runtime failure" }),
    };
    // JSON escapes embedded NUL bytes, so this cannot contain an interior NUL.
    CString::new(value.to_string())
        .expect("JSON contains no NUL")
        .into_raw()
}

fn with_session<T>(
    handle: u64,
    action: impl FnOnce(&mut HostSession) -> Result<T, String>,
) -> Result<T, String> {
    SESSIONS.with(|sessions| {
        let mut sessions = sessions
            .try_borrow_mut()
            .map_err(|_| "reentrant host call")?;
        let runtime = sessions
            .get_mut(&handle)
            .ok_or("unknown session or wrong thread")?;
        action(runtime)
    })
}

fn dispatch(handle: u64, action: Action) -> *mut c_char {
    dispatch_call(handle, action, None)
}

/// 派发一个动作。`host` 只对命令 19、20 有值：动作同 Cancel、Finish，生命周期里的类别是 `HostCommit`（结束原因记 HostCommit）或 `FocusLoss`（上屏来源记 FocusLoss，不出声不画）。
///
/// 选择加入生命周期的会话按这个顺序：调用前采样并发 `input` → `runtime.dispatch`（脚本的 `candidates` 在里面的 refresh 和展开之后）→ 统计 → 读派发事实 → `complete_transition`（全角转换、再中英文之间加空格都在这里，之后才是宿主拿到的字节，宿主再做简繁转换）→ `before_commit`（脚本可以改写 body）→ 其余事件和内置消费者 → 序列化，FEEDBACK 开着时插入 `feedback`。没选择加入的会话只走原来的几步。
fn dispatch_call(handle: u64, action: Action, host: Option<lifecycle::InputClass>) -> *mut c_char {
    response(|| dispatch_value(handle, action, host))
}

/// [`dispatch_call`] 的本体，返回值不经 C 字符串，`candidate_preview` 在本进程里打字时直接用它。
fn dispatch_value(
    handle: u64,
    action: Action,
    host: Option<lifecycle::InputClass>,
) -> Result<Value, String> {
    let (result, feedback) = with_session(handle, |session| {
        dispatch_session_transition(session, action, host)
    })?;
    transition_to_value(result, feedback)
}

pub(crate) fn transition_to_value(
    result: Transition,
    feedback: Option<u32>,
) -> Result<Value, String> {
    let mut value = serde_json::to_value(result).map_err(|e| e.to_string())?;
    if let (Some(effect), Value::Object(fields)) = (feedback, &mut value) {
        fields.insert("feedback".into(), json!({ "typing_effect": effect }));
    }
    Ok(value)
}

/// 零拷贝快照与 JSON 派发共用的核心会话推进逻辑。
pub(crate) fn dispatch_session_transition(
    session: &mut HostSession,
    action: Action,
    host: Option<lifecycle::InputClass>,
) -> Result<(Transition, Option<u32>), String> {
    // 大写锁定改道先于其余处理，后面看到的都是改道后的动作。
    let action = session.caps_lock_punctuation(action);
    let action = session.runtime.candidate_shortcut(action);
    #[cfg(unix)]
    if matches!(
        action,
        Action::SelectHighlighted | Action::Command(Command::CommitCandidate) | Action::Finish
    ) && session.neural_v2_active()
    {
        if let Some((target_gen, remaining)) = session.runtime.provisional_remaining(Instant::now())
        {
            if !remaining.is_zero() {
                let response = session.neural_client.as_ref().and_then(|client| {
                    client.wait_for_decode_generation(
                        session.runtime.session_id(),
                        target_gen,
                        remaining,
                    )
                });
                if let Some(resp) = response {
                    let _ = session.runtime.apply_neural_response(&resp, false);
                }
            }
            session.runtime.mark_presented(target_gen);
        }
    }
    let prev_comp_epoch = session.runtime.composition_epoch();
    let currency_punctuation = matches!(
        action,
        Action::Character { value: b'$', .. } | Action::Punctuation(b'$')
    );
    let pending = session.lifecycle_input(&action, host);
    // Which row the user reached for, read before dispatching because the view it is
    // relative to is gone afterwards. Every platform host routes candidate selection
    // through here, so counting it here covers all of them without a line of platform
    // code; doing it per host would have meant six chances to forget.
    let position = selected_position(&action);
    let efficiency = position.and_then(|_| session.efficiency_candidate(&action));
    let result = session.runtime.dispatch(action).map_err(|e| e.to_string());
    // 报错前 runtime 可能已经改了组字，生命周期照样收尾。
    let mut result = session.lifecycle_on_error(pending, result)?;
    // Text the Engine generated (a calculator result, a command, a mention) was picked, not typed, and stays out of the statistics the way it stays out of learning.
    let counts_as_typing = result
        .commit_context
        .as_ref()
        .is_none_or(|context| context.typing_statistics);
    if result.commit.is_some() && counts_as_typing {
        // 先记效率再记位置：位置攒满一批会立刻写盘并带走待写的效率，这一次的效率要赶上同一批，否则进程在下一批之前被杀就丢了。
        if let Some(candidate) = efficiency {
            session.count_efficiency(candidate);
        }
        if let Some(position) = position {
            session.count_selection(position);
        }
    }
    let facts = session.runtime.facts();
    // 固定标点表返回全角人民币；半角只改本次 `$` 产生的尾巴，候选和短语里的符号保持原文。
    if currency_punctuation && result.view.character_width == CharacterWidth::Halfwidth {
        if let Some(commit) = result.commit.as_mut() {
            if facts.tail_chars == 1 && commit.ends_with('￥') {
                commit.truncate(commit.len() - '￥'.len_utf8());
                commit.push('¥');
            }
        }
    }
    let mut result = session.complete_transition(result);
    // 脚本插件可能在这里改写上屏的 body（`before_commit`），宿主拿到的是改写之后的文本；引擎学习和 AI 上文已经按原文记下。
    let feedback = session.lifecycle_after(pending, facts, &mut result);
    #[cfg(unix)]
    if session.neural_v2_active() {
        let session_id = session.runtime.session_id();
        let post_comp_epoch = session.runtime.composition_epoch();
        let had_commit = result
            .commit
            .as_deref()
            .is_some_and(|commit| !commit.is_empty());
        if !had_commit
            && post_comp_epoch != prev_comp_epoch
            && !result.view.editing_text.is_empty()
            && result.view.page == 0
            && !result.view.prediction
            && !result.view.dedicated_english
            && result.view.local_mode == "none"
            && result.view.conversion.is_empty()
            && !result.view.caret_edit_suffix
            && result
                .view
                .candidates
                .first()
                .is_some_and(|candidate| candidate.highlighted)
        {
            if let Some(req) = session.runtime.take_neural_request() {
                let enqueued = session.neural_client.as_ref().is_some_and(|client| {
                    client.enqueue_decode(session_id, &req, NEURAL_PROVISIONAL_WINDOW)
                });
                if enqueued {
                    session
                        .runtime
                        .begin_provisional(req.generation, NEURAL_PROVISIONAL_WINDOW);
                    result.view.provisional = session.runtime.is_provisional();
                }
            }
        } else if had_commit && result.view.editing_text.is_empty() {
            session.schedule_neural_prefill_if_idle();
        } else if result.view.editing_text.is_empty() {
            if let Some(client) = session.neural_client.as_ref() {
                client.cancel_pending_decode_for_session(session_id);
            }
        }
    }
    Ok((result, feedback))
}

/// The one-based position of the candidate an action is about, or `None` when it is not about one.
///
/// Both selection actions carry the absolute candidate index from the runtime's generation, so
/// adding a view page offset would double-count candidates after the first page.
fn selected_position(action: &Action) -> Option<usize> {
    match action {
        // Candidate IDs carry the absolute index in the cached generation, even
        // when the view only shows one page. Adding the page offset here counted
        // every selection after the first page twice (and pushed it into the
        // aggregate `beyond` bucket in typing statistics).
        Action::Select(id) => Some(id.index + 1),
        Action::SelectAnyCandidate(id) => Some(id.index + 1),
        _ => None,
    }
}

/// 上屏效率（少按键、联想、整句）只在 Android 上计：只有 Android 的统计页显示它，其他宿主不为它多查读音。测试里也打开，好覆盖计法。
const COUNTS_COMMIT_EFFICIENCY: bool = cfg!(any(target_os = "android", test));

/// Android 宿主在隐私模式和不学习的输入框里、鸿蒙和 iOS 宿主在隐私模式里经 `msime_client_set_private_session` 标出隐私会话；这时选词位置和上屏效率都不计（鸿蒙和 iOS 本来就不计上屏效率）。其他宿主不变。
const PRIVATE_SESSIONS_SKIP_STATISTICS: bool = cfg!(any(
    target_os = "android",
    target_env = "ohos",
    target_os = "ios",
    test
));

/// 候选词翻译开着且范围是「仅当前高亮」：翻译查询只为高亮的候选请求候选词翻译，运行时只把它画在高亮行。
pub(crate) fn translation_scope_highlighted(preferences: &Preferences) -> bool {
    preferences.candidate_translations
        && preferences.candidate_translation_scope
            == msime_client_core::preferences::CandidateTranslationScope::Highlighted
}

/// `translation_query` 使用的偏好字段；这些变化都要撤销旧代次的释义回包。
fn translation_query_preferences_changed(previous: &Preferences, next: &Preferences) -> bool {
    previous.candidate_translations != next.candidate_translations
        || previous.candidate_translation_scope != next.candidate_translation_scope
        || previous.candidate_english_gloss != next.candidate_english_gloss
        || previous.candidate_pronunciation != next.candidate_pronunciation
        || previous.translation_target_language != next.translation_target_language
        || previous.translation_secondary_language != next.translation_secondary_language
        || previous.translation_account != next.translation_account
        || previous.custom_translation != next.custom_translation
        || previous.tencent_tmt != next.tencent_tmt
        || previous.niutrans != next.niutrans
}

/// 一个会话在两次写统计之间最多记下多少次上屏。正常情况下 `SELECTION_BATCH` 次选词就会写一次，这只是兜底。
const EFFICIENCY_BATCH_LIMIT: usize = 256;

/// 等后台写进统计的上屏批次，和是否已有一个线程在写。
struct EfficiencyQueue {
    pending: Vec<(EngineOptions, Vec<EfficiencyCandidate>)>,
    running: bool,
}

static EFFICIENCY_QUEUE: Mutex<EfficiencyQueue> = Mutex::new(EfficiencyQueue {
    pending: Vec::new(),
    running: false,
});

fn efficiency_queue() -> std::sync::MutexGuard<'static, EfficiencyQueue> {
    EFFICIENCY_QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 同一时间只有一个线程写效率：已有线程在写时，这一批排进队列由它接着写，不再每次写统计都另开一个线程、另开一次词库。线程在队列空了时才在同一把锁下退出，所以排进去的批次不会没人写。
fn record_efficiency_in_background(options: EngineOptions, commits: Vec<EfficiencyCandidate>) {
    {
        let mut queue = efficiency_queue();
        queue.pending.push((options, commits));
        if queue.running {
            return;
        }
        queue.running = true;
    }
    /// 写的时候 panic 也要放下 `running`，否则之后的效率再也没人写。正常退出在看到队列为空的同一把锁下放下，不经过这里。
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            if std::thread::panicking() {
                efficiency_queue().running = false;
            }
        }
    }
    let worker = move || {
        let _running = Running;
        loop {
            let batches = {
                let mut queue = efficiency_queue();
                if queue.pending.is_empty() {
                    queue.running = false;
                    return;
                }
                std::mem::take(&mut queue.pending)
            };
            for (options, commits) in batches {
                record_efficiency(&options, &commits);
            }
        }
    };
    if std::thread::Builder::new()
        .name("msime-typing-efficiency".into())
        .spawn(worker)
        .is_err()
    {
        // 开不了线程时批次留在队列里，下一次写统计再试。
        efficiency_queue().running = false;
    }
}

/// 把一批上屏算成效率计数写进统计：`typed_keys` 是候选输入码里的字母和数字，`spelled_keys` 是用拼音逐字打出这段文字要按的键数（按全拼数读音字母，双拼也以全拼为基准）；查不到读音（英文、表情、非拼音方案）时按打了多少算多少，不算少按也不算多按。尽力而为：写不进就丢掉。
fn record_efficiency(options: &EngineOptions, commits: &[EfficiencyCandidate]) {
    let directory = std::path::Path::new(&options.user_data);
    if !directory.is_absolute() {
        return;
    }
    let mut efficiency = CommitEfficiency::default();
    for commit in commits {
        let spelled_keys = spelling_keys(options, &commit.text).unwrap_or(commit.typed_keys);
        efficiency.count_commit(
            commit.typed_keys,
            spelled_keys,
            commit.sentence,
            commit.prediction,
        );
    }
    let _ = TypingStatisticsStore::new(directory).record_efficiency(&efficiency);
}

/// 用全拼打出 `text` 要按的键数，作为「少按键」的基准：只有拼音系方案（全拼含九键、双拼）有答案，双拼也按全拼计，这样统计页的「比全拼少按」对双拼用户才有意义。读音来自 Engine 的 `hanzi_to_pinyin`，它先查整词、再逐字回退。
fn spelling_keys(options: &EngineOptions, text: &str) -> Option<u64> {
    match SchemeType::from_u8(options.scheme)? {
        SchemeType::Quanpin | SchemeType::Shuangpin => {}
        _ => return None,
    }
    let reading = msime_engine::host::hanzi_to_pinyin(options, text);
    if reading.is_empty() {
        return None;
    }
    Some(
        reading
            .split('\'')
            .map(|syllable| syllable.bytes().filter(u8::is_ascii_alphabetic).count() as u64)
            .sum(),
    )
}

/// 派发选择前从视图里读出的那个候选。
struct EfficiencyCandidate {
    text: String,
    typed_keys: u64,
    sentence: bool,
    prediction: bool,
}

/// How many committing selections a session holds in memory before writing them to typing statistics.
///
/// Writing one means locking, reading and parsing the whole document, then an fsync (F_FULLFSYNC on Apple platforms) and a rename, all on the host's input thread, which cost a few milliseconds per tapped or clicked candidate. A session also writes what it holds on focus-out and on destroy, so this only bounds what a process killed mid-field loses - the iOS keyboard extension and the Android IME process can be killed without either. 32 selections is some tens of seconds of typing, a small loss for a statistic that only ever reports a rate, and it turns 32 writes into one.
const SELECTION_BATCH: u64 = 32;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod shurufa_tests;
