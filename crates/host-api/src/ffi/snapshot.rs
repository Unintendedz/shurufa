//! 面向 macOS 宿主热路径的零拷贝 C ABI 视图与转换快照。
//!
//! 视图与候选文本全部写入会话持有的 `Box<ViewBuffer>` 连续字节缓冲，先记录各字段偏移与长度，
//! 待全部字符串追加完成后再一次性换算只读指针，确保稳态按键与翻页过程中零 `serde_json` 与零额外堆分配。

use crate::*;
use msime_client_core::preferences::BracketStyle;
use msime_client_core::punctuation::QuoteMarks;
use msime_input_runtime::View;
use std::ffi::{c_char, CString};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShurufaStringSlice {
    pub ptr: *const u8,
    pub len: usize,
}

impl Default for ShurufaStringSlice {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ShurufaPunctuationMarks {
    pub quotes: u8,
    pub brackets: u8,
    pub enumeration_comma_backslash: bool,
    pub enumeration_comma_slash: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShurufaCandidateItem {
    pub session: u64,
    pub generation: u64,
    pub index: usize,
    pub text: ShurufaStringSlice,
    pub code: ShurufaStringSlice,
    pub annotation: ShurufaStringSlice,
    pub translation: ShurufaStringSlice,
    pub source: u8,
    pub fixed_position: u8,
    pub has_translation: bool,
    pub corrected: bool,
    pub highlighted: bool,
    pub translation_reserved: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShurufaViewSnapshot {
    pub session: u64,
    pub generation: u64,
    pub scheme: u8,
    pub character_width: u8,
    pub prediction: bool,
    pub chinese_text: bool,
    pub script_conversion: bool,
    pub microsoft_shuangpin: bool,
    pub dedicated_english: bool,
    pub focused: bool,
    pub caret_edit_suffix: bool,
    pub provisional: bool,
    pub punctuation_marks: ShurufaPunctuationMarks,
    pub shuangpin_profile: ShurufaStringSlice,
    pub local_mode: ShurufaStringSlice,
    pub spelling_symbols: ShurufaStringSlice,
    pub preedit: ShurufaStringSlice,
    pub phrase_prefix: ShurufaStringSlice,
    pub reading: ShurufaStringSlice,
    pub display_reading: ShurufaStringSlice,
    pub editing_text: ShurufaStringSlice,
    pub caret_position: usize,
    pub conversion: ShurufaStringSlice,
    pub conversion_focus_start: usize,
    pub conversion_focus_end: usize,
    pub caret_edit_prefix: ShurufaStringSlice,
    pub page: usize,
    pub page_size: usize,
    pub page_count: usize,
    pub candidates: *const ShurufaCandidateItem,
    pub candidates_len: usize,
}

impl Default for ShurufaViewSnapshot {
    fn default() -> Self {
        Self {
            session: 0,
            generation: 0,
            scheme: 0,
            character_width: 0,
            prediction: false,
            chinese_text: false,
            script_conversion: false,
            microsoft_shuangpin: false,
            dedicated_english: false,
            focused: false,
            caret_edit_suffix: false,
            provisional: false,
            punctuation_marks: ShurufaPunctuationMarks::default(),
            shuangpin_profile: ShurufaStringSlice::default(),
            local_mode: ShurufaStringSlice::default(),
            spelling_symbols: ShurufaStringSlice::default(),
            preedit: ShurufaStringSlice::default(),
            phrase_prefix: ShurufaStringSlice::default(),
            reading: ShurufaStringSlice::default(),
            display_reading: ShurufaStringSlice::default(),
            editing_text: ShurufaStringSlice::default(),
            caret_position: 0,
            conversion: ShurufaStringSlice::default(),
            conversion_focus_start: 0,
            conversion_focus_end: 0,
            caret_edit_prefix: ShurufaStringSlice::default(),
            page: 0,
            page_size: 0,
            page_count: 0,
            candidates: std::ptr::null(),
            candidates_len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ShurufaOutputContext {
    pub scheme: u8,
    pub script_conversion: bool,
    pub typing_statistics: bool,
    pub local_mode: ShurufaStringSlice,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ShurufaTransitionSnapshot {
    pub handled: bool,
    pub has_commit: bool,
    pub commit: ShurufaStringSlice,
    pub has_commit_context: bool,
    pub commit_context: ShurufaOutputContext,
    pub has_feedback: bool,
    pub typing_effect: u32,
    pub view: ShurufaViewSnapshot,
}

#[derive(Clone, Copy, Default)]
struct ByteRange {
    offset: usize,
    len: usize,
}

#[derive(Clone, Copy)]
struct CandidateRange {
    session: u64,
    generation: u64,
    index: usize,
    text: ByteRange,
    code: ByteRange,
    annotation: ByteRange,
    translation: ByteRange,
    source: u8,
    fixed_position: u8,
    has_translation: bool,
    corrected: bool,
    highlighted: bool,
    translation_reserved: bool,
}

struct ViewRanges {
    shuangpin_profile: ByteRange,
    local_mode: ByteRange,
    spelling_symbols: ByteRange,
    preedit: ByteRange,
    phrase_prefix: ByteRange,
    reading: ByteRange,
    display_reading: ByteRange,
    editing_text: ByteRange,
    conversion: ByteRange,
    caret_edit_prefix: ByteRange,
}

/// 会话持有的可复用快照 arena：每次打包先清空长度保留容量，写入全部字节后再统一解析指针。
pub(crate) struct ViewBuffer {
    bytes: Vec<u8>,
    item_ranges: Vec<CandidateRange>,
    items: Vec<ShurufaCandidateItem>,
    view: ShurufaViewSnapshot,
    transition: ShurufaTransitionSnapshot,
}

impl Default for ViewBuffer {
    fn default() -> Self {
        Self {
            bytes: Vec::with_capacity(4096),
            item_ranges: Vec::with_capacity(16),
            items: Vec::with_capacity(16),
            view: ShurufaViewSnapshot::default(),
            transition: ShurufaTransitionSnapshot::default(),
        }
    }
}

impl ViewBuffer {
    fn push_str(&mut self, text: &str) -> ByteRange {
        if text.is_empty() {
            return ByteRange::default();
        }
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(text.as_bytes());
        ByteRange {
            offset,
            len: text.len(),
        }
    }

    fn resolve(&self, range: ByteRange) -> ShurufaStringSlice {
        if range.len == 0 {
            ShurufaStringSlice::default()
        } else {
            // SAFETY: `range.offset + range.len <= self.bytes.len()` 且此后不再向 `self.bytes` 追加。
            let ptr = unsafe { self.bytes.as_ptr().add(range.offset) };
            ShurufaStringSlice {
                ptr,
                len: range.len,
            }
        }
    }

    fn push_view_strings(&mut self, view: &View) -> ViewRanges {
        self.item_ranges.clear();
        for candidate in &view.candidates {
            let text = self.push_str(&candidate.text);
            let code = self.push_str(&candidate.code);
            let annotation = self.push_str(&candidate.annotation);
            let translation = candidate
                .translation
                .as_deref()
                .map(|value| self.push_str(value))
                .unwrap_or_default();
            self.item_ranges.push(CandidateRange {
                session: candidate.id.session,
                generation: candidate.id.generation,
                index: candidate.id.index,
                text,
                code,
                annotation,
                translation,
                source: candidate.source,
                fixed_position: candidate.fixed_position.unwrap_or(0),
                has_translation: candidate.translation.is_some(),
                corrected: candidate.corrected,
                highlighted: candidate.highlighted,
                translation_reserved: candidate.translation_reserved,
            });
        }
        ViewRanges {
            shuangpin_profile: self.push_str(&view.shuangpin_profile),
            local_mode: self.push_str(&view.local_mode),
            spelling_symbols: self.push_str(&view.spelling_symbols),
            preedit: self.push_str(&view.preedit),
            phrase_prefix: self.push_str(&view.phrase_prefix),
            reading: self.push_str(&view.reading),
            display_reading: self.push_str(&view.display_reading),
            editing_text: self.push_str(&view.editing_text),
            conversion: self.push_str(&view.conversion),
            caret_edit_prefix: self.push_str(&view.caret_edit_prefix),
        }
    }

    fn build_view(&mut self, view: &View, ranges: &ViewRanges) -> ShurufaViewSnapshot {
        self.items.clear();
        for range in &self.item_ranges {
            let item = ShurufaCandidateItem {
                session: range.session,
                generation: range.generation,
                index: range.index,
                text: self.resolve(range.text),
                code: self.resolve(range.code),
                annotation: self.resolve(range.annotation),
                translation: self.resolve(range.translation),
                source: range.source,
                fixed_position: range.fixed_position,
                has_translation: range.has_translation,
                corrected: range.corrected,
                highlighted: range.highlighted,
                translation_reserved: range.translation_reserved,
            };
            self.items.push(item);
        }
        let candidates = if self.items.is_empty() {
            std::ptr::null()
        } else {
            self.items.as_ptr()
        };
        ShurufaViewSnapshot {
            session: view.session,
            generation: view.generation,
            scheme: view.scheme,
            character_width: match view.character_width {
                CharacterWidth::Halfwidth => 0,
                CharacterWidth::Fullwidth => 1,
            },
            prediction: view.prediction,
            chinese_text: view.chinese_text,
            script_conversion: view.script_conversion,
            microsoft_shuangpin: view.microsoft_shuangpin,
            dedicated_english: view.dedicated_english,
            focused: view.focused,
            caret_edit_suffix: view.caret_edit_suffix,
            provisional: view.provisional,
            punctuation_marks: ShurufaPunctuationMarks {
                quotes: match view.punctuation_marks.quotes {
                    QuoteMarks::Curly => 0,
                    QuoteMarks::Corner => 1,
                },
                brackets: match view.punctuation_marks.brackets {
                    BracketStyle::Solid => 0,
                    BracketStyle::Hollow => 1,
                },
                enumeration_comma_backslash: view.punctuation_marks.enumeration_comma_backslash,
                enumeration_comma_slash: view.punctuation_marks.enumeration_comma_slash,
            },
            shuangpin_profile: self.resolve(ranges.shuangpin_profile),
            local_mode: self.resolve(ranges.local_mode),
            spelling_symbols: self.resolve(ranges.spelling_symbols),
            preedit: self.resolve(ranges.preedit),
            phrase_prefix: self.resolve(ranges.phrase_prefix),
            reading: self.resolve(ranges.reading),
            display_reading: self.resolve(ranges.display_reading),
            editing_text: self.resolve(ranges.editing_text),
            caret_position: view.caret_position,
            conversion: self.resolve(ranges.conversion),
            conversion_focus_start: view.conversion_focus_start,
            conversion_focus_end: view.conversion_focus_end,
            caret_edit_prefix: self.resolve(ranges.caret_edit_prefix),
            page: view.page,
            page_size: view.page_size,
            page_count: view.page_count,
            candidates,
            candidates_len: self.items.len(),
        }
    }

    pub(crate) fn pack_view(&mut self, view: &View) -> *const ShurufaViewSnapshot {
        self.bytes.clear();
        let ranges = self.push_view_strings(view);
        self.view = self.build_view(view, &ranges);
        &self.view
    }

    pub(crate) fn pack_transition(
        &mut self,
        transition: &Transition,
        feedback: Option<u32>,
    ) -> *const ShurufaTransitionSnapshot {
        self.bytes.clear();
        let commit_range = transition
            .commit
            .as_deref()
            .map(|text| self.push_str(text))
            .unwrap_or_default();
        let ctx_mode_range = transition
            .commit_context
            .as_ref()
            .map(|ctx| self.push_str(&ctx.local_mode))
            .unwrap_or_default();
        let view_ranges = self.push_view_strings(&transition.view);
        let view = self.build_view(&transition.view, &view_ranges);
        let commit_context = match transition.commit_context.as_ref() {
            Some(ctx) => ShurufaOutputContext {
                scheme: ctx.scheme,
                script_conversion: ctx.script_conversion,
                typing_statistics: ctx.typing_statistics,
                local_mode: self.resolve(ctx_mode_range),
            },
            None => ShurufaOutputContext::default(),
        };
        self.transition = ShurufaTransitionSnapshot {
            handled: transition.handled,
            has_commit: transition.commit.is_some(),
            commit: self.resolve(commit_range),
            has_commit_context: transition.commit_context.is_some(),
            commit_context,
            has_feedback: feedback.is_some(),
            typing_effect: feedback.unwrap_or(0),
            view,
        };
        &self.transition
    }
}

unsafe fn clear_error(error_out: *mut *mut c_char) {
    if !error_out.is_null() {
        unsafe {
            *error_out = std::ptr::null_mut();
        }
    }
}

unsafe fn write_error(error_out: *mut *mut c_char, message: &str) {
    if error_out.is_null() {
        return;
    }
    let sanitized = if message.as_bytes().contains(&0) {
        message.replace('\0', " ")
    } else {
        message.to_owned()
    };
    let cstr = CString::new(sanitized)
        .unwrap_or_else(|_| CString::new("internal runtime failure").expect("static ASCII string"));
    unsafe {
        *error_out = cstr.into_raw();
    }
}

unsafe fn run_snapshot_ptr<T>(
    error_out: *mut *mut c_char,
    operation: impl FnOnce() -> Result<*const T, String>,
) -> *const T {
    unsafe { clear_error(error_out) };
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(error)) => {
            unsafe { write_error(error_out, &error) };
            std::ptr::null()
        }
        Err(_) => {
            unsafe { write_error(error_out, "internal runtime failure") };
            std::ptr::null()
        }
    }
}

fn dispatch_snapshot_transition(
    handle: u64,
    action: Action,
    host: Option<lifecycle::InputClass>,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(handle, |session| {
                let (transition, feedback) = dispatch_session_transition(session, action, host)?;
                Ok(session.view_buffer.pack_transition(&transition, feedback))
            })
        })
    }
}

/// 读取当前会话的零拷贝视图快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回的快照借用同一会话的
/// `ViewBuffer`，仅在同一会话下一次视图或派发调用之前有效，且须在同一会话所属线程上使用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_view(
    session: u64,
    error_out: *mut *mut c_char,
) -> *const ShurufaViewSnapshot {
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(session, |session| {
                let view = session.runtime.view();
                Ok(session.view_buffer.pack_view(&view))
            })
        })
    }
}

/// 派发单个 ASCII 按键事件并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_character(
    session: u64,
    ascii: u8,
    shift: bool,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    dispatch_snapshot_transition(
        session,
        Action::Character {
            value: ascii,
            shift,
        },
        None,
        error_out,
    )
}

/// 派发输入控制命令并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_command(
    session: u64,
    command: u32,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    match ffi::input::command_action(command) {
        Ok((action, host)) => dispatch_snapshot_transition(session, action, host, error_out),
        Err(error) => {
            unsafe {
                clear_error(error_out);
                write_error(error_out, &error);
            }
            std::ptr::null()
        }
    }
}

/// 派发标点输入并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_punctuation(
    session: u64,
    ascii: u8,
    preceding: u32,
    has_preceding: bool,
    ascii_literal: bool,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(session, |session| {
                let action = if ascii_literal {
                    Action::PunctuationAscii(ascii)
                } else if has_preceding {
                    ffi::input::resolve_punctuation_action(session, ascii, preceding)?
                } else {
                    Action::Punctuation(ascii)
                };
                let (transition, feedback) = dispatch_session_transition(session, action, None)?;
                Ok(session.view_buffer.pack_transition(&transition, feedback))
            })
        })
    }
}

/// 按当前页候选索引选词并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_select(
    session: u64,
    generation: u64,
    index: usize,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    dispatch_snapshot_transition(
        session,
        Action::Select(CandidateId {
            session,
            generation,
            index,
        }),
        None,
        error_out,
    )
}

/// 按整代候选索引选词（展开网格）并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_select_any(
    session: u64,
    generation: u64,
    index: usize,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    dispatch_snapshot_transition(
        session,
        Action::SelectAnyCandidate(CandidateId {
            session,
            generation,
            index,
        }),
        None,
        error_out,
    )
}

/// 以词定字（取首字或尾字）并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_select_edge(
    session: u64,
    generation: u64,
    index: usize,
    edge: u8,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    match ffi::candidates::select_edge_action(session, generation, index, edge) {
        Ok(action) => dispatch_snapshot_transition(session, action, None, error_out),
        Err(error) => {
            unsafe {
                clear_error(error_out);
                write_error(error_out, &error);
            }
            std::ptr::null()
        }
    }
}

/// 设置会话聚焦状态并返回零拷贝迁移快照。
///
/// # Safety
/// `error_out` 须为空指针或指向合法可写 `*mut c_char` 的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在同一会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_focus(
    session: u64,
    focused: bool,
    error_out: *mut *mut c_char,
) -> *const ShurufaTransitionSnapshot {
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(session, |session| {
                let transition = ffi::session::focus_session_transition(session, focused)?;
                Ok(session.view_buffer.pack_transition(&transition, None))
            })
        })
    }
}

/// 轮询神经重排结果并返回当前零拷贝视图快照。
///
/// # Safety
/// `ready_out`、`moved_out` 与 `error_out` 须为空指针或指向合法可写内存的指针；返回指针
/// 借用同一会话的 `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_poll_neural(
    session: u64,
    ready_out: *mut bool,
    moved_out: *mut bool,
    error_out: *mut *mut c_char,
) -> *const ShurufaViewSnapshot {
    if !ready_out.is_null() {
        unsafe { *ready_out = false };
    }
    if !moved_out.is_null() {
        unsafe { *moved_out = false };
    }
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(session, |session| {
                let polled = ffi::neural::poll_neural_session(session);
                if !ready_out.is_null() {
                    *ready_out = polled.ready;
                }
                if !moved_out.is_null() {
                    *moved_out = polled.moved;
                }
                let ptr = match polled.view.as_ref() {
                    Some(view) => session.view_buffer.pack_view(view),
                    None => {
                        let current = session.runtime.view();
                        session.view_buffer.pack_view(&current)
                    }
                };
                Ok(ptr)
            })
        })
    }
}

/// 冲刷挂起的神经重排临时态并返回最新零拷贝视图快照。
///
/// # Safety
/// `flushed_out` 与 `error_out` 须为空指针或指向合法可写内存的指针；返回指针借用同一会话的
/// `ViewBuffer`，仅在该会话下一次视图或派发调用之前有效，且须在会话所属线程上调用。
#[no_mangle]
pub unsafe extern "C" fn shurufa_session_flush_provisional_neural(
    session: u64,
    flushed_out: *mut bool,
    error_out: *mut *mut c_char,
) -> *const ShurufaViewSnapshot {
    if !flushed_out.is_null() {
        unsafe { *flushed_out = false };
    }
    unsafe {
        run_snapshot_ptr(error_out, || {
            with_session(session, |session| {
                let flushed = session.runtime.flush_provisional();
                if !flushed_out.is_null() {
                    *flushed_out = flushed;
                }
                let view = session.runtime.view();
                Ok(session.view_buffer.pack_view(&view))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn slice_to_str<'a>(slice: ShurufaStringSlice) -> &'a str {
        if slice.len == 0 {
            ""
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
            std::str::from_utf8(bytes).expect("snapshot slice must be valid UTF-8")
        }
    }

    fn unpack_json(raw: *mut c_char) -> Value {
        assert!(!raw.is_null());
        let text = unsafe { std::ffi::CStr::from_ptr(raw) }
            .to_str()
            .expect("FFI JSON must be UTF-8")
            .to_owned();
        unsafe { msime_client_string_free(raw) };
        let parsed: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed["ok"], true, "{parsed}");
        parsed["value"].clone()
    }

    fn create_test_session() -> Option<u64> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = [
            root.join("target/resources"),
            root.join("target/macos/EngineResources"),
        ]
        .into_iter()
        .find(|candidate| candidate.join("msime-pinyin.db").is_file())?;
        let dir = std::fs::canonicalize(dir).ok()?;
        let temp =
            std::env::temp_dir().join(format!("shurufa-snapshot-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp);
        let req = json!({
            "resources": dir,
            "state_root": temp,
            "edition": "shurufa",
        })
        .to_string();
        let options = unpack_json(unsafe { msime_client_prepare_host(req.as_ptr(), req.len()) });
        let raw = options.to_string();
        let created = unpack_json(unsafe { msime_client_create(raw.as_ptr(), raw.len()) });
        Some(created["session"].as_u64().expect("session id"))
    }

    #[test]
    fn zero_copy_snapshot_matches_json_and_reuses_buffer_capacity() {
        let Some(handle) = create_test_session() else {
            eprintln!("skipped: target/resources/msime-pinyin.db not present");
            return;
        };
        let mut err: *mut c_char = std::ptr::null_mut();

        unsafe {
            let focus = shurufa_session_focus(handle, true, &mut err);
            assert!(err.is_null());
            assert!(!focus.is_null());
            assert!((*focus).view.focused);

            // 输入 "nihao"，逐字符核对快照字段。
            for ch in b"nihao" {
                let step = shurufa_session_character(handle, *ch, false, &mut err);
                assert!(err.is_null());
                assert!(!step.is_null());
                assert!((*step).handled);
            }

            let snap = shurufa_session_view(handle, &mut err);
            assert!(err.is_null());
            assert!(!snap.is_null());
            let json_view = unpack_json(msime_client_view(handle));

            assert_eq!((*snap).session, json_view["session"].as_u64().unwrap());
            assert_eq!(
                (*snap).generation,
                json_view["generation"].as_u64().unwrap()
            );
            assert_eq!(
                slice_to_str((*snap).editing_text),
                json_view["editing_text"].as_str().unwrap()
            );
            assert_eq!(
                slice_to_str((*snap).preedit),
                json_view["preedit"].as_str().unwrap()
            );
            assert_eq!((*snap).page, json_view["page"].as_u64().unwrap() as usize);
            assert_eq!(
                (*snap).page_count,
                json_view["page_count"].as_u64().unwrap() as usize
            );
            let json_candidates = json_view["candidates"].as_array().unwrap();
            assert_eq!((*snap).candidates_len, json_candidates.len());
            let items = std::slice::from_raw_parts((*snap).candidates, (*snap).candidates_len);
            for (item, jc) in items.iter().zip(json_candidates.iter()) {
                assert_eq!(slice_to_str(item.text), jc["text"].as_str().unwrap());
                assert_eq!(item.index, jc["id"]["index"].as_u64().unwrap() as usize);
                assert_eq!(item.highlighted, jc["highlighted"].as_bool().unwrap());
            }

            // 验证稳态反复翻页后 ViewBuffer 容量不继续增长。
            for _ in 0..3 {
                let _ = shurufa_session_command(handle, 100, &mut err);
                assert!(err.is_null());
            }
            let (warm_bytes_cap, warm_items_cap) = with_session(handle, |s| {
                Ok((
                    s.view_buffer.bytes.capacity(),
                    s.view_buffer.items.capacity(),
                ))
            })
            .unwrap();
            for _ in 0..5 {
                let next = shurufa_session_command(handle, 100, &mut err);
                assert!(err.is_null());
                assert!(!next.is_null());
                let prev = shurufa_session_command(handle, 101, &mut err);
                assert!(err.is_null());
                assert!(!prev.is_null());
            }
            let (after_bytes_cap, after_items_cap) = with_session(handle, |s| {
                Ok((
                    s.view_buffer.bytes.capacity(),
                    s.view_buffer.items.capacity(),
                ))
            })
            .unwrap();
            assert_eq!(warm_bytes_cap, after_bytes_cap);
            assert_eq!(warm_items_cap, after_items_cap);

            // 验证标点、选词上屏与错误输出。
            let top = shurufa_session_view(handle, &mut err);
            assert!(err.is_null());
            assert!(!top.is_null() && (*top).candidates_len > 0);
            let gen = (*top).generation;
            let first_seat = (*(*top).candidates).index;
            let sel = shurufa_session_select(handle, gen, first_seat, &mut err);
            assert!(err.is_null());
            assert!(!sel.is_null());
            assert!((*sel).has_commit);
            assert!(!slice_to_str((*sel).commit).is_empty());

            let punct = shurufa_session_punctuation(handle, b',', 0, false, false, &mut err);
            assert!(err.is_null());
            assert!((*punct).has_commit);
            assert_eq!(slice_to_str((*punct).commit), "，");

            let bad = shurufa_session_view(u64::MAX, &mut err);
            assert!(bad.is_null());
            assert!(!err.is_null());
            msime_client_string_free(err);
        }

        let _ = unpack_json(msime_client_destroy(handle));
    }
}
