//! 产品版本（edition）。
//!
//! 版本表 `shared/contracts/editions.json` 是各版本的单一事实源：每个版本提供哪些输入方案、默认方案是什么、在 `Preferences::default()` 之上叠加哪些默认值、随包带哪些资源和功能。full 是现有产品本身，所有值都等于今天写死在代码里的那个；其他版本是它的收窄。多个版本可以同时安装，彼此完全隔离，因此这里只描述一个版本自己的样子，不涉及版本之间的共享。
//!
//! 版本表在编译期嵌入，结构由本模块的类型解析，跨字段和跨文件的约束（资源组件与锁文件一致、功能依赖的组件等）由 `scripts/test-editions.py` 检查。Rust 进程在运行时仅解析 macOS 段的身份标识。

use crate::preferences::{AccountPreferenceValue, InputScheme, TouchKeyboardScheme};
use crate::resources::{ResourceError, ResourceSet};
use serde::Deserialize;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::OsStr;
use std::io::Read;
#[cfg(not(unix))]
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

const EDITIONS_JSON: &str = include_str!("../../../shared/contracts/editions.json");

/// 本模块认识的版本表格式版本。
const SCHEMA_VERSION: u32 = 1;

/// 一个产品版本。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Edition {
    /// 版本 id，例如 `full`（未打包单测基线）与 `shurufa`（独立 macOS 产品）。
    pub id: String,
    /// 系统输入法列表和设置应用里显示的产品名。所有版本共用同一个图标。
    pub display_name: DisplayName,
    /// 本版本提供的输入方案，顺序与 `host_surface::compiled_input_schemes()` 一致。不在列表里的方案在本版本中不存在。
    pub input_schemes: Vec<InputScheme>,
    /// 默认方案，也是偏好里的方案不在 `input_schemes` 里时的回退值。
    pub default_scheme: InputScheme,
    /// 叠加在 `Preferences::default()` 之上的版本默认值，用户之后仍可修改。
    pub preference_defaults: PreferenceDefaults,
    /// 随包带的资源。
    pub resources: EditionResources,
    /// 本版本是否提供这些可选功能。
    pub features: EditionFeatures,
    /// 本版本需要的语言词库，取值是 `resources/language-dictionaries.lock.json` 里的条目名。
    pub language_dictionaries: Vec<String>,
    /// 各平台的身份标识中 Rust 进程用得到的那几段。
    #[serde(default)]
    platforms: EditionPlatforms,
}

/// 版本表 `platforms` 里本模块解析的段。只保留 macOS 身份标识，其余已收敛移除的平台键在反序列化时忽略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct EditionPlatforms {
    #[serde(default)]
    macos: Option<MacosIdentity>,
}

/// 一个版本在 macOS 上的身份标识（版本表 `platforms.macos`）。所有版本两两不同，所以多个版本可以同时安装，互不覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosIdentity {
    /// InputMethodKit bundle 的 CFBundleIdentifier，也是它的 NSUserDefaults 域。
    pub input_method_bundle_id: String,
    /// 输入法 bundle 的文件名（不含 `.app`）和可执行文件名。
    pub input_method_name: String,
    /// 设置应用的 bundle identifier，也是 Application Support 下状态目录的名字。
    pub settings_bundle_id: String,
    /// 与设置应用标识分开的本机数据目录名；未声明时沿用设置应用标识。
    #[serde(default)]
    pub state_directory_name: Option<String>,
    /// 本版本随包的本地语音辅助进程名。
    #[serde(default)]
    pub voice_helper_name: Option<String>,
    /// 原生账号窗口存访问令牌的钥匙串服务名，刷新令牌在它加 `.refresh` 的服务名下。
    pub keychain_service: String,
    /// Homebrew cask 名。
    pub cask: String,
    /// 发布 DMG 的文件名前缀。
    pub dmg_prefix: String,
}

impl MacosIdentity {
    /// 输入法 bundle 的文件名，例如 `水杉输入法.app`。
    pub fn input_method_bundle_name(&self) -> String {
        format!("{}.app", self.input_method_name)
    }
}

/// 版本的产品名。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayName {
    #[serde(rename = "zh-Hans")]
    pub zh_hans: String,
    pub en: String,
}

/// 版本默认值。`None` 表示沿用 `Preferences::default()`。
///
/// 拒绝未知字段：拼错的键如果被静默忽略，这个版本就会带着错误的默认值发出去。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceDefaults {
    /// 五笔混拼的默认值。
    #[serde(default)]
    pub wubi_mixed_pinyin: Option<bool>,
}

/// 版本随包带的资源。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditionResources {
    /// 版本表 `resource_components` 里的组件名。
    pub components: Vec<String>,
}

/// 版本提供的可选功能。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditionFeatures {
    /// 临时日文。
    pub temporary_japanese: bool,
    /// 神经网络整句重排。
    pub neural_keyboard: bool,
    /// 非英文目标语言的离线候选释义（`offline-glosses/zh-<语言>.db`）。这些数据库按中文候选词查释义，所以只有提供中文方案的版本带它们（`scripts/test-editions.py` 检查两者相等）；为 false 的版本各平台都不打包。
    pub offline_glosses: bool,
    /// 手写输入。桌面的 Zinnia 模型（`handwriting-zh_CN.model`）和 Android 的 ML Kit 模型（`zh-Hani-CN`）都只认汉字，所以同样只有提供中文方案的版本有；为 false 的版本不打包、不下载手写模型，也不提供手写面板、手写入口和手写设置页。
    pub handwriting: bool,
    /// Tab 笔画筛选：全拼、双拼组字时按 Tab，再按笔顺筛候选的第一个字。笔顺来自 `msime-stroke.db`，所以为 true 的版本在 `language_dictionaries` 里列出它（`scripts/test-editions.py` 检查）。只有 macOS 宿主兑现，host-api 在别的宿主上不交给 Engine。
    pub tab_stroke_filter: bool,
}

#[derive(Deserialize)]
struct EditionTable {
    schema_version: u32,
    editions: Vec<Edition>,
}

fn editions() -> &'static [Edition] {
    static EDITIONS: OnceLock<Vec<Edition>> = OnceLock::new();
    EDITIONS.get_or_init(|| {
        let table: EditionTable =
            serde_json::from_str(EDITIONS_JSON).expect("shared/contracts/editions.json is valid");
        assert_eq!(
            table.schema_version, SCHEMA_VERSION,
            "shared/contracts/editions.json schema_version"
        );
        table.editions
    })
}

impl Edition {
    /// full 版本的 id。没有任何版本信息的文档和调用方都按 full 处理。
    pub const FULL_ID: &'static str = "full";

    /// 全部版本，第一个是 full。
    pub fn all() -> &'static [Edition] {
        editions()
    }

    /// 按 id 查找版本；版本表里没有这个 id 时返回 `None`。
    pub fn by_id(id: &str) -> Option<&'static Edition> {
        editions().iter().find(|edition| edition.id == id)
    }

    /// full 版本，即现有产品本身。
    pub fn full() -> &'static Edition {
        Self::by_id(Self::FULL_ID).expect("shared/contracts/editions.json defines the full edition")
    }

    /// HostOptions 文档里记录版本 id 的键。full 的文档不写这个键，所以 full 的文档与引入版本之前逐字节相同，旧版读取方（`HostOptions` 拒绝未知键）照样能读。
    pub const HOST_OPTIONS_KEY: &'static str = "edition";

    /// 是否就是 full 版本。
    pub fn is_full(&self) -> bool {
        self.id == Self::FULL_ID
    }

    /// 本版本资源锁的原文。full 的锁就是 `resources/desktop-dictionary.lock.json` 本身，所以 full 的资源目录、校验和用户词库代次（`ResourceSet::generation`）与引入版本之前完全相同；其他版本的锁由 `scripts/editions.py gen-locks` 按版本表的 `resources.components` 从那份文件机械生成并提交，`scripts/test-editions.py` 检查它们没有漂移。版本表里没有对应锁文件的 id 返回 `None`。
    ///
    /// 逐个列出而不是在构建期生成：`include_str!` 只接受字面路径，加一个版本就在这里加一行。
    pub fn resource_lock(&self) -> Option<&'static str> {
        match self.id.as_str() {
            Self::FULL_ID => Some(include_str!(
                "../../../resources/desktop-dictionary.lock.json"
            )),
            "shurufa" => Some(include_str!(
                "../../../resources/editions/shurufa.lock.json"
            )),
            _ => None,
        }
    }

    /// 本版本的资源锁：资源目录必须恰好是这些文件，用户词库代次也按它计算。没有锁文件或锁文件不合法时是 [`ResourceError::InvalidManifest`]。
    pub fn resource_set(&self) -> Result<ResourceSet, ResourceError> {
        let lock = self.resource_lock().ok_or(ResourceError::InvalidManifest)?;
        let set: ResourceSet =
            serde_json::from_str(lock).map_err(|_| ResourceError::InvalidManifest)?;
        set.validate()?;
        Ok(set)
    }

    /// 本版本是否提供这个方案。
    pub fn offers(&self, scheme: InputScheme) -> bool {
        self.input_schemes.contains(&scheme)
    }

    /// 本版本的触屏键盘是否提供这个方案入口：入口背后的输入方案在本版本里时提供。手写不属于任何一个输入方案（识别由平台的手写识别器完成，不经过 Engine 的方案），由版本表的 `features.handwriting` 决定：手写面板写出的是汉字（Android 用的是 ML Kit 的 `zh-Hani-CN` 模型），所以只有提供中文方案的版本有手写，full、拼音版和五笔版都有，日文、越南文和藏文版没有。
    pub fn offers_touch_scheme(&self, scheme: TouchKeyboardScheme) -> bool {
        let input = match scheme {
            TouchKeyboardScheme::Handwriting => return self.features.handwriting,
            TouchKeyboardScheme::Quanpin
            | TouchKeyboardScheme::NineKey
            | TouchKeyboardScheme::FourteenKey
            | TouchKeyboardScheme::EighteenKey
            | TouchKeyboardScheme::NineKeyShuangpin => InputScheme::Quanpin,
            TouchKeyboardScheme::Xiaohe
            | TouchKeyboardScheme::Ziranma
            | TouchKeyboardScheme::Microsoft
            | TouchKeyboardScheme::Shoudao => InputScheme::Shuangpin,
            TouchKeyboardScheme::Wubi => InputScheme::Wubi,
            TouchKeyboardScheme::JapaneseNineKey | TouchKeyboardScheme::Japanese => {
                InputScheme::Japanese
            }
            TouchKeyboardScheme::Korean => InputScheme::Korean,
            TouchKeyboardScheme::Cantonese | TouchKeyboardScheme::CantoneseShuangpin => {
                InputScheme::Cantonese
            }
            TouchKeyboardScheme::Zhuyin | TouchKeyboardScheme::ZhuyinNineKey => InputScheme::Zhuyin,
            TouchKeyboardScheme::Vietnamese => InputScheme::Vietnamese,
            TouchKeyboardScheme::Tibetan => InputScheme::Tibetan,
            TouchKeyboardScheme::Stroke => InputScheme::Stroke,
        };
        self.offers(input)
    }

    /// 本版本在 AI 助手配置文件 `mcpServers` 下登记 `msime-mcp` 用的键：full 仍是 `msime`，其他版本是 `msime-<id>`。多个版本同时安装时，每个版本各登记一条，互不覆盖。
    pub fn mcp_server_name(&self) -> String {
        if self.is_full() {
            "msime".to_owned()
        } else {
            format!("msime-{}", self.id)
        }
    }

    /// HostOptions 文档记录的版本：没有 `edition` 键（或为 null）是 full；键的值不是版本表里的 id 时返回 `None`，调用方应拒绝这份文档，而不是猜成 full。
    pub fn of_host_options(document: &serde_json::Value) -> Option<&'static Edition> {
        match document.get(Self::HOST_OPTIONS_KEY) {
            None | Some(serde_json::Value::Null) => Some(Self::full()),
            Some(serde_json::Value::String(id)) => Self::by_id(id),
            Some(_) => None,
        }
    }

    /// 本版本在 macOS 上的身份标识；版本表里这个版本还没有 macOS 段时为 `None`。
    pub fn macos(&self) -> Option<&MacosIdentity> {
        self.platforms.macos.as_ref()
    }

    /// 安装包里声明版本的文件名。内容是 `{"edition": "<id>"}`，未打包单测进程缺省视作 full。
    pub const PACKAGE_MARKER_FILE: &'static str = "edition.json";

    /// 读取安装包里的版本声明（见 [`Edition::PACKAGE_MARKER_FILE`]）。文件不存在时是 full；文件存在但读不了、不是合法的声明、或声明了版本表里没有的 id 时是错误：一个声明了版本的包不能被当成 full 运行，否则它会去读写 full 的状态目录和输入法。
    pub fn declared_by_package(marker: &Path) -> std::io::Result<&'static Edition> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Marker {
            edition: String,
        }
        let file = match crate::storage::open_private_file(marker) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::full()),
            Err(error) => return Err(error),
        };
        let mut text = String::new();
        file.take(STATE_RECORD_LIMIT).read_to_string(&mut text)?;
        let marker: Marker = serde_json::from_str(&text)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        Self::by_id(&marker.edition)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "unknown edition"))
    }

    /// 本进程所在 macOS 安装包声明的版本。设置应用和 `msime-mcp` 的可执行文件都在 `<App>.app/Contents/MacOS/` 下，声明在 `Contents/Resources/edition.json`（[`Edition::PACKAGE_MARKER_FILE`]）。没有这个文件就是 full：开发运行和测试进程都是这样。只在第一次调用时读，之后返回同一个结果。
    ///
    /// 声明坏了（读不了、不是合法的声明、或者是本构建不认识的版本）时是错误而不是 full：一个声明了版本的包不能去停、去改 full 的状态目录。
    pub fn of_macos_bundle() -> Result<&'static Edition, &'static str> {
        static EDITION: OnceLock<Result<&'static Edition, &'static str>> = OnceLock::new();
        *EDITION.get_or_init(|| {
            let executable =
                std::env::current_exe().map_err(|_| "cannot locate the running executable")?;
            // macOS 的 `current_exe` 返回的是启动时用的路径，不解析符号链接；从 PATH 里的链接（如 `~/.local/bin/msime`）启动时要先找到链接指向的安装包。
            let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
            let Some(contents) = executable.parent().and_then(Path::parent) else {
                return Ok(Self::full());
            };
            Self::declared_by_package(&contents.join("Resources").join(Self::PACKAGE_MARKER_FILE))
                .map_err(|_| "the package declares an edition this build does not know")
        })
    }

    /// 状态目录里记录它属于哪个版本的文件名，内容就是版本 id。
    ///
    /// 版本之间完全隔离，一个状态目录只属于一个版本，而各平台读写偏好的 C ABI 和设置应用的偏好存储只拿到这个目录：偏好文件不见了或要修复时，靠这份记录才知道该用哪个版本的默认偏好（`PreferencesStore::new`）。full 不写这个文件，所以 full 的状态目录与引入版本之前相同。
    pub const STATE_RECORD_FILE: &'static str = "edition";

    /// `state_root` 里记录的版本（见 [`Edition::STATE_RECORD_FILE`]）。没有记录、读不到、或记录的 id 不在版本表里时返回 `None`，调用方按 full 处理：这份记录只决定缺省值，认不出时用 full 的缺省值不会让任何方案超出 HostOptions 文档记录的版本。
    pub fn recorded_in(state_root: &Path) -> Option<&'static Edition> {
        let path = state_root.join(Self::STATE_RECORD_FILE);
        crate::storage::reject_symlink(&path).ok()?;
        let mut text = String::new();
        crate::storage::open_private_file_in(&path)
            .ok()?
            .take(STATE_RECORD_LIMIT)
            .read_to_string(&mut text)
            .ok()?;
        Self::by_id(text.trim())
    }

    /// 把 `state_root` 记成属于本版本：不是 full 时写下 [`Edition::STATE_RECORD_FILE`]，是 full 时删掉可能留着的记录。准备宿主（`prepare_host_configuration_for_edition`）时调用，`state_root` 必须已经存在。记录已经是本版本时不重写。
    pub fn record_in(&self, state_root: &Path) -> std::io::Result<()> {
        let path = state_root.join(Self::STATE_RECORD_FILE);
        crate::storage::reject_symlink(&path)?;
        if self.is_full() {
            return match crate::storage::remove_private_file(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                result => result,
            };
        }
        if Self::recorded_in(state_root).is_some_and(|recorded| recorded.id == self.id) {
            return Ok(());
        }
        let bytes = format!("{}\n", self.id);
        #[cfg(unix)]
        {
            let directory = crate::storage::open_private_directory(state_root)?;
            crate::storage::write_private_file_at(
                &directory,
                OsStr::new(Self::STATE_RECORD_FILE),
                bytes.as_bytes(),
            )?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let mut temporary = tempfile::NamedTempFile::new_in(state_root)?;
            temporary.write_all(bytes.as_bytes())?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|error| error.error)?;
            Ok(())
        }
    }
}

/// 状态目录里的版本记录最多读这么多字节；版本 id 远比它短。
const STATE_RECORD_LIMIT: u64 = 256;

/// 账号偏好里记录输入方案的键。
const ACCOUNT_SCHEME_KEY: &str = "input.schema";
/// iOS 的九键开关：它说的是 `input.schema` 那个方案用不用九键，只随方案一起同步。
const ACCOUNT_NINE_KEY_KEY: &str = "platform.ios.nine_key";
/// iOS 的触屏布局（26 键、九键、14 键、手写），取代九键开关说明 `input.schema` 那个方案用哪种键盘；旧版本只认九键开关，所以两个键一起上传。和 Android 的触屏布局一样只随方案一起同步。
const ACCOUNT_IOS_LAYOUT_KEY: &str = "platform.ios.keyboard_layout";
/// Android 的触屏布局（26 键、九键、手写）。它和 `input.schema` 一起才决定键盘上的那个入口（全拼加九键是全拼 9 键，全拼加手写是手写），所以同样只随方案一起同步：五笔版不同步方案，也就不同步布局，否则 full 那边选的全拼 9 键会把五笔版的键盘换成九键布局。
const ACCOUNT_ANDROID_LAYOUT_KEY: &str = "platform.android.keyboard_layout";
/// 鸿蒙的触屏布局，取值与 Android 的相同，理由同上：鸿蒙和 Android 是两台各有键盘的设备，各用自己的命名空间。
const ACCOUNT_HARMONY_LAYOUT_KEY: &str = "platform.harmony.keyboard_layout";
/// 双拼方案的键。不提供双拼的版本里它只是本机的缺省值，不该盖掉账号里别的设备选的方案。
const ACCOUNT_SHUANGPIN_KEY: &str = "input.shuangpin_schema";
/// 五笔版本（86/98）的键，理由同上。
const ACCOUNT_WUBI_KEY: &str = "input.wubi_schema";

/// 账号设置里 `input.schema` 的值对应的方案；不是字符串或不认识的取值返回 `None`。
fn account_scheme(settings: &BTreeMap<String, AccountPreferenceValue>) -> Option<InputScheme> {
    match settings.get(ACCOUNT_SCHEME_KEY) {
        Some(AccountPreferenceValue::String(value)) => {
            serde_json::from_value(serde_json::Value::String(value.clone())).ok()
        }
        _ => None,
    }
}

/// 本版本是否同步 `input.schema`：只有一个方案的版本没有可选的方案，既不上传也不应用它，否则会把 full 等其他版本记在账号里的方案盖掉，或把账号里的方案带进本版本。`None` 是认不出的版本，同样不同步。
fn syncs_account_scheme(edition: Option<&Edition>) -> bool {
    edition.is_some_and(|edition| edition.input_schemes.len() > 1)
}

/// 本版本不同步 `input.schema`，或它的值是本版本不提供的方案时，去掉它和随它的 iOS 九键开关、iOS、Android 与鸿蒙的触屏布局。
fn drop_unsynced_account_scheme(
    edition: Option<&Edition>,
    settings: &mut BTreeMap<String, AccountPreferenceValue>,
) {
    let keeps_scheme = syncs_account_scheme(edition)
        && account_scheme(settings)
            .is_none_or(|scheme| edition.is_some_and(|edition| edition.offers(scheme)));
    if !keeps_scheme {
        settings.remove(ACCOUNT_SCHEME_KEY);
        settings.remove(ACCOUNT_NINE_KEY_KEY);
        settings.remove(ACCOUNT_IOS_LAYOUT_KEY);
        settings.remove(ACCOUNT_ANDROID_LAYOUT_KEY);
        settings.remove(ACCOUNT_HARMONY_LAYOUT_KEY);
    }
}

/// 上传前按版本过滤本机整理出的账号设置。各平台把本机偏好换成账号设置之后、合并进账号文档之前调用。
///
/// - 只有一个方案的版本（以及认不出的版本，`edition` 为 `None`）不上传 `input.schema` 和随它的 iOS 九键开关、iOS、Android 与鸿蒙的触屏布局；
/// - 有多个方案的版本只上传本版本提供的方案；
/// - 不提供双拼、五笔的版本不上传双拼方案、五笔版本这两项。
///
/// full 提供全部方案，什么也不去掉。
pub fn filter_uploaded_account_settings(
    edition: Option<&Edition>,
    settings: &mut BTreeMap<String, AccountPreferenceValue>,
) {
    drop_unsynced_account_scheme(edition, settings);
    if !edition.is_some_and(|edition| edition.offers(InputScheme::Shuangpin)) {
        settings.remove(ACCOUNT_SHUANGPIN_KEY);
    }
    if !edition.is_some_and(|edition| edition.offers(InputScheme::Wubi)) {
        settings.remove(ACCOUNT_WUBI_KEY);
    }
}

/// 应用前按版本过滤从账号下载的设置，各平台在解析账号文档之前调用。
///
/// - 只有一个方案的版本（以及认不出的版本）忽略 `input.schema` 和随它的 iOS 九键开关、iOS、Android 与鸿蒙的触屏布局；
/// - 有多个方案的版本把本版本不提供的方案当作账号里没有这一项，本机方案保持不变，文档其余部分照常应用。不认识的取值留给平台代码按原来的规则处理。
///
/// full 提供全部方案，什么也不去掉。
pub fn filter_downloaded_account_settings(
    edition: Option<&Edition>,
    settings: &mut BTreeMap<String, AccountPreferenceValue>,
) {
    drop_unsynced_account_scheme(edition, settings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_surface::compiled_input_schemes;
    use crate::preferences::ChineseScheme;
    use crate::preferences::Preferences;
    use std::collections::BTreeSet;

    #[test]
    fn the_table_parses_and_full_comes_first() {
        let ids: Vec<&str> = Edition::all()
            .iter()
            .map(|edition| edition.id.as_str())
            .collect();
        assert_eq!(ids, ["full", "shurufa"]);
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    }

    #[test]
    fn full_matches_the_product_as_it_is_today() {
        let full = Edition::full();
        assert_eq!(full.id, Edition::FULL_ID);
        assert_eq!(full.input_schemes, compiled_input_schemes());
        assert_eq!(full.default_scheme, InputScheme::Quanpin);
        assert_eq!(full.default_scheme, Preferences::default().scheme);
        assert_eq!(full.preference_defaults, PreferenceDefaults::default());
        assert_eq!(
            full.features,
            EditionFeatures {
                temporary_japanese: true,
                neural_keyboard: true,
                offline_glosses: true,
                handwriting: true,
                tab_stroke_filter: true,
            }
        );
        assert_eq!(full.display_name.zh_hans, "水杉输入法");
        assert_eq!(full.display_name.en, "MSIME");
    }

    #[test]
    fn shurufa_offers_quanpin_and_shuangpin_without_extra_modules() {
        let shurufa = Edition::by_id("shurufa").unwrap();
        assert_eq!(
            shurufa.input_schemes,
            [InputScheme::Quanpin, InputScheme::Shuangpin]
        );
        assert_eq!(shurufa.default_scheme, InputScheme::Quanpin);
        assert_eq!(shurufa.preference_defaults, PreferenceDefaults::default());
        assert_eq!(
            shurufa.features,
            EditionFeatures {
                temporary_japanese: false,
                neural_keyboard: false,
                offline_glosses: false,
                handwriting: false,
                tab_stroke_filter: false,
            }
        );
        assert!(shurufa.language_dictionaries.is_empty());
        assert_eq!(shurufa.display_name.zh_hans, "Shurufa");
        assert_eq!(shurufa.display_name.en, "Shurufa");
        assert_eq!(shurufa.mcp_server_name(), "msime-shurufa");
    }

    #[test]
    fn every_edition_defaults_to_a_scheme_it_offers() {
        for edition in Edition::all() {
            assert!(edition.offers(edition.default_scheme), "{}", edition.id);
            assert!(
                edition
                    .input_schemes
                    .iter()
                    .all(|scheme| compiled_input_schemes().contains(scheme)),
                "{}",
                edition.id
            );
        }
    }

    /// 上游中文版本提供离线释义和手写；精简的 Shurufa 明确关闭这两项。
    #[test]
    fn chinese_only_features_follow_the_chinese_schemes() {
        for edition in Edition::all() {
            let writes_chinese = edition
                .input_schemes
                .iter()
                .any(|scheme| ChineseScheme::of(*scheme).is_some());
            let offers_extra = writes_chinese && edition.id != "shurufa";
            assert_eq!(
                edition.features.offline_glosses, offers_extra,
                "{}",
                edition.id
            );
            assert_eq!(edition.features.handwriting, offers_extra, "{}", edition.id);
            assert_eq!(
                edition.offers_touch_scheme(TouchKeyboardScheme::Handwriting),
                offers_extra,
                "{}",
                edition.id
            );
        }
    }

    /// Tab 笔画筛选读 `msime-stroke.db`：提供它的版本都列出这个词库，而且都有全拼或双拼。
    #[test]
    fn tab_stroke_filter_ships_the_stroke_dictionary() {
        let offering: Vec<_> = Edition::all()
            .iter()
            .filter(|edition| edition.features.tab_stroke_filter)
            .map(|edition| edition.id.as_str())
            .collect();
        assert_eq!(offering, ["full"]);
        for id in offering {
            let edition = Edition::by_id(id).unwrap();
            assert!(edition
                .language_dictionaries
                .iter()
                .any(|name| name == "msime-stroke.db"));
            assert!(edition.offers(InputScheme::Quanpin) || edition.offers(InputScheme::Shuangpin));
        }
    }

    #[test]
    fn an_unknown_id_is_not_an_edition() {
        assert!(Edition::by_id("unknown").is_none());
        assert!(Edition::by_id("").is_none());
    }

    #[test]
    fn host_options_without_an_edition_are_full() {
        use serde_json::json;
        let of = |document: serde_json::Value| {
            Edition::of_host_options(&document).map(|e| e.id.as_str())
        };
        assert_eq!(of(json!({ "api_version": 1 })), Some("full"));
        assert_eq!(of(json!({ "edition": null })), Some("full"));
        assert_eq!(of(json!({ "edition": "full" })), Some("full"));
        assert_eq!(of(json!({ "edition": "shurufa" })), Some("shurufa"));
        assert_eq!(of(json!({ "edition": "klingon" })), None);
        assert_eq!(of(json!({ "edition": 3 })), None);
    }

    #[test]
    fn full_keeps_the_mcp_server_name_and_the_others_get_their_own() {
        assert_eq!(Edition::full().mcp_server_name(), "msime");
        assert_eq!(
            Edition::by_id("shurufa").unwrap().mcp_server_name(),
            "msime-shurufa"
        );
    }

    #[test]
    fn touch_scheme_entries_follow_the_input_schemes_and_handwriting_needs_a_chinese_scheme() {
        let full = Edition::full();
        assert!(TouchKeyboardScheme::ALL
            .into_iter()
            .all(|scheme| full.offers_touch_scheme(scheme)));
        // Shurufa 只保留全拼与双拼对应布局，关闭手写识别入口。
        let shurufa = Edition::by_id("shurufa").unwrap();
        let offered: Vec<_> = TouchKeyboardScheme::ALL
            .into_iter()
            .filter(|scheme| shurufa.offers_touch_scheme(*scheme))
            .collect();
        assert_eq!(
            offered,
            [
                TouchKeyboardScheme::Quanpin,
                TouchKeyboardScheme::NineKey,
                TouchKeyboardScheme::Xiaohe,
                TouchKeyboardScheme::Ziranma,
                TouchKeyboardScheme::Microsoft,
                TouchKeyboardScheme::Shoudao,
                TouchKeyboardScheme::FourteenKey,
                TouchKeyboardScheme::NineKeyShuangpin,
                TouchKeyboardScheme::EighteenKey,
            ]
        );
    }

    fn artifact_names(set: &ResourceSet) -> BTreeSet<&str> {
        set.artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect()
    }

    /// full 的资源锁逐字节就是原文件，代次与引入版本之前相同：用户词库目录 `user/dictionaries/<代次>` 不变，升级不会重新准备。期望值是按当前这份锁（msime-dictionary 的 `dict-v2.0.24`）算出的代次；换词库版本时它理应变化，届时连同锁文件一起更新。
    #[test]
    fn full_keeps_the_desktop_lock_and_its_generation() {
        let full = Edition::full();
        assert_eq!(
            full.resource_lock(),
            Some(include_str!(
                "../../../resources/desktop-dictionary.lock.json"
            ))
        );
        let set = full.resource_set().unwrap();
        assert_eq!(set.artifacts.len(), 12);
        assert_eq!(
            set.generation().unwrap(),
            "7356c4a452d8ba15475c0e57f134fccf7b7cbe267440461541c74d09b11b6ffe"
        );
    }

    /// 每个版本都有资源锁，锁里的文件恰好是它的组件的并集，每个条目与原文件的同名条目逐字段相同。
    #[test]
    fn every_edition_lock_is_the_union_of_its_components() {
        let table: serde_json::Value = serde_json::from_str(EDITIONS_JSON).unwrap();
        let components = table["resource_components"].as_object().unwrap();
        let full = Edition::full().resource_set().unwrap();
        for edition in Edition::all() {
            let set = edition.resource_set().unwrap();
            let expected: BTreeSet<&str> = edition
                .resources
                .components
                .iter()
                .flat_map(|component| components[component].as_array().unwrap())
                .map(|name| name.as_str().unwrap())
                .collect();
            assert_eq!(artifact_names(&set), expected, "{}", edition.id);
            assert_eq!(set.source_commit, full.source_commit, "{}", edition.id);
            for artifact in &set.artifacts {
                let original = full
                    .artifacts
                    .iter()
                    .find(|candidate| candidate.name == artifact.name)
                    .unwrap();
                assert_eq!(artifact.sha256, original.sha256, "{}", artifact.name);
                assert_eq!(artifact.size, original.size, "{}", artifact.name);
                assert_eq!(artifact.url, original.url, "{}", artifact.name);
            }
        }
    }

    /// Shurufa 只打包核心资源、拼音主词库和 n-gram 表，不包含五笔、临时日文和旧 safetensors 模型。
    #[test]
    fn shurufa_lock_keeps_only_core_and_pinyin_components() {
        let set = Edition::by_id("shurufa").unwrap().resource_set().unwrap();
        assert_eq!(
            artifact_names(&set),
            BTreeSet::from([
                "msime-bigram.bin",
                "msime-dictionary-manifest.json",
                "msime-english.db",
                "msime-others.db",
                "msime-pinyin.db",
                "msime-scowl_Copyright.txt",
                "msime-trigram.bin",
            ])
        );
        assert_ne!(
            set.generation().unwrap(),
            Edition::full()
                .resource_set()
                .unwrap()
                .generation()
                .unwrap()
        );
    }

    #[test]
    fn a_misspelled_preference_default_is_rejected() {
        let error = serde_json::from_str::<PreferenceDefaults>(r#"{"wubi_mixed_pinyn": true}"#)
            .unwrap_err();
        assert!(error.to_string().contains("wubi_mixed_pinyn"), "{error}");
    }

    fn account_settings(scheme: &str) -> BTreeMap<String, AccountPreferenceValue> {
        BTreeMap::from([
            (
                "input.schema".to_owned(),
                AccountPreferenceValue::String(scheme.to_owned()),
            ),
            (
                "platform.ios.nine_key".to_owned(),
                AccountPreferenceValue::Boolean(true),
            ),
            (
                "platform.ios.keyboard_layout".to_owned(),
                AccountPreferenceValue::String("nine_key".to_owned()),
            ),
            (
                "platform.android.keyboard_layout".to_owned(),
                AccountPreferenceValue::String("nine_key".to_owned()),
            ),
            (
                "platform.harmony.keyboard_layout".to_owned(),
                AccountPreferenceValue::String("nine_key".to_owned()),
            ),
            (
                "input.shuangpin_schema".to_owned(),
                AccountPreferenceValue::String("ziranma".to_owned()),
            ),
            (
                "input.wubi_schema".to_owned(),
                AccountPreferenceValue::String("wubi98".to_owned()),
            ),
            (
                "input.learning".to_owned(),
                AccountPreferenceValue::Boolean(false),
            ),
        ])
    }

    fn keys(settings: &BTreeMap<String, AccountPreferenceValue>) -> Vec<&str> {
        settings.keys().map(String::as_str).collect()
    }

    #[test]
    fn full_syncs_every_account_setting_unchanged() {
        let full = Some(Edition::full());
        for scheme in [
            "quanpin",
            "shuangpin",
            "wubi",
            "japanese",
            "korean",
            "cantonese",
            "klingon",
        ] {
            let mut uploaded = account_settings(scheme);
            filter_uploaded_account_settings(full, &mut uploaded);
            assert_eq!(uploaded, account_settings(scheme), "{scheme}");
            let mut downloaded = account_settings(scheme);
            filter_downloaded_account_settings(full, &mut downloaded);
            assert_eq!(downloaded, account_settings(scheme), "{scheme}");
        }
    }

    #[test]
    fn a_multi_scheme_edition_syncs_only_its_own_schemes() {
        let shurufa = Edition::by_id("shurufa");
        for scheme in ["quanpin", "shuangpin"] {
            let mut uploaded = account_settings(scheme);
            filter_uploaded_account_settings(shurufa, &mut uploaded);
            assert_eq!(
                keys(&uploaded),
                [
                    "input.learning",
                    "input.schema",
                    "input.shuangpin_schema",
                    "platform.android.keyboard_layout",
                    "platform.harmony.keyboard_layout",
                    "platform.ios.keyboard_layout",
                    "platform.ios.nine_key"
                ],
                "{scheme}"
            );
            let mut downloaded = account_settings(scheme);
            filter_downloaded_account_settings(shurufa, &mut downloaded);
            assert_eq!(downloaded, account_settings(scheme), "{scheme}");
        }
        // 账号里是本版本没有的方案：当作没有这一项，本机方案不变，其余设置照常应用。
        for scheme in ["wubi", "japanese", "korean", "cantonese"] {
            let mut downloaded = account_settings(scheme);
            filter_downloaded_account_settings(shurufa, &mut downloaded);
            assert_eq!(
                keys(&downloaded),
                [
                    "input.learning",
                    "input.shuangpin_schema",
                    "input.wubi_schema"
                ],
                "{scheme}"
            );
            let mut uploaded = account_settings(scheme);
            filter_uploaded_account_settings(shurufa, &mut uploaded);
            assert_eq!(
                keys(&uploaded),
                ["input.learning", "input.shuangpin_schema"],
                "{scheme}"
            );
        }
        // 不认识的取值留给平台代码，按原来的规则保留本机方案。
        let mut downloaded = account_settings("klingon");
        filter_downloaded_account_settings(shurufa, &mut downloaded);
        assert!(downloaded.contains_key("input.schema"));
    }

    #[test]
    fn full_keeps_its_macos_identifiers_and_every_edition_has_its_own() {
        let full = Edition::full().macos().unwrap();
        assert_eq!(
            full.input_method_bundle_id,
            "app.msime.inputmethod.MetasequoiaIME"
        );
        assert_eq!(full.input_method_bundle_name(), "水杉输入法.app");
        assert_eq!(full.settings_bundle_id, "app.msime.macos");
        assert_eq!(full.keychain_service, "com.metasequoia.msime.account");
        assert_eq!(full.cask, "msime");
        assert_eq!(full.dmg_prefix, "msime-macos");
        let shurufa = Edition::by_id("shurufa").unwrap().macos().unwrap();
        assert_eq!(
            shurufa.input_method_bundle_id,
            "dev.shurufa.inputmethod.Shurufa"
        );
        assert_eq!(shurufa.input_method_bundle_name(), "Shurufa.app");
        let sections: Vec<_> = Edition::all().iter().filter_map(Edition::macos).collect();
        assert_eq!(sections.len(), 2);
        let bundles: BTreeSet<_> = sections
            .iter()
            .map(|section| section.input_method_bundle_id.to_lowercase())
            .collect();
        assert_eq!(bundles.len(), sections.len());
        let states: BTreeSet<_> = sections
            .iter()
            .map(|section| section.settings_bundle_id.to_lowercase())
            .collect();
        assert_eq!(states.len(), sections.len());
    }

    #[test]
    fn a_test_process_is_not_inside_a_macos_bundle_and_runs_as_full() {
        assert!(Edition::of_macos_bundle().unwrap().is_full());
    }

    #[test]
    fn a_package_without_a_marker_is_full_and_a_bad_marker_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join(Edition::PACKAGE_MARKER_FILE);
        assert!(Edition::declared_by_package(&marker).unwrap().is_full());
        std::fs::write(&marker, br#"{"edition":"shurufa"}"#).unwrap();
        assert_eq!(Edition::declared_by_package(&marker).unwrap().id, "shurufa");
        for bad in [
            &br#"{"edition":"klingon"}"#[..],
            br#"{"edition":3}"#,
            br#"{"edition":"shurufa","extra":1}"#,
            b"shurufa",
        ] {
            std::fs::write(&marker, bad).unwrap();
            assert!(Edition::declared_by_package(&marker).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_package_marker_does_not_follow_a_symlinked_leaf() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join(Edition::PACKAGE_MARKER_FILE);
        std::fs::write(&target, br#"{"edition":"shurufa"}"#).unwrap();
        let marker = directory.path().join(Edition::PACKAGE_MARKER_FILE);
        symlink(&target, &marker).unwrap();

        assert!(Edition::declared_by_package(&marker).is_err());
    }

    #[test]
    fn an_unknown_edition_does_not_touch_the_scheme() {
        let mut uploaded = account_settings("quanpin");
        filter_uploaded_account_settings(None, &mut uploaded);
        assert_eq!(keys(&uploaded), ["input.learning"]);
        let mut downloaded = account_settings("quanpin");
        filter_downloaded_account_settings(None, &mut downloaded);
        assert_eq!(
            keys(&downloaded),
            [
                "input.learning",
                "input.shuangpin_schema",
                "input.wubi_schema"
            ]
        );
    }
}
