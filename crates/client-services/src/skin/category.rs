//! 社区皮肤（键盘皮肤与候选窗口皮肤）共用的发布分类。

use serde::{Deserialize, Serialize};

/// 社区皮肤的发布分类，键盘皮肤和候选窗口皮肤用同一套 id。分类只是发布元数据，不属于皮肤内容本身，也不计入任何请求摘要。服务端将来新增的分类 id 一律读作 [`SkinCategory::Other`]，这样旧客户端不会因新分类而读取失败。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinCategory {
    Nature,
    Guofeng,
    Acg,
    Cute,
    Food,
    Tech,
    Minimal,
    #[default]
    #[serde(other)]
    Other,
}

impl SkinCategory {
    /// 全部分类，顺序即界面上筛选按钮的顺序。
    pub const ALL: [Self; 8] = [
        Self::Nature,
        Self::Guofeng,
        Self::Acg,
        Self::Cute,
        Self::Food,
        Self::Tech,
        Self::Minimal,
        Self::Other,
    ];

    /// 服务端使用的分类 id。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nature => "nature",
            Self::Guofeng => "guofeng",
            Self::Acg => "acg",
            Self::Cute => "cute",
            Self::Food => "food",
            Self::Tech => "tech",
            Self::Minimal => "minimal",
            Self::Other => "other",
        }
    }
}
