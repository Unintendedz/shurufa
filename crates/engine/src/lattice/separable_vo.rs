//! 已清退的动宾离合词特判表占位模块。
//!
//! Shurufa v2 废除了 `+20*(s-1)` 词长断崖并建立统一对数概率尺度（见 `decode.rs`），
//! 高频单字拆插路径（如 `许+个+愿`、`请+个+假`、`睡+了+个+觉`）在统一概率与 `.gram`
//! 搭配信号下即可自然竞争，不再依赖硬编码 356 条离合词补丁表。保留空实现仅用于兼容模块接口。

use super::decode::{Graph, LatticeOptions};

/// 空操作：v2 统一概率词网格不再从静态特判表合成离合词边。
pub(super) fn append_separable_vo_edges(
    _syllables: &[String],
    _graph: &mut Graph,
    _options: &LatticeOptions<'_>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separable_vo_is_a_no_op_without_hardcoded_table() {
        let syllables = vec!["xu".to_owned(), "ge".to_owned(), "yuan".to_owned()];
        let mut graph: Graph = vec![Vec::new(), Vec::new(), Vec::new()];
        append_separable_vo_edges(&syllables, &mut graph, &LatticeOptions::default());
        assert!(graph.iter().all(Vec::is_empty));
    }
}
