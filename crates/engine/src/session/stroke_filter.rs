//! Tab 笔画筛选（`SessionOptions::tab_stroke_filter`），搜狗拼音「Tab 加笔画、部首」的对应物：全拼、双拼组字时按 Tab 进入筛选，之后打的小写字母是筛选码，候选只留第一个字合这串码的那些（词也留，比的是它的第一个字）。一个字合码有两种方式，满足一种就留下：
//!
//! - 笔顺：码里只有 `h` `s` `p` `n` `z`（横竖撇点折）、`d`（点，和 `n` 同义）和 `x`（任意一笔）时，按笔顺比，读 `msime-stroke.db`；
//! - 部件：码是这个字某种拆法的各部件拼音首字母的开头：口读 kou，所以 `k` 找带口的字，`kz` 找 嚏（口、疐），读 `local::chaizi` 编进引擎的拆字表。
//!
//! 一串码只按一种方式读，不在同一串里先比部件再比笔画。
//!
//! 码不进组字原文：原文、光标和 `editing_text` 都不变，只有快照的预编辑在拼音后面接上 `｜` 和码。Backspace 先删码，删光后再按一次退出筛选；大写字母和 Tab 被吞掉；数字、空格、标点和翻页照常。筛选只属于按 Tab 时的那串原文：选中一行、编辑原文、移动光标、进入本地模式或改字都结束它，`update_mixed_candidates` 每次刷新都核对一遍。
//!
//! 一个字母筛不出任何候选时不收它，候选窗不会被筛空。筛掉的行仍留在 `ranking_candidates` 里，学习按它们在原列表里的名次算：靠筛选从后面翻出来的字照常往前调。

use std::collections::HashSet;

use super::input::InputSession;
use crate::diagnostics;
use crate::language_dictionary;
use crate::local::chaizi;
use crate::stroke;
use crate::types::{Command, KeyResult, LocalInputMode, SchemeType, WordItem};

/// 预编辑里码前的分隔，按 Tab 后还没打码时也让人看得出在筛选。
const PREEDIT_MARK: char = '｜';
/// 码最长这么多个字母，与笔画方案的上限相同。
const MAX_CODE: usize = stroke::MAX_STROKES;

#[derive(Debug, Default)]
pub(super) struct StrokeFilter {
    /// 按 Tab 时的组字原文（带大小写）。
    raw: String,
    /// 打过的筛选码，小写字母。
    code: String,
    /// 码能当笔顺读时，笔顺以它开头的字；不能时为 `None`。码为空时不用。
    strokes: Option<HashSet<char>>,
}

impl StrokeFilter {
    fn keeps(&self, row: &WordItem) -> bool {
        row.word.chars().next().is_some_and(|first| {
            self.strokes
                .as_ref()
                .is_some_and(|strokes| strokes.contains(&first))
                || chaizi::components_start_with(first, &self.code)
        })
    }
}

/// 码当笔顺读时的笔画键串（`d` 换成 `n`）；码里有笔画以外的字母时为 `None`。
fn stroke_keys(code: &str) -> Option<String> {
    code.bytes()
        .map(|key| match key {
            b'd' => Some('n'),
            _ if stroke::is_key(key) => Some(char::from(key)),
            _ => None,
        })
        .collect()
}

impl InputSession {
    /// 快照的 `spelling_symbols` 要不要带 Tab：能进筛选，或者已经在筛选里（这时 Tab 被吞掉，不交给宿主翻页）。
    pub(super) fn stroke_filter_takes_tab(&self) -> bool {
        self.stroke_filter
            .as_ref()
            .is_some_and(|filter| self.stroke_filter_applies(filter))
            || self.stroke_filter_available()
    }

    /// Tab 能进入笔画筛选：开关开着、有笔画词库，全拼或双拼在组字、光标在末尾，不在本地模式、专用英文、整句改字、光标后编辑和显式辅助码前导里。双拼开着 Tab 手动辅助码、这时 Tab 能开辅助码时，Tab 归辅助码。
    fn stroke_filter_available(&self) -> bool {
        self.tab_stroke_filter
            && self.stroke_filter.is_none()
            && !self.stroke_dictionary_path.as_os_str().is_empty()
            && matches!(self.scheme(), SchemeType::Quanpin | SchemeType::Shuangpin)
            && self.helpcode_entry.is_none()
            && !self.caret_edit.active
            && self.composing_at_end()
            && self.shuangpin_helpcode_marker(b'\t').is_none()
    }

    fn composing_at_end(&self) -> bool {
        self.local_mode == LocalInputMode::None
            && !self.dedicated_english
            && self.conversion.is_none()
            && self.has_composition()
            && self.caret_position() == self.editing_text_len()
    }

    /// 筛选还属于眼下的组字：原文没变，光标仍在末尾，没有进本地模式、专用英文或改字。
    fn stroke_filter_applies(&self, filter: &StrokeFilter) -> bool {
        self.composing_at_end() && !self.caret_edit.active && filter.raw == self.raw_with_cases()
    }

    /// 筛选里的一键，或者进入筛选的 Tab；`None` 是不归筛选管，照常处理。
    pub(super) fn stroke_filter_character(&mut self, value: u8) -> Option<KeyResult> {
        if self
            .stroke_filter
            .as_ref()
            .is_some_and(|filter| !self.stroke_filter_applies(filter))
        {
            self.stroke_filter = None;
        }
        if self.stroke_filter.is_none() {
            return (value == b'\t' && self.stroke_filter_available())
                .then(|| self.enter_stroke_filter());
        }
        if value.is_ascii_lowercase() {
            return Some(self.push_filter_key(value));
        }
        // 大写字母和 Tab 不改变筛选也不进组字；数字、空格和标点照常选词、上屏。
        (value.is_ascii_uppercase() || value == b'\t').then(KeyResult::handled)
    }

    /// 筛选里的命令；`None` 是照常处理。Backspace 删一个字母，没有码时退出筛选；移动光标和向后删先退出筛选再照常处理；选词、上屏和取消各自结束筛选。
    pub(super) fn stroke_filter_command(&mut self, command: Command) -> Option<KeyResult> {
        let filter = self.stroke_filter.as_ref()?;
        if !self.stroke_filter_applies(filter) {
            self.stroke_filter = None;
            return None;
        }
        match command {
            Command::Backspace => Some(self.pop_filter_key()),
            Command::MoveLeft
            | Command::MoveRight
            | Command::MoveHome
            | Command::MoveEnd
            | Command::DeleteForward
            | Command::ConversionLeft
            | Command::ConversionRight => {
                self.stroke_filter = None;
                self.update_mixed_candidates();
                None
            }
            _ => None,
        }
    }

    fn enter_stroke_filter(&mut self) -> KeyResult {
        if self.stroke_dictionary.is_none() {
            match language_dictionary::open_read_only(&self.stroke_dictionary_path) {
                Ok(dictionary) => self.stroke_dictionary = Some(dictionary),
                Err(_) => return self.stroke_dictionary_unavailable(),
            }
        }
        // 只有一个字母的首段只给前几十个单字，筛选要在全部单字里找。
        self.expand_initial_candidates();
        self.stroke_filter = Some(StrokeFilter {
            raw: self.raw_with_cases().to_owned(),
            ..StrokeFilter::default()
        });
        self.update_mixed_candidates();
        KeyResult::handled()
    }

    fn push_filter_key(&mut self, key: u8) -> KeyResult {
        let Some(filter) = self.stroke_filter.as_ref() else {
            return KeyResult::unhandled();
        };
        if filter.code.len() >= MAX_CODE {
            return KeyResult::handled();
        }
        let mut next = StrokeFilter {
            raw: String::new(),
            code: filter.code.clone(),
            strokes: None,
        };
        next.code.push(char::from(key));
        match self.strokes_for(&next.code) {
            Ok(strokes) => next.strokes = strokes,
            Err(result) => return result,
        }
        // 筛完一个都不剩的字母不收：多半是笔顺或部件记错了，留着原来的候选让人改。
        if !self
            .unfiltered_candidates()
            .iter()
            .any(|row| next.keeps(row))
        {
            return KeyResult::handled();
        }
        if let Some(filter) = self.stroke_filter.as_mut() {
            filter.code = next.code;
            filter.strokes = next.strokes;
        }
        self.update_mixed_candidates();
        KeyResult::handled()
    }

    fn pop_filter_key(&mut self) -> KeyResult {
        let Some(filter) = self.stroke_filter.as_mut() else {
            return KeyResult::unhandled();
        };
        if filter.code.pop().is_none() {
            self.stroke_filter = None;
        } else {
            let code = filter.code.clone();
            match self.strokes_for(&code) {
                Ok(strokes) => {
                    if let Some(filter) = self.stroke_filter.as_mut() {
                        filter.strokes = strokes;
                    }
                }
                Err(result) => {
                    self.stroke_filter = None;
                    self.update_mixed_candidates();
                    return result;
                }
            }
        }
        self.update_mixed_candidates();
        KeyResult::handled()
    }

    /// 码当笔顺读时笔顺以它开头的字；码为空或不是笔画码时为 `None`，不查词库。
    fn strokes_for(&self, code: &str) -> Result<Option<HashSet<char>>, KeyResult> {
        let Some(keys) = stroke_keys(code).filter(|keys| !keys.is_empty()) else {
            return Ok(None);
        };
        self.stroke_dictionary
            .as_ref()
            .and_then(|dictionary| stroke::characters_with_stroke_prefix(dictionary, &keys).ok())
            .map(Some)
            .ok_or_else(|| self.stroke_dictionary_unavailable())
    }

    fn stroke_dictionary_unavailable(&self) -> KeyResult {
        KeyResult::handled().with_diagnostic(Some(
            diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE.to_owned(),
        ))
    }

    /// 筛选前的整张列表：有码时 `apply_stroke_filter` 把它留在 `ranking_candidates` 里，还没有码时屏幕上的就是它。
    fn unfiltered_candidates(&self) -> &[WordItem] {
        match &self.stroke_filter {
            Some(filter) if !filter.code.is_empty() => self
                .ranking_candidates
                .as_deref()
                .unwrap_or(&self.mixed_candidates),
            _ => &self.mixed_candidates,
        }
    }

    /// `update_mixed_candidates` 的最后一步。筛选不再属于眼下的组字时结束它；有码时只留第一个字合码的行，筛选前的列表留在 `ranking_candidates`（个人上下文重排已经留了一份同样的行时用它），学习和固定位置按原来的名次找行（`ranking_index`）。
    pub(super) fn apply_stroke_filter(&mut self) {
        let Some(filter) = &self.stroke_filter else {
            return;
        };
        if !self.stroke_filter_applies(filter) {
            self.stroke_filter = None;
            return;
        }
        if filter.code.is_empty() {
            return;
        }
        if self.ranking_candidates.is_none() {
            self.ranking_candidates = Some(self.mixed_candidates.clone());
        }
        self.mixed_candidates.retain(|row| filter.keeps(row));
    }

    /// 快照预编辑在拼音后面接的筛选码（`｜kz`）；没有在筛选时为 `None`。
    pub(super) fn stroke_filter_preedit(&self) -> Option<String> {
        let filter = self
            .stroke_filter
            .as_ref()
            .filter(|filter| self.stroke_filter_applies(filter))?;
        let mut text = String::from(PREEDIT_MARK);
        text.push_str(&filter.code);
        Some(text)
    }
}
