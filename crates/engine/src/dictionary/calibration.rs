//! 主拼音词库只读权重重标定边车表（`SLXC` v1）。
//!
//! 在不改动 `EngineResources/msime-pinyin.db` 原始字节与锁文件 SHA-256、不干扰用户词典代次复制与学习回放的前提下，
//! 由离线构建工具生成紧凑有序的 `(hash64, raw_weight, calibrated_weight)` 二进制表：
//! - 压制 `tbl_1_*` 中 `weight == 0` 且主读音高频的方言/讹误单字幻影（如 `me->没/末`、`ti->是`、`wei->有`）；
//! - 将 `tbl_2_*` 中非词汇化的组合伪词「`X的`」（如 `鱼的(72540)`、`雨的(39575)`、`到的(500445)`）降权至低频兜底值；
//! - 借助语法语料对 `tbl_3..tbl_5` 中真实高频但出厂被填为 `weight == 100` 的多字词条（如 `怎么个事`、`遇得到`、`按理来说`）
//!   赋予平滑标定权重，而无语料实证的孤立占位词（如 `先哲之石`）保持原值 `100` 不动。

use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lru::LruCache;

use crate::paths::RuntimePaths;

/// 边车文件名（位于 `EngineResources` 同级的 `language-dictionaries/` 目录下）。
pub const LEXICON_CALIBRATION_FILENAME: &str = "shurufa-lexicon-calibration.bin";

const MAGIC: &[u8; 4] = b"SLXC";
const FORMAT_VERSION: u32 = 1;
const HEADER_BYTES: usize = 16;
const RECORD_BYTES: usize = 16;
const MAX_RECORDS: usize = 1_000_000;
const SHARED_CACHE_CAPACITY: usize = 4;

/// 单条校准记录（16 字节定长，按 `hash64` 严格升序排列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationRecord {
    pub hash64: u64,
    pub raw_weight: i32,
    pub calibrated_weight: i32,
}

/// 词典行校准动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationAction {
    /// 未命中或用户已学习改写权重，保持原样。
    Unchanged,
    /// 替换为标定后的新权重。
    UpdateWeight(i64),
    /// 压制未改动过的单字讹误/方言幻影行（`calibrated_weight < 0`）。
    Suppress,
}

/// 进程内共享的只读词库校准表。
#[derive(Debug)]
pub struct LexiconCalibration {
    seed: u32,
    records: Vec<CalibrationRecord>,
}

static SHARED: OnceLock<Mutex<LruCache<PathBuf, Option<Arc<LexiconCalibration>>>>> =
    OnceLock::new();

/// 计算 `(key, value)` 的 64 位确定性哈希（FNV-1a + SplitMix64 雪崩终混）。
pub fn hash_key_value(seed: u32, key: &str, value: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (u64::from(seed).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    for &b in key.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= 0xff;
    h = h.wrapping_mul(0x0000_0100_0000_01b3);
    for &b in value.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

impl LexiconCalibration {
    /// 由内存记录构造校准表（自动排序并校验无重复键哈希）。
    pub fn from_records(seed: u32, mut records: Vec<CalibrationRecord>) -> io::Result<Self> {
        if records.len() > MAX_RECORDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "calibration record count exceeds safety limit",
            ));
        }
        records.sort_unstable_by_key(|r| r.hash64);
        if records.windows(2).any(|w| w[0].hash64 == w[1].hash64) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate hash64 in lexicon calibration records",
            ));
        }
        Ok(Self { seed, records })
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 序列化写出 `SLXC` v1 二进制表。
    pub fn write_to(&self, mut writer: impl Write) -> io::Result<()> {
        let mut header = [0u8; HEADER_BYTES];
        header[0..4].copy_from_slice(MAGIC);
        header[4..8].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&self.seed.to_le_bytes());
        header[12..16].copy_from_slice(&(self.records.len() as u32).to_le_bytes());
        writer.write_all(&header)?;

        let mut buf = Vec::with_capacity(self.records.len() * RECORD_BYTES);
        for rec in &self.records {
            buf.extend_from_slice(&rec.hash64.to_le_bytes());
            buf.extend_from_slice(&rec.raw_weight.to_le_bytes());
            buf.extend_from_slice(&rec.calibrated_weight.to_le_bytes());
        }
        writer.write_all(&buf)?;
        writer.flush()
    }

    /// 从磁盘文件安全读取 `SLXC` v1 校准表；缺失、魔数不符、长度失配或未按升序排列时返回 `None`。
    pub fn load(path: &Path) -> Option<Self> {
        if path.as_os_str().is_empty() {
            return None;
        }
        if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
            return None;
        }
        let mut file = crate::paths::open_file_no_follow(path).ok()?;
        let size = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if size < HEADER_BYTES || !(size - HEADER_BYTES).is_multiple_of(RECORD_BYTES) {
            return None;
        }
        let count_from_size = (size - HEADER_BYTES) / RECORD_BYTES;
        if count_from_size > MAX_RECORDS {
            return None;
        }
        let mut header = [0u8; HEADER_BYTES];
        file.read_exact(&mut header).ok()?;
        if &header[0..4] != MAGIC {
            return None;
        }
        let version = u32::from_le_bytes(header[4..8].try_into().ok()?);
        if version != FORMAT_VERSION {
            return None;
        }
        let seed = u32::from_le_bytes(header[8..12].try_into().ok()?);
        let declared_count = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
        if declared_count != count_from_size {
            return None;
        }
        let mut raw_bytes = vec![0u8; count_from_size * RECORD_BYTES];
        file.read_exact(&mut raw_bytes).ok()?;

        let mut records = Vec::with_capacity(count_from_size);
        let mut prev_hash: Option<u64> = None;
        for chunk in raw_bytes.chunks_exact(RECORD_BYTES) {
            let hash64 = u64::from_le_bytes(chunk[0..8].try_into().ok()?);
            if prev_hash.is_some_and(|prev| hash64 <= prev) {
                return None;
            }
            prev_hash = Some(hash64);
            let raw_weight = i32::from_le_bytes(chunk[8..12].try_into().ok()?);
            let calibrated_weight = i32::from_le_bytes(chunk[12..16].try_into().ok()?);
            records.push(CalibrationRecord {
                hash64,
                raw_weight,
                calibrated_weight,
            });
        }
        Some(Self { seed, records })
    }

    /// 进程内按路径共享校准表实例（缺失路径同样缓存为 `None`，避免热路径重复 stat）。
    pub fn shared(path: &Path) -> Option<Arc<Self>> {
        if path.as_os_str().is_empty() {
            return None;
        }
        let mut cache = SHARED
            .get_or_init(|| {
                Mutex::new(LruCache::new(
                    NonZeroUsize::new(SHARED_CACHE_CAPACITY).expect("non-zero capacity"),
                ))
            })
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(cached) = cache.get(path) {
            return cached.clone();
        }
        let loaded = Self::load(path).map(Arc::new);
        cache.put(path.to_path_buf(), loaded.clone());
        loaded
    }

    /// 根据 `RuntimePaths` 解析 `resources` 同级 `language-dictionaries/shurufa-lexicon-calibration.bin`。
    pub fn runtime_path(paths: &RuntimePaths) -> Option<PathBuf> {
        if paths.resources.as_os_str().is_empty() {
            return None;
        }
        let parent = paths.resources.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(
            parent
                .join("language-dictionaries")
                .join(LEXICON_CALIBRATION_FILENAME),
        )
    }

    /// 按运行时路径加载共享校准表；单测临时目录无该文件时返回 `None`。
    pub fn shared_for_runtime(paths: &RuntimePaths) -> Option<Arc<Self>> {
        let path = Self::runtime_path(paths)?;
        Self::shared(&path)
    }

    /// 针对词库查出的 `(key, value, current_weight)` 计算校准结果（全程零堆分配）。
    #[inline]
    pub fn apply(&self, key: &str, value: &str, current_weight: i64) -> CalibrationAction {
        let h = hash_key_value(self.seed, key, value);
        let Ok(idx) = self.records.binary_search_by_key(&h, |r| r.hash64) else {
            return CalibrationAction::Unchanged;
        };
        let rec = self.records[idx];
        let raw = i64::from(rec.raw_weight);
        let cal = i64::from(rec.calibrated_weight);
        if current_weight == raw {
            if cal < 0 {
                CalibrationAction::Suppress
            } else if cal != current_weight {
                CalibrationAction::UpdateWeight(cal)
            } else {
                CalibrationAction::Unchanged
            }
        } else if cal > raw && current_weight > 0 && current_weight < cal {
            // 用户轻量学习调频未超过标定基准时，保底使用标定权重；超过标定基准后完全尊重用户学习值。
            CalibrationAction::UpdateWeight(cal)
        } else {
            CalibrationAction::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_apply_preserve_user_learning_and_suppression() {
        let seed = 7;
        let records = vec![
            CalibrationRecord {
                hash64: hash_key_value(seed, "me", "没"),
                raw_weight: 0,
                calibrated_weight: -1,
            },
            CalibrationRecord {
                hash64: hash_key_value(seed, "yu'de", "鱼的"),
                raw_weight: 72_540,
                calibrated_weight: 80,
            },
            CalibrationRecord {
                hash64: hash_key_value(seed, "zen'me'ge'shi", "怎么个事"),
                raw_weight: 100,
                calibrated_weight: 4_800,
            },
        ];
        let table = LexiconCalibration::from_records(seed, records).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(LEXICON_CALIBRATION_FILENAME);
        table
            .write_to(std::fs::File::create(&file_path).unwrap())
            .unwrap();

        let loaded = LexiconCalibration::load(&file_path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.apply("me", "没", 0), CalibrationAction::Suppress);
        // 若用户显式学习过该字（权重不再为 0），不再压制。
        assert_eq!(
            loaded.apply("me", "没", 10_000),
            CalibrationAction::Unchanged
        );
        assert_eq!(
            loaded.apply("yu'de", "鱼的", 72_540),
            CalibrationAction::UpdateWeight(80)
        );
        assert_eq!(
            loaded.apply("yu'de", "鱼的", 82_540),
            CalibrationAction::Unchanged
        );
        assert_eq!(
            loaded.apply("zen'me'ge'shi", "怎么个事", 100),
            CalibrationAction::UpdateWeight(4_800)
        );
        assert_eq!(
            loaded.apply("zen'me'ge'shi", "怎么个事", 20_000),
            CalibrationAction::Unchanged
        );
        assert_eq!(
            loaded.apply("ni'hao", "你好", 500_000),
            CalibrationAction::Unchanged
        );
    }
}
