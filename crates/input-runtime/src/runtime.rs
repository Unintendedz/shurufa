//! `Runtime`: the orchestration between keystrokes, the engine, and everything the
//! providers return asynchronously.

use super::*;
use msime_engine::ordering::{apply_order_with, runner_up_order, CandidateOrderRows};
use msime_engine::{
    CandidateEntry, CandidateOrigin, CursorSnapshot, LockedSegment, PurePresenter, SchemeType,
    UnifiedScorer,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub enum Action {
    ResetCache,
    Character { value: u8, shift: bool },
    Punctuation(u8),
    PunctuationAscii(u8),
    Command(Command),
    SegmentBackspace,
    SegmentMoveLeft,
    SegmentMoveRight,
    Select(CandidateId),
    SelectAnyCandidate(CandidateId),
    SelectEdge(CandidateId, CandidateEdge),
    PinCandidate(CandidateId),
    RemoveCandidate(CandidateId),
    FixCandidatePosition(CandidateId, u8),
    ClearCandidatePosition(CandidateId),
    SelectHighlighted,
    Finish,
    FinishOnFocusLoss,
    NextPage,
    PreviousPage,
    NextCandidate,
    PreviousCandidate,
    FirstCandidate,
    LastCandidate,
    FirstCandidateOrHome,
}

pub struct Runtime<E: InputEngine = Session> {
    pub(crate) engine: E,
    pub(crate) session: u64,
    pub(crate) generation: u64,
    pub(crate) focused: bool,
    pub(crate) page_size: usize,
    pub(crate) highlighted: usize,
    pub(crate) translations: HashMap<String, String>,
    pub(crate) highlight_translations: HashMap<String, String>,
    pub(crate) translation_scope_highlighted: bool,
    pub(crate) display_page: Option<(u64, Vec<usize>)>,
    pub(crate) cached: EngineSnapshot,
    pub(crate) online_order_scratch: Vec<usize>,
    pub(crate) inline_emoji_projection: bool,
    pub(crate) emoji_mru: crate::emoji_mru::EmojiMruStore,
    pub(crate) snapshot_valid: bool,
    pub(crate) character_width: CharacterWidth,
    pub(crate) punctuation_marks: PunctuationMarks,
    pub(crate) touch_keyboard_layout: TouchKeyboardLayout,
    pub(crate) second_third_candidate:
        msime_client_core::preferences::SecondThirdCandidatePreferences,
    pub(crate) phrase_preedit: bool,
    pub(crate) backspace_reverts_selection: bool,
    pub(crate) escape_keeps_selected_word: bool,
    pub(crate) left_right_navigation: bool,
    pub(crate) vertical_candidates: bool,
    pub(crate) english_first_page_count: usize,
    /// 段内暂存前缀（单次会话内已选定但尚未整句上屏的词段），可直接拼入左上下文与退格拆回。
    pub(crate) locked_prefix: Vec<LockedSegment>,
    pub(crate) ai_context: String,
    /// 当前会话的前台应用粗粒度场景桶键名（仅保存 5 个静态枚举标签之一，绝不存原始 `bundleIdentifier`）。
    pub(crate) app_scope_key: &'static str,
    pub(crate) neural_v2_enabled: bool,
    pub(crate) neural_context: String,
    pub(crate) neural_right_context: String,
    pub(crate) neural_pending_request: Option<msime_engine::neural_v2::NeuralDecodeRequest>,
    pub(crate) neural_fusion_weights: msime_engine::neural_v2::NeuralFusionWeights,
    pub(crate) presentation_phase: PresentationPhase,
    pub(crate) facts: DispatchFacts,
    pub(crate) selected_text: String,
    pub(crate) composition_epoch: u64,
    pub(crate) candidates_epoch: u64,
}

fn scheme_type(scheme: u8) -> Option<SchemeType> {
    SchemeType::from_u8(scheme)
}

/// [`View::script_conversion`] for a scheme ordinal and local mode name.
pub(crate) fn script_conversion(scheme: u8, local_mode: &str) -> bool {
    scheme_type(scheme).is_some_and(SchemeType::script_conversion_applies)
        && local_mode != "unicode"
}

/// 「二三候选」键在当前方案里是否允许接管。
fn second_third_candidate_scheme(scheme: u8) -> bool {
    scheme_type(scheme).is_none_or(SchemeType::is_pinyin)
}

/// 微软双拼（及零声母编码用 `;` 的方案）把 `;` 当韵母 ing：光标前自上一个 `'` 起的片段长度为奇数时它是输入。
fn microsoft_shuangpin_final_position(editing: &[u8], caret: usize) -> bool {
    let caret = caret.min(editing.len());
    let mut start = caret;
    while start > 0 && editing[start - 1] != b'\'' {
        start -= 1;
    }
    (caret - start) % 2 == 1
}

#[cfg(test)]
pub(crate) fn move_to_back<T>(items: &mut [T], moved: &[bool]) {
    let mut head_len = 0;
    for index in 0..items.len() {
        if moved.get(index).copied().unwrap_or(false) {
            continue;
        }
        if head_len != index {
            items[head_len..=index].rotate_right(1);
        }
        head_len += 1;
    }
}

fn retain_candidate_translations(
    translations: &mut HashMap<String, String>,
    candidates: &[CandidateEntry],
) {
    if translations.is_empty() {
        return;
    }
    if candidates.is_empty() {
        translations.clear();
    } else if translations.len() <= 8 || candidates.len() <= 8 {
        translations.retain(|text, _| candidates.iter().any(|entry| entry.text == *text));
    } else {
        let candidates: HashSet<&str> =
            candidates.iter().map(|entry| entry.text.as_str()).collect();
        translations.retain(|text, _| candidates.contains(text.as_str()));
    }
}

fn order_rows(snapshot: &EngineSnapshot) -> CandidateOrderRows<'_> {
    CandidateOrderRows(&snapshot.candidates)
}

/// [`Runtime::apply_neural_response`] 的应用结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeuralApplyOutcome {
    /// 响应代次匹配且成功执行融合（含重排后无变化的合法回包）。
    pub applied: bool,
    /// 最终候选顺序或集合发生变化（调用方须重绘候选窗）。
    pub moved: bool,
    /// 响应处理完成后的当前候选代次。
    pub generation: u64,
    /// 本次融合是否锁定了槽 0（迟到、已移动高亮或首槽受保护）。
    pub slot0_locked: bool,
    /// 本次融合是否通过 Tier 1 受限束搜索独有救回了前排候选。
    pub tier1_rescued: bool,
}

/// 两阶段防闪与槽 0 锁定状态机。
/// - `Provisional`：首帧等待 `~28 ms` 无感窗口，宿主暂缓画候选面板且不锁槽 0；
/// - `Presented`：候选窗已实际呈现，后续迟到神经回包强制硬锁槽 0 仅升级槽 1..。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationPhase {
    Idle,
    Provisional { generation: u64, deadline: Instant },
    Presented { generation: u64 },
}

impl Runtime<Session> {
    /// 打开或关闭云候选固定席位；宿主偏好变化时立即刷新当前组字。
    pub fn set_cloud_candidates_enabled(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        if self.engine.cloud_candidates_enabled() == enabled {
            return Ok(());
        }
        self.advance()?;
        self.engine.set_cloud_candidates_enabled(enabled);
        self.refresh()
    }

    /// 英文在线授权的改变立即失效旧查询，并刷新已经显示的英文列表。
    pub fn set_english_online_candidates_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<(), RuntimeError> {
        if self.engine.english_online_candidates_enabled() == enabled {
            return Ok(());
        }
        self.advance()?;
        self.engine.set_english_online_candidates_enabled(enabled);
        self.refresh()
    }

    /// A live host mode changes neither composition nor candidate identity.
    pub fn set_chinese_punctuation_enabled(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        self.engine
            .set_chinese_punctuation_enabled(enabled)
            .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        self.refresh_idle_spelling_symbols()
    }

    pub fn online_query(&self) -> Result<Option<OnlineQuery>, RuntimeError> {
        let query = self
            .engine
            .online_query()
            .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        if !query.available {
            return Ok(None);
        }
        Ok(Some(OnlineQuery {
            mode: query.mode,
            scheme: query.scheme,
            generation: query.generation,
            identity: query.identity,
            query_text: query.query_text,
            cache_key: query.cache_key,
            pinyin_segments: query.pinyin_segments,
            cloud_eligible: query.cloud_eligible,
            ai_eligible: query.ai_eligible,
            cloud_candidates: true,
            cloud_request: None,
            session_id: query.session_id,
            ai_context: if query.ai_eligible {
                self.ai_context.clone()
            } else {
                String::new()
            },
            ai_assistant: None,
            ai_cache_only: false,
        }))
    }

    pub fn apply_online_candidate(
        &mut self,
        query: &OnlineQuery,
        candidate: &str,
        source: u8,
    ) -> Result<bool, RuntimeError> {
        let cloud_candidates = query.cloud_candidates;
        let query = OnlineQuerySnapshot {
            available: true,
            mode: query.mode,
            scheme: query.scheme,
            generation: query.generation,
            identity: query.identity.clone(),
            query_text: query.query_text.clone(),
            cache_key: query.cache_key.clone(),
            pinyin_segments: query.pinyin_segments.clone(),
            cloud_eligible: query.cloud_eligible,
            ai_eligible: query.ai_eligible,
            session_id: query.session_id,
        };
        self.apply_online_candidate_snapshot(query, candidate, source, cloud_candidates)
    }

    pub fn apply_online_candidate_owned(
        &mut self,
        query: OnlineQuery,
        candidate: &str,
        source: u8,
    ) -> Result<bool, RuntimeError> {
        let cloud_candidates = query.cloud_candidates;
        let OnlineQuery {
            mode,
            scheme,
            generation,
            identity,
            query_text,
            cache_key,
            pinyin_segments,
            cloud_eligible,
            ai_eligible,
            session_id,
            ..
        } = query;
        let query = OnlineQuerySnapshot {
            available: true,
            mode,
            scheme,
            generation,
            identity,
            query_text,
            cache_key,
            pinyin_segments,
            cloud_eligible,
            ai_eligible,
            session_id,
        };
        self.apply_online_candidate_snapshot(query, candidate, source, cloud_candidates)
    }

    fn apply_online_candidate_snapshot(
        &mut self,
        query: OnlineQuerySnapshot,
        candidate: &str,
        source: u8,
        cloud_candidates: bool,
    ) -> Result<bool, RuntimeError> {
        if candidate.is_empty()
            || !msime_client_core::is_bounded_text(candidate, 4096)
            || source > 1
            || (source == 0 && self.cached.candidates.is_empty())
            || (source == 0 && (!cloud_candidates || !query.cloud_eligible))
            || (source == 1 && !query.ai_eligible)
        {
            return Ok(false);
        }
        if self.generation == u64::MAX {
            return Err(RuntimeError::IdentityExhausted);
        }
        let applied = self
            .engine
            .apply_online_candidate(&query, candidate, source)
            .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        if applied {
            self.advance()?;
            self.refresh_online_candidates()
                .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        }
        Ok(applied)
    }

    pub fn apply_online_candidates(
        &mut self,
        query: &OnlineQuery,
        candidates: &[String],
        source: u8,
    ) -> Result<bool, RuntimeError> {
        let cloud_candidates = query.cloud_candidates;
        let limit = if source == 0 {
            1
        } else {
            query.ai_candidate_limit()
        };
        let query = OnlineQuerySnapshot {
            available: true,
            mode: query.mode,
            scheme: query.scheme,
            generation: query.generation,
            identity: query.identity.clone(),
            query_text: query.query_text.clone(),
            cache_key: query.cache_key.clone(),
            pinyin_segments: query.pinyin_segments.clone(),
            cloud_eligible: query.cloud_eligible,
            ai_eligible: query.ai_eligible,
            session_id: query.session_id,
        };
        self.apply_online_candidates_snapshot(query, candidates, source, cloud_candidates, limit)
    }

    pub fn apply_online_candidates_owned(
        &mut self,
        query: OnlineQuery,
        candidates: &[String],
        source: u8,
    ) -> Result<bool, RuntimeError> {
        let cloud_candidates = query.cloud_candidates;
        let limit = if source == 0 {
            1
        } else {
            query.ai_candidate_limit()
        };
        let OnlineQuery {
            mode,
            scheme,
            generation,
            identity,
            query_text,
            cache_key,
            pinyin_segments,
            cloud_eligible,
            ai_eligible,
            session_id,
            ..
        } = query;
        let query = OnlineQuerySnapshot {
            available: true,
            mode,
            scheme,
            generation,
            identity,
            query_text,
            cache_key,
            pinyin_segments,
            cloud_eligible,
            ai_eligible,
            session_id,
        };
        self.apply_online_candidates_snapshot(query, candidates, source, cloud_candidates, limit)
    }

    pub fn clear_online_candidates(&mut self, source: u8) -> Result<(), RuntimeError> {
        if source > 1 {
            return Err(RuntimeError::Engine(
                "invalid online candidate source".into(),
            ));
        }
        if self.generation == u64::MAX {
            return Err(RuntimeError::IdentityExhausted);
        }
        self.engine
            .clear_online_candidates(source)
            .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        self.advance()?;
        self.refresh_online_candidates()
    }

    fn apply_online_candidates_snapshot(
        &mut self,
        query: OnlineQuerySnapshot,
        candidates: &[String],
        source: u8,
        cloud_candidates: bool,
        limit: usize,
    ) -> Result<bool, RuntimeError> {
        if candidates.is_empty()
            || candidates.len() > limit
            || candidates
                .iter()
                .any(|text| text.is_empty() || !msime_client_core::is_bounded_text(text, 4096))
            || source > 1
            || (source == 0 && self.cached.candidates.is_empty())
            || (source == 0 && (!cloud_candidates || !query.cloud_eligible))
            || (source == 1 && !query.ai_eligible)
        {
            return Ok(false);
        }
        if self.generation == u64::MAX {
            return Err(RuntimeError::IdentityExhausted);
        }
        let applied = self
            .engine
            .apply_online_candidates(&query, candidates, source)
            .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        if applied {
            self.advance()?;
            self.refresh_online_candidates()
                .map_err(|error| RuntimeError::Engine(error.to_string()))?;
        }
        Ok(applied)
    }
}

impl<E: InputEngine> Runtime<E> {
    /// 返回当前会话内已锁定的分段前缀切片。
    pub fn locked_prefix(&self) -> &[LockedSegment] {
        &self.locked_prefix
    }

    /// 拼接当前段内暂存前缀文本。
    pub fn locked_prefix_text(&self) -> String {
        let cap: usize = self.locked_prefix.iter().map(|seg| seg.text.len()).sum();
        let mut out = String::with_capacity(cap);
        for seg in &self.locked_prefix {
            out.push_str(&seg.text);
        }
        out
    }

    /// 构造单向候选管线的不可变输入现场快照 [`CursorSnapshot`]。
    pub fn cursor_snapshot(&self) -> CursorSnapshot {
        CursorSnapshot {
            generation: self.generation,
            scheme: self.cached.scheme,
            raw_input: self.cached.editing_text.clone(),
            caret: self.cached.caret_position,
            locked_prefix: self.locked_prefix.clone(),
            left_context: self.neural_context.clone(),
            right_context: self.neural_right_context.clone(),
            script_conversion: script_conversion(self.cached.scheme, &self.cached.local_mode),
            emoji_enabled: self.inline_emoji_projection,
            english_autocomplete_enabled: !self.cached.dedicated_english,
            page_size: u8::try_from(self.page_size).unwrap_or(6),
        }
    }

    pub(crate) fn visible_len(&self) -> usize {
        self.cached.candidates.len()
    }

    pub(crate) fn first_engine_seat(&self) -> usize {
        (0..self.cached.candidates.len())
            .find(|&seat| !self.is_projected_emoji(seat) && !self.is_neural_injected(seat))
            .unwrap_or(0)
    }

    pub fn set_paired_punctuation_enabled(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        self.engine.set_paired_punctuation_enabled(enabled)
    }

    pub fn balance_paired_punctuation_after_auto_close(
        &mut self,
        opening: u8,
    ) -> Result<(), RuntimeError> {
        if opening != b'<' {
            return Err(RuntimeError::InvalidPunctuation);
        }
        self.engine
            .balance_paired_punctuation_after_auto_close(opening)
    }

    pub fn set_punctuation_lock(&mut self, lock: u8) -> Result<(), RuntimeError> {
        self.engine.set_punctuation_lock(lock)?;
        self.refresh_idle_spelling_symbols()
    }

    fn refresh_idle_spelling_symbols(&mut self) -> Result<(), RuntimeError> {
        if self.cached.editing_text.is_empty() {
            self.refresh()?;
        }
        Ok(())
    }

    /// 一次原地循环置换重排候选列表（`<= 128` 行时零堆分配），每个 [`CandidateEntry`] 已自带 `origin`。
    pub(crate) fn apply_candidate_order(&mut self, order: &[usize]) {
        let candidates = &mut self.cached.candidates;
        debug_assert_eq!(candidates.len(), order.len());
        apply_order_with(order, |left, right| {
            candidates.swap(left, right);
        });
    }

    pub fn set_dedicated_english(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_dedicated_english(enabled)?;
        if self.cached.dedicated_english != enabled && !self.locked_prefix.is_empty() {
            self.locked_prefix.clear();
            self.bump_composition_epoch();
        }
        self.refresh()
    }

    /// 更新即时隐私门禁，并使之前显示的联想候选身份过期。
    pub fn set_prediction_allowed(&mut self, allowed: bool) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_prediction_allowed(allowed);
        self.refresh()
    }

    pub fn set_backspace_reverts_selection(&mut self, enabled: bool) {
        self.backspace_reverts_selection = enabled;
    }

    pub fn set_escape_keeps_selected_word(&mut self, enabled: bool) {
        self.escape_keeps_selected_word = enabled;
    }

    pub fn set_left_right_navigation(&mut self, enabled: bool) {
        self.left_right_navigation = enabled;
    }

    pub fn set_english_first_page_count(&mut self, count: u8) {
        self.english_first_page_count = usize::from(count.clamp(1, 3));
    }

    pub fn set_vertical_candidates(&mut self, vertical: bool) {
        self.vertical_candidates = vertical;
    }

    pub fn set_second_third_candidate(
        &mut self,
        settings: msime_client_core::preferences::SecondThirdCandidatePreferences,
    ) {
        self.second_third_candidate = settings;
    }

    pub fn second_third_candidate(
        &self,
    ) -> &msime_client_core::preferences::SecondThirdCandidatePreferences {
        &self.second_third_candidate
    }

    /// 将已解码的选词字符转成当前页的候选动作。
    pub fn candidate_shortcut(&self, action: Action) -> Action {
        if !self.focused || self.cached.prediction {
            return action;
        }
        let shown = self.display_page_current();
        self.second_third_candidate_shortcut(&action)
            .map(|id| {
                if shown {
                    Action::SelectAnyCandidate(id)
                } else {
                    Action::Select(id)
                }
            })
            .unwrap_or(action)
    }

    pub fn set_display_page(&mut self, generation: u64, indices: Vec<usize>) -> bool {
        if generation != self.generation {
            return false;
        }
        if indices.is_empty() {
            self.display_page = None;
            return true;
        }
        let visible = self.visible_len();
        if indices.iter().any(|&index| index >= visible) {
            return false;
        }
        self.display_page = Some((generation, indices));
        true
    }

    fn display_page_current(&self) -> bool {
        self.display_page
            .as_ref()
            .is_some_and(|(generation, _)| *generation == self.generation)
    }

    fn page_seats(&self) -> Vec<usize> {
        if let Some((_, indices)) = self
            .display_page
            .as_ref()
            .filter(|_| self.display_page_current())
        {
            return indices.clone();
        }
        let start = (self.highlighted / self.page_size) * self.page_size;
        (start..self.visible_len().min(start + self.page_size)).collect()
    }

    fn page_seat_candidate(&self, slot: usize) -> Option<CandidateId> {
        let index = *self.page_seats().get(slot)?;
        (index < self.visible_len()).then_some(CandidateId {
            session: self.session,
            generation: self.generation,
            index,
        })
    }

    fn second_third_candidate_shortcut(&self, action: &Action) -> Option<CandidateId> {
        if !self.second_third_candidate.enabled
            || !self.snapshot_valid
            || self.cached.dedicated_english
            || self.cached.local_mode != "none"
            || self.cached.editing_text.is_empty()
            || !second_third_candidate_scheme(self.cached.scheme)
        {
            return None;
        }
        let value = match action {
            Action::Character { value, .. }
            | Action::Punctuation(value)
            | Action::PunctuationAscii(value) => *value,
            _ => return None,
        };
        let slot = self.second_third_candidate.slot(value)?;
        if self.cached.spelling_symbols.contains(char::from(value))
            || (value == b';'
                && self.cached.microsoft_shuangpin
                && microsoft_shuangpin_final_position(
                    self.cached.editing_text.as_bytes(),
                    self.cached.caret_position,
                ))
        {
            return None;
        }
        self.page_seat_candidate(slot)
    }

    fn caret_edge_candidate_shortcut(&self, action: Action) -> Action {
        if !self.left_right_navigation
            || self.vertical_candidates
            || !self.snapshot_valid
            || self.visible_len() == 0
            || !self.cached.conversion.is_empty()
        {
            return action;
        }
        let backwards = match action {
            Action::Command(Command::MoveLeft | Command::ConversionLeft) => true,
            Action::Command(Command::MoveRight | Command::ConversionRight) => false,
            _ => return action,
        };
        let edits = self.highlighted == 0
            && (backwards || self.cached.caret_position < self.cached.editing_text.len());
        if edits {
            return action;
        }
        if backwards {
            Action::PreviousCandidate
        } else {
            Action::NextCandidate
        }
    }

    fn home_key_action(&self, action: Action) -> Action {
        if !matches!(action, Action::FirstCandidateOrHome) {
            return action;
        }
        let caret_home = self.snapshot_valid
            && self.highlighted == 0
            && self.cached.conversion.is_empty()
            && !self.cached.editing_text.is_empty();
        if caret_home {
            Action::Command(Command::MoveHome)
        } else {
            Action::FirstCandidate
        }
    }

    pub fn set_command_table(&mut self, table: &[CommandTableEntry]) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_command_table(table)?;
        self.refresh()
    }

    pub fn set_quick_phrase_table(
        &mut self,
        table: &[QuickPhraseEntry],
    ) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_quick_phrase_table(table)?;
        self.refresh()
    }

    pub fn set_mention_entries(&mut self, entries: &[MentionEntry]) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_mention_entries(entries)?;
        self.refresh()
    }

    pub fn set_mention_places(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_mention_places(enabled)?;
        self.refresh()
    }

    pub fn new(engine: E, page_size: u8) -> Result<Self, RuntimeError> {
        Self::new_with_touch_layout(engine, page_size, TouchKeyboardLayout::default())
    }

    pub fn new_with_touch_layout(
        engine: E,
        page_size: u8,
        touch_keyboard_layout: TouchKeyboardLayout,
    ) -> Result<Self, RuntimeError> {
        if !(1..=MAX_CANDIDATE_PAGE_SIZE).contains(&page_size) {
            return Err(RuntimeError::InvalidPageSize);
        }
        let session = NEXT_SESSION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| RuntimeError::IdentityExhausted)?;
        let cached = engine.snapshot()?;
        Ok(Self {
            engine,
            session,
            generation: 0,
            ai_context: String::new(),
            app_scope_key: "general",
            neural_v2_enabled: false,
            neural_context: String::new(),
            neural_right_context: String::new(),
            neural_pending_request: None,
            neural_fusion_weights:
                msime_engine::neural_v2::NeuralFusionWeights::from_env_or_default(),
            presentation_phase: PresentationPhase::Idle,
            focused: false,
            page_size: page_size.into(),
            highlighted: 0,
            translations: HashMap::new(),
            highlight_translations: HashMap::new(),
            translation_scope_highlighted: false,
            display_page: None,
            cached,
            online_order_scratch: Vec::new(),
            inline_emoji_projection: false,
            emoji_mru: crate::emoji_mru::EmojiMruStore::default(),
            snapshot_valid: true,
            character_width: CharacterWidth::Halfwidth,
            punctuation_marks: PunctuationMarks::default(),
            touch_keyboard_layout,
            second_third_candidate: Default::default(),
            phrase_preedit: false,
            backspace_reverts_selection: true,
            escape_keeps_selected_word: false,
            left_right_navigation: false,
            english_first_page_count: 1,
            vertical_candidates: false,
            locked_prefix: Vec::new(),
            facts: DispatchFacts::default(),
            selected_text: String::new(),
            composition_epoch: 0,
            candidates_epoch: 0,
        })
    }

    pub fn set_phrase_preedit(&mut self, enabled: bool) -> Option<String> {
        if self.phrase_preedit == enabled {
            return None;
        }
        self.phrase_preedit = enabled;
        if enabled || self.locked_prefix.is_empty() {
            self.locked_prefix.clear();
            return None;
        }
        let held = self.locked_prefix_text();
        self.locked_prefix.clear();
        self.bump_composition_epoch();
        (!held.is_empty()).then_some(held)
    }

    pub fn set_inline_emoji_projection(&mut self, enabled: bool, mru_path: Option<PathBuf>) {
        self.inline_emoji_projection = enabled;
        self.emoji_mru.configure(mru_path);
        if !enabled {
            if self.strip_projected_emojis() {
                let _ = self.advance();
                self.bump_candidates_epoch();
            }
        } else if !self.is_idle() && self.apply_slot_allocator() {
            let _ = self.advance();
            self.bump_candidates_epoch();
        }
    }

    pub fn set_neural_v2_enabled(&mut self, enabled: bool) {
        self.neural_v2_enabled = enabled;
        if !enabled {
            self.neural_pending_request = None;
            self.presentation_phase = PresentationPhase::Idle;
        }
    }

    pub fn neural_v2_enabled(&self) -> bool {
        self.neural_v2_enabled
    }

    pub fn session_id(&self) -> u64 {
        self.session
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn presentation_phase(&self) -> PresentationPhase {
        self.presentation_phase
    }

    pub fn is_provisional(&self) -> bool {
        matches!(
            self.presentation_phase,
            PresentationPhase::Provisional { generation, .. }
                if generation == self.generation && !self.is_idle()
        )
    }

    pub fn provisional_remaining(&self, now: Instant) -> Option<(u64, Duration)> {
        match self.presentation_phase {
            PresentationPhase::Provisional {
                generation,
                deadline,
            } if generation == self.generation && !self.is_idle() => {
                Some((generation, deadline.saturating_duration_since(now)))
            }
            _ => None,
        }
    }

    pub fn begin_provisional(&mut self, generation: u64, window: Duration) {
        if !self.is_idle() && self.generation == generation {
            self.presentation_phase = PresentationPhase::Provisional {
                generation,
                deadline: Instant::now() + window,
            };
        }
    }

    pub fn mark_presented(&mut self, generation: u64) -> bool {
        match self.presentation_phase {
            PresentationPhase::Provisional { generation: g, .. } if g == generation => {
                self.presentation_phase = PresentationPhase::Presented { generation };
                true
            }
            _ => false,
        }
    }

    pub fn flush_provisional(&mut self) -> bool {
        if matches!(
            self.presentation_phase,
            PresentationPhase::Provisional { .. }
        ) {
            self.presentation_phase = PresentationPhase::Presented {
                generation: self.generation,
            };
            true
        } else {
            false
        }
    }

    pub fn set_neural_fusion_weights(
        &mut self,
        weights: msime_engine::neural_v2::NeuralFusionWeights,
    ) {
        self.neural_fusion_weights = weights;
    }

    pub fn neural_prefill_context(&self) -> &str {
        if self.focused {
            &self.neural_context
        } else {
            ""
        }
    }

    pub fn take_neural_request(&mut self) -> Option<msime_engine::neural_v2::NeuralDecodeRequest> {
        use msime_engine::neural_v2::{
            build_syllable_paths, is_neural_v2_source, NeuralCandidateInput, NeuralDecodeRequest,
            DEFAULT_BEAM_WIDTH, DEFAULT_MAX_BEAM_RESULTS, DEFAULT_MAX_NEURAL_CANDIDATES,
            DEFAULT_MAX_SYLLABLE_PATHS, DEFAULT_TYPO_CHANNEL_PENALTY,
            DEFAULT_USER_DICTIONARY_LOG_PRIOR,
        };

        if !self.neural_v2_enabled
            || !self.focused
            || self.is_idle()
            || self.cached.prediction
            || self.cached.local_mode != "none"
            || self.cached.dedicated_english
            || !self.cached.conversion.is_empty()
            || self.cached.caret_edit_suffix
            || self.cached.candidates.is_empty()
        {
            return None;
        }
        let scheme = scheme_type(self.cached.scheme)?;
        if !scheme.is_pinyin() {
            return None;
        }

        let engine_meta = self.engine.neural_v2_metadata();
        let (raw_input, segmentation, syllable_paths) = if let Some(meta) = &engine_meta {
            (
                meta.raw_input.clone(),
                meta.segmentation.clone(),
                meta.syllable_paths.clone(),
            )
        } else {
            let raw = self.cached.editing_text.clone();
            let seg = if !self.cached.preedit.is_empty() {
                self.cached.preedit.clone()
            } else {
                raw.clone()
            };
            let paths = build_syllable_paths(&raw, &seg, DEFAULT_MAX_SYLLABLE_PATHS);
            (raw, seg, paths)
        };

        let primary_syllables = segmentation
            .split('\'')
            .filter(|piece| !piece.is_empty())
            .count();

        let max_neural_candidates = if primary_syllables == 1 {
            24
        } else {
            DEFAULT_MAX_NEURAL_CANDIDATES
        };
        let mut candidates: Vec<NeuralCandidateInput> =
            Vec::with_capacity(self.cached.candidates.len().min(max_neural_candidates));
        let mut pinned_first_index: Option<usize> = None;
        let mut pinned_seats: Vec<(usize, usize)> = Vec::new();
        let mut seen_texts: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(max_neural_candidates);
        const MAX_DEFERRED_DICT_NEURAL_RESCUE: usize = 2;
        let mut deferred_dict_rescued = 0usize;
        let mut base_idx = 0usize;
        for entry in &self.cached.candidates {
            if matches!(entry.origin, CandidateOrigin::ProjectedEmoji { .. }) {
                continue;
            }
            let current_base_idx = base_idx;
            base_idx += 1;
            if current_base_idx >= 128 {
                break;
            }
            let eng_idx = match entry.origin {
                CandidateOrigin::EngineRow(idx) => Some(idx),
                CandidateOrigin::InjectedBeam | CandidateOrigin::ProjectedEmoji { .. } => None,
            };
            let meta = eng_idx.and_then(|i| engine_meta.as_ref().and_then(|m| m.candidates.get(i)));
            let fixed_pos = if entry.effective_fixed_position() > 0 {
                entry.effective_fixed_position()
            } else {
                meta.map_or(0, |m| {
                    if m.fixed_position > 0 {
                        m.fixed_position
                    } else if m.pinned {
                        1
                    } else {
                        0
                    }
                })
            };
            if fixed_pos > 0 {
                // L4 固定/置顶席位不发往前缀树，统一记录目标席位交由融合层原地锁死。
                if fixed_pos == 1 && pinned_first_index.is_none() {
                    pinned_first_index = Some(current_base_idx);
                }
                pinned_seats.push((current_base_idx, usize::from(fixed_pos - 1)));
                continue;
            }
            let source = entry.source;
            let is_dict_source = source == msime_engine::CandidateSource::Database as u8
                || source == msime_engine::CandidateSource::UserDatabase as u8;
            let pool_full = candidates.len() >= max_neural_candidates;
            if pool_full
                && !(primary_syllables >= 3
                    && is_dict_source
                    && deferred_dict_rescued < MAX_DEFERRED_DICT_NEURAL_RESCUE)
            {
                continue;
            }
            if !is_neural_v2_source(source) {
                continue;
            }
            let text = entry.text.clone();
            let pinyin = entry.code.clone();
            let char_count = text.chars().count();

            let (
                stat_log_prob,
                stat_from_lattice,
                channel_log_prob,
                user_log_prob,
                collocation_log_prob,
                exact_span,
                exact_literal_dict,
                pinned,
                has_lexical_head,
            ) = if let Some(m) = meta {
                (
                    m.stat_log_prob,
                    m.stat_from_lattice,
                    m.channel_log_prob,
                    m.user_log_prob,
                    m.collocation_log_prob,
                    m.exact_span,
                    m.exact_literal_dict,
                    m.pinned || entry.pinned_first || entry.fixed_position > 0,
                    m.has_lexical_head,
                )
            } else {
                let stat = entry
                    .static_evidence
                    .map(|e| e.log_prob_milli_nats as f64 / 1000.0);
                let stat_from_lattice = is_dict_source && entry.static_evidence.is_some();
                let channel = if entry.corrected {
                    DEFAULT_TYPO_CHANNEL_PENALTY
                } else {
                    entry.channel_log_prob
                };
                let user = if source == msime_engine::CandidateSource::UserDatabase as u8 {
                    DEFAULT_USER_DICTIONARY_LOG_PRIOR
                } else {
                    entry.user_log_prob
                };
                let exact = entry.answers_key
                    && char_count > 0
                    && (primary_syllables == 0 || char_count <= primary_syllables + 1);
                (
                    stat,
                    stat_from_lattice,
                    channel,
                    user,
                    0.0,
                    exact,
                    false,
                    entry.pinned_first || entry.fixed_position > 0,
                    entry.lexical_head,
                )
            };

            if pinned {
                if pinned_first_index.is_none() {
                    pinned_first_index = Some(current_base_idx);
                }
                pinned_seats.push((current_base_idx, 0));
                continue;
            }
            if !exact_span {
                continue;
            }
            if pool_full {
                // 3+ 音节输入下，若被词网格后置且未进入前 16 条生成句的精确词典整词落在后排（如冷门/未测频词典整词），
                // 置换末尾非词典生成句槽位（绝不触碰前 4 席）确保其进入 Tier 1 神经打分池接受语境裁决。
                if channel_log_prob != 0.0 || seen_texts.contains(&text) {
                    continue;
                }
                let replace_pos = candidates
                    .iter()
                    .enumerate()
                    .skip(4)
                    .rev()
                    .find(|(_, c)| {
                        c.source != msime_engine::CandidateSource::Database as u8
                            && c.source != msime_engine::CandidateSource::UserDatabase as u8
                    })
                    .map(|(pos, _)| pos);
                let Some(replace_pos) = replace_pos else {
                    continue;
                };
                seen_texts.insert(text.clone());
                candidates[replace_pos] = NeuralCandidateInput {
                    index: current_base_idx,
                    text,
                    pinyin,
                    source,
                    char_count,
                    exact_span,
                    exact_literal_dict,
                    stat_log_prob,
                    stat_from_lattice,
                    channel_log_prob,
                    user_log_prob,
                    collocation_log_prob,
                    pinned,
                    has_lexical_head,
                };
                deferred_dict_rescued += 1;
                continue;
            }
            if !seen_texts.insert(text.clone()) {
                continue;
            }

            candidates.push(NeuralCandidateInput {
                index: current_base_idx,
                text,
                pinyin,
                source,
                char_count,
                exact_span,
                exact_literal_dict,
                stat_log_prob,
                stat_from_lattice,
                channel_log_prob,
                user_log_prob,
                collocation_log_prob,
                pinned,
                has_lexical_head,
            });
        }

        if candidates.is_empty() && syllable_paths.is_empty() {
            return None;
        }

        let cursor = self.cursor_snapshot();
        let context = cursor.effective_left_context();
        let right_context = cursor.effective_right_context().to_owned();
        let has_any_context = !context.trim().is_empty() || !right_context.trim().is_empty();
        let has_single_char_exact_dict = candidates.iter().any(|c| {
            c.char_count == 1
                && (c.source == msime_engine::CandidateSource::Database as u8
                    || c.source == msime_engine::CandidateSource::UserDatabase as u8)
                && c.channel_log_prob == 0.0
        });
        if !has_any_context && primary_syllables == 1 && has_single_char_exact_dict {
            return None;
        }

        let request = NeuralDecodeRequest {
            session: self.session,
            generation: self.generation,
            context,
            right_context,
            raw_input,
            segmentation,
            syllable_paths,
            candidates,
            pinned_first_index,
            pinned_seats,
            beam_width: DEFAULT_BEAM_WIDTH,
            max_beam_results: DEFAULT_MAX_BEAM_RESULTS,
            enable_constrained_decode: true,
        };
        self.neural_pending_request = Some(request.clone());
        Some(request)
    }

    pub fn apply_neural_response(
        &mut self,
        response: &msime_engine::neural_v2::NeuralDecodeResponse,
        lock_slot_0: bool,
    ) -> NeuralApplyOutcome {
        use msime_engine::neural_v2::{FusedSeat, SOURCE_NEURAL_CONSTRAINED};

        let rejected = |generation: u64, locked: bool| NeuralApplyOutcome {
            applied: false,
            moved: false,
            generation,
            slot0_locked: locked,
            tier1_rescued: false,
        };

        if !response.ok
            || !self.neural_v2_enabled
            || !self.focused
            || self.is_idle()
            || self.generation.checked_add(1).is_none()
            || response.generation != self.generation
            || (response.session != 0 && response.session != self.session)
        {
            return rejected(self.generation, lock_slot_0);
        }

        let request = match &self.neural_pending_request {
            Some(pending) if pending.generation == self.generation => pending.clone(),
            _ => match self.take_neural_request() {
                Some(fresh) => fresh,
                None => return rejected(self.generation, lock_slot_0),
            },
        };

        let marked = self.settled_highlight();
        let effective_lock_slot_0 = lock_slot_0 || self.highlighted > 0;

        let had_projected = self.strip_projected_emojis();
        let base_len = self.cached.candidates.len();
        let outcome = UnifiedScorer::score(
            base_len,
            &request,
            response,
            self.neural_fusion_weights,
            effective_lock_slot_0,
        );

        if outcome.moved {
            let mut old_candidates: Vec<Option<CandidateEntry>> =
                std::mem::take(&mut self.cached.candidates)
                    .into_iter()
                    .map(Some)
                    .collect();
            self.cached.candidates.reserve(outcome.seats.len());

            for seat in &outcome.seats {
                match seat {
                    FusedSeat::Existing { index, .. } => {
                        if let Some(mut entry) =
                            old_candidates.get_mut(*index).and_then(Option::take)
                        {
                            if let Some(scored) = response
                                .candidate_scores
                                .iter()
                                .find(|item| item.text == entry.text)
                            {
                                entry.tier1_neural_log_prob = Some(scored.neural_log_prob);
                            }
                            self.cached.candidates.push(entry);
                        }
                    }
                    FusedSeat::InjectedBeam {
                        text,
                        pinyin,
                        neural_log_prob,
                        channel_log_prob,
                    } => {
                        let mut entry = CandidateEntry::new(
                            text.clone(),
                            pinyin.clone(),
                            SOURCE_NEURAL_CONSTRAINED,
                        );
                        entry.answers_key = true;
                        entry.channel_log_prob = *channel_log_prob;
                        entry.tier1_neural_log_prob = Some(*neural_log_prob);
                        entry.origin = CandidateOrigin::InjectedBeam;
                        entry.stamp_generation(self.generation);
                        self.cached.candidates.push(entry);
                    }
                }
            }

            self.normalize_online_slots();
            self.apply_slot_allocator();
            self.snapshot_valid = true;
            if let Some((previous_highlight, text, code)) = &marked {
                self.keep_highlight_on_same_candidate(*previous_highlight, text, code.as_deref());
            } else {
                self.highlighted = 0;
            }
            let _ = self.advance();
            self.bump_candidates_epoch();
            self.presentation_phase = PresentationPhase::Presented {
                generation: self.generation,
            };
            self.neural_pending_request = None;
            return NeuralApplyOutcome {
                applied: true,
                moved: true,
                generation: self.generation,
                slot0_locked: outcome.slot0_locked,
                tier1_rescued: outcome.tier1_rescued,
            };
        }

        if had_projected {
            self.apply_slot_allocator();
            if let Some((previous_highlight, text, code)) = &marked {
                self.keep_highlight_on_same_candidate(*previous_highlight, text, code.as_deref());
            }
        }
        if matches!(
            self.presentation_phase,
            PresentationPhase::Provisional { generation, .. } if generation == self.generation
        ) {
            self.presentation_phase = PresentationPhase::Presented {
                generation: self.generation,
            };
        }
        self.neural_pending_request = None;
        NeuralApplyOutcome {
            applied: true,
            moved: false,
            generation: self.generation,
            slot0_locked: outcome.slot0_locked,
            tier1_rescued: outcome.tier1_rescued,
        }
    }

    fn settled_highlight(&self) -> Option<(usize, String, Option<String>)> {
        if self.highlighted == 0 || self.is_projected_emoji(self.highlighted) {
            return None;
        }
        let entry = self.cached.candidates.get(self.highlighted)?;
        Some((
            self.highlighted,
            entry.text.clone(),
            Some(entry.code.clone()),
        ))
    }

    pub fn set_character_width(&mut self, width: CharacterWidth) {
        self.character_width = width;
    }

    pub fn set_punctuation_marks(&mut self, marks: PunctuationMarks) {
        self.punctuation_marks = marks;
    }

    pub fn seed_context(&mut self, text: &str) {
        self.remember_commit(text);
    }

    pub fn clear_context(&mut self) {
        self.ai_context.clear();
        self.neural_context.clear();
        self.neural_right_context.clear();
        self.engine.set_rescoring_context("");
    }

    pub fn set_surrounding_context(&mut self, text: &str) {
        self.neural_right_context.clear();
        if !self.focused {
            self.ai_context.clear();
            self.neural_context.clear();
            self.engine.set_rescoring_context("");
            return;
        }
        let neural_cleaned = sanitize_neural_context(text);
        if (neural_cleaned.is_empty() || !self.neural_context.ends_with(neural_cleaned.as_ref()))
            && self.neural_context != neural_cleaned.as_ref()
        {
            self.neural_context.clear();
            self.neural_context.push_str(neural_cleaned.as_ref());
        }
        let cleaned = sanitize_surrounding_context(text);
        if !cleaned.is_empty() && self.ai_context.ends_with(cleaned.as_ref()) {
            return;
        }
        if self.ai_context != cleaned.as_ref() {
            self.engine.reset_context();
            self.ai_context.clear();
            self.ai_context.push_str(cleaned.as_ref());
            self.engine.set_rescoring_context(&self.ai_context);
        }
    }

    pub fn set_following_context(&mut self, text: &str) {
        self.neural_right_context.clear();
        if !self.focused {
            return;
        }
        let cleaned = sanitize_neural_right_context(text);
        if !cleaned.is_empty() {
            self.neural_right_context.push_str(cleaned.as_ref());
        }
    }

    /// 根据前台客户端 `bundle_id` 解析粗粒度 `AppScope` 键并同步到底层引擎；不保存原始 `bundle_id`。
    pub fn set_app_bundle_id(&mut self, bundle_id: &str) {
        self.set_app_scope_key(msime_engine::host::Session::resolve_app_scope_key(
            bundle_id,
        ));
    }

    /// 直接设置规范化的 `AppScope` 键（未知值一律回退为 `"general"`）。
    pub fn set_app_scope_key(&mut self, scope_key: &str) {
        let canonical = canonical_app_scope_key(scope_key);
        self.app_scope_key = canonical;
        self.engine.set_app_scope_key(canonical);
    }

    /// 返回当前会话使用的粗粒度 `AppScope` 键。
    #[must_use]
    pub fn app_scope_key(&self) -> &'static str {
        self.app_scope_key
    }

    pub fn set_caret(&mut self, caret: Option<usize>) -> Result<(), RuntimeError> {
        self.advance()?;
        self.engine.set_caret(caret);
        self.refresh()
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }

    pub fn prefix_end(&self) -> usize {
        self.engine.prefix_end()
    }

    pub fn pending_suffix(&self) -> String {
        self.engine.pending_suffix()
    }

    pub fn view(&self) -> View {
        let page = self.highlighted / self.page_size;
        let start = page * self.page_size;
        let visible = self.visible_len();
        let page_len = visible.saturating_sub(start).min(self.page_size);
        let reserve_translation = self.translation_scope_highlighted
            && page_len > 0
            && !self.cached.prediction
            && self.page_requests_translations();
        let mut candidates = Vec::with_capacity(page_len);
        candidates.extend(
            self.cached
                .candidates
                .iter()
                .enumerate()
                .skip(start)
                .take(page_len)
                .map(|(index, entry)| self.candidate_row(index, entry, reserve_translation)),
        );
        View {
            prediction: self.cached.prediction,
            scheme: self.cached.scheme,
            chinese_text: scheme_type(self.cached.scheme).is_some_and(SchemeType::is_chinese),
            script_conversion: script_conversion(self.cached.scheme, &self.cached.local_mode),
            touch_keyboard_layout: self.touch_keyboard_layout,
            character_width: self.character_width,
            punctuation_marks: self.punctuation_marks,
            microsoft_shuangpin: self.cached.microsoft_shuangpin,
            shuangpin_profile: self.cached.shuangpin_profile.clone(),
            local_mode: self.cached.local_mode.clone(),
            spelling_symbols: if self.locked_prefix.is_empty() || self.cached.local_mode != "none" {
                self.cached.spelling_symbols.clone()
            } else {
                String::new()
            },
            dedicated_english: self.cached.dedicated_english,
            session: self.session,
            generation: self.generation,
            focused: self.focused,
            preedit: self.cached.preedit.clone(),
            phrase_prefix: self.locked_prefix_text(),
            reading: self.cached.reading.clone(),
            display_reading: self.cached.display_reading.clone(),
            editing_text: self.cached.editing_text.clone(),
            caret_position: self.cached.caret_position,
            conversion: self.cached.conversion.clone(),
            conversion_focus_start: self.cached.conversion_focus_start,
            conversion_focus_end: self.cached.conversion_focus_end,
            caret_edit_suffix: self.cached.caret_edit_suffix,
            caret_edit_prefix: self.cached.caret_edit_prefix.clone(),
            provisional: self.is_provisional(),
            page,
            page_size: self.page_size,
            page_count: visible.div_ceil(self.page_size),
            candidates,
        }
    }

    pub fn candidate_page_len(&self) -> usize {
        let start = (self.highlighted / self.page_size) * self.page_size;
        self.visible_len().saturating_sub(start).min(self.page_size)
    }

    pub fn scheme(&self) -> u8 {
        self.cached.scheme
    }

    pub fn local_mode(&self) -> &str {
        &self.cached.local_mode
    }

    pub fn punctuation_host_context_available(&self, english_mode: bool) -> bool {
        !english_mode
            && !self.cached.dedicated_english
            && self.cached.local_mode == "none"
            && scheme_type(self.cached.scheme).is_some_and(SchemeType::host_smart_punctuation)
    }

    pub fn translation_candidates(&self) -> Option<TranslationCandidates> {
        let seats = self.page_seats();
        let visible = self.visible_len();
        let mut candidates = Vec::with_capacity(seats.len());
        let mut highlighted = None;
        for index in seats.into_iter().filter(|&index| index < visible) {
            let Some(entry) = self.cached.candidates.get(index) else {
                continue;
            };
            if index == self.highlighted {
                highlighted = Some(candidates.len());
            }
            candidates.push(TranslationCandidate {
                text: entry.text.clone(),
                source: entry.source,
            });
        }
        (!candidates.is_empty()).then_some(TranslationCandidates {
            generation: self.generation,
            scheme: self.cached.scheme,
            local_mode: self.cached.local_mode.clone(),
            highlighted,
            candidates,
        })
    }

    pub fn all_candidates(&mut self) -> CandidateSnapshot {
        if self.generation == u64::MAX {
            return self.all_candidates_cached();
        }
        if self.expand_cached_candidates().unwrap_or(false) {
            let advanced = self.advance();
            debug_assert!(advanced.is_ok());
        }
        self.all_candidates_cached()
    }

    fn all_candidates_cached(&self) -> CandidateSnapshot {
        let visible = self.visible_len();
        let mut candidates = Vec::with_capacity(visible);
        candidates.extend(
            self.cached
                .candidates
                .iter()
                .take(visible)
                .enumerate()
                .map(|(index, entry)| self.candidate_row(index, entry, false)),
        );
        CandidateSnapshot {
            session: self.session,
            generation: self.generation,
            preedit: self.cached.preedit.clone(),
            reading: self.cached.reading.clone(),
            candidates,
        }
    }

    pub fn visible_candidates_snapshot(&self) -> Vec<(String, u8)> {
        let visible = self.visible_len();
        self.cached
            .candidates
            .iter()
            .take(visible)
            .map(|entry| (entry.text.clone(), entry.source))
            .collect()
    }

    pub fn raw_engine_candidates(&self) -> Vec<(String, u8)> {
        let mut rows: Vec<(usize, (String, u8))> = self
            .cached
            .candidates
            .iter()
            .filter_map(|entry| match entry.origin {
                CandidateOrigin::EngineRow(idx) => Some((idx, (entry.text.clone(), entry.source))),
                CandidateOrigin::InjectedBeam | CandidateOrigin::ProjectedEmoji { .. } => None,
            })
            .collect();
        rows.sort_by_key(|(idx, _)| *idx);
        rows.into_iter().map(|(_, pair)| pair).collect()
    }

    fn displayed_candidate_text(&self, index: usize, text: &str) -> String {
        if index == 0 && !self.cached.caret_edit_prefix.is_empty() {
            let mut shown = self.cached.caret_edit_prefix.clone();
            shown.push_str(text);
            return shown;
        }
        text.to_owned()
    }

    fn candidate_row(
        &self,
        index: usize,
        entry: &CandidateEntry,
        reserve_translation: bool,
    ) -> Candidate {
        let highlighted = index == self.highlighted;
        let text = entry.text.as_str();
        let translation = ((!self.translations.is_empty()
            || !self.highlight_translations.is_empty())
            && !msime_client_core::translation::source_gets_no_gloss(entry.source))
        .then(|| {
            highlighted
                .then(|| self.highlight_translations.get(text))
                .flatten()
                .or_else(|| self.translations.get(text))
                .cloned()
        })
        .flatten();
        let translation_runs = translation
            .as_deref()
            .map(msime_client_core::translation::translation_runs)
            .unwrap_or_default();
        Candidate {
            id: CandidateId {
                session: self.session,
                generation: self.generation,
                index,
            },
            text: self.displayed_candidate_text(index, text),
            code: entry.code.clone(),
            annotation: entry.annotation.clone(),
            source: entry.source,
            corrected: entry.corrected,
            highlighted,
            fixed_position: {
                let pos = entry.effective_fixed_position();
                (pos > 0).then_some(pos)
            },
            translation_reserved: reserve_translation && highlighted,
            translation,
            translation_runs,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.snapshot_valid
            && self.locked_prefix.is_empty()
            && self.cached.preedit.is_empty()
            && self.cached.editing_text.is_empty()
            && (self.cached.prediction || self.cached.candidates.is_empty())
    }

    pub fn has_composition(&self) -> bool {
        !self.locked_prefix.is_empty()
            || !self.cached.preedit.is_empty()
            || !self.cached.editing_text.is_empty()
            || (!self.cached.prediction && !self.cached.candidates.is_empty())
    }

    pub fn facts(&self) -> DispatchFacts {
        self.facts
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.facts
            .selection
            .is_some()
            .then_some(self.selected_text.as_str())
    }

    pub fn composition_epoch(&self) -> u64 {
        self.composition_epoch
    }

    pub fn candidates_epoch(&self) -> u64 {
        self.candidates_epoch
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn command_translation(&self) -> Option<CommandTranslation> {
        let query = self.engine.command_translation_query()?;
        Some(CommandTranslation {
            generation: self.generation,
            session_id: query.session_id,
            text: query.text,
        })
    }

    pub fn apply_command_translation(
        &mut self,
        query: &CommandTranslation,
        translation: &str,
    ) -> Result<bool, RuntimeError> {
        if query.generation != self.generation
            || !msime_client_core::is_bounded_text(translation, 4096)
        {
            return Ok(false);
        }
        if self.generation == u64::MAX {
            return Err(RuntimeError::IdentityExhausted);
        }
        let request = CommandTranslationQuery {
            session_id: query.session_id,
            text: query.text.clone(),
        };
        if !self.engine.apply_command_translation(&request, translation) {
            return Ok(false);
        }
        self.advance()?;
        self.refresh()?;
        Ok(true)
    }

    pub fn invalidate_translations(&mut self) -> Result<(), RuntimeError> {
        self.advance()?;
        if !self.translations.is_empty() || !self.highlight_translations.is_empty() {
            self.translations.clear();
            self.highlight_translations.clear();
            self.bump_candidates_epoch();
        }
        Ok(())
    }

    pub fn apply_translations(
        &mut self,
        generation: u64,
        translations: impl IntoIterator<Item = (String, String)>,
    ) -> bool {
        self.apply_translation_answers(
            generation,
            translations
                .into_iter()
                .map(|(text, translation)| (text, translation, false)),
        )
    }

    pub fn apply_translation_answers(
        &mut self,
        generation: u64,
        answers: impl IntoIterator<Item = (String, String, bool)>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let scoped = self.translation_scope_highlighted && self.highlight_on_page_seats();
        let mut highlight_translations: HashMap<String, String> = HashMap::new();
        let translations: HashMap<String, String> = if scoped {
            let mut translations = HashMap::new();
            for (text, translation, gloss) in answers {
                if gloss {
                    translations.insert(text, translation);
                } else {
                    highlight_translations.insert(text, translation);
                }
            }
            translations
        } else {
            answers
                .into_iter()
                .map(|(text, translation, _)| (text, translation))
                .collect()
        };
        let any = !translations.is_empty() || !highlight_translations.is_empty();
        if let Some(query) = self.command_translation() {
            if let Some(translation) = translations
                .get(&query.text)
                .or_else(|| highlight_translations.get(&query.text))
            {
                let translation = translation.clone();
                return self
                    .apply_command_translation(&query, &translation)
                    .unwrap_or(false);
            }
        }
        let mut changed = false;
        if !any {
            changed = !self.translations.is_empty() || !self.highlight_translations.is_empty();
            self.translations.clear();
            self.highlight_translations.clear();
        } else {
            for (incoming, stored) in [
                (translations, &mut self.translations),
                (highlight_translations, &mut self.highlight_translations),
            ] {
                let mut incoming = incoming;
                retain_candidate_translations(&mut incoming, &self.cached.candidates);
                for (text, translation) in incoming {
                    if stored.get(&text) != Some(&translation) {
                        stored.insert(text, translation);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.bump_candidates_epoch();
        }
        true
    }

    pub fn set_translation_scope_highlighted(&mut self, highlighted: bool) {
        if self.translation_scope_highlighted != highlighted {
            self.translation_scope_highlighted = highlighted;
            self.bump_candidates_epoch();
        }
    }

    fn highlight_on_page_seats(&self) -> bool {
        if self.highlighted >= self.visible_len() {
            return false;
        }
        match self
            .display_page
            .as_ref()
            .filter(|_| self.display_page_current())
        {
            Some((_, indices)) => indices.contains(&self.highlighted),
            None => true,
        }
    }

    pub fn page_requests_translations(&self) -> bool {
        scheme_type(self.cached.scheme).is_some_and(SchemeType::shows_glosses)
            && self.cached.local_mode != "url"
    }

    pub fn set_page_size(&mut self, page_size: u8) -> Result<(), RuntimeError> {
        if !(1..=MAX_CANDIDATE_PAGE_SIZE).contains(&page_size) {
            return Err(RuntimeError::InvalidPageSize);
        }
        if self.page_size == usize::from(page_size) {
            return Ok(());
        }
        if !self.is_idle() {
            return Err(RuntimeError::CompositionActive);
        }
        self.advance()?;
        self.page_size = page_size.into();
        self.engine.set_first_page_rows(page_size.into());
        Ok(())
    }

    pub fn replace_engine(&mut self, engine: E, page_size: u8) -> Result<(), RuntimeError> {
        self.replace_engine_with_touch_layout(engine, page_size, self.touch_keyboard_layout)
    }

    pub fn replace_engine_with_touch_layout(
        &mut self,
        engine: E,
        page_size: u8,
        touch_keyboard_layout: TouchKeyboardLayout,
    ) -> Result<(), RuntimeError> {
        if !(1..=MAX_CANDIDATE_PAGE_SIZE).contains(&page_size) {
            return Err(RuntimeError::InvalidPageSize);
        }
        if !self.is_idle() {
            return Err(RuntimeError::CompositionActive);
        }
        let cached = engine.snapshot()?;
        self.advance()?;
        self.engine = engine;
        self.engine.set_rescoring_context(&self.ai_context);
        self.engine.set_app_scope_key(self.app_scope_key);
        self.load_snapshot(cached);
        self.snapshot_valid = true;
        self.presentation_phase = PresentationPhase::Idle;
        self.page_size = page_size.into();
        self.highlighted = 0;
        self.touch_keyboard_layout = touch_keyboard_layout;
        Ok(())
    }

    fn expand_for_next_page(&mut self) -> Result<bool, RuntimeError> {
        let len = self.visible_len();
        if len == 0 {
            return Ok(false);
        }
        let page = self.highlighted / self.page_size;
        let last_page = (len - 1) / self.page_size;
        let next_is_partial_last = page + 1 == last_page && !len.is_multiple_of(self.page_size);
        if page != last_page && !next_is_partial_last {
            return Ok(false);
        }
        let page_was_full = (page + 1) * self.page_size <= len;
        if !self.expand_cached_candidates()? {
            return Ok(false);
        }
        let grown = self.visible_len();
        let stayed = page == last_page && !page_was_full;
        let grown_last_page = grown.saturating_sub(1) / self.page_size;
        if stayed && grown_last_page > last_page {
            return Ok(false);
        }
        Ok(stayed)
    }

    fn expand_for_next_candidate(&mut self) -> Result<(), RuntimeError> {
        let len = self.visible_len();
        if len == 0 {
            return Ok(());
        }
        let page = self.highlighted / self.page_size;
        let last_page = (len - 1) / self.page_size;
        let at_last_candidate = self.highlighted + 1 == len;
        let at_page_end = (self.highlighted + 1).is_multiple_of(self.page_size);
        let next_is_partial_last = page + 1 == last_page && !len.is_multiple_of(self.page_size);
        if !at_last_candidate && !(at_page_end && next_is_partial_last) {
            return Ok(());
        }
        self.expand_cached_candidates()?;
        Ok(())
    }

    fn expand_cached_candidates(&mut self) -> Result<bool, RuntimeError> {
        if !self.engine.expand_initial_candidates()? {
            return Ok(false);
        }
        let snapshot = self.engine.snapshot()?;
        self.load_snapshot(snapshot);
        self.order_presentation();
        self.apply_slot_allocator();
        self.bump_candidates_epoch();
        Ok(true)
    }

    fn load_snapshot(&mut self, snapshot: EngineSnapshot) {
        self.cached = snapshot;
    }

    fn engine_index(&self, seat: usize) -> usize {
        match self.cached.candidates.get(seat).map(|entry| &entry.origin) {
            Some(CandidateOrigin::EngineRow(index)) => *index,
            Some(CandidateOrigin::InjectedBeam | CandidateOrigin::ProjectedEmoji { .. }) => {
                usize::MAX
            }
            None => seat,
        }
    }

    fn advance(&mut self) -> Result<(), RuntimeError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(RuntimeError::IdentityExhausted)?;
        Ok(())
    }

    fn bump_composition_epoch(&mut self) {
        self.composition_epoch = self.composition_epoch.wrapping_add(1);
    }

    fn bump_candidates_epoch(&mut self) {
        self.candidates_epoch = self.candidates_epoch.wrapping_add(1);
    }

    fn record_selection(
        &mut self,
        how: SelectionHow,
        seat: usize,
        result: &EngineResult,
        edge: Option<CandidateEdge>,
    ) {
        self.facts.selection = self.selection_fact(how, seat, result, edge);
        self.selected_text.clear();
        if self.facts.selection.is_some() {
            if let Some(entry) = self.cached.candidates.get(seat) {
                self.selected_text.push_str(&entry.text);
            }
        }
    }

    fn selection_fact(
        &self,
        how: SelectionHow,
        seat: usize,
        result: &EngineResult,
        edge: Option<CandidateEdge>,
    ) -> Option<SelectionFact> {
        let entry = self.cached.candidates.get(seat)?;
        let text = &entry.text;
        let outcome = if result.has_commit {
            let matches = if self.cached.caret_edit_suffix {
                let tail = if how == SelectionHow::Edge {
                    edge.map_or("", |edge| edge.character(text))
                } else {
                    text.as_str()
                };
                !tail.is_empty() && result.commit.ends_with(tail)
            } else if how == SelectionHow::Edge {
                edge.is_some_and(|edge| {
                    let character = edge.character(text);
                    !character.is_empty() && result.commit.starts_with(character)
                })
            } else {
                !text.is_empty() && result.commit.starts_with(text.as_str())
            };
            if !matches {
                return None;
            }
            SelectionOutcome::Committed
        } else if result.handled
            && !self.cached.conversion.is_empty()
            && matches!(
                how,
                SelectionHow::Select
                    | SelectionHow::AnyCandidate
                    | SelectionHow::Digit
                    | SelectionHow::Highlighted
                    | SelectionHow::SpaceOverList
            )
        {
            SelectionOutcome::Pinned
        } else {
            return None;
        };
        Some(SelectionFact {
            seat: u32::try_from(seat).unwrap_or(u32::MAX),
            source: entry.source,
            how,
            outcome,
        })
    }

    pub(crate) fn remember_commit(&mut self, text: &str) {
        if !self.focused {
            self.ai_context.clear();
            self.neural_context.clear();
            self.neural_right_context.clear();
        } else {
            self.ai_context.push_str(text);
            if self.ai_context.len() > 1024 {
                let mut cut = self.ai_context.len() - 1024;
                while cut < self.ai_context.len() && !self.ai_context.is_char_boundary(cut) {
                    cut += 1;
                }
                self.ai_context.drain(..cut);
            }
            self.neural_context.push_str(text);
            let keep = msime_engine::neural_v2::neural_context_window(&self.neural_context).len();
            if keep < self.neural_context.len() {
                let cut = self.neural_context.len() - keep;
                self.neural_context.drain(..cut);
            }
        }
        self.engine.set_rescoring_context(&self.ai_context);
    }

    fn retreat_phrase_selection(
        &mut self,
        action: &Action,
    ) -> Result<Option<Transition>, RuntimeError> {
        if !self.phrase_preedit || self.locked_prefix.is_empty() {
            return Ok(None);
        }
        let reading = self.cached.editing_text.as_str();
        let caret = self.cached.caret_position;
        let restore = match action {
            Action::Command(Command::Backspace) => {
                if !self.backspace_reverts_selection
                    || reading.chars().count() > 1
                    || caret != reading.len()
                    || self
                        .locked_prefix
                        .last()
                        .is_none_or(|seg| seg.reading.is_empty())
                {
                    return Ok(None);
                }
                true
            }
            Action::SegmentBackspace => {
                if caret != 0 {
                    return Ok(None);
                }
                false
            }
            _ => return Ok(None),
        };
        let selection = self
            .locked_prefix
            .pop()
            .expect("the stack was checked above");
        self.bump_composition_epoch();
        self.facts.retracted = Some(if restore {
            SelectionRetraction::Restore
        } else {
            SelectionRetraction::Drop
        });
        if !restore {
            self.refresh()?;
            return Ok(Some(self.transition(empty_result(true))));
        }
        self.engine.command(Command::Cancel)?;
        for byte in selection.reading.bytes() {
            self.engine.character(byte, byte.is_ascii_uppercase())?;
        }
        self.refresh()?;
        Ok(Some(self.transition(empty_result(true))))
    }

    fn hold_phrase_progress(
        &mut self,
        picked: bool,
        discard: bool,
        keep_empty: bool,
        keep_for_escape: bool,
        consumed: &str,
        result: &mut EngineResult,
    ) {
        if !self.phrase_preedit
            || scheme_type(self.cached.scheme).is_some_and(|scheme| !scheme.holds_phrase_progress())
        {
            return;
        }
        let composing = !self.cached.editing_text.is_empty();
        if picked && result.has_commit && composing {
            self.locked_prefix.push(LockedSegment {
                reading: consumed.to_owned(),
                text: std::mem::take(&mut result.commit),
            });
            result.has_commit = false;
            self.facts.held_grew = true;
            self.facts.tail_chars = 0;
            if let Some(selection) = self.facts.selection.as_mut() {
                selection.outcome = SelectionOutcome::Held;
            }
            self.bump_composition_epoch();
            return;
        }
        if self.locked_prefix.is_empty() || composing {
            return;
        }
        if keep_for_escape && !discard && !result.has_commit {
            result.handled = true;
            return;
        }
        if keep_empty
            && !discard
            && !result.has_commit
            && self.locked_prefix.iter().any(|seg| !seg.reading.is_empty())
        {
            result.handled = true;
            return;
        }
        let held = self.locked_prefix_text();
        self.locked_prefix.clear();
        self.bump_composition_epoch();
        self.facts.held_flushed = !discard;
        if result.has_commit {
            if !discard {
                result.commit = held + &result.commit;
            }
            return;
        }
        result.handled = true;
        if !discard {
            result.has_commit = true;
            result.commit = held;
        }
    }

    fn output_context(&self) -> OutputContext {
        OutputContext {
            scheme: self.cached.scheme,
            local_mode: self.cached.local_mode.clone(),
            script_conversion: script_conversion(self.cached.scheme, &self.cached.local_mode)
                && !self.commits_emoji_only(),
            typing_statistics: local_mode_counts_as_typing(&self.cached.local_mode),
        }
    }

    fn commits_emoji_only(&self) -> bool {
        const EMOJI: u8 = msime_engine::CandidateSource::Emoji as u8;
        self.locked_prefix.is_empty()
            && self.facts.selection.is_some_and(|selection| {
                selection.source == EMOJI && selection.outcome == SelectionOutcome::Committed
            })
    }

    fn commit_projected_emoji(
        &mut self,
        anchor_word: &str,
        emoji: &str,
    ) -> Result<EngineResult, RuntimeError> {
        self.emoji_mru.record(anchor_word, emoji);
        let mut result = self.discard_composition()?;
        result.handled = true;
        result.has_commit = !emoji.is_empty();
        result.commit = emoji.to_owned();
        Ok(result)
    }

    pub(crate) fn is_neural_injected(&self, seat: usize) -> bool {
        matches!(
            self.cached.candidates.get(seat).map(|e| &e.origin),
            Some(CandidateOrigin::InjectedBeam)
        )
    }

    fn commit_neural_injected(&mut self, seat: usize) -> Result<EngineResult, RuntimeError> {
        let (text, entry_code) = self
            .cached
            .candidates
            .get(seat)
            .map(|entry| (entry.text.clone(), entry.code.clone()))
            .unwrap_or_default();
        if !text.is_empty() {
            let (learn_code, learn_text) = if !self.locked_prefix.is_empty()
                && self.cached.scheme == msime_engine::SchemeType::Quanpin as u8
                && self.locked_prefix.iter().all(|seg| {
                    seg.text.chars().count() == 1
                        && !seg.reading.is_empty()
                        && seg.reading.bytes().all(|b| b.is_ascii_alphabetic())
                }) {
                let mut code = self
                    .locked_prefix
                    .iter()
                    .map(|seg| seg.reading.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("'");
                if !entry_code.is_empty() {
                    if !code.is_empty() {
                        code.push('\'');
                    }
                    code.push_str(&entry_code);
                }
                (code, format!("{}{}", self.locked_prefix_text(), text))
            } else {
                (entry_code, text.clone())
            };
            let _ = self
                .engine
                .learn_injected_candidate(&learn_code, &learn_text);
        }
        let mut result = self.discard_composition()?;
        result.handled = true;
        result.has_commit = !text.is_empty();
        result.commit = text;
        Ok(result)
    }

    fn select_seat(&mut self, seat: usize) -> Result<EngineResult, RuntimeError> {
        if let Some((anchor_word, emoji)) = self.projected_emoji_at(seat) {
            return self.commit_projected_emoji(&anchor_word, emoji);
        }
        if self.is_neural_injected(seat) {
            return self.commit_neural_injected(seat);
        }
        self.engine.select(self.engine_index(seat))
    }

    fn finish_seat(&mut self, seat: usize) -> Result<EngineResult, RuntimeError> {
        if let Some((anchor_word, emoji)) = self.projected_emoji_at(seat) {
            return self.commit_projected_emoji(&anchor_word, emoji);
        }
        if self.is_neural_injected(seat) {
            return self.commit_neural_injected(seat);
        }
        self.engine.finish(self.engine_index(seat))
    }

    fn transition(&mut self, result: EngineResult) -> Transition {
        if result.has_commit {
            self.remember_commit(&result.commit);
        }
        Transition {
            commit_context: result.has_commit.then(|| self.output_context()),
            handled: result.handled,
            commit: result.has_commit.then_some(result.commit),
            diagnostic: (!result.diagnostic.is_empty()).then_some(result.diagnostic),
            view: self.view(),
        }
    }

    fn normalize_online_slots(&mut self) -> bool {
        const CLOUD: u8 = 2;
        const CLOUD_PENDING: u8 = 15;
        const AI: u8 = 3;
        const ENGLISH: u8 = 4;
        const EMOJI: u8 = 6;
        const KAOMOJI: u8 = 7;
        const AI_PENDING: u8 = 12;
        let is_ai = |source: u8| matches!(source, AI | AI_PENDING);

        let snapshot = &self.cached;
        let count = snapshot.candidates.len();
        if count < 2 {
            return false;
        }
        if !snapshot
            .candidates
            .iter()
            .any(|entry| entry.source == CLOUD || is_ai(entry.source))
        {
            return false;
        }
        let mut order_scratch = std::mem::take(&mut self.online_order_scratch);

        const LOCAL_GROUP: usize = 0;
        const CLOUD_GROUP: usize = 1;
        const AI_GROUP: usize = 2;
        const ENGLISH_GROUP: usize = 3;
        const EMOJI_GROUP: usize = 4;
        const KAOMOJI_GROUP: usize = 5;
        let group_of = |source: u8| match source {
            CLOUD | CLOUD_PENDING => CLOUD_GROUP,
            AI | AI_PENDING => AI_GROUP,
            ENGLISH => ENGLISH_GROUP,
            EMOJI => EMOJI_GROUP,
            KAOMOJI => KAOMOJI_GROUP,
            _ => LOCAL_GROUP,
        };
        let mut counts = [0; 6];
        for entry in &snapshot.candidates {
            counts[group_of(entry.source)] += 1;
        }
        let has_cloud = counts[CLOUD_GROUP] > 0;
        let ai_count = counts[AI_GROUP];
        let non_ai = count - ai_count;
        let contact_prefix = snapshot
            .candidates
            .iter()
            .take_while(|entry| entry.source == msime_engine::CandidateSource::Contact as u8)
            .count();
        let ai_seat = self
            .page_size
            .saturating_sub(1)
            .max(1)
            .max(contact_prefix)
            .min(non_ai);

        let mut head_starts = [0; 6];
        let mut head_counts = [0; 6];
        let mut tail_starts = [0; 6];
        if has_cloud {
            let promoted_english =
                counts[LOCAL_GROUP] > 0 && snapshot.candidates[0].source == ENGLISH;
            let local_prefix = 1usize.max(contact_prefix);

            let mut next_seat = 0;
            let mut head = |group: usize, limit: usize| {
                let limit = if group == ENGLISH_GROUP {
                    let following = if promoted_english {
                        counts[LOCAL_GROUP].min(local_prefix) + counts[CLOUD_GROUP]
                    } else {
                        0
                    };
                    limit.min(
                        self.page_size
                            .saturating_sub(next_seat + following + usize::from(ai_count > 0))
                            .max(1),
                    )
                } else {
                    limit
                };
                head_starts[group] = next_seat;
                head_counts[group] = counts[group].min(limit);
                next_seat += head_counts[group];
            };
            if promoted_english {
                head(ENGLISH_GROUP, self.english_first_page_count);
            }
            head(LOCAL_GROUP, local_prefix);
            head(CLOUD_GROUP, counts[CLOUD_GROUP]);
            if !promoted_english {
                head(ENGLISH_GROUP, self.english_first_page_count);
            }
            head(EMOJI_GROUP, 1);
            head(KAOMOJI_GROUP, 1);
            for group in [LOCAL_GROUP, ENGLISH_GROUP, EMOJI_GROUP, KAOMOJI_GROUP] {
                tail_starts[group] = next_seat;
                next_seat += counts[group] - head_counts[group];
            }
            debug_assert_eq!(next_seat, non_ai);
        }

        let mut order = None;
        let mut seen = [0; 6];
        let mut arranged_without_ai = 0;
        for (index, entry) in snapshot.candidates.iter().enumerate() {
            let group = group_of(entry.source);
            let ordinal = seen[group];
            seen[group] += 1;
            let seat = if group == AI_GROUP {
                if ordinal == 0 {
                    ai_seat
                } else {
                    non_ai + ordinal
                }
            } else {
                let arranged = if !has_cloud {
                    arranged_without_ai
                } else if ordinal < head_counts[group] {
                    head_starts[group] + ordinal
                } else {
                    tail_starts[group] + ordinal - head_counts[group]
                };
                arranged_without_ai += 1;
                arranged + usize::from(ai_count > 0 && arranged >= ai_seat)
            };
            if seat != index && order.is_none() {
                let mut candidate_order = std::mem::take(&mut order_scratch);
                candidate_order.clear();
                candidate_order.extend(0..count);
                order = Some(candidate_order);
            }
            if let Some(order) = &mut order {
                order[seat] = index;
            }
        }
        let Some(mut order) = order else {
            self.online_order_scratch = order_scratch;
            return false;
        };
        preserve_fixed_seats_in_order(&snapshot.candidates, &mut order);
        debug_assert_eq!(order.len(), count);
        if order.len() != count || order.iter().enumerate().all(|(seat, index)| seat == *index) {
            self.online_order_scratch = order;
            return false;
        }
        self.apply_candidate_order(&order);
        self.online_order_scratch = order;
        true
    }

    fn order_presentation(&mut self) -> bool {
        if self.cached.prediction {
            return false;
        }
        let mut moved = self.demote_runner_up_readings();
        moved |= self.normalize_online_slots();
        moved
    }

    pub(crate) fn is_projected_emoji(&self, seat: usize) -> bool {
        matches!(
            self.cached.candidates.get(seat).map(|e| &e.origin),
            Some(CandidateOrigin::ProjectedEmoji { .. })
        )
    }

    fn projected_emoji_at(&self, seat: usize) -> Option<(String, &'static str)> {
        match self.cached.candidates.get(seat).map(|e| &e.origin) {
            Some(CandidateOrigin::ProjectedEmoji { anchor_word, emoji }) => {
                Some((anchor_word.clone(), emoji))
            }
            _ => None,
        }
    }

    fn strip_projected_emojis(&mut self) -> bool {
        if !self
            .cached
            .candidates
            .iter()
            .any(|entry| matches!(entry.origin, CandidateOrigin::ProjectedEmoji { .. }))
        {
            return false;
        }
        let mut removed_before_highlight = 0usize;
        let mut seat = 0usize;
        let highlighted = self.highlighted;
        self.cached.candidates.retain(|entry| {
            let is_emoji = matches!(entry.origin, CandidateOrigin::ProjectedEmoji { .. });
            if is_emoji && seat < highlighted {
                removed_before_highlight += 1;
            }
            seat += 1;
            !is_emoji
        });
        self.highlighted = self.highlighted.saturating_sub(removed_before_highlight);
        true
    }

    fn apply_slot_allocator(&mut self) -> bool {
        self.strip_projected_emojis();
        if !self.inline_emoji_projection
            || self.cached.local_mode != "none"
            || self.cached.dedicated_english
            || self.cached.prediction
            || !self.cached.conversion.is_empty()
            || self.cached.caret_edit_suffix
            || self.cached.caret_position != self.cached.editing_text.len()
            || self.cached.editing_text.len() < 2
            || self
                .cached
                .editing_text
                .bytes()
                .any(|byte| byte.is_ascii_uppercase())
            || self.cached.preedit.contains('｜')
            || self.cached.candidates.is_empty()
        {
            return false;
        }
        self.emoji_mru.ensure_loaded();
        if let Some(engine_meta) = self.engine.neural_v2_metadata() {
            for entry in &mut self.cached.candidates {
                if let CandidateOrigin::EngineRow(idx) = entry.origin {
                    if let Some(meta) = engine_meta.candidates.get(idx) {
                        let fixed_pos = if meta.fixed_position > 0 {
                            meta.fixed_position
                        } else if meta.pinned {
                            1
                        } else {
                            0
                        };
                        if fixed_pos > 0 && entry.fixed_position == 0 {
                            entry.fixed_position = fixed_pos;
                        }
                        if fixed_pos == 1 {
                            entry.pinned_first = true;
                        }
                    }
                }
            }
        }
        let prev_len = self.cached.candidates.len();
        let cursor = self.cursor_snapshot();
        let old = std::mem::take(&mut self.cached.candidates);
        self.cached.candidates =
            PurePresenter::present(old, &cursor, |word| self.emoji_mru.preferred(word));
        self.cached.candidates.len() != prev_len
    }

    pub(crate) fn demote_runner_up_readings(&mut self) -> bool {
        if !self.cached.conversion.is_empty() {
            return false;
        }
        let snapshot = &self.cached;
        if snapshot.candidates.len() < 2 {
            return false;
        }
        let Some(mut order) = runner_up_order(snapshot.scheme, &order_rows(snapshot)) else {
            return false;
        };
        // 全部 L4 固定/置顶席位不得被次选整句后移逻辑挤离指定位置。
        let pinned_first_meta = self
            .engine
            .neural_v2_metadata()
            .and_then(|m| m.candidates.get(self.engine_index(0)).map(|c| c.pinned))
            .unwrap_or(false);
        preserve_fixed_seats_in_order(&snapshot.candidates, &mut order);
        if pinned_first_meta && order.first().copied() != Some(0) {
            if let Some(pos) = order.iter().position(|&idx| idx == 0) {
                order[..=pos].rotate_right(1);
            }
        }
        if order.iter().enumerate().all(|(seat, index)| seat == *index) {
            return false;
        }
        self.apply_candidate_order(&order);
        true
    }

    pub(crate) fn refresh_online_candidates(&mut self) -> Result<(), RuntimeError> {
        let mut translations = std::mem::take(&mut self.translations);
        let mut highlight_translations = std::mem::take(&mut self.highlight_translations);
        retain_candidate_translations(&mut translations, &self.cached.candidates);
        retain_candidate_translations(&mut highlight_translations, &self.cached.candidates);
        self.refresh()?;
        retain_candidate_translations(&mut translations, &self.cached.candidates);
        retain_candidate_translations(&mut highlight_translations, &self.cached.candidates);
        self.translations = translations;
        self.highlight_translations = highlight_translations;
        Ok(())
    }

    fn keep_highlight_on_same_candidate(
        &mut self,
        previous_highlight: usize,
        text: &str,
        code: Option<&str>,
    ) {
        let visible = self.visible_len();
        if visible == 0 {
            self.highlighted = 0;
            return;
        }
        let page_size = self.page_size.max(1);
        let page = previous_highlight / page_size;
        let page_start = page * page_size;
        let found = self
            .cached
            .candidates
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                *index < visible
                    && *index / page_size >= page
                    && entry.text == text
                    && code.is_none_or(|expected| entry.code == expected)
            })
            .min_by_key(|(index, _)| index.abs_diff(previous_highlight))
            .map(|(index, _)| index);
        self.highlighted = found.unwrap_or(page_start).min(visible - 1);
    }

    pub(crate) fn refresh(&mut self) -> Result<(), RuntimeError> {
        self.snapshot_valid = false;
        self.presentation_phase = PresentationPhase::Idle;
        let had_translations =
            !self.translations.is_empty() || !self.highlight_translations.is_empty();
        self.translations.clear();
        self.highlight_translations.clear();
        self.neural_pending_request = None;
        let previous_highlight = self.highlighted;
        self.highlighted = 0;
        let snapshot = match self.engine.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.cached = EngineSnapshot {
                    scheme: 255,
                    local_mode: "unknown".into(),
                    ..EngineSnapshot::default()
                };
                self.bump_composition_epoch();
                self.bump_candidates_epoch();
                return Err(error);
            }
        };
        let previous = std::mem::replace(&mut self.cached, snapshot);
        self.order_presentation();
        self.apply_slot_allocator();
        self.snapshot_valid = true;
        let composition_held = self.cached.editing_text == previous.editing_text
            && self.cached.preedit == previous.preedit
            && self.cached.scheme == previous.scheme
            && self.cached.local_mode == previous.local_mode
            && self.cached.reading == previous.reading
            && self.cached.dedicated_english == previous.dedicated_english
            && self.cached.conversion == previous.conversion
            && self.cached.conversion_focus_start == previous.conversion_focus_start;
        if composition_held {
            let candidates_held = self.cached.candidates == previous.candidates;
            if candidates_held {
                self.highlighted = previous_highlight.min(self.visible_len().saturating_sub(1));
            } else if previous_highlight > 0 {
                if let Some(entry) = previous.candidates.get(previous_highlight) {
                    self.keep_highlight_on_same_candidate(
                        previous_highlight,
                        &entry.text,
                        Some(&entry.code),
                    );
                }
            }
        }
        let cached = &self.cached;
        if cached.preedit != previous.preedit
            || cached.editing_text != previous.editing_text
            || cached.caret_position != previous.caret_position
            || cached.reading != previous.reading
            || cached.display_reading != previous.display_reading
            || cached.conversion != previous.conversion
            || cached.conversion_focus_start != previous.conversion_focus_start
            || cached.conversion_focus_end != previous.conversion_focus_end
            || cached.caret_edit_suffix != previous.caret_edit_suffix
            || cached.caret_edit_prefix != previous.caret_edit_prefix
        {
            self.bump_composition_epoch();
        }
        let cached = &self.cached;
        if had_translations
            || self.highlighted != previous_highlight
            || cached.candidates != previous.candidates
            || cached.prediction != previous.prediction
        {
            self.bump_candidates_epoch();
        }
        Ok(())
    }

    pub fn focus(&mut self, focused: bool) -> Result<Transition, RuntimeError> {
        self.facts = DispatchFacts::default();
        self.selected_text.clear();
        self.advance()?;
        self.focused = false;
        if focused && !self.locked_prefix.is_empty() {
            self.locked_prefix.clear();
            self.bump_composition_epoch();
        }
        let commits_on_blur = !focused
            && scheme_type(self.cached.scheme).is_some_and(SchemeType::commits_on_blur)
            && !self.cached.dedicated_english
            && self.cached.local_mode == "none"
            && !self.cached.editing_text.is_empty();
        let commit_context = self.output_context();
        let result = if commits_on_blur {
            self.engine.finish(0)
        } else {
            self.discard_composition()
        };
        let may_commit = result
            .as_ref()
            .is_ok_and(|result| result.has_commit || (!focused && !self.locked_prefix.is_empty()));
        let refresh_error = match self.refresh() {
            Ok(()) => None,
            Err(error) if may_commit => Some(error),
            Err(error) => return Err(error),
        };
        let mut result = result?;
        if let Some(error) = refresh_error {
            result.diagnostic = format!("Candidate refresh failed: {error}");
        }
        self.hold_phrase_progress(false, false, false, false, "", &mut result);
        self.focused = focused;
        self.presentation_phase = PresentationPhase::Idle;
        self.ai_context.clear();
        self.neural_context.clear();
        self.neural_right_context.clear();
        self.engine.set_rescoring_context("");
        self.engine.reset_context();
        let mut transition = self.transition(result);
        if transition.commit.is_some() {
            transition.commit_context = Some(commit_context);
        }
        Ok(transition)
    }

    fn discard_composition(&mut self) -> Result<EngineResult, RuntimeError> {
        let result = self.engine.command(Command::Cancel)?;
        if scheme_type(self.cached.scheme).is_some_and(SchemeType::cancel_keeps_composition)
            && !self.engine.snapshot()?.editing_text.is_empty()
        {
            return self.engine.command(Command::Cancel);
        }
        Ok(result)
    }

    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        if self.cached.spelling_symbols.as_bytes().contains(&value)
            && (self.cached.local_mode != "none" || self.locked_prefix.is_empty())
        {
            return self.engine.character(value, false);
        }
        if value == b'\'' && self.engine.takes_local_separator() {
            return self.engine.character(value, false);
        }
        if self.bare_mode_prefix() {
            return self.engine.punctuation(value);
        }
        let mut finished = self.finish_seat(self.highlighted)?;
        self.record_selection(
            SelectionHow::PunctuationFinish,
            self.highlighted,
            &finished,
            None,
        );
        let punctuation = match self.engine.punctuation(value) {
            Ok(result) => result,
            Err(error) if finished.has_commit => {
                finished.commit.push(char::from(value));
                self.facts.tail_chars = 1;
                finished.handled = true;
                finished.diagnostic =
                    format!("{} Punctuation failed: {error}", finished.diagnostic)
                        .trim()
                        .to_owned();
                return Ok(finished);
            }
            Err(error) => return Err(error),
        };
        if !finished.has_commit {
            if !punctuation.handled && !self.locked_prefix.is_empty() {
                self.facts.tail_chars = 1;
                return Ok(literal_mark(value, punctuation.diagnostic));
            }
            if punctuation.has_commit {
                self.facts.tail_chars = char_count(&punctuation.commit);
            }
            return Ok(punctuation);
        }
        finished.handled = true;
        if punctuation.has_commit {
            finished.commit.push_str(&punctuation.commit);
            self.facts.tail_chars = char_count(&punctuation.commit);
        } else if !punctuation.handled {
            finished.commit.push(char::from(value));
            self.facts.tail_chars = 1;
        }
        if !punctuation.diagnostic.is_empty() {
            finished.diagnostic = format!("{} {}", finished.diagnostic, punctuation.diagnostic)
                .trim()
                .to_owned();
        }
        Ok(finished)
    }

    fn bare_mode_prefix(&self) -> bool {
        matches!(self.cached.local_mode.as_str(), "command" | "mention")
            && self.cached.editing_text.len() == 1
    }

    fn punctuation_ascii(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        let spells = self.cached.local_mode != "none"
            || (self.locked_prefix.is_empty()
                && (scheme_type(self.cached.scheme)
                    .is_some_and(|scheme| !scheme.opens_table_modes())
                    || (!self.cached.editing_text.is_empty()
                        && !matches!(value, b'/' | b'@' | b'.' | b':'))));
        if spells && self.cached.spelling_symbols.as_bytes().contains(&value) {
            return self.engine.character(value, false);
        }
        if value == b'\'' && self.engine.takes_local_separator() {
            return self.engine.character(value, false);
        }
        let mut finished = if self.bare_mode_prefix() {
            let raw = self.engine.command(Command::CommitRaw)?;
            self.facts.raw = raw.has_commit;
            raw
        } else {
            let finished = self.finish_seat(self.highlighted)?;
            self.record_selection(
                SelectionHow::PunctuationFinish,
                self.highlighted,
                &finished,
                None,
            );
            finished
        };
        if !finished.has_commit {
            if !self.locked_prefix.is_empty() {
                self.facts.tail_chars = 1;
                return Ok(literal_mark(value, finished.diagnostic));
            }
            return Ok(finished);
        }
        finished.handled = true;
        finished.commit.push(char::from(value));
        self.facts.tail_chars = 1;
        Ok(finished)
    }

    pub fn dispatch(&mut self, action: Action) -> Result<Transition, RuntimeError> {
        self.dispatch_action(action)
    }

    pub(crate) fn dispatch_action(&mut self, action: Action) -> Result<Transition, RuntimeError> {
        self.facts = DispatchFacts::default();
        self.selected_text.clear();
        if matches!(&action, Action::Punctuation(value) | Action::PunctuationAscii(value) if !value.is_ascii_punctuation())
        {
            return Err(RuntimeError::InvalidPunctuation);
        }
        if !self.focused && !matches!(&action, Action::ResetCache) {
            return Ok(self.transition(empty_result(false)));
        }
        if self.cached.prediction
            && matches!(
                &action,
                Action::NextCandidate
                    | Action::PreviousCandidate
                    | Action::NextPage
                    | Action::PreviousPage
                    | Action::FirstCandidate
                    | Action::FirstCandidateOrHome
                    | Action::LastCandidate
                    | Action::Command(
                        Command::MoveLeft
                            | Command::MoveRight
                            | Command::MoveHome
                            | Command::MoveEnd
                            | Command::ConversionLeft
                            | Command::ConversionRight
                    )
                    | Action::SegmentMoveLeft
                    | Action::SegmentMoveRight
            )
        {
            self.advance()?;
            self.engine.command(Command::Cancel)?;
            self.refresh()?;
            return Ok(self.transition(empty_result(false)));
        }
        let action = self.candidate_shortcut(action);
        let action = self.caret_edge_candidate_shortcut(action);
        let action = self.home_key_action(action);
        if let Action::SelectAnyCandidate(id) = &action {
            if id.session != self.session
                || id.generation != self.generation
                || id.index >= self.visible_len()
            {
                return Err(RuntimeError::StaleCandidate);
            }
        }
        if let Action::Select(id)
        | Action::SelectEdge(id, _)
        | Action::PinCandidate(id)
        | Action::RemoveCandidate(id)
        | Action::FixCandidatePosition(id, _)
        | Action::ClearCandidatePosition(id) = &action
        {
            let start = (self.highlighted / self.page_size) * self.page_size;
            if id.session != self.session
                || id.generation != self.generation
                || id.index < start
                || id.index >= (start + self.page_size).min(self.visible_len())
            {
                return Err(RuntimeError::StaleCandidate);
            }
        }
        self.advance()?;
        let filled_current_page =
            matches!(action, Action::NextPage) && self.expand_for_next_page()?;
        if matches!(action, Action::NextCandidate) {
            self.expand_for_next_candidate()?;
        }
        if matches!(action, Action::LastCandidate) {
            self.expand_cached_candidates()?;
        }
        let len = self.visible_len();
        let next_highlight = match &action {
            Action::NextPage if filled_current_page => Some(self.highlighted),
            Action::NextPage if len > 0 => Some(
                (self.highlighted / self.page_size + 1).min((len - 1) / self.page_size)
                    * self.page_size,
            ),
            Action::PreviousPage if len > 0 => {
                Some((self.highlighted / self.page_size).saturating_sub(1) * self.page_size)
            }
            Action::NextCandidate if len > 0 => Some((self.highlighted + 1).min(len - 1)),
            Action::PreviousCandidate if len > 0 => Some(self.highlighted.saturating_sub(1)),
            Action::FirstCandidate if len > 0 => Some(0),
            Action::LastCandidate if len > 0 => Some(len - 1),
            _ => None,
        };
        if let Some(index) = next_highlight {
            if index != self.highlighted {
                self.bump_candidates_epoch();
            }
            self.highlighted = index;
            self.flush_provisional();
            return Ok(self.transition(empty_result(true)));
        }
        if let Some(transition) = self.retreat_phrase_selection(&action)? {
            return Ok(transition);
        }
        let selection_action = matches!(
            &action,
            Action::Character { .. }
                | Action::Select(_)
                | Action::SelectAnyCandidate(_)
                | Action::SelectEdge(..)
                | Action::SelectHighlighted
                | Action::Command(Command::CommitCandidate)
        );
        let reading_before =
            (self.phrase_preedit && selection_action).then(|| self.cached.editing_text.clone());
        let mut selected_by_digit = false;
        let mut pick: Option<(SelectionHow, usize)> = None;
        let mut raw_route = false;
        let escape_keeps_word = self.escape_keeps_selected_word
            && matches!(action, Action::Command(Command::Cancel))
            && !self.cached.editing_text.is_empty()
            && !self.locked_prefix.is_empty();
        let result = match action {
            Action::ResetCache => {
                self.engine.reset_cache()?;
                Ok(EngineResult {
                    handled: true,
                    has_commit: false,
                    commit: String::new(),
                    diagnostic: String::new(),
                })
            }
            Action::Punctuation(value) => self.punctuation(value),
            Action::PunctuationAscii(value) => self.punctuation_ascii(value),
            Action::Finish | Action::FinishOnFocusLoss if self.bare_mode_prefix() => {
                raw_route = true;
                self.engine.command(Command::CommitRaw)
            }
            Action::FinishOnFocusLoss if self.is_projected_emoji(self.highlighted) => {
                let seat = self.first_engine_seat();
                pick = Some((SelectionHow::Finish, seat));
                self.engine.finish(self.engine_index(seat))
            }
            Action::Finish | Action::FinishOnFocusLoss => {
                pick = Some((SelectionHow::Finish, self.highlighted));
                self.finish_seat(self.highlighted)
            }
            Action::Character { value, .. }
                if !self.locked_prefix.is_empty()
                    && self.cached.local_mode == "none"
                    && self.cached.spelling_symbols.as_bytes().contains(&value) =>
            {
                self.punctuation(value)
            }
            Action::Character { value, shift } => {
                self.engine.character(value, shift).and_then(|result| {
                    if !result.handled && !result.has_commit && value.is_ascii_punctuation() {
                        return self.punctuation(value);
                    }
                    let Some(slot) = candidate_digit_slot(value, self.page_size) else {
                        return Ok(result);
                    };
                    if result.handled
                        || result.has_commit
                        || (self.cached.local_mode == "none"
                            && self.cached.spelling_symbols.as_bytes().contains(&value))
                        || self.cached.prediction
                        || len == 0
                    {
                        return Ok(result);
                    }
                    let page_start = (self.highlighted / self.page_size) * self.page_size;
                    if slot >= self.page_size || page_start + slot >= len {
                        return Ok(empty_result(true));
                    }
                    selected_by_digit = true;
                    pick = Some((SelectionHow::Digit, page_start + slot));
                    self.select_seat(page_start + slot)
                })
            }
            Action::Command(Command::ConversionLeft) if self.cached.conversion.is_empty() => {
                let seat = if self.is_projected_emoji(self.highlighted)
                    || self.is_neural_injected(self.highlighted)
                {
                    self.first_engine_seat()
                } else {
                    self.highlighted
                };
                self.engine.conversion_left_from(self.engine_index(seat))
            }
            Action::Command(Command::CommitCandidate)
                if len > 0 && !self.cached.prediction && self.cached.conversion.is_empty() =>
            {
                pick = Some((SelectionHow::FirstCandidateCommand, 0));
                self.select_seat(0)
            }
            Action::Command(Command::CommitCandidate) => {
                self.engine.command(Command::CommitCandidate)
            }
            Action::Command(command) => {
                raw_route = matches!(
                    command,
                    Command::CommitRaw | Command::CommitRawWithoutLearning
                );
                self.engine.command(command)
            }
            Action::SegmentBackspace => self.engine.segment_command(SegmentCommand::Backspace),
            Action::SegmentMoveLeft => self.engine.segment_command(SegmentCommand::MoveLeft),
            Action::SegmentMoveRight => self.engine.segment_command(SegmentCommand::MoveRight),
            Action::Select(id) => {
                pick = Some((SelectionHow::Select, id.index));
                self.select_seat(id.index)
            }
            Action::SelectAnyCandidate(id) => {
                pick = Some((SelectionHow::AnyCandidate, id.index));
                self.select_seat(id.index)
            }
            Action::SelectEdge(id, _)
            | Action::PinCandidate(id)
            | Action::RemoveCandidate(id)
            | Action::FixCandidatePosition(id, _)
            | Action::ClearCandidatePosition(id)
                if self.is_projected_emoji(id.index) || self.is_neural_injected(id.index) =>
            {
                Ok(empty_result(true))
            }
            Action::SelectEdge(id, edge) => {
                pick = Some((SelectionHow::Edge, id.index));
                self.engine.select_edge(self.engine_index(id.index), edge)
            }
            Action::PinCandidate(id) => self.engine.pin_candidate(self.engine_index(id.index)),
            Action::RemoveCandidate(id) => {
                self.engine.remove_candidate(self.engine_index(id.index))
            }
            Action::FixCandidatePosition(id, position) => self
                .engine
                .fix_candidate_position(self.engine_index(id.index), position),
            Action::ClearCandidatePosition(id) => self
                .engine
                .clear_candidate_position(self.engine_index(id.index)),
            Action::SelectHighlighted
                if self.cached.spelling_symbols.as_bytes().contains(&b' ') =>
            {
                self.engine.character(b' ', false)
            }
            Action::SelectHighlighted if len > 0 && !self.cached.prediction => {
                pick = Some((SelectionHow::Highlighted, self.highlighted));
                self.select_seat(self.highlighted)
            }
            Action::SelectHighlighted => self.engine.command(Command::CommitCandidate),
            _ => return Ok(self.transition(empty_result(false))),
        };
        if let Ok(result) = &result {
            if let Some((how, seat)) = pick {
                self.record_selection(
                    how,
                    seat,
                    result,
                    match action {
                        Action::SelectEdge(_, edge) => Some(edge),
                        _ => None,
                    },
                );
            }
            if raw_route {
                self.facts.raw = result.has_commit;
            }
        }
        let needs_commit_context =
            result.as_ref().is_ok_and(|result| result.has_commit) || !self.locked_prefix.is_empty();
        let commit_context = needs_commit_context.then(|| self.output_context());
        let refresh = self.refresh();
        let mut result = result?;
        if let Err(error) = refresh {
            result.diagnostic = format!("Candidate refresh failed: {error}");
        }
        let consumed = reading_before
            .as_deref()
            .and_then(|reading| reading.strip_suffix(self.cached.editing_text.as_str()))
            .unwrap_or("");
        let picked = selected_by_digit || selection_action;
        let discarded = matches!(action, Action::Command(Command::Cancel)) && !escape_keeps_word;
        let keep_empty = matches!(
            action,
            Action::SegmentBackspace | Action::SegmentMoveLeft | Action::SegmentMoveRight
        );
        self.hold_phrase_progress(
            picked,
            discarded,
            keep_empty,
            escape_keeps_word,
            consumed,
            &mut result,
        );
        if let Some(selection) = self.facts.selection.as_mut() {
            if selection.outcome == SelectionOutcome::Committed
                && !self.cached.editing_text.is_empty()
            {
                selection.outcome = SelectionOutcome::Partial;
            }
        }
        let mut transition = self.transition(result);
        if let Some(commit_context) = commit_context {
            if transition.commit.is_some() {
                transition.commit_context = Some(commit_context);
            }
        }
        Ok(transition)
    }
}

fn literal_mark(value: u8, diagnostic: String) -> EngineResult {
    EngineResult {
        handled: true,
        has_commit: true,
        commit: char::from(value).to_string(),
        diagnostic,
    }
}

fn char_count(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

fn candidate_digit_slot(value: u8, page_size: usize) -> Option<usize> {
    match value {
        b'1'..=b'9' => Some(usize::from(value - b'1')),
        b'0' if page_size >= usize::from(MAX_CANDIDATE_PAGE_SIZE) => Some(9),
        _ => None,
    }
}

pub(crate) fn empty_result(handled: bool) -> EngineResult {
    EngineResult {
        handled,
        has_commit: false,
        commit: String::new(),
        diagnostic: String::new(),
    }
}

fn sanitize_neural_context(text: &str) -> std::borrow::Cow<'_, str> {
    let allowed_char =
        |c: char| c != '\u{fffc}' && (!c.is_control() || matches!(c, '\n' | '\r' | '\t'));
    if text.chars().all(allowed_char) {
        return std::borrow::Cow::Borrowed(msime_engine::neural_v2::neural_context_window(text));
    }
    let cleaned: String = text.chars().filter(|&c| allowed_char(c)).collect();
    let tail = msime_engine::neural_v2::neural_context_window(&cleaned);
    if tail.len() == cleaned.len() {
        std::borrow::Cow::Owned(cleaned)
    } else {
        std::borrow::Cow::Owned(tail.to_owned())
    }
}

fn sanitize_neural_right_context(text: &str) -> std::borrow::Cow<'_, str> {
    let head = msime_engine::neural_v2::neural_right_context_window(text);
    if head.is_empty() {
        return std::borrow::Cow::Borrowed("");
    }
    let allowed_char = |c: char| c != '\u{fffc}' && (!c.is_control() || c == '\t');
    if head.chars().all(allowed_char) {
        return std::borrow::Cow::Borrowed(head);
    }
    let cleaned: String = head.chars().filter(|&c| allowed_char(c)).collect();
    let trimmed = msime_engine::neural_v2::neural_right_context_window(&cleaned);
    std::borrow::Cow::Owned(trimmed.to_owned())
}

fn sanitize_surrounding_context(text: &str) -> std::borrow::Cow<'_, str> {
    const MAX_BYTES: usize = 1024;
    let allowed_char =
        |c: char| c != '\u{fffc}' && (!c.is_control() || matches!(c, '\n' | '\r' | '\t'));
    if text.chars().all(allowed_char) {
        if text.len() <= MAX_BYTES {
            return std::borrow::Cow::Borrowed(text);
        }
        let mut cut = text.len() - MAX_BYTES;
        while cut < text.len() && !text.is_char_boundary(cut) {
            cut += 1;
        }
        return std::borrow::Cow::Borrowed(&text[cut..]);
    }
    let mut cleaned: String = text.chars().filter(|&c| allowed_char(c)).collect();
    if cleaned.len() > MAX_BYTES {
        let mut cut = cleaned.len() - MAX_BYTES;
        while cut < cleaned.len() && !cleaned.is_char_boundary(cut) {
            cut += 1;
        }
        cleaned.drain(..cut);
    }
    std::borrow::Cow::Owned(cleaned)
}

/// 保持排列 `order`（新座位 -> 旧座位）中所有 L4 固定/置顶席位（`effective_fixed_position() > 0`）稳坐目标席位 `pos - 1`，
/// 其余未固定行按 `order` 原先的相对顺序顺流填入空余席位，杜绝次选降位或在线槽位重排挤离用户固定位置。
fn preserve_fixed_seats_in_order(candidates: &[CandidateEntry], order: &mut [usize]) {
    let count = order.len();
    if count < 2
        || candidates.len() != count
        || candidates
            .iter()
            .all(|entry| entry.effective_fixed_position() == 0)
    {
        return;
    }
    let mut fixed_entries: Vec<(usize, usize)> = Vec::new();
    for (old_idx, entry) in candidates.iter().enumerate() {
        let pos = entry.effective_fixed_position();
        if pos > 0 {
            let target = usize::from(pos - 1).min(count - 1);
            fixed_entries.push((target, old_idx));
        }
    }
    fixed_entries.sort_by_key(|&(target, old_idx)| (target, old_idx));
    let mut reserved: Vec<Option<usize>> = vec![None; count];
    for (target, old_idx) in fixed_entries {
        let mut seat = target;
        while seat < count && reserved[seat].is_some() {
            seat += 1;
        }
        if seat >= count {
            if let Some(free) = reserved.iter().position(|entry| entry.is_none()) {
                seat = free;
            } else {
                continue;
            }
        }
        reserved[seat] = Some(old_idx);
    }
    let unpinned: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&old_idx| {
            candidates
                .get(old_idx)
                .is_none_or(|entry| entry.effective_fixed_position() == 0)
        })
        .collect();
    let mut unpinned_iter = unpinned.into_iter();
    for (seat, slot) in order.iter_mut().enumerate() {
        if let Some(old_idx) = reserved[seat] {
            *slot = old_idx;
        } else if let Some(old_idx) = unpinned_iter.next() {
            *slot = old_idx;
        }
    }
}

fn canonical_app_scope_key(scope_key: &str) -> &'static str {
    match scope_key {
        "im_chat" => "im_chat",
        "code_ide" => "code_ide",
        "docs_writing" => "docs_writing",
        "browser_search" => "browser_search",
        _ => "general",
    }
}
