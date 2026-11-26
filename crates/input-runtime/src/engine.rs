//! The engine boundary: the trait the runtime drives, and the `msime_engine::host::Session` implementation of it.

use super::*;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("candidate page size must be between 1 and 10")]
    InvalidPageSize,
    #[error("candidate belongs to an expired view or another session")]
    StaleCandidate,
    #[error("session identity exhausted")]
    IdentityExhausted,
    #[error("cannot replace an engine while composition is active")]
    CompositionActive,
    #[error("punctuation action requires an ASCII punctuation character")]
    InvalidPunctuation,
    #[error("engine action failed: {0}")]
    Engine(String),
}

pub trait InputEngine {
    /// 隐私输入禁止上屏后联想；不提供该能力的引擎没有状态可清。
    fn set_prediction_allowed(&mut self, _allowed: bool) {}
    /// Ends the committed-word context the Engine learns and ranks against. Engines without one ignore it.
    fn reset_context(&mut self) {}
    /// The committed text the Engine's neural sentence models condition on, pushed by the runtime whenever the text it keeps changes. Engines without such models ignore it.
    fn set_rescoring_context(&mut self, _context: &str) {}
    /// 首页放得下几行，宿主改每页候选数时跟着分页一起下发；没有按切分查词库的引擎忽略它。
    fn set_first_page_rows(&mut self, _rows: usize) {}
    /// Moves the caret prefix decoding answers for; `None` returns to the end of the composition.
    fn set_caret(&mut self, _caret: Option<usize>) {}
    fn prefix_end(&self) -> usize {
        0
    }
    fn pending_suffix(&self) -> String {
        String::new()
    }
    /// Whether an apostrophe is input to the open local mode (`3jin'g`, `/fyhello'world`) rather than punctuation that ends it.
    fn takes_local_separator(&self) -> bool {
        false
    }
    /// The `/fy` translation request, the only request a local mode makes.
    fn command_translation_query(&self) -> Option<CommandTranslationQuery> {
        None
    }
    /// Put a translation of the live `/fy` text first; false for a stale query.
    fn apply_command_translation(
        &mut self,
        _query: &CommandTranslationQuery,
        _translation: &str,
    ) -> bool {
        false
    }
    fn reset_cache(&mut self) -> Result<(), RuntimeError> {
        Err(RuntimeError::Engine(
            "Engine cache reset is unsupported".into(),
        ))
    }
    /// Remove cached rows for one online provider (host source 0 = cloud, 1 = AI).
    fn clear_online_candidates(&mut self, _source: u8) -> Result<(), RuntimeError> {
        Err(RuntimeError::Engine(
            "Online candidate clearing is unsupported".into(),
        ))
    }
    /// Whether the host allows the cloud provider to reserve its stable candidate seat.
    fn set_cloud_candidates_enabled(&mut self, _enabled: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_paired_punctuation_enabled(&mut self, _enabled: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn balance_paired_punctuation_after_auto_close(
        &mut self,
        _opening: u8,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_punctuation_lock(&mut self, _lock: u8) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_dedicated_english(&mut self, _enabled: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Replace the `/` mode's command table.
    fn set_command_table(&mut self, _table: &[CommandTableEntry]) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// 替换 K 模式的宿主短语表。
    fn set_quick_phrase_table(&mut self, _table: &[QuickPhraseEntry]) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Replace the `@` mode's name list.
    fn set_mention_entries(&mut self, _entries: &[MentionEntry]) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Offer the places after the `@` mode's name list.
    fn set_mention_places(&mut self, _enabled: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Ask for candidates withheld from the first answer, reporting whether the list grew.
    fn expand_initial_candidates(&mut self) -> Result<bool, RuntimeError> {
        Ok(false)
    }
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError>;
    /// 为 Tier 1 两阶段异步神经解码提供权威音节路径与候选静态分；未实现时由 runtime 按快照降级推导。
    fn neural_v2_metadata(&self) -> Option<msime_engine::neural_v2::EngineNeuralMetadata> {
        None
    }
    fn character(&mut self, value: u8, shift: bool) -> Result<EngineResult, RuntimeError>;
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError>;
    fn segment_command(&mut self, command: SegmentCommand) -> Result<EngineResult, RuntimeError> {
        let snapshot = self.snapshot()?;
        let has_segment_boundaries = !snapshot.segment_raw_boundaries.is_empty();
        let fallback = match command {
            SegmentCommand::Backspace => Command::Backspace,
            SegmentCommand::MoveLeft => Command::MoveLeft,
            SegmentCommand::MoveRight => Command::MoveRight,
        };
        let caret = snapshot.caret_position as u64;
        let steps = match command {
            SegmentCommand::Backspace | SegmentCommand::MoveLeft => snapshot
                .segment_raw_boundaries
                .iter()
                .rev()
                .find(|&&boundary| boundary < caret)
                .map_or(1, |&boundary| (caret - boundary) as usize),
            SegmentCommand::MoveRight => snapshot
                .segment_raw_boundaries
                .iter()
                .find(|&&boundary| boundary > caret)
                .map_or(1, |&boundary| (boundary - caret) as usize),
        };
        let mut result = EngineResult {
            handled: false,
            has_commit: false,
            commit: String::new(),
            diagnostic: String::new(),
        };
        for _ in 0..steps {
            result = self.command(fallback)?;
        }
        if matches!(command, SegmentCommand::Backspace) && has_segment_boundaries {
            let after = self.snapshot()?;
            if needs_dangling_segment_delimiter_backspace(&after.editing_text, after.caret_position)
            {
                result = self.command(Command::Backspace)?;
            }
        }
        Ok(result)
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError>;
    /// 将运行时选中的受限束搜索/注入多字候选持久化到用户词库与个人叠加层。
    fn learn_injected_candidate(
        &mut self,
        _canonical_pinyin: &str,
        _word: &str,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// 整句改字的左移，进入改字时从引擎的第 `index` 个候选开始。
    fn conversion_left_from(&mut self, _index: usize) -> Result<EngineResult, RuntimeError> {
        self.command(Command::ConversionLeft)
    }
    fn pin_candidate(&mut self, _index: usize) -> Result<EngineResult, RuntimeError> {
        Err(RuntimeError::Engine(
            "Candidate pinning is unsupported".into(),
        ))
    }
    fn remove_candidate(&mut self, _index: usize) -> Result<EngineResult, RuntimeError> {
        Err(RuntimeError::Engine(
            "Candidate removal is unsupported".into(),
        ))
    }
    fn fix_candidate_position(
        &mut self,
        _index: usize,
        _position: u8,
    ) -> Result<EngineResult, RuntimeError> {
        Err(RuntimeError::Engine(
            "Candidate position fixing is unsupported".into(),
        ))
    }
    fn clear_candidate_position(&mut self, _index: usize) -> Result<EngineResult, RuntimeError> {
        Err(RuntimeError::Engine(
            "Candidate position clearing is unsupported".into(),
        ))
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError>;
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError>;
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentCommand {
    Backspace,
    MoveLeft,
    MoveRight,
}

impl InputEngine for Session {
    fn set_prediction_allowed(&mut self, allowed: bool) {
        Session::set_prediction_allowed(self, allowed);
    }
    fn reset_context(&mut self) {
        Session::reset_context(self);
    }
    fn set_rescoring_context(&mut self, context: &str) {
        Session::set_rescoring_context(self, context);
    }
    fn set_first_page_rows(&mut self, rows: usize) {
        Session::set_first_page_rows(self, rows);
    }
    fn set_caret(&mut self, caret: Option<usize>) {
        Session::set_caret(self, caret);
    }
    fn prefix_end(&self) -> usize {
        Session::prefix_end(self)
    }
    fn pending_suffix(&self) -> String {
        Session::pending_suffix(self)
    }
    fn takes_local_separator(&self) -> bool {
        Session::takes_local_separator(self)
    }
    fn command_translation_query(&self) -> Option<CommandTranslationQuery> {
        Session::command_translation_query(self)
    }
    fn apply_command_translation(
        &mut self,
        query: &CommandTranslationQuery,
        translation: &str,
    ) -> bool {
        Session::apply_command_translation(self, query, translation)
    }
    fn reset_cache(&mut self) -> Result<(), RuntimeError> {
        Session::reset_cache(self);
        Ok(())
    }
    fn clear_online_candidates(&mut self, source: u8) -> Result<(), RuntimeError> {
        Session::clear_online_candidates(self, source)
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn set_cloud_candidates_enabled(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        Session::set_cloud_candidates_enabled(self, enabled);
        Ok(())
    }
    fn set_paired_punctuation_enabled(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        Session::set_paired_punctuation_enabled(self, enabled)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn balance_paired_punctuation_after_auto_close(
        &mut self,
        opening: u8,
    ) -> Result<(), RuntimeError> {
        Session::balance_paired_punctuation_after_auto_close(self, opening)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_punctuation_lock(&mut self, lock: u8) -> Result<(), RuntimeError> {
        Session::set_punctuation_lock(self, lock).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_dedicated_english(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        Session::set_dedicated_english(self, enabled)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_command_table(&mut self, table: &[CommandTableEntry]) -> Result<(), RuntimeError> {
        Session::set_command_table(self, table).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_quick_phrase_table(&mut self, table: &[QuickPhraseEntry]) -> Result<(), RuntimeError> {
        Session::set_quick_phrase_table(self, table)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_mention_entries(&mut self, entries: &[MentionEntry]) -> Result<(), RuntimeError> {
        Session::set_mention_entries(self, entries).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn set_mention_places(&mut self, enabled: bool) -> Result<(), RuntimeError> {
        Session::set_mention_places(self, enabled).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn expand_initial_candidates(&mut self) -> Result<bool, RuntimeError> {
        Session::expand_initial_candidates(self).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn punctuation(&mut self, value: u8) -> Result<EngineResult, RuntimeError> {
        Session::punctuation(self, value).map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn snapshot(&self) -> Result<EngineSnapshot, RuntimeError> {
        Session::snapshot(self).map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn neural_v2_metadata(&self) -> Option<msime_engine::neural_v2::EngineNeuralMetadata> {
        Session::neural_v2_metadata(self)
    }
    fn character(&mut self, value: u8, shift: bool) -> Result<EngineResult, RuntimeError> {
        Session::character(self, value, shift)
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn command(&mut self, command: Command) -> Result<EngineResult, RuntimeError> {
        Session::command(self, command).map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn select(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::select(self, index).map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn learn_injected_candidate(
        &mut self,
        canonical_pinyin: &str,
        word: &str,
    ) -> Result<(), RuntimeError> {
        Session::learn_injected_candidate(self, canonical_pinyin, word)
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn conversion_left_from(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::conversion_left_from(self, index)
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn pin_candidate(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::pin_candidate(self, index).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn remove_candidate(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::remove_candidate(self, index).map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn fix_candidate_position(
        &mut self,
        index: usize,
        position: u8,
    ) -> Result<EngineResult, RuntimeError> {
        Session::fix_candidate_position(self, index, position)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn clear_candidate_position(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::clear_candidate_position(self, index)
            .map_err(|e| RuntimeError::Engine(e.to_string()))
    }
    fn finish(&mut self, index: usize) -> Result<EngineResult, RuntimeError> {
        Session::finish(self, index).map_err(|error| RuntimeError::Engine(error.to_string()))
    }
    fn select_edge(
        &mut self,
        index: usize,
        edge: CandidateEdge,
    ) -> Result<EngineResult, RuntimeError> {
        Session::select_edge(self, index, edge)
            .map_err(|error| RuntimeError::Engine(error.to_string()))
    }
}
