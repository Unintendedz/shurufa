//! 客户端本地用户内容与服务支持：诊断归档、外观皮肤、本地插件包、个人词库与打字统计。

pub use uuid;

use msime_client_base::text::{
    has_disallowed_control_with_allowed, is_ascii_identifier_with_dots, is_ascii_lowercase,
    is_bounded_ascii_identifier, is_bounded_utf16,
};
use msime_client_base::{bounded_io, calendar, file_lock, storage, text};
#[cfg(any(windows, test))]
use msime_client_core::edition;
use msime_client_core::{host_surface, preferences, theme};

pub mod diagnostics;
pub mod dictionary;
pub mod plugins;
pub mod skin;
pub mod typing_statistics;
