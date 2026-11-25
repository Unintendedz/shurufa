//! 词网格整句解码模块：图构造、束搜索、三元重排、纠错整句、候选合并、n 元表、八股文 `.gram` 语法模型与个人模型。

mod cxx_sort;
pub mod decode;
pub mod merge;
pub mod ngram;
pub mod octagram;
mod separable_vo;

#[cfg(test)]
pub(crate) mod active_tables_tests;
pub mod personal;

pub use decode::{LatticeLexeme, SentencePath, TypoEdge};

/// 跨度查词闭包：返回键恰为给定音节串的词条。
pub type LatticeLookup<'a> = dyn FnMut(&[String]) -> Vec<LatticeLexeme> + 'a;

/// 按字面最优路径规划纠错边的闭包。
pub type TypoEdgeSource<'a> = dyn FnMut(&SentencePath) -> Vec<TypoEdge> + 'a;
