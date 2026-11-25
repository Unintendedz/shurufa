//! 从插件目录读进 Engine 选项的表：`/` 模式的指令表、K 模式的短语表和 `@` 模式的名单。
//!
//! 它们都是设置页在输入进程运行期间写的文件：导入的指令表包、短语表包和 `mentions.json`。会话记下读过的每个文件的修改时间、长度和（Unix 上的）inode，输入框获得焦点和偏好应用时再看一遍，有变动才重新读。关着的模式不读其专属表；联系人名单独立于 `@` 模式装载，供普通拼音匹配。

use msime_client_core::preferences::PluginPreferences;
use msime_client_services::plugins::{
    command_table, mentions, pack_directory, phrase_table, BuiltinDirs, PluginKind, MANIFEST_FILE,
};
use msime_engine::host::{CommandTableEntry, EngineOptions, MentionEntry, QuickPhraseEntry};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 读文件时它的样子：修改时间、长度和（Unix 上的）inode；文件不在时为 `None`。重新导入会把整个包目录换掉，新清单与旧清单可能长度相同、修改时间又落在同一个计时粒度里（秒级时间戳的文件系统），只有 inode 不同，与 `key_sound::PackStamp` 同理。
type FileStamp = Option<(Option<SystemTime>, u64, u64)>;

fn stamp(path: &Path) -> FileStamp {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&metadata);
    #[cfg(not(unix))]
    let inode = 0;
    Some((metadata.modified().ok(), metadata.len(), inode))
}

/// The files a session's Engine options were filled from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PluginTables {
    /// The enabled command-table ids with each manifest's stamp, while the `/` mode is on.
    commands: Option<Vec<(String, FileStamp)>>,
    /// 本地名单的文件戳；普通联系人候选不依赖 `@` 模式开关。
    mentions: Option<FileStamp>,
    /// K 模式打开时，启用的短语表 id 与各自清单的戳。
    phrases: Option<Vec<(String, FileStamp)>>,
    /// 官方插件包目录（`PluginRoots::builtin_packs`）：启用的官方短语表从这里读、在这里盖戳。会话里不变。
    builtin_packs: Option<PathBuf>,
}

impl PluginTables {
    /// 给选项来源盖戳：只做少量 `stat`；未启用的模式专属表和没有插件目录的会话不读取文件。
    pub(crate) fn stamp(
        root: Option<&Path>,
        builtin_packs: Option<&Path>,
        options: &EngineOptions,
        plugins: &PluginPreferences,
    ) -> Self {
        let Some(root) = root else {
            return Self::default();
        };
        // 官方短语表的清单在内置目录里，其余在插件目录里；内置目录没有时它没有戳（读不到）。
        let manifests = |kind: PluginKind, enabled: &[String]| -> Vec<(String, FileStamp)> {
            enabled
                .iter()
                .map(|id| {
                    let manifest =
                        pack_directory(root, BuiltinDirs::packs(builtin_packs), kind, id)
                            .map(|directory| directory.join(MANIFEST_FILE));
                    (id.clone(), manifest.as_deref().and_then(stamp))
                })
                .collect()
        };
        Self {
            commands: options
                .local_command
                .then(|| manifests(PluginKind::CommandTable, &plugins.command_tables)),
            mentions: Some(stamp(&mentions::document_path(root))),
            phrases: options
                .local_quick_phrase
                .then(|| manifests(PluginKind::PhraseTable, &plugins.phrase_tables)),
            builtin_packs: builtin_packs.map(Path::to_path_buf),
        }
    }

    pub(crate) fn commands_differ(&self, previous: &Self) -> bool {
        self.commands != previous.commands
    }

    pub(crate) fn mentions_differ(&self, previous: &Self) -> bool {
        self.mentions != previous.mentions
    }

    pub(crate) fn phrases_differ(&self, previous: &Self) -> bool {
        self.phrases != previous.phrases
    }

    /// 启用的短语表合并后的行；K 模式关闭时为空。
    pub(crate) fn quick_phrase_table(&self, root: Option<&Path>) -> Vec<QuickPhraseEntry> {
        let (Some(root), Some(phrases)) = (root, &self.phrases) else {
            return Vec::new();
        };
        let enabled: Vec<String> = phrases.iter().map(|(id, _)| id.clone()).collect();
        phrase_table::enabled_phrases(root, self.builtin_packs.as_deref(), &enabled)
            .into_iter()
            .map(|row| QuickPhraseEntry {
                key: row.key,
                text: row.text,
            })
            .collect()
    }

    /// The merged rows of the enabled command tables; empty while the `/` mode is off.
    pub(crate) fn command_table(&self, root: Option<&Path>) -> Vec<CommandTableEntry> {
        let (Some(root), Some(commands)) = (root, &self.commands) else {
            return Vec::new();
        };
        let enabled: Vec<String> = commands.iter().map(|(id, _)| id.clone()).collect();
        command_table::enabled_commands(root, &enabled)
            .into_iter()
            .map(|row| CommandTableEntry {
                trigger: row.trigger,
                title: row.title,
                template: row.template,
            })
            .collect()
    }

    /// 本地保存的名单；无插件目录或读取失败时为空。未填拼音的中文姓名用引擎词典补全读音，与 `@` 模式开关无关。
    pub(crate) fn mention_entries(
        &self,
        root: Option<&Path>,
        options: &EngineOptions,
    ) -> Vec<MentionEntry> {
        let (Some(root), Some(_)) = (root, &self.mentions) else {
            return Vec::new();
        };
        let entries = mentions::MentionStore::new(root).load().unwrap_or_default();
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| {
                if entry.key.is_empty() && !entry.text.is_ascii() {
                    entry.text.as_str()
                } else {
                    ""
                }
            })
            .collect();
        let readings = msime_engine::host::hanzi_to_pinyin_batch(options, &names);
        entries
            .into_iter()
            .zip(readings)
            .map(|(entry, reading)| {
                let key = if entry.key.is_empty() && !entry.text.is_ascii() {
                    reading
                } else {
                    entry.key
                };
                MentionEntry {
                    text: entry.text,
                    key,
                    phone: entry.phone,
                }
            })
            .collect()
    }

    /// Fill `options` from the files that moved since `previous` was stamped.
    pub(crate) fn fill(&self, previous: &Self, root: Option<&Path>, options: &mut EngineOptions) {
        if self.commands_differ(previous) {
            options.command_table = self.command_table(root);
        }
        if self.mentions_differ(previous) {
            options.mention_entries = self.mention_entries(root, options);
        }
        if self.phrases_differ(previous) {
            options.quick_phrase_table = self.quick_phrase_table(root);
        }
    }
}
