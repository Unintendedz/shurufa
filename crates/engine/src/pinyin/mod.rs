//! 方案无关的拼音基础模块：音节表、切分、音节图、简拼判定、模糊音展开与纠错。全拼和双拼共用此层，不接触 SQLite。

pub mod autocorrect;
pub mod fuzzy;
pub mod graph;
pub mod jianpin;
pub mod normalize;
pub mod segment;
pub mod syllables;
pub mod typos;
