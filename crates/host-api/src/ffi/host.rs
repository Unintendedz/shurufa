//! Host configuration, preferences, the skin catalog and clipboard history.
//!
//! Part of the C ABI; see the parent module for what these shims guarantee.

use crate::*;
use msime_client_core::is_bounded_text;

/// Candidate and commit text normally stays far below this bound. Keep the direct conversion ABI
/// bounded as well so a malformed native length cannot make it scan an unbounded buffer or allocate
/// an arbitrarily large converted string.
const MAX_TRADITIONAL_CONVERSION_BYTES: usize = 1 << 20;

/// 规范化快捷符号列表，默认值与偏好校验都由 client-core 持有。
/// # Safety
/// `value` 指向 `length` 字节的 JSON `{symbols: string[]}`，最多 64 KiB；结果用 string_free 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_touch_quick_symbols(
    value: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if value.is_null() || length > 64 * 1024 {
            return Err("invalid_touch_symbols".into());
        }
        // 安全：调用方保证缓冲区可读，长度已检查。
        let bytes = unsafe { std::slice::from_raw_parts(value, length) };
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Request {
            symbols: Vec<String>,
        }
        let request: Request =
            serde_json::from_slice(bytes).map_err(|_| "invalid_touch_symbols")?;
        Ok(json!(
            msime_client_core::preferences::normalize_touch_quick_symbols(&request.symbols)
        ))
    })
}

fn clipboard_history_limit(directory: &std::path::Path) -> usize {
    msime_client_core::preferences::PreferencesStore::new(directory)
        .load()
        .map(|snapshot| snapshot.preferences.clipboard_history_limit)
        .unwrap_or(msime_client_core::clipboard::DEFAULT_CLIPBOARD_HISTORY_LIMIT)
}

#[no_mangle]
pub extern "C" fn msime_client_abi_version() -> u32 {
    3
}

/// Convert Simplified Chinese text to Traditional Chinese with the shared OpenCC `s2t` tables.
///
/// Returns the converted text itself rather than the standard JSON response: hosts call this for every candidate on a page and every commit, and wrapping each string in a document only to parse it back out would put a JSON round trip on the typing path. Returns null for a null pointer, invalid UTF-8 or an interior NUL, so the caller keeps its own text.
/// # Safety
/// `text` points to `length` readable bytes. The returned string must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_simplified_to_traditional(
    text: *const u8,
    length: usize,
) -> *mut c_char {
    if text.is_null() || length > MAX_TRADITIONAL_CONVERSION_BYTES {
        return std::ptr::null_mut();
    }
    // SAFETY: guaranteed by the documented caller contract.
    let bytes = unsafe { std::slice::from_raw_parts(text, length) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return std::ptr::null_mut();
    };
    let converted = msime_client_core::chinese_conversion::simplified_to_traditional(text);
    CString::new(converted).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// 与 `msime_client_simplified_to_traditional` 相同，但按 `variant` 选 OpenCC 配置：0 `s2t`、1 `s2tw`、2 `s2twp`，即偏好 `traditional_chinese_variant` 的三个取值。未知编号和其它非法输入一样返回 null，调用方保留原文。
/// # Safety
/// `text` points to `length` readable bytes. The returned string must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_convert_to_traditional(
    text: *const u8,
    length: usize,
    variant: u32,
) -> *mut c_char {
    let Some(variant) =
        msime_client_core::chinese_conversion::TraditionalVariant::from_code(variant)
    else {
        return std::ptr::null_mut();
    };
    if text.is_null() || length > MAX_TRADITIONAL_CONVERSION_BYTES {
        return std::ptr::null_mut();
    }
    // SAFETY: guaranteed by the documented caller contract.
    let bytes = unsafe { std::slice::from_raw_parts(text, length) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return std::ptr::null_mut();
    };
    let converted = msime_client_core::chinese_conversion::convert_to_traditional(text, variant);
    CString::new(converted).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Resolve display font families using the same adapter as the shared preview.
/// # Safety
/// `value` points to `length` readable bytes containing a JSON string array.
/// The returned response must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_resolve_font_families(
    value: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if value.is_null() || length > 32 * 1024 {
            return Err("font_family".into());
        }
        // SAFETY: guaranteed by the caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(value, length) };
        let names: Vec<String> = serde_json::from_slice(bytes).map_err(|_| "font_family")?;
        let resolved = system_fonts::resolve_css_families(names).map_err(str::to_owned)?;
        Ok(json!(resolved))
    })
}

/// The installed font family names, from the same catalog the shared app's 刷新字体列表 reads
/// (`list_font_families` in the Tauri shell): sorted, without repeats, each a bounded display name.
/// Fails with `unsupported` on a host without a catalog and `font_catalog` / `font_catalog_limit`
/// when enumeration fails or exceeds its bound. Enumerates the system's fonts, so call it off a
/// UI thread.
#[no_mangle]
pub extern "C" fn msime_client_list_font_families() -> *mut c_char {
    response(|| {
        system_fonts::list()
            .map(|names| json!(names))
            .map_err(str::to_owned)
    })
}

/// Resolve a shared surface route so a native host launches the shared shell by
/// name instead of hardcoding window labels. Returns the canonical route plus
/// the panel label, query and geometry, or an error for an unknown route.
/// # Safety
/// `value` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_parse_surface_route(
    value: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if value.is_null() || length > 1024 {
            return Err("invalid surface route buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(value, length) };
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid surface route encoding")?;
        let route = SurfaceRoute::parse(text).map_err(|e| e.to_string())?;
        let mut document = json!({ "route": route.as_arg(), "surface": route });
        if let Some(panel) = route.panel() {
            document["panel"] = json!({
                "label": panel.label,
                "query": panel.query,
                "title": panel.title,
                "width": panel.width,
                "height": panel.height,
            });
        }
        Ok(document)
    })
}

/// Describe what the named host can do, so the shared UI renders from injected
/// capabilities instead of sniffing the user agent.
/// # Safety
/// `platform` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_host_capabilities(
    platform: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if platform.is_null() || length > 64 {
            return Err("invalid host platform buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(platform, length) };
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid host platform encoding")?;
        let platform = HostPlatform::parse(text).map_err(|e| e.to_string())?;
        serde_json::to_value(HostCapabilities::for_platform(platform)).map_err(|e| e.to_string())
    })
}

/// Verify packaged resources and prepare an isolated new host. No live sessions
/// may use the state root while this runs. Returns a HostOptions object.
/// # Safety
/// `options` points to `length` readable UTF-8 JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_prepare_host(
    options: *const u8,
    length: usize,
) -> *mut c_char {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bootstrap {
        resources: String,
        state_root: String,
        /// 版本 id，缺省是 full；见 `prepare_host_configuration_for_edition`。
        #[serde(default)]
        edition: Option<String>,
        /// 已发布的 runtime-options.json；给了时把这次准备的代次写进去，见 [`publish_prepared_dictionaries`]。
        #[serde(default)]
        runtime_options: Option<String>,
    }
    response(|| {
        if options.is_null() || length > 16384 {
            return Err("invalid bootstrap buffer".into());
        }
        // SAFETY: guaranteed by the caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(options, length) };
        let options: Bootstrap =
            serde_json::from_slice(bytes).map_err(|_| "invalid bootstrap document")?;
        let resources = std::path::Path::new(&options.resources);
        let state = std::path::Path::new(&options.state_root);
        let published = options.runtime_options.as_deref().map(std::path::Path::new);
        if !resources.is_absolute()
            || !state.is_absolute()
            || published.is_some_and(|path| !path.is_absolute())
        {
            return Err("bootstrap paths must be absolute".into());
        }
        let edition = match options.edition.as_deref() {
            None => msime_client_core::edition::Edition::full(),
            Some(id) => msime_client_core::edition::Edition::by_id(id).ok_or("unknown edition")?,
        };
        let document = prepare_host_configuration_for_edition(resources, state, edition)
            .map_err(|e| e.to_string())?;
        let document: Value = serde_json::from_str(&document).map_err(|e| e.to_string())?;
        // 发布失败不挡住宿主启动：会话照样用这份文档，设置窗口和 TIP 留在旧代次上，下次启动再试。
        if let Some(path) = published {
            if let Err(error) = publish_prepared_dictionaries(path, &document) {
                crate::diagnostics::report(
                    "runtime options generation not published",
                    &error.to_string(),
                );
            }
        }
        Ok(document)
    })
}

/// Re-prepare a published runtime options file whose working dictionaries belong to an older resource generation, as after a package upgrade. Returns whether the file was rewritten. Only the input method host calls this, before creating any session from that file.
/// # Safety
/// `path` points to `length` readable UTF-8 bytes naming an absolute file. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_refresh_host(path: *const u8, length: usize) -> *mut c_char {
    response(|| {
        if path.is_null() || length > 4096 {
            return Err("invalid options path buffer".into());
        }
        // SAFETY: guaranteed by the caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(path, length) };
        let path = std::path::Path::new(
            std::str::from_utf8(bytes).map_err(|_| "invalid options path encoding")?,
        );
        if !path.is_absolute() {
            return Err("options path must be absolute".into());
        }
        refresh_host_options(path)
            .map(Value::Bool)
            .map_err(|e| e.to_string())
    })
}

/// The shared preference defaults, as the document a host would have to produce.
///
/// A host that patches one key into a nested preference object needs the rest of
/// that object's fields, because the nested structures require all of them - only
/// the object as a whole is optional. Writing those defaults into a platform host
/// would put a second copy of this contract in another language, so they are
/// published here instead.
#[no_mangle]
pub extern "C" fn msime_client_default_preferences() -> *mut c_char {
    response(|| {
        serde_json::to_value(msime_client_core::preferences::Preferences::default())
            .map_err(|e| e.to_string())
    })
}

/// 触控板设置控件的默认值和范围，宿主只负责展示；结果用 `msime_client_string_free` 释放。
#[no_mangle]
pub extern "C" fn msime_client_text_trackpad_options() -> *mut c_char {
    use msime_client_core::preferences::TextTrackpadPreferences as Trackpad;
    response(|| {
        Ok(json!({
            "defaults": Trackpad::default(),
            "horizontal_dp": {"minimum": Trackpad::HORIZONTAL_MIN_DP, "maximum": Trackpad::HORIZONTAL_MAX_DP},
            "vertical_dp": {"minimum": Trackpad::VERTICAL_MIN_DP, "maximum": Trackpad::VERTICAL_MAX_DP},
        }))
    })
}

/// The transcription provider and the optional rewrite, resolved from a preferences directory.
///
/// The Android keyboard has its own voice entry and never goes through the desktop shell, so the
/// resolution it needs is here rather than in that shell. Contains credentials: never log the
/// response; release with `msime_client_string_free`.
/// # Safety
/// `directory` points to `length` readable UTF-8 bytes naming an absolute path. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_mobile_voice_configuration(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length == 0 || length > 16_384 {
            return Err("invalid preferences directory".into());
        }
        // SAFETY: guaranteed by the documented caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = std::str::from_utf8(bytes).map_err(|_| "invalid preferences directory")?;
        if !std::path::Path::new(directory).is_absolute() {
            return Err("invalid preferences directory".into());
        }
        let store = msime_client_core::preferences::PreferencesStore::new(directory);
        let snapshot = store.load().map_err(|_| "preferences unavailable")?;
        let provider =
            msime_client_providers::voice::provider::mobile_voice_provider_configuration(
                &snapshot.preferences,
            )
            .map(|value| {
                json!({
                    "provider": value.provider,
                    "requestFormat": value.request_format,
                    "endpoint": value.endpoint,
                    "model": value.model,
                    "token": value.token,
                    "headers": value
                        .headers
                        .iter()
                        .map(|header| json!({"name": header.name, "value": header.value}))
                        .collect::<Vec<_>>(),
                    "enableItn": value.enable_itn,
                    "enablePunctuation": value.enable_punctuation,
                    "enableDdc": value.enable_ddc,
                    "boostingTableId": value.boosting_table_id,
                    "modelPath": value.model_path,
                })
            });
        let polish = msime_client_providers::voice::provider::mobile_voice_polish_configuration(
            &snapshot.preferences,
        )
        .map(|value| {
            json!({
                "endpoint": value.endpoint,
                "model": value.model,
                "token": value.token,
                "promptId": value.prompt_id,
                "promptCustom1": value.prompt_custom_1,
                "promptCustom2": value.prompt_custom_2,
                "promptCustom3": value.prompt_custom_3,
            })
        });
        Ok(json!({ "provider": provider, "polish": polish }))
    })
}

/// 本库编译到的宿主平台。主题目录按它列出主题、会话按它截断每页候选数：这些接口不带平台参数，而每个原生宿主链接的都是为自己的平台编出的库。
pub(crate) fn compiled_platform() -> HostPlatform {
    if cfg!(target_os = "ios") {
        HostPlatform::Ios
    } else if cfg!(target_os = "android") {
        HostPlatform::Android
    } else if cfg!(target_env = "ohos") {
        HostPlatform::Harmony
    } else if cfg!(target_os = "macos") {
        HostPlatform::Macos
    } else if cfg!(windows) {
        HostPlatform::Windows
    } else {
        HostPlatform::Linux
    }
}

/// 全局主题选择器：本平台提供的主题 id，按选择器顺序，带标题和调色板，以及默认 id。
///
/// 只列出本库编译到的平台提供的主题（`theme::catalog_for`）：`native` 只在 iOS 的库里出现，其他宿主的选择器因此不必各自过滤。`system`、`native` 和 `custom` 在这里不带调色板：`system` 和 `native` 由宿主自己取色，`custom` 要按用户的 `custom_theme` 解析后才知道。宿主据此画选择器，不保存 id、标题或颜色的副本。
#[no_mangle]
pub extern "C" fn msime_client_theme_catalog() -> *mut c_char {
    response(|| {
        Ok(serde_json::json!({
            "themes": msime_client_services::skin::theme::catalog_for(compiled_platform()),
            "default": msime_client_services::skin::theme::GlobalTheme::default(),
        }))
    })
}

/// Largest resolve request. A custom keyboard design may carry a photo of up to 682,668 base64 bytes, and the rest of the request is small.
const MAX_THEME_REQUEST_BYTES: usize = 1 << 20;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveThemeRequest {
    global_theme: msime_client_services::skin::theme::GlobalTheme,
    #[serde(default)]
    custom_theme: msime_client_core::preferences::CustomTheme,
    dark: bool,
    layout: msime_client_core::preferences::CandidateLayout,
    skins_directory: Option<String>,
    package: Option<serde_json::Value>,
    /// 宿主确认当前面板贴图已解码且将绘制，才使用贴图原配色；旧宿主缺省使用可读回退。
    #[serde(default)]
    canvas_panel_drawn: bool,
}

/// 应用主题请求里的月份所在的季节；没传月份时按 UTC 月份，月份不在 1..=12 时请求失败。
fn requested_season(
    month: Option<u8>,
    invalid: &str,
) -> Result<msime_client_services::skin::season::Season, String> {
    match month {
        Some(month) => msime_client_services::skin::season::season_for_month(month)
            .ok_or_else(|| invalid.to_owned()),
        None => Ok(msime_client_services::skin::season::current_utc_season()),
    }
}

/// 解析宿主绘制全局主题所用的颜色。
///
/// 请求带着宿主读到的两个偏好字段，所以这里不需要偏好目录，也不做偏好 I/O：`global_theme`（`GlobalTheme::ALL` 的八个 id 之一，本平台目录里没有的 `native` 也接受，解析结果与 `system` 相同、只是 `id` 为 `native`；其他 id，包括已停用的皮肤 id，都以 `invalid theme request` 失败）和 `custom_theme`（可省略，按偏好本身同样严格地校验）。`dark` 是宿主正在绘制的界面当前的明暗模式，`layout`（`horizontal` 或 `vertical`）是它的候选排列：皮肤包只在清单声明的排列和模式下绘制，宿主不另设一道关卡。`dark` 同时决定取哪个槽位：深色模式取 `custom_theme.candidate_skin_dark`，没设时与浅色模式一样取 `custom_theme.candidate_skin`；选中的包只在它清单 `base` 的明暗下绘制（见 `theme::resolve`）。`custom` 主题要用皮肤包时，宿主最多用下面两者之一说明包从哪里来：`skins_directory`（皮肤根目录的绝对路径；包按 `msime_client_skin_package` 的规则加载和校验；直接扫描皮肤的宿主及已准备 canvas 面板的 Fcitx5 使用）或 `package`（已发布的 `candidate_skin_catalog` 中的一项，只有 Linux 宿主读它，宿主要传当前模式槽位指名的那一项）。`package` 不是这样的一项时（包括 `msime_client_skin_catalog` 给出的 `SkinSummary`）调用失败：那是宿主的缺陷，照读会悄悄丢掉它声明的模式和选中条。根目录里没有、磁盘上无效、或不是当前槽位指名的那个包会被略过而不是让调用失败：主题照常在它的底上解析。
/// `canvas_panel_drawn` 缺省为 false。只有已成功解码且实际绘制当前 canvas 面板的宿主才传 true；共享层在包配色上套用原有取色器规则，不能由宿主另行覆盖最终配色。此标志只作用于 `skins_directory` 读取的含面板包。Linux 公共 catalog 的配色仍是可读回退；Fcitx5 准备好面板后改用根目录协商。
/// `candidate_details` 可选提供规范化的预编辑文字／光标／背景／分隔线、选中条、释义及词性／义项分隔符的普通与选中色、菜单四槽位；宿主忽略不支持的键。文字取色器同时覆盖预编辑文字，宿主不从原始包重新取色。
/// # Safety
/// `request` 指向 `length` 个可读的 UTF-8 JSON 字节，不接受空指针。
/// 返回的响应必须通过 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_resolve_theme(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > MAX_THEME_REQUEST_BYTES {
            return Err("invalid theme request".into());
        }
        // SAFETY: guaranteed by the documented caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: ResolveThemeRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid theme request")?;
        request
            .custom_theme
            .validate()
            .map_err(|error| error.to_string())?;
        if request.skins_directory.is_some() && request.package.is_some() {
            return Err("theme request names both a skins directory and a package".into());
        }
        if request
            .skins_directory
            .as_deref()
            .is_some_and(|directory| !Path::new(directory).is_absolute())
        {
            return Err("skin directory must be absolute".into());
        }
        let theme = request.global_theme;
        // 按当前明暗取槽位：深色模式先取 `candidate_skin_dark`。
        let wanted = request
            .custom_theme
            .candidate_skin_for(request.dark)
            .filter(|_| theme == msime_client_services::skin::theme::GlobalTheme::Custom);
        let entry = request
            .package
            .map(msime_client_services::skin::theme::ThemePackage::from_host_catalog_entry)
            .transpose()?;
        let package = match (wanted, request.skins_directory, entry) {
            (Some(id), Some(directory), _) => {
                msime_client_services::skin::catalog::load_package(&directory, id)
                    .ok()
                    .map(|mut summary| {
                        if request.canvas_panel_drawn {
                            if let Some(colors) = summary.panel_candidate.take() {
                                summary.candidate = colors;
                            }
                        }
                        msime_client_services::skin::theme::ThemePackage::from(&summary)
                    })
            }
            (Some(_), None, entry) => entry,
            _ => None,
        };
        let resolved = msime_client_services::skin::theme::resolve(
            theme,
            &request.custom_theme,
            request.dark,
            request.layout,
            package.as_ref(),
        );
        serde_json::to_value(resolved).map_err(|error| error.to_string())
    })
}

/// 应用主题的选择器：每个应用主题的 ID、标题、固定的季节和浅色、深色的颜色，以及默认 ID。
///
/// `siji` 的 `season` 为 `null`、`seasonal` 为真，它的颜色固定画秋杉，所以这份目录不随时钟变化；宿主画当季颜色时调 `msime_client_resolve_app_theme`。宿主不保存 ID、标题或色值的副本。
#[no_mangle]
pub extern "C" fn msime_client_app_theme_catalog() -> *mut c_char {
    response(|| {
        Ok(json!({
            "app_themes": msime_client_services::skin::app_theme::catalog(),
            "default": msime_client_services::skin::app_theme::AppTheme::default(),
        }))
    })
}

/// 解析应用主题请求的上限。请求只有三个短字段。
const MAX_APP_THEME_REQUEST_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveAppThemeRequest {
    app_theme: msime_client_services::skin::app_theme::AppTheme,
    #[serde(default)]
    month: Option<u8>,
    dark: bool,
}

/// 应用主题在宿主当前月份和明暗模式下的颜色：`{id, season, accent, accent_soft, on_accent, background, card, hair}`。纯计算，不读写文件。
/// # Safety
/// `request` points to `length` readable UTF-8 JSON bytes. Null is rejected.
/// The returned response must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_resolve_app_theme(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > MAX_APP_THEME_REQUEST_BYTES {
            return Err("invalid app theme request".into());
        }
        // SAFETY: guaranteed by the documented caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: ResolveAppThemeRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid app theme request")?;
        let season = requested_season(request.month, "invalid app theme request")?;
        let resolved = msime_client_services::skin::app_theme::resolve_app_theme(
            request.app_theme,
            season,
            request.dark,
        );
        serde_json::to_value(resolved).map_err(|error| error.to_string())
    })
}

/// 「重置所有设置」：在偏好锁里读出当前文档，换成本存储所属版本的默认偏好（服务凭据和 `fuzzy_pinyin.seeded` 保留，见 `Preferences::restored_to_defaults_for`），再按 `expected_revision` 比较并交换写回，返回新的快照。修订号不符时以 `preferences changed; reload before saving` 失败，什么也不写。词库、统计和剪贴板历史不受影响；默认关闭剪贴板历史时与保存偏好一样清空已存的历史。
/// # Safety
/// `directory` must point to `length` readable bytes. Null is rejected.
/// The returned response must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_restore_default_preferences(
    directory: *const u8,
    length: usize,
    expected_revision: u64,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = super::parse_absolute_path(
            bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let store = PreferencesStore::new(directory);
        let restored = store
            .restore_defaults(expected_revision)
            .map_err(|e| e.to_string())?;
        if !restored.preferences.clipboard_history {
            store
                .clear_disabled_clipboard_history()
                .map_err(|e| e.to_string())?;
        }
        serde_json::to_value(restored).map_err(|e| e.to_string())
    })
}

/// 「导出设置」：读出偏好目录里的当前偏好，返回设置文件的全文（字符串，见 `msime_client_core::settings_document`）。语音、AI 辅助和翻译服务的配置与密钥、诊断日志、使用统计和剪贴板历史开关不写进文件。宿主负责让用户选位置并写盘。
/// # Safety
/// `directory` 必须指向 `length` 个可读字节，空指针被拒绝。
/// 返回的响应必须交给 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_export_settings(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        // SAFETY: 由上面写明的调用方契约保证。
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = super::parse_absolute_path(
            bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let snapshot = PreferencesStore::new(directory)
            .load()
            .map_err(|e| e.to_string())?;
        msime_client_core::settings_document::export_document(&snapshot.preferences)
            .map(Value::String)
            .map_err(|e| e.to_string())
    })
}

/// 「导入设置」：把用户选的设置文件换算成偏好（本机的服务配置与密钥、诊断日志、使用统计和剪贴板历史开关保留，按本目录所属的版本收窄输入方案），再按 `expected_revision` 比较并交换写回，返回新的快照。剪贴板历史开关不随文件变，所以导入不会像保存偏好那样清空已存的剪贴板历史，保存成功就是整个导入成功。失败时的错误是 `settings_document_invalid`（不是设置文件）、`settings_document_macos`（macOS 原生设置窗口导出的文件）、`settings_document_unsupported`（更新版本的文件或读不懂的设置）、`settings_conflict`（设置已在别处更新）之一，或者存储错误的说明；失败时什么也不写。
/// # Safety
/// `directory` 必须指向 `directory_length` 个可读字节，`document` 必须指向 `document_length` 个可读字节，空指针被拒绝。
/// 返回的响应必须交给 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_import_settings(
    directory: *const u8,
    directory_length: usize,
    expected_revision: u64,
    document: *const u8,
    document_length: usize,
) -> *mut c_char {
    use msime_client_core::settings_document;
    response(|| {
        if directory.is_null() || document.is_null() || directory_length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        if document_length > settings_document::MAX_DOCUMENT_BYTES {
            return Err(
                settings_document::SettingsDocumentError::NotSettingsDocument
                    .code()
                    .into(),
            );
        }
        // SAFETY: 由上面写明的调用方契约保证。
        let directory_bytes = unsafe { std::slice::from_raw_parts(directory, directory_length) };
        let directory = super::parse_absolute_path(
            directory_bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        // SAFETY: 由上面写明的调用方契约保证。
        let document = unsafe { std::slice::from_raw_parts(document, document_length) };
        let store = PreferencesStore::new(directory);
        let current = store.load().map_err(|e| e.to_string())?;
        let imported =
            settings_document::import_document(document, &current.preferences, store.edition())
                .map_err(|error| error.code().to_owned())?;
        let saved = store
            .save(expected_revision, imported)
            .map_err(|error| match error {
                msime_client_core::preferences::PreferencesError::Conflict => {
                    "settings_conflict".to_owned()
                }
                error => error.to_string(),
            })?;
        serde_json::to_value(saved).map_err(|e| e.to_string())
    })
}

/// Per-key double-pinyin hint text for one profile, read out of the Engine's own
/// profile tables.
///
/// A touch keyboard labels its letter keys with the units they carry, and a face
/// that keeps its own copy of that keymap drifts from the scheme the session
/// actually runs. The hints depend only on the profile, not on session state, so
/// this takes no handle. An unknown name yields an empty object rather than the
/// default profile's hints: labelling the keys with a scheme the session is not
/// running is worse than labelling nothing.
/// # Safety
/// `profile` points to `length` readable UTF-8 bytes. Null is rejected.
/// The returned response must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_shuangpin_key_hints(
    profile: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if profile.is_null() || length > 64 {
            return Err("invalid shuangpin profile buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(profile, length) };
        let name = std::str::from_utf8(bytes).map_err(|_| "invalid shuangpin profile encoding")?;
        let entries = msime_engine::host::shuangpin_key_hints(name);
        let mut hints = serde_json::Map::with_capacity(entries.len());
        hints.extend(
            entries
                .into_iter()
                .map(|entry| (entry.key, serde_json::Value::String(entry.hint))),
        );
        Ok(serde_json::Value::Object(hints))
    })
}

/// The double-pinyin codes of the whole zero-initial syllables for one profile, read out of the Engine's own profile tables, as a JSON object such as `{"a":"aa","ang":"ah",...}`.
///
/// A keymap panel shows these next to the key face, and like the key hints they depend only on the profile, so this takes no handle. An unknown name yields an empty object.
/// # Safety
/// `profile` points to `length` readable UTF-8 bytes. Null is rejected.
/// The returned response must be released with `msime_client_string_free`.
#[no_mangle]
pub unsafe extern "C" fn msime_client_shuangpin_zero_initials(
    profile: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if profile.is_null() || length > 64 {
            return Err("invalid shuangpin profile buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract; size checked above.
        let bytes = unsafe { std::slice::from_raw_parts(profile, length) };
        let name = std::str::from_utf8(bytes).map_err(|_| "invalid shuangpin profile encoding")?;
        let entries = msime_engine::host::shuangpin_zero_initials(name);
        let mut codes = serde_json::Map::with_capacity(entries.len());
        codes.extend(entries.into_iter().map(|(syllable, code)| {
            (
                syllable.to_string(),
                serde_json::Value::String(code.to_string()),
            )
        }));
        Ok(serde_json::Value::Object(codes))
    })
}

/// Load the shared store on a worker thread; no session handle is accessed.
/// # Safety
/// `directory` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_load_preferences(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = super::parse_absolute_path(
            bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let snapshot = PreferencesStore::new(directory)
            .load()
            .map_err(|e| e.to_string())?;
        serde_json::to_value(snapshot).map_err(|e| e.to_string())
    })
}

/// 可重绑切换热键（`keybindings.toggle_english_candidates`、`toggle_full_width`、
/// `toggle_punctuation`、`toggle_voice_input`）按共享 `KeyChord` 契约解码：每个键给出 `null`（不绑定）或
/// `{"key": 主键规范名, "ctrl", "alt", "shift", "meta"}`，宿主按规范名认键，
/// 写法解析只在 Rust 一侧。keybindings 缺键或值为 null 时按出厂值。
/// # Safety
/// `request` 指向 `length` 字节的 JSON `{"keybindings": object}`，最多 64 KiB；
/// keybindings 不是表时返回错误。结果用 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_toggle_hotkey_chords(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        #[serde(default)]
        keybindings: Value,
    }
    response(|| {
        if request.is_null() || length > 64 * 1024 {
            return Err("invalid toggle hotkey request buffer".into());
        }
        // SAFETY: 调用方保证缓冲区可读，上方已检查长度上限。
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: Request = serde_json::from_slice(bytes)
            .map_err(|_| "invalid toggle hotkey request".to_owned())?;
        let keys = if request.keybindings.is_null() {
            msime_client_core::preferences::KeybindingPreferences::default()
        } else {
            serde_json::from_value(request.keybindings)
                .map_err(|_| "invalid keybindings".to_owned())?
        };
        Ok(json!({
            "english_candidates": toggle_hotkey_chord(keys.english_candidates_chord()),
            "full_width": toggle_hotkey_chord(keys.full_width_chord()),
            "punctuation": toggle_hotkey_chord(keys.punctuation_chord()),
            "voice_input": toggle_hotkey_chord(keys.toggle_voice_input.parse().ok()),
        }))
    })
}

/// 原生设置录键的试写检查：复用共享偏好的语法、规范化和冲突表，不写入配置。
/// # Safety
/// `request` 指向不超过 64 KiB 的有效 JSON；返回字符串交由 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_check_toggle_hotkey(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    use msime_client_core::preferences::{
        KeybindingPreferences, ToggleHotkeyRejection, ToggleHotkeySlot,
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        field: ToggleHotkeySlot,
        chord: String,
        #[serde(default)]
        keybindings: KeybindingPreferences,
        voice_ctrl_f9: bool,
    }
    response(|| {
        if request.is_null() || length > 64 * 1024 {
            return Err("invalid toggle hotkey request buffer".into());
        }
        // SAFETY: 调用方保证缓冲区可读，上方已检查长度上限。
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: Request = serde_json::from_slice(bytes)
            .map_err(|_| "invalid toggle hotkey request".to_owned())?;
        Ok(
            match request.keybindings.check_toggle_hotkey(
                request.field,
                &request.chord,
                request.voice_ctrl_f9,
            ) {
                Ok(chord) => json!({"ok": true, "chord": chord}),
                Err(ToggleHotkeyRejection::Invalid(_)) => json!({"ok": false, "kind": "invalid"}),
                Err(ToggleHotkeyRejection::Conflict { owner, .. }) => json!({
                    "ok": false, "kind": "conflict", "owner": owner.id(),
                }),
            },
        )
    })
}

fn toggle_hotkey_chord(chord: Option<msime_client_core::key_chord::KeyChord>) -> Value {
    chord.map_or(Value::Null, |chord| {
        json!({
            "key": chord.key.to_string(),
            "ctrl": chord.modifiers.ctrl,
            "alt": chord.modifiers.alt,
            "shift": chord.modifiers.shift,
            "meta": chord.modifiers.meta,
        })
    })
}

/// Read or update private aggregate typing statistics without retaining submitted text.
/// # Safety
/// `request` points to `length` readable JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_typing_statistics(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        directory: String,
        action: StatisticsAction,
        /// 设置页要的是状态：load、set_enabled、set_retention 和 reset 回答 `{statistics, availability, lastWrittenMs}`（client-services 的 `TypingStatisticsStatus`），而不只是统计本身。
        #[serde(default)]
        status: bool,
    }
    #[derive(Deserialize)]
    #[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
    enum StatisticsAction {
        Load,
        Record {
            text: String,
            source: TypingSource,
            day: String,
            /// Local hour of the commit, 0-23. Absent from hosts that have not been taught to
            /// send one, whose days then have characters but no hourly breakdown - which is the
            /// honest result, since this layer cannot resolve the host's timezone itself.
            #[serde(default)]
            hour: Option<u8>,
            /// Set by a host that must keep effect sounds quiet right now (Windows while a full-screen application is in front): the commit is still counted and the milestone still reported, but no jingle plays.
            #[serde(default)]
            quiet: bool,
        },
        SetEnabled {
            enabled: bool,
        },
        SetRetention {
            retention: String,
            /// The caller's local day, for the same reason `record` takes one: only the host
            /// knows which day the window is counted back from.
            day: String,
        },
        /// Key presses a host batched in memory, as `{"KeyA": 3, ...}` press counts for the local day they happened on. Ids outside `KEY_IDS` or zero counts reject the whole batch.
        RecordKeys {
            day: String,
            keys: std::collections::BTreeMap<String, u64>,
        },
        /// 派生指标（概览、习惯、按键、徽章），新解锁的徽章在锁内写进 `achievements`。隐私模式不影响它：它只读已经记下的数据。
        Summary {
            day: String,
            #[serde(default)]
            user_words: Option<u64>,
        },
        /// 一次语音输入的时长，计在宿主的本地日 `day` 上。
        RecordVoice {
            day: String,
            milliseconds: u64,
        },
        /// 用过一款皮肤（徽章「换装达人」）。
        RecordSkin {
            id: String,
        },
        Reset,
    }
    response(|| {
        if request.is_null() || length > 65_536 {
            return Err("invalid typing statistics buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: Request = serde_json::from_slice(bytes)
            .map_err(|_| "invalid typing statistics request".to_owned())?;
        if request.directory.len() > 16_384
            || !std::path::Path::new(&request.directory).is_absolute()
        {
            return Err("invalid typing statistics directory".into());
        }
        let store = TypingStatisticsStore::new(request.directory);
        let status = request.status;
        if status
            && !matches!(
                request.action,
                StatisticsAction::Load
                    | StatisticsAction::SetEnabled { .. }
                    | StatisticsAction::SetRetention { .. }
                    | StatisticsAction::Reset
            )
        {
            return Err(
                "typing statistics status applies to load, set_enabled, set_retention and reset"
                    .into(),
            );
        }
        // 统计本身，或者设置页要的状态。
        let answer = |statistics: msime_client_services::typing_statistics::TypingStatistics| {
            if status {
                serde_json::to_value(
                    store
                        .status(statistics)
                        .map_err(|error| error.to_string())?,
                )
            } else {
                serde_json::to_value(statistics)
            }
            .map_err(|_| "typing statistics response failed".to_owned())
        };
        match request.action {
            StatisticsAction::Load => answer(store.load().map_err(|error| error.to_string())?),
            StatisticsAction::Record {
                text,
                source,
                day,
                hour,
                quiet,
            } => {
                // The count is read first only when a session asked for achievement sounds, so recording stays one read and one write for everyone else.
                let before = key_sound::achievements_armed()
                    .then(|| store.load().ok().map(|statistics| statistics.total))
                    .flatten();
                let recorded = store
                    .record(&text, source, &day, hour)
                    .map_err(|error| error.to_string())?;
                let milestone = before.and_then(|before| {
                    msime_client_services::plugins::achievement_milestone(
                        before,
                        before.saturating_add(recorded),
                    )
                });
                if milestone.is_some() && !quiet {
                    key_sound::achievement();
                }
                Ok(json!({"recorded": recorded, "milestone": milestone}))
            }
            StatisticsAction::RecordKeys { day, keys } => {
                let recorded = store
                    .record_keys(&day, &keys)
                    .map_err(|error| error.to_string())?;
                Ok(json!({"recorded": recorded}))
            }
            StatisticsAction::Summary { day, user_words } => serde_json::to_value(
                store
                    .summary(
                        &day,
                        &msime_client_services::typing_statistics::SummaryInputs { user_words },
                    )
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|_| "typing statistics response failed".to_owned()),
            StatisticsAction::RecordVoice { day, milliseconds } => {
                let recorded = store
                    .record_voice(&day, milliseconds)
                    .map_err(|error| error.to_string())?;
                Ok(json!({"recorded": recorded}))
            }
            StatisticsAction::RecordSkin { id } => {
                let recorded = store.record_skin(&id).map_err(|error| error.to_string())?;
                Ok(json!({"recorded": recorded}))
            }
            StatisticsAction::SetEnabled { enabled } => answer(
                store
                    .set_enabled(enabled)
                    .map_err(|error| error.to_string())?,
            ),
            StatisticsAction::SetRetention { retention, day } => answer(
                store
                    .set_retention(
                        msime_client_services::typing_statistics::StatisticsRetention::parse(
                            &retention,
                        ),
                        &day,
                    )
                    .map_err(|error| error.to_string())?,
            ),
            StatisticsAction::Reset => answer(store.reset().map_err(|error| error.to_string())?),
        }
    })
}

/// Read the typing-statistics master switch without accepting committed text.
///
/// Native hosts cache this result and refresh it when settings change, so an opt-out can stop at
/// the capture boundary instead of serializing text merely to discover that recording is off.
/// # Safety
/// `directory` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_typing_statistics_enabled(
    directory: *const u8,
    length: usize,
) -> i32 {
    if directory.is_null() || length > 16_384 {
        return -1;
    }
    // SAFETY: guaranteed by the documented caller contract.
    let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
    let Ok(directory) = std::str::from_utf8(bytes) else {
        return -1;
    };
    if !std::path::Path::new(directory).is_absolute() {
        return -1;
    }
    match TypingStatisticsStore::new(directory).load() {
        Ok(statistics) => i32::from(statistics.enabled),
        Err(_) => -1,
    }
}

/// Scan a skin root so native presenters read the same catalog the settings
/// page edits. Unreadable roots return an empty catalog, not an error; a
/// package that fails validation is reported as an issue and never rendered.
/// # Safety
/// `directory` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_catalog(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid skin directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory =
            std::str::from_utf8(bytes).map_err(|_| "invalid skin directory encoding")?;
        if !std::path::Path::new(directory).is_absolute() {
            return Err("skin directory must be absolute".into());
        }
        serde_json::to_value(msime_client_services::skin::catalog::scan(directory))
            .map_err(|e| e.to_string())
    })
}

#[derive(Debug, Deserialize)]
struct SkinPackageRequest {
    directory: String,
    id: String,
}

/// Validate one installed skin package with the loader the settings page uses, so a native presenter resolving the selected skin accepts exactly the manifests the catalog lists (full TOML 1.0, the Windows toml++ baseline). The value is one camelCase entry of `msime_client_skin_catalog`'s `packages`; a package that fails validation answers `{ok:false,error}` with the loader's reason.
/// # Safety
/// `request` points to `length` readable UTF-8 JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_package(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > 65_536 {
            return Err("invalid skin package request".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: SkinPackageRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid skin package request")?;
        if !Path::new(&request.directory).is_absolute() {
            return Err("skin directory must be absolute".into());
        }
        let package =
            msime_client_services::skin::catalog::load_package(&request.directory, &request.id)?;
        serde_json::to_value(package).map_err(|e| e.to_string())
    })
}

#[derive(Debug, Deserialize)]
struct SkinResourceRequest {
    directory: String,
    id: String,
    relative: String,
    kind: String,
    /// `"base64"` returns the bytes as one base64 string, `bytesBase64`, instead of the `bytes` array. Absent keeps the array for older callers; every msime host asks for base64.
    #[serde(default)]
    encoding: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkinToolbarImageLayoutRequest {
    toolbar: msime_client_services::skin::catalog::SkinToolbarImage,
    shown: Vec<[String; 2]>,
}

/// 根据扫描缓存里的 `toolbarImage` 与当前显示的 `[按钮键, 状态]` 计算图片工具栏。
/// 不读文件；状态切换时不应重新加载或解码整个皮肤。返回窗口尺寸及按钮矩形、底图和状态图。
/// # Safety
/// `request` 指向 `length` 字节可读 UTF-8 JSON，结果由 `msime_client_string_free` 释放。
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_toolbar_image_layout(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > 65_536 {
            return Err("invalid toolbar image layout request".into());
        }
        // 安全性：调用方保证请求缓冲可读，上面已限制长度。
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: SkinToolbarImageLayoutRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid toolbar image layout request")?;
        if request.shown.len() > msime_client_services::skin::catalog::TOOLBAR_BUTTON_KEYS.len()
            || request
                .shown
                .iter()
                .any(|[key, state]| key.len() > 32 || state.len() > 32)
        {
            return Err("invalid toolbar image button request".into());
        }
        let layout = request.toolbar.checked_layout(
            request
                .shown
                .iter()
                .map(|[key, state]| (key.as_str(), state.as_str())),
        )?;
        serde_json::to_value(layout).map_err(|error| error.to_string())
    })
}

#[derive(Debug, Deserialize)]
struct SkinStylesheetRequest {
    directory: String,
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkinResourceResponse {
    content_type: &'static str,
    bytes: Vec<u8>,
}

/// Read one validated image or font from an external skin package.
///
/// The request keeps the root directory explicit because Harmony's settings
/// bridge and the input-method ability share the same C ABI but have different
/// lifetimes. The package manifest is revalidated by `read_resource` on every
/// call, so a stale catalog cannot turn this endpoint into an arbitrary file
/// reader. `kind` is deliberately checked here as well: a WebView image reader
/// must never receive CSS or a font by mistake.
///
/// The default `bytes` array costs about 64 bytes of heap per resource byte while the response is built (a `serde_json::Value` per byte, measured on a 1 MiB image), and a JSON parser on the host side spends tens more turning it back into bytes. The iOS keyboard extension reads candidate-skin images under the system's per-extension memory limit, where that spike for a single 1 MiB image is larger than everything else the keyboard holds, so it asks for `encoding: "base64"`. So do the Android and HarmonyOS keyboards, the HarmonyOS settings page and both Windows settings apps: at the 8 MiB resource limit the array is about 512 MiB on this side alone. The array stays the default so an older caller keeps working.
/// # Safety
/// `request` points to `length` readable UTF-8 JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_resource(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > 65_536 {
            return Err("invalid skin resource request".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: SkinResourceRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid skin resource request")?;
        if request.kind != "image" && request.kind != "font" {
            return Err("unsupported skin resource kind".into());
        }
        let as_base64 = match request.encoding.as_deref() {
            None => false,
            Some("base64") => true,
            Some(_) => return Err("unsupported skin resource encoding".into()),
        };
        if !Path::new(&request.directory).is_absolute() {
            return Err("skin directory must be absolute".into());
        }
        let resource = msime_client_services::skin::catalog::read_resource(
            &request.directory,
            &request.id,
            &request.relative,
        )
        .map_err(|_| "skin resource unavailable")?;
        let expected = if request.kind == "image" {
            "image/"
        } else {
            "font/"
        };
        if !resource.content_type.starts_with(expected) {
            return Err("skin resource type mismatch".into());
        }
        if as_base64 {
            use base64::Engine as _;
            return Ok(json!({
                "contentType": resource.content_type,
                "bytesBase64": base64::engine::general_purpose::STANDARD.encode(&resource.bytes),
            }));
        }
        serde_json::to_value(SkinResourceResponse {
            content_type: resource.content_type,
            bytes: resource.bytes,
        })
        .map_err(|_| "skin resource response failed".into())
    })
}

/// Read the stylesheet declared by an external skin package.
/// # Safety
/// `request` points to `length` readable UTF-8 JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_toolbar_stylesheet(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > 65_536 {
            return Err("invalid skin stylesheet request".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: SkinStylesheetRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid skin stylesheet request")?;
        if !Path::new(&request.directory).is_absolute() {
            return Err("skin directory must be absolute".into());
        }
        let stylesheet = msime_client_services::skin::catalog::read_toolbar_stylesheet(
            &request.directory,
            &request.id,
        )
        .map_err(|_| "skin stylesheet unavailable")?;
        serde_json::to_value(stylesheet).map_err(|_| "skin stylesheet response failed".into())
    })
}

#[derive(Debug, Deserialize)]
struct SkinImportRequest {
    source: String,
    directory: String,
}

/// Copy a skin folder the user picked into the host's skin root, for a host whose root no file manager reaches (the iOS App Group). Returns `{id}`, the folder name the catalog lists it under; a failure's message is the import's code (`skin_name`, `skin_manifest` or `storage`) so the host can explain it.
/// # Safety
/// `request` points to `length` readable UTF-8 JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_skin_import(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length == 0 || length > 65_536 {
            return Err("invalid skin import request".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: SkinImportRequest =
            serde_json::from_slice(bytes).map_err(|_| "invalid skin import request")?;
        if !Path::new(&request.source).is_absolute() || !Path::new(&request.directory).is_absolute()
        {
            return Err("skin paths must be absolute".into());
        }
        let id = msime_client_services::skin::folder_import::import(
            Path::new(&request.source),
            Path::new(&request.directory),
        )
        .map_err(str::to_owned)?;
        Ok(json!({ "id": id }))
    })
}

/// 读取共用键盘皮肤目录，或显式重置、选择模板／背景及优化对比度；不写设置或命名库。
/// # Safety
/// `request` 指向 `length` 个可读的 UTF-8 JSON 字节；空指针和超过 1 MiB 的请求被拒绝。
#[no_mangle]
pub unsafe extern "C" fn msime_client_keyboard_skin_design(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        // SAFETY: 调用方保证指针有效，公共边界先检查长度和空指针。
        unsafe {
            with_bounded_bytes(
                request,
                length,
                1024 * 1024,
                "keyboard_skin_invalid",
                |bytes| {
                    let action = serde_json::from_slice(bytes)
                        .map_err(|_| "keyboard_skin_invalid".to_owned())?;
                    msime_client_core::keyboard_skin::edit(action).map_err(str::to_owned)
                },
            )
        }
    })
}

/// What dictionary is installed, for the settings page to show.
///
/// The packaged dictionary ships with a manifest naming which specification it
/// was built to and which upstream commit it came from. Apple's settings read
/// it straight out of the app bundle; a host whose resources are staged into a
/// sandbox cannot, and the path is not something a settings page should be
/// told anyway.
///
/// Only two fields come back. The manifest also records journal modes, format
/// contracts and every third-party reference, none of which answers the
/// question the page is asking — which is "what do I have, and where did it
/// come from". A missing or unreadable manifest is reported rather than
/// guessed at: showing the wrong dictionary version is worse than showing
/// none.
/// # Safety
/// `resources` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_dictionary_manifest(
    resources: *const u8,
    length: usize,
) -> *mut c_char {
    #[derive(Deserialize)]
    struct Source {
        commit: String,
    }
    #[derive(Deserialize)]
    struct Manifest {
        profile: String,
        source: Source,
    }
    response(|| {
        if resources.is_null() || length > 16384 {
            return Err("dictionary_manifest_unavailable".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(resources, length) };
        let directory =
            std::str::from_utf8(bytes).map_err(|_| "dictionary_manifest_unavailable")?;
        let path = Path::new(directory);
        if !path.is_absolute() {
            return Err("dictionary_manifest_unavailable".into());
        }
        // Bounded before parsing: this is a packaged file, and one that has grown to megabytes is
        // not a manifest whatever it parses as.
        let file = path.join("msime-dictionary-manifest.json");
        let bytes = crate::bounded_file::read(
            crate::bounded_file::open_private(&file)
                .map_err(|_| "dictionary_manifest_unavailable")?,
            1024 * 1024,
        )
        .map_err(|_| "dictionary_manifest_unavailable")?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "dictionary_manifest_unavailable")?;
        let manifest: Manifest =
            serde_json::from_str(text).map_err(|_| "dictionary_manifest_unavailable")?;
        if manifest.profile.is_empty()
            || !is_bounded_text(&manifest.profile, 64)
            || !msime_client_core::is_ascii_hex(&manifest.source.commit, 40)
        {
            return Err("dictionary_manifest_unavailable".into());
        }
        Ok(json!({"profile": manifest.profile, "sourceCommit": manifest.source.commit}))
    })
}

/// Read saved clipboard history without observing or modifying the system clipboard.
/// # Safety
/// `directory` points to `length` readable UTF-8 bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_load_clipboard_history(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid history directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory =
            std::str::from_utf8(bytes).map_err(|_| "invalid history directory encoding")?;
        let path = std::path::Path::new(directory);
        if !path.is_absolute() {
            return Err("history directory must be absolute".into());
        }
        let preferences = PreferencesStore::new(path)
            .load()
            .map_err(|_| "history preferences unavailable")?
            .preferences;
        let enabled = preferences.clipboard_history;
        if !enabled {
            return Ok(serde_json::json!({"enabled": false, "entries": []}));
        }
        let mut history = msime_client_core::clipboard::ClipboardHistoryStore::open_with_limit(
            path.join("clipboard_history.json"),
            preferences.clipboard_history_limit,
        );
        history
            .load()
            .map_err(|_| "clipboard history unavailable")?;
        let mut entries = Vec::with_capacity(history.entries().len());
        entries.extend(history.entries().iter().map(|entry| entry.text.as_str()));
        // This ABI returns plain entry text; the structured history is not exposed through it.
        Ok(serde_json::json!({"enabled": true, "entries": entries}))
    })
}

const MAX_MOBILE_CLIPBOARD_REQUEST_BYTES: usize = 524_288;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MobileClipboardRequest {
    directory: String,
    preferences_directory: Option<String>,
    action: MobileClipboardAction,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum MobileClipboardAction {
    Load,
    Categories,
    AddCategory {
        name: String,
    },
    RenameCategory {
        id: String,
        name: String,
    },
    RemoveCategory {
        id: String,
    },
    SetCategory {
        text: String,
        id: Option<String>,
    },
    ExportText,
    Capture {
        text: String,
    },
    SetPinned {
        text: String,
        pinned: bool,
    },
    Remove {
        text: String,
    },
    RemoveMany {
        texts: Vec<String>,
    },
    RestoreMany {
        entries: Vec<msime_client_core::clipboard::ClipboardHistoryEntry>,
    },
    Replace {
        text: String,
        replacement: String,
    },
    Clear,
}

/// 移动端结构化剪贴板历史：`directory/MSIME` 存历史；可选的 `preferences_directory` 单独指定容量偏好所在目录，省略时沿用 `directory`。移动端访问由宿主权限控制，不读取或改变桌面自动记录开关。
/// # Safety
/// `request` points to `length` readable JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_mobile_clipboard_history(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length > MAX_MOBILE_CLIPBOARD_REQUEST_BYTES {
            return Err("invalid mobile clipboard request buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: MobileClipboardRequest = serde_json::from_slice(bytes)
            .map_err(|_| "invalid mobile clipboard request document")?;
        let root = std::path::Path::new(&request.directory);
        if !root.is_absolute() || request.directory.len() > 16384 {
            return Err("invalid mobile clipboard directory".into());
        }
        let preferences_directory = request
            .preferences_directory
            .as_deref()
            .unwrap_or(&request.directory);
        let preferences_root = std::path::Path::new(preferences_directory);
        if !preferences_root.is_absolute() || preferences_directory.len() > 16384 {
            return Err("invalid mobile clipboard preferences directory".into());
        }
        let path = root.join("MSIME").join("clipboard_history.json");
        let mut history = msime_client_core::clipboard::ClipboardHistoryStore::open_with_limit(
            path,
            clipboard_history_limit(preferences_root),
        );
        match request.action {
            MobileClipboardAction::Categories => {
                let categories = history
                    .categories()
                    .map_err(|_| "mobile clipboard categories unavailable")?;
                Ok(
                    json!({"categories": categories, "maxCategories": msime_client_core::clipboard::MAX_CATEGORIES,
                    "maxNameCharacters": msime_client_core::clipboard::MAX_CATEGORY_NAME_CHARACTERS}),
                )
            }
            MobileClipboardAction::AddCategory { name } => {
                history.add_category(&name).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::InvalidInput {
                        "invalid clipboard category name or category limit reached"
                    } else {
                        "mobile clipboard categories unavailable"
                    }
                })?;
                Ok(json!({"updated": true}))
            }
            MobileClipboardAction::RenameCategory { id, name } => {
                let updated = history
                    .rename_category(&id, &name)
                    .map_err(|_| "mobile clipboard category rename failed")?;
                Ok(json!({"updated": updated}))
            }
            MobileClipboardAction::RemoveCategory { id } => {
                let updated = history
                    .remove_category(&id)
                    .map_err(|_| "mobile clipboard category removal failed")?;
                Ok(json!({"updated": updated}))
            }
            MobileClipboardAction::SetCategory { text, id } => {
                if text.is_empty()
                    || text.len() > msime_client_core::clipboard::MAX_MOBILE_TEXT_BYTES
                {
                    return Err("invalid mobile clipboard entry".into());
                }
                let updated = history
                    .set_category(&text, id.as_deref())
                    .map_err(|_| "mobile clipboard category update failed")?;
                Ok(json!({"updated": updated}))
            }
            MobileClipboardAction::ExportText => {
                history
                    .load()
                    .map_err(|_| "mobile clipboard history unavailable")?;
                Ok(
                    json!({"text": history.export_text(), "count": history.entries().len(), "limit": history.limit()}),
                )
            }
            MobileClipboardAction::Load => {
                history
                    .load()
                    .map_err(|_| "mobile clipboard history unavailable")?;
                Ok(
                    json!({"entries": history.entries(), "limit": history.limit(),
                    "categories": history.categories().map_err(|_| "mobile clipboard categories unavailable")?}),
                )
            }
            MobileClipboardAction::Capture { text } => {
                if !msime_client_core::clipboard::mobile_text_is_valid(&text) {
                    return Ok(json!({"captured": false, "reason": "invalid"}));
                }
                let captured = history
                    .push_mobile(text)
                    .map_err(|_| "mobile clipboard capture failed")?;
                Ok(json!({
                    "captured": captured,
                    "reason": (!captured).then_some("full"),
                    "limit": history.limit(),
                    "entries": history.entries()
                }))
            }
            MobileClipboardAction::SetPinned { text, pinned } => {
                if text.is_empty()
                    || text.len() > msime_client_core::clipboard::MAX_MOBILE_TEXT_BYTES
                {
                    return Err("invalid mobile clipboard entry".into());
                }
                let updated = history
                    .set_pinned(&text, pinned)
                    .map_err(|_| "mobile clipboard pin update failed")?;
                Ok(json!({
                    "updated": updated,
                    "limit": history.limit(),
                    "entries": history.entries()
                }))
            }
            MobileClipboardAction::Remove { text } => {
                if text.is_empty()
                    || text.len() > msime_client_core::clipboard::MAX_MOBILE_TEXT_BYTES
                {
                    return Err("invalid mobile clipboard entry".into());
                }
                let removed = history
                    .remove(&text)
                    .map_err(|_| "mobile clipboard removal failed")?;
                Ok(json!({
                    "removed": removed,
                    "limit": history.limit(),
                    "entries": history.entries()
                }))
            }
            MobileClipboardAction::RemoveMany { texts } => {
                if texts.is_empty()
                    || texts.len() > msime_client_core::clipboard::MAX_ENTRIES
                    || texts.iter().any(|text| {
                        text.is_empty()
                            || text.len() > msime_client_core::clipboard::MAX_MOBILE_TEXT_BYTES
                    })
                {
                    return Err("invalid mobile clipboard entries".into());
                }
                let removed = history
                    .remove_many(&texts)
                    .map_err(|_| "mobile clipboard removal failed")?;
                Ok(
                    json!({"removed": removed, "limit": history.limit(), "entries": history.entries()}),
                )
            }
            MobileClipboardAction::RestoreMany { entries } => {
                if entries.len() > msime_client_core::clipboard::MAX_ENTRIES {
                    return Err("invalid mobile clipboard entries".into());
                }
                history
                    .restore_many(&entries)
                    .map_err(|_| "mobile clipboard restore failed")?;
                Ok(
                    json!({"restored": true, "limit": history.limit(), "entries": history.entries()}),
                )
            }
            MobileClipboardAction::Replace { text, replacement } => {
                if text.is_empty()
                    || text.len() > msime_client_core::clipboard::MAX_MOBILE_TEXT_BYTES
                {
                    return Err("invalid mobile clipboard entry".into());
                }
                use msime_client_core::clipboard::ReplaceOutcome;
                let outcome = history
                    .replace(&text, replacement)
                    .map_err(|_| "mobile clipboard replace failed")?;
                // 改成已存在的文字时两条合并成一条，宿主据 merged 告诉用户；旧条目不在了或新文字不合规时 replaced 为 false，reason 说明是哪一种。
                let reason = match outcome {
                    ReplaceOutcome::NotFound => Some("not_found"),
                    ReplaceOutcome::Invalid => Some("invalid"),
                    ReplaceOutcome::Replaced
                    | ReplaceOutcome::Merged
                    | ReplaceOutcome::Unchanged => None,
                };
                Ok(json!({
                    "replaced": reason.is_none(),
                    "merged": outcome == ReplaceOutcome::Merged,
                    "reason": reason,
                    "limit": history.limit(),
                    "entries": history.entries()
                }))
            }
            MobileClipboardAction::Clear => {
                history
                    .clear()
                    .map_err(|_| "mobile clipboard clear failed")?;
                Ok(json!({"cleared": true, "limit": history.limit(), "entries": []}))
            }
        }
    })
}

/// Save host-sampled text only while shared history is enabled.
/// # Safety
/// `request` points to `length` readable JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_capture_clipboard_history(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length > 131072 {
            return Err("invalid history capture buffer".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Capture {
            directory: String,
            text: String,
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let capture: Capture =
            serde_json::from_slice(bytes).map_err(|_| "invalid history capture document")?;
        if !std::path::Path::new(&capture.directory).is_absolute()
            || capture.directory.len() > 16384
            || capture.text.len() > msime_client_core::clipboard::MAX_TEXT_BYTES
        {
            return Err("invalid history capture parameters".into());
        }
        let captured = PreferencesStore::new(&capture.directory)
            .capture_clipboard_text(capture.text)
            .map_err(|_| "clipboard history capture failed")?;
        Ok(json!({"captured": captured}))
    })
}

/// Remove one saved history entry by exact content, without touching the clipboard.
/// # Safety
/// `request` points to `length` readable JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_remove_clipboard_history(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if request.is_null() || length > 131072 {
            return Err("invalid history removal buffer".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Removal {
            directory: String,
            text: String,
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let removal: Removal =
            serde_json::from_slice(bytes).map_err(|_| "invalid history removal document")?;
        let path = std::path::Path::new(&removal.directory);
        if !path.is_absolute() || removal.directory.len() > 16384 {
            return Err("invalid history directory".into());
        }
        if removal.text.is_empty()
            || removal.text.len() > msime_client_core::clipboard::MAX_TEXT_BYTES
        {
            return Err("invalid history entry".into());
        }
        if !PreferencesStore::new(path)
            .load()
            .map_err(|_| "history preferences unavailable")?
            .preferences
            .clipboard_history
        {
            return Err("clipboard history disabled".into());
        }
        let mut history = msime_client_core::clipboard::ClipboardHistoryStore::open_with_limit(
            path.join("clipboard_history.json"),
            clipboard_history_limit(path),
        );
        let removed = history
            .remove(&removal.text)
            .map_err(|_| "clipboard history removal failed")?;
        Ok(json!({"removed": removed}))
    })
}

const MAX_DESKTOP_CLIPBOARD_REQUEST_BYTES: usize = 131_072;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopClipboardRequest {
    directory: String,
    action: DesktopClipboardAction,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum DesktopClipboardAction {
    List,
    Sync,
    Clear,
    SetPinned { text: String, pinned: bool },
    Remove { text: String },
}

/// The desktop clipboard history as the 剪贴板 settings page manages it, with the timestamps and pins `msime_client_load_clipboard_history` leaves out. Every operation answers with the history as it now stands, empty while the preference is off; see the header for the operations and codes.
/// # Safety
/// `request` points to `length` readable JSON bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_desktop_clipboard_history(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    use crate::clipboard_history::{self, ClipboardHistoryError};
    use msime_client_core::clipboard::{ClipboardHistoryStore, MAX_TEXT_BYTES};
    response(|| {
        const INVALID: &str = "clipboard_history_invalid";
        const UNAVAILABLE: &str = "clipboard_history_unavailable";
        if request.is_null() || length > MAX_DESKTOP_CLIPBOARD_REQUEST_BYTES {
            return Err(INVALID.into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(request, length) };
        let request: DesktopClipboardRequest =
            serde_json::from_slice(bytes).map_err(|_| INVALID)?;
        let directory = std::path::Path::new(&request.directory);
        if !directory.is_absolute() || request.directory.len() > 16384 {
            return Err(INVALID.into());
        }
        let failed = |error: ClipboardHistoryError| match error {
            ClipboardHistoryError::Disabled => "clipboard_history_disabled".to_owned(),
            ClipboardHistoryError::Unavailable => UNAVAILABLE.to_owned(),
        };
        let entry = |text: &str| {
            if text.is_empty() || text.len() > MAX_TEXT_BYTES {
                Err(INVALID.to_owned())
            } else {
                Ok(())
            }
        };
        let preferences = PreferencesStore::new(directory);
        let limit = clipboard_history_limit(directory);
        let mut history =
            ClipboardHistoryStore::open_with_limit(directory.join("clipboard_history.json"), limit);
        match &request.action {
            DesktopClipboardAction::List => {}
            DesktopClipboardAction::Sync => {
                #[cfg(windows)]
                let read = clipboard_history::windows_clipboard_text;
                // Only Windows reads its clipboard here; the other desktops sync in their own shells.
                #[cfg(not(windows))]
                let read = || Err::<Option<String>, ()>(());
                clipboard_history::capture_clipboard(&preferences, read).map_err(failed)?;
            }
            DesktopClipboardAction::Clear => history.clear().map_err(|_| UNAVAILABLE)?,
            DesktopClipboardAction::SetPinned { text, pinned } => {
                entry(text)?;
                history.set_pinned(text, *pinned).map_err(|_| UNAVAILABLE)?;
            }
            DesktopClipboardAction::Remove { text } => {
                entry(text)?;
                history.remove(text).map_err(|_| UNAVAILABLE)?;
            }
        }
        let enabled = clipboard_history::enabled(&preferences).map_err(failed)?;
        let entries = if enabled {
            clipboard_history::entries(&mut history).map_err(failed)?
        } else {
            Vec::new()
        };
        Ok(json!({ "enabled": enabled, "entries": entries }))
    })
}

/// Try to read preferences without waiting for the writer lock.
/// # Safety
/// `directory` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_try_load_preferences(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = super::parse_absolute_path(
            bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let snapshot = PreferencesStore::new(directory)
            .try_load()
            .map_err(|e| e.to_string())?;
        serde_json::to_value(snapshot).map_err(|e| e.to_string())
    })
}

/// Repair a preferences document that is not well-formed JSON, backing it up first.
/// See `PreferencesStore::recover_malformed`; a valid or missing document is left untouched.
/// # Safety
/// `directory` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_recover_preferences(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    // SAFETY: forwarded under the same caller contract.
    unsafe { recover_preferences_in(directory, length, RecoveryScope::Malformed) }
}

/// The settings page's explicit 修复配置文件: repair a preferences document `load` rejects for any reason, a newer build's included, backing it up first.
/// See `PreferencesStore::recover`; a valid or missing document is left untouched. Only call it on the user's request: a document an older build cannot read is usually a newer build's, and the repair keeps only what this schema accepts.
/// # Safety
/// `directory` must point to `length` readable bytes. Null is rejected.
#[no_mangle]
pub unsafe extern "C" fn msime_client_repair_preferences(
    directory: *const u8,
    length: usize,
) -> *mut c_char {
    // SAFETY: forwarded under the same caller contract.
    unsafe { recover_preferences_in(directory, length, RecoveryScope::Unreadable) }
}

#[derive(Clone, Copy)]
enum RecoveryScope {
    /// Only a document that is not well-formed JSON; what input method hosts may do on their own.
    Malformed,
    /// Anything `load` rejects; only on the user's request.
    Unreadable,
}

/// # Safety
/// `directory` must point to `length` readable bytes. Null is rejected.
unsafe fn recover_preferences_in(
    directory: *const u8,
    length: usize,
    scope: RecoveryScope,
) -> *mut c_char {
    response(|| {
        if directory.is_null() || length > 16384 {
            return Err("invalid preferences directory buffer".into());
        }
        // SAFETY: guaranteed by the documented caller contract.
        let bytes = unsafe { std::slice::from_raw_parts(directory, length) };
        let directory = super::parse_absolute_path(
            bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let store = PreferencesStore::new(directory);
        let outcome = match scope {
            RecoveryScope::Malformed => store.recover_malformed(),
            RecoveryScope::Unreadable => store.recover(),
        }
        .map_err(|e| e.to_string())?;
        Ok(match outcome {
            msime_client_core::preferences::RecoveryOutcome::NotNeeded(snapshot) => json!({
                "recovered": false,
                "snapshot": snapshot,
            }),
            msime_client_core::preferences::RecoveryOutcome::Recovered {
                snapshot,
                backup_path,
                salvaged,
            } => json!({
                "recovered": true,
                "snapshot": snapshot,
                "backup_path": backup_path.to_string_lossy(),
                "backup_name": backup_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "salvaged": salvaged,
            }),
        })
    })
}

/// Compare-and-swap save for a validated PreferencesSnapshot.
/// A successful save with clipboard history disabled clears the default history
/// under the shared preference/history locks, matching the desktop settings path.
///
/// # Safety
/// The caller must provide non-null pointers to readable UTF-8 buffers whose
/// lengths match the supplied lengths and remain valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn msime_client_save_preferences(
    directory: *const u8,
    directory_length: usize,
    expected_revision: u64,
    snapshot: *const u8,
    snapshot_length: usize,
) -> *mut c_char {
    response(|| {
        if directory.is_null()
            || snapshot.is_null()
            || directory_length > 16384
            || snapshot_length > PREFERENCES_DOCUMENT_LIMIT
        {
            return Err("invalid preferences save buffer".into());
        }
        let directory_bytes = unsafe { std::slice::from_raw_parts(directory, directory_length) };
        let directory = super::parse_absolute_path(
            directory_bytes,
            "invalid preferences directory encoding",
            "preferences directory must be absolute",
        )?;
        let snapshot_bytes = unsafe { std::slice::from_raw_parts(snapshot, snapshot_length) };
        let snapshot: PreferencesSnapshot =
            serde_json::from_slice(snapshot_bytes).map_err(|_| "invalid preferences snapshot")?;
        if snapshot.format_version != 1 {
            return Err("unsupported preferences format".into());
        }
        let store = PreferencesStore::new(directory);
        let saved = store
            .save(expected_revision, snapshot.preferences)
            .map_err(|e| e.to_string())?;
        if !saved.preferences.clipboard_history {
            store
                .clear_disabled_clipboard_history()
                .map_err(|e| e.to_string())?;
        }
        serde_json::to_value(saved).map_err(|e| e.to_string())
    })
}
