//! Tab 笔画与偏旁筛选（`SessionOptions::tab_stroke_filter`）：全拼、双拼组字时按 Tab 进入筛选，
//! 之后打的小写字母是筛选码，候选只留第一个字合这串码的那些（词也留，比的是它的第一个字）。一个字合码有两种方式：
//!
//! - 笔顺：码里只有 `h` `s` `p` `n` `z`（横竖撇点折）、`d`（点，与 `n` 同义）和 `x`（任意一笔），
//!   且至少含一笔具体笔画（纯 `x` 不查笔顺，留给心/穴/小等部件筛选）时，按笔顺比，读只读 `msime-stroke.db`；
//! - 部件：由 `local::chaizi::matches_component_filter` 匹配，支持按顺序各部件首字母或全拼前缀、常用部首俗称、
//!   右/底后置部首、三部件及以上首尾跨部件组合与常用独体部首字。
//!
//! 码不进组字原文：原文、光标和 `editing_text` 都不变，只有快照的预编辑在拼音后面接上 `｜` 和码。
//! Backspace 先删码，删光后再按一次退出筛选；大写字母和 Tab 被吞掉；数字、空格、标点和翻页照常。
//! 一个字母筛不出任何候选时不收它；筛掉的行仍留在 `ranking_candidates` 里，学习按原名次算。

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::input::InputSession;
use crate::diagnostics;
use crate::error::{EngineError, Result};
use crate::local::chaizi;
use crate::paths::sqlite_path_no_follow_allow_parent_symlinks;
use crate::text::count_utf8_chars;
use crate::types::{Command, KeyResult, LocalInputMode, SchemeType, WordItem};

/// 预编辑里码前的分隔，按 Tab 后还没打码时也让人看得出在筛选。
const PREEDIT_MARK: char = '｜';
/// 笔画码上限（与传统笔画方案一致），超过部分吞掉。
const MAX_CODE: usize = 64;
/// 筛选态下三字及以上长句最多保留的整句行条数，避免几十条尾词次选挤走前排短词与首字单字。
const LONG_FILTERED_SENTENCE_LIMIT: usize = 2;

/// `msime-stroke.db` 契约格式版本。
const FORMAT_VERSION: u32 = 1;
const METADATA_FORMAT_VERSION: &str = "format_version";
const WILDCARD: char = 'x';

/// 轻量只读笔画词库（`msime-stroke.db`），仅在首次进入 Tab 笔画筛选时打开。
#[derive(Debug)]
pub(crate) struct StrokeDictionary {
    connection: Connection,
}

impl StrokeDictionary {
    /// 只读打开 `path` 并校验 `format_version == 1`，拒绝创建缺失文件或非法版本。
    pub(crate) fn open_read_only(path: &Path) -> Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(EngineError::failed(
                diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE,
            ));
        }
        let unavailable =
            |_: rusqlite::Error| EngineError::failed(diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE);
        let path = sqlite_path_no_follow_allow_parent_symlinks(path)
            .map_err(|_| EngineError::failed(diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE))?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(unavailable)?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(unavailable)?;
        let dictionary = Self { connection };
        let version = dictionary
            .metadata(METADATA_FORMAT_VERSION)
            .map_err(|_| EngineError::failed(diagnostics::LANGUAGE_DICTIONARY_UNAVAILABLE))?;
        if version.as_deref() != Some(FORMAT_VERSION.to_string().as_str()) {
            return Err(EngineError::failed(
                diagnostics::LANGUAGE_DICTIONARY_VERSION_UNSUPPORTED,
            ));
        }
        Ok(dictionary)
    }

    fn metadata(&self, name: &str) -> Result<Option<String>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT value FROM metadata WHERE name = ?1")?;
        Ok(statement.query_row((name,), |row| row.get(0)).optional()?)
    }

    /// 找出 `msime-stroke.db` 中笔顺前缀匹配 `pattern`（`x` 匹配任意一笔）的全部单字。
    fn characters_with_stroke_prefix(&self, pattern: &str) -> Result<HashSet<char>> {
        let texts = self.texts_matching_key_prefix(pattern, WILDCARD)?;
        Ok(texts
            .into_iter()
            .filter_map(|text| {
                let mut characters = text.chars();
                let first = characters.next()?;
                characters.next().is_none().then_some(first)
            })
            .collect())
    }

    fn texts_matching_key_prefix(&self, pattern: &str, wildcard: char) -> Result<Vec<String>> {
        if !pattern.contains(wildcard) {
            let Some(upper) = completion_upper_bound(pattern) else {
                return Ok(Vec::new());
            };
            let mut statement = self
                .connection
                .prepare_cached("SELECT text FROM entries WHERE key >= ?1 AND key < ?2")?;
            let rows = statement.query_map((pattern, upper), |row| row.get(0))?;
            return Ok(rows.collect::<rusqlite::Result<_>>()?);
        }
        let glob = glob_pattern(pattern, wildcard);
        let literal = pattern.split(wildcard).next().unwrap_or_default();
        let texts = match completion_upper_bound(literal) {
            Some(upper) => self
                .connection
                .prepare_cached(
                    "SELECT text FROM entries WHERE key >= ?1 AND key < ?2 AND key GLOB ?3",
                )?
                .query_map((literal, upper, glob), |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
            None => self
                .connection
                .prepare_cached("SELECT text FROM entries WHERE key GLOB ?1")?
                .query_map((glob,), |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        };
        Ok(texts)
    }
}

fn completion_upper_bound(prefix: &str) -> Option<String> {
    let last = prefix.chars().next_back()?;
    let next = char::from_u32(u32::from(last) + 1)?;
    let head = &prefix[..prefix.len() - last.len_utf8()];
    let mut upper = String::with_capacity(head.len() + next.len_utf8());
    upper.push_str(head);
    upper.push(next);
    Some(upper)
}

fn glob_pattern(pattern: &str, wildcard: char) -> String {
    let mut glob = String::with_capacity(pattern.len() * 4 + 1);
    for character in pattern.chars() {
        match character {
            _ if character == wildcard => glob.push_str("[^ ]"),
            '*' | '?' | '[' => {
                glob.push('[');
                glob.push(character);
                glob.push(']');
            }
            _ => glob.push(character),
        }
    }
    glob.push('*');
    glob
}

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
                || chaizi::matches_component_filter(first, &self.code)
        })
    }
}

/// 码当笔顺读时的笔画键串（`d` 换成 `n`）；码里有笔画以外的字母、或只有通配符 `x` 时为 `None`。
fn stroke_keys(code: &str) -> Option<String> {
    let keys: String = code
        .bytes()
        .map(|key| match key {
            b'd' => Some('n'),
            b'h' | b's' | b'p' | b'n' | b'z' | b'x' => Some(char::from(key)),
            _ => None,
        })
        .collect::<Option<String>>()?;
    keys.bytes().any(|key| key != b'x').then_some(keys)
}

impl InputSession {
    /// 快照的 `spelling_symbols` 要不要带 Tab：能进筛选，或者已经在筛选里（这时 Tab 被吞掉，不交给宿主翻页）。
    pub(super) fn stroke_filter_takes_tab(&self) -> bool {
        self.stroke_filter
            .as_ref()
            .is_some_and(|filter| self.stroke_filter_applies(filter))
            || self.stroke_filter_available()
    }

    /// Tab 能进入笔画筛选：开关开着、有笔画词库，全拼或双拼在组字、光标在末尾，不在本地模式、专用英文、整句改字与光标后编辑里。
    fn stroke_filter_available(&self) -> bool {
        self.tab_stroke_filter
            && self.stroke_filter.is_none()
            && !self.stroke_dictionary_path.as_os_str().is_empty()
            && matches!(self.scheme(), SchemeType::Quanpin | SchemeType::Shuangpin)
            && !self.caret_edit.active
            && self.composing_at_end()
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
            match StrokeDictionary::open_read_only(&self.stroke_dictionary_path) {
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
    fn strokes_for(&self, code: &str) -> std::result::Result<Option<HashSet<char>>, KeyResult> {
        let Some(keys) = stroke_keys(code).filter(|keys| !keys.is_empty()) else {
            return Ok(None);
        };
        self.stroke_dictionary
            .as_ref()
            .and_then(|dictionary| dictionary.characters_with_stroke_prefix(&keys).ok())
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

    /// `update_mixed_candidates` 的最后一步。筛选不再属于眼下的组字时结束它；
    /// 有码时只留第一个字合码的行，筛选前的列表留在 `ranking_candidates`，学习按原名次算。
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
        self.compact_filtered_sentences();
    }

    /// 筛选后收敛整句数量并将来自末尾的合法次选整句归位到精确整键词典词之后、短前缀词与单字之前，避免大量尾词排列组合或整串生僻单字挤占前排。
    fn compact_filtered_sentences(&mut self) {
        if !self
            .mixed_candidates
            .iter()
            .any(|item| item.sentence_association && self.selection_completes_composition(item))
        {
            return;
        }
        let candidates = std::mem::take(&mut self.mixed_candidates);
        let is_sentence = |item: &WordItem| {
            item.sentence_association && self.selection_completes_composition(item)
        };
        let Some(first_sentence) = candidates.iter().find(|item| is_sentence(item)) else {
            self.mixed_candidates = candidates;
            return;
        };
        let long_sentence = count_utf8_chars(&first_sentence.word) >= 3;
        let has_primary = candidates
            .iter()
            .any(|item| is_sentence(item) && !item.keyboard_runner_up);
        let exact_dictionary_lead = candidates
            .iter()
            .take_while(|item| {
                item.source.is_dictionary()
                    && self.selection_completes_composition(item)
                    && self.reading_answers_key(item)
            })
            .count();

        let sentence_limit = if long_sentence {
            LONG_FILTERED_SENTENCE_LIMIT
        } else if has_primary || exact_dictionary_lead == 0 {
            1
        } else {
            // 两音节输入已有命中的精确双字词典词（如 `daobi｜ps` 的 倒闭、倒毙、倒逼）时，丢弃末尾合成的双字次选整句。
            0
        };

        let mut kept_sentences = 0usize;
        let mut insert_at = exact_dictionary_lead;
        let mut promoted = Vec::new();
        let mut kept = Vec::with_capacity(candidates.len());
        for (index, mut item) in candidates.into_iter().enumerate() {
            if is_sentence(&item) {
                let eligible = long_sentence || !has_primary || !item.keyboard_runner_up;
                if eligible && kept_sentences < sentence_limit {
                    kept_sentences += 1;
                    item.keyboard_runner_up = false;
                    if !item.pinned_first
                        && item.fixed_position == 0
                        && index >= exact_dictionary_lead
                    {
                        promoted.push(item);
                        continue;
                    }
                    if index < exact_dictionary_lead {
                        insert_at = insert_at.saturating_sub(1);
                    }
                    kept.push(item);
                }
                continue;
            }
            kept.push(item);
        }
        if !promoted.is_empty() {
            let at = insert_at.min(kept.len());
            kept.splice(at..at, promoted);
        }
        self.mixed_candidates = kept;
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
