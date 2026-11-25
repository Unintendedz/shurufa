//! 双拼：内置方案与自定义方案（`custom`）、编码到音节转换、切分、方案状态机，以及复用全拼词典与词网格的双拼引擎。

pub mod custom;
pub mod dictionary;
pub mod engine;
pub mod hints;
pub mod profile;
pub mod query;
pub mod scheme;
#[cfg(test)]
mod tests;
pub mod typo_edges;
pub mod utils;

pub use engine::ShuangpinEngine;
pub use profile::ShuangpinProfile;
pub use scheme::ShuangpinScheme;
