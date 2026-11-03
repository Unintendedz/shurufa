//! 语音存档：每次录音的音频与听写记录，用宿主交来的 32 字节密钥加密后存在本机目录里。
//!
//! 目录里每次录音最多两份文件，文件名以录音 id 开头：
//!
//! - `<id>.audio`：8 字节文件头 [`AUDIO_MAGIC`]，之后是一串密封块，每块是
//!   `u32 LE 密文长度 ‖ 24 字节 nonce ‖ XChaCha20-Poly1305(密钥, nonce, PCM, aad)`。PCM 是 16 kHz 单声道 16 位小端；
//!   aad 是 [`AUDIO_AAD`] ‖ id ‖ `u64 LE 块序号`，块被换序或挪到别的录音里都解不开。录音一边进行一边落盘，
//!   进程崩溃或被杀时最后一块之前的音频都在，读到不完整的尾块就停。
//! - `<id>.record`：文件头 [`RECORD_MAGIC`] ‖ 24 字节 nonce ‖ 密文，明文是 [`Record`] 的 JSON，aad 是
//!   [`RECORD_AAD`] ‖ id。每次改动整份重写：先写同目录的临时文件，再改名替换。
//!
//! id 是 `<13 位毫秒时间戳>-<32 位小写十六进制>`。保留期限按 id 里的时间判断，清理不用解密。
//! 密钥由宿主保管（macOS 放在登录钥匙串里），这里只用不存。

use chacha20poly1305::{
    aead::{Aead, KeyInit as _, Payload},
    Key, XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

pub const AUDIO_MAGIC: &[u8; 8] = b"SRFAUD01";
pub const RECORD_MAGIC: &[u8; 8] = b"SRFREC01";
pub const AUDIO_AAD: &[u8] = b"shurufa-voice-audio-v1";
pub const RECORD_AAD: &[u8] = b"shurufa-voice-record-v1";
pub const SAMPLE_RATE: u32 = 16_000;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const TIMESTAMP_DIGITS: usize = 13;
const RANDOM_HEX: usize = 32;
const ID_LENGTH: usize = TIMESTAMP_DIGITS + 1 + RANDOM_HEX;
/// 攒够半秒再密封一块：崩溃时最多丢这半秒，块的额外开销（长度、nonce、校验）也只占约 4%。
const FLUSH_SAMPLES: usize = SAMPLE_RATE as usize / 2;
/// 单块密文的上限，读到更大的长度就当作文件损坏，不按它分配内存。
const MAX_CHUNK_BYTES: usize = FLUSH_SAMPLES * 2 * 8 + TAG_BYTES;
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("voice archive io: {0}")]
    Io(#[from] io::Error),
    #[error("invalid voice archive id")]
    InvalidId,
    #[error("voice archive entry cannot be decrypted")]
    Corrupt,
    #[error("system random source unavailable")]
    Random,
}

/// 一次录音的听写记录。识别结果按到达顺序追加在 `segments` 里：持续录音一次能投递好几段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub started_at_ms: u64,
    #[serde(default)]
    pub capture_ended_at_ms: Option<u64>,
    #[serde(default)]
    pub audio_ms: u64,
    /// 录音时前台客户端的 bundle id。
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub segments: Vec<String>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub cancelled: bool,
    /// 从存档重新听写的最近一次结果。
    #[serde(default)]
    pub retranscription: Option<String>,
}

/// 记录在列表里显示的状态，由记录本身和它是不是正在录的那一条推出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Recording,
    Recognized,
    Failed,
    Cancelled,
    /// 录音没有正常结束（进程退出、崩溃或被杀），音频保存到中断前。
    Interrupted,
    NoResult,
}

impl Record {
    pub fn status(&self, live: bool) -> Status {
        if live {
            Status::Recording
        } else if !self.segments.is_empty() {
            Status::Recognized
        } else if self.failure.is_some() {
            Status::Failed
        } else if self.cancelled {
            Status::Cancelled
        } else if self.capture_ended_at_ms.is_none() {
            Status::Interrupted
        } else {
            Status::NoResult
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// 新的在前。
    pub records: Vec<Record>,
    /// 解不开的记录文件数（换了密钥或文件损坏），不报内容。
    pub unreadable: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub audio_removed: usize,
    pub records_removed: usize,
}

pub struct Archive {
    dir: PathBuf,
    cipher: XChaCha20Poly1305,
}

/// 正在写的一次录音。`append` 收 16 kHz 单声道浮点样本，攒够半秒密封一块写进文件。
pub struct Recording {
    id: String,
    file: File,
    cipher: XChaCha20Poly1305,
    index: u64,
    pending: Vec<u8>,
    samples: u64,
}

impl Archive {
    pub fn open(dir: impl Into<PathBuf>, key: &[u8; 32]) -> Result<Self, ArchiveError> {
        let dir = dir.into();
        create_private_dir(&dir)?;
        Ok(Self {
            dir,
            cipher: XChaCha20Poly1305::new(&Key::from(*key)),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 开始一次录音：先写好记录和音频文件头，之后无论怎样中断，列表里都有这一条。
    pub fn begin(&self, now_ms: u64, app: &str, provider: &str) -> Result<Recording, ArchiveError> {
        let id = new_id(now_ms)?;
        let record = Record {
            id: id.clone(),
            started_at_ms: now_ms,
            capture_ended_at_ms: None,
            audio_ms: 0,
            app: app.to_owned(),
            provider: provider.to_owned(),
            segments: Vec::new(),
            failure: None,
            cancelled: false,
            retranscription: None,
        };
        self.write_record(&record)?;
        let mut file = create_private_file(&self.path(&id, "audio"))?;
        file.write_all(AUDIO_MAGIC)?;
        Ok(Recording {
            id,
            file,
            cipher: self.cipher.clone(),
            index: 0,
            pending: Vec::with_capacity(FLUSH_SAMPLES * 2),
            samples: 0,
        })
    }

    /// 录音停止：写完剩下的音频，记下结束时间和时长。
    pub fn end_capture(&self, recording: Recording, now_ms: u64) -> Result<Record, ArchiveError> {
        let id = recording.id.clone();
        let audio_ms = recording.finish()?;
        self.update(&id, |record| {
            record.capture_ended_at_ms = Some(now_ms);
            record.audio_ms = audio_ms;
        })
    }

    pub fn record(&self, id: &str) -> Result<Record, ArchiveError> {
        validate_id(id)?;
        self.read_record(&self.path(id, "record"), id)
    }

    pub fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut Record),
    ) -> Result<Record, ArchiveError> {
        let mut record = self.record(id)?;
        change(&mut record);
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn list(&self) -> Result<Listing, ArchiveError> {
        let mut listing = Listing::default();
        for (id, extension, path) in self.entries()? {
            if extension != "record" {
                continue;
            }
            match self.read_record(&path, &id) {
                Ok(record) => listing.records.push(record),
                Err(ArchiveError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => listing.unreadable += 1,
            }
        }
        listing.records.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(listing)
    }

    pub fn has_audio(&self, id: &str) -> bool {
        validate_id(id).is_ok() && self.path(id, "audio").is_file()
    }

    /// 解出整段 16 位样本。最后一块不完整（录音中途进程退出）时只返回完整的块。
    pub fn samples(&self, id: &str) -> Result<Vec<i16>, ArchiveError> {
        validate_id(id)?;
        let mut file = File::open(self.path(id, "audio"))?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != AUDIO_MAGIC {
            return Err(ArchiveError::Corrupt);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut samples = Vec::new();
        let mut offset = 0usize;
        let mut index = 0u64;
        while let Some(header) = bytes.get(offset..offset + 4 + NONCE_BYTES) {
            let length = u32::from_le_bytes(header[..4].try_into().expect("four bytes")) as usize;
            if !(TAG_BYTES..=MAX_CHUNK_BYTES).contains(&length) {
                return Err(ArchiveError::Corrupt);
            }
            let start = offset + 4 + NONCE_BYTES;
            let Some(ciphertext) = bytes.get(start..start + length) else {
                break;
            };
            let aad = audio_aad(id, index);
            let plain = self
                .cipher
                .decrypt(
                    &XNonce::try_from(&header[4..]).map_err(|_| ArchiveError::Corrupt)?,
                    Payload {
                        msg: ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| ArchiveError::Corrupt)?;
            samples.extend(
                plain
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| i16::from_le_bytes(*pair)),
            );
            offset = start + length;
            index += 1;
        }
        Ok(samples)
    }

    pub fn wav(&self, id: &str) -> Result<Vec<u8>, ArchiveError> {
        Ok(wav_bytes(&self.samples(id)?))
    }

    /// 重新听写用的 16 kHz 单声道浮点 PCM（小端 f32），与录音时交给识别服务的格式相同。
    pub fn pcm_f32(&self, id: &str) -> Result<Vec<u8>, ArchiveError> {
        Ok(self
            .samples(id)?
            .into_iter()
            .flat_map(|sample| (f32::from(sample) / 32768.0).to_le_bytes())
            .collect())
    }

    pub fn delete(&self, id: &str) -> Result<(), ArchiveError> {
        validate_id(id)?;
        for extension in ["audio", "record"] {
            remove_if_present(&self.path(id, extension))?;
        }
        Ok(())
    }

    /// 删除超过保留期限的音频和记录。`None` 表示不限期；记录过期时连同它的音频一起删。`keep` 是正在录的那一条。
    pub fn purge(
        &self,
        now_ms: u64,
        audio_ttl_ms: Option<u64>,
        record_ttl_ms: Option<u64>,
        keep: Option<&str>,
    ) -> Result<PurgeReport, ArchiveError> {
        let mut report = PurgeReport::default();
        for (id, extension, path) in self.entries()? {
            if keep == Some(id.as_str()) {
                continue;
            }
            let age = now_ms.saturating_sub(id_timestamp(&id));
            let ttl = if extension == "audio" {
                match (audio_ttl_ms, record_ttl_ms) {
                    (Some(a), Some(r)) => Some(a.min(r)),
                    (a, r) => a.or(r),
                }
            } else {
                record_ttl_ms
            };
            if ttl.is_some_and(|ttl| age > ttl) && remove_if_present(&path)? {
                if extension == "audio" {
                    report.audio_removed += 1;
                } else {
                    report.records_removed += 1;
                }
            }
        }
        Ok(report)
    }

    fn path(&self, id: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{id}.{extension}"))
    }

    /// 目录里文件名是合法 id 加 `.audio` 或 `.record` 的条目；别的文件一概不碰。
    fn entries(&self) -> Result<Vec<(String, &'static str, PathBuf)>, ArchiveError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some((id, extension)) = name.split_once('.') else {
                continue;
            };
            let extension = match extension {
                "audio" => "audio",
                "record" => "record",
                _ => continue,
            };
            if validate_id(id).is_ok() {
                entries.push((id.to_owned(), extension, entry.path()));
            }
        }
        Ok(entries)
    }

    fn read_record(&self, path: &Path, id: &str) -> Result<Record, ArchiveError> {
        let file = File::open(path)?;
        if file.metadata()?.len() > MAX_RECORD_BYTES {
            return Err(ArchiveError::Corrupt);
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES).read_to_end(&mut bytes)?;
        let header = RECORD_MAGIC.len() + NONCE_BYTES;
        if bytes.len() < header + TAG_BYTES || &bytes[..RECORD_MAGIC.len()] != RECORD_MAGIC {
            return Err(ArchiveError::Corrupt);
        }
        let aad = [RECORD_AAD, id.as_bytes()].concat();
        let plain = self
            .cipher
            .decrypt(
                &XNonce::try_from(&bytes[RECORD_MAGIC.len()..header])
                    .map_err(|_| ArchiveError::Corrupt)?,
                Payload {
                    msg: &bytes[header..],
                    aad: &aad,
                },
            )
            .map_err(|_| ArchiveError::Corrupt)?;
        let record: Record = serde_json::from_slice(&plain).map_err(|_| ArchiveError::Corrupt)?;
        if record.id != id {
            return Err(ArchiveError::Corrupt);
        }
        Ok(record)
    }

    fn write_record(&self, record: &Record) -> Result<(), ArchiveError> {
        validate_id(&record.id)?;
        let plain = serde_json::to_vec(record).expect("a record always serializes");
        let nonce = random_nonce()?;
        let aad = [RECORD_AAD, record.id.as_bytes()].concat();
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &plain,
                    aad: &aad,
                },
            )
            .expect("XChaCha20-Poly1305 accepts any record far below its length limit");
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        file.write_all(RECORD_MAGIC)?;
        file.write_all(&nonce)?;
        file.write_all(&ciphertext)?;
        file.as_file().sync_all()?;
        file.persist(self.path(&record.id, "record"))
            .map_err(|error| ArchiveError::Io(error.error))?;
        Ok(())
    }
}

impl Recording {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn append(&mut self, samples: &[f32]) -> Result<(), ArchiveError> {
        for sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.pending.extend_from_slice(&value.to_le_bytes());
            self.samples += 1;
            if self.pending.len() >= FLUSH_SAMPLES * 2 {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// 写完剩下的样本，返回音频时长（毫秒）。
    pub fn finish(mut self) -> Result<u64, ArchiveError> {
        self.flush()?;
        self.file.sync_all()?;
        Ok(self.samples * 1000 / u64::from(SAMPLE_RATE))
    }

    fn flush(&mut self) -> Result<(), ArchiveError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let nonce = random_nonce()?;
        let aad = audio_aad(&self.id, self.index);
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &self.pending,
                    aad: &aad,
                },
            )
            .expect("XChaCha20-Poly1305 accepts a half-second chunk");
        let length = u32::try_from(ciphertext.len()).expect("a chunk is far below 4 GiB");
        let mut block = Vec::with_capacity(4 + NONCE_BYTES + ciphertext.len());
        block.extend_from_slice(&length.to_le_bytes());
        block.extend_from_slice(&nonce);
        block.extend_from_slice(&ciphertext);
        self.file.write_all(&block)?;
        self.pending.clear();
        self.index += 1;
        Ok(())
    }
}

pub fn validate_id(id: &str) -> Result<(), ArchiveError> {
    let bytes = id.as_bytes();
    let valid = bytes.len() == ID_LENGTH
        && bytes[..TIMESTAMP_DIGITS].iter().all(u8::is_ascii_digit)
        && bytes[TIMESTAMP_DIGITS] == b'-'
        && bytes[TIMESTAMP_DIGITS + 1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    if valid {
        Ok(())
    } else {
        Err(ArchiveError::InvalidId)
    }
}

fn id_timestamp(id: &str) -> u64 {
    id[..TIMESTAMP_DIGITS]
        .parse()
        .expect("validated ids start with 13 digits")
}

fn new_id(now_ms: u64) -> Result<String, ArchiveError> {
    let mut random = [0u8; RANDOM_HEX / 2];
    getrandom::fill(&mut random).map_err(|_| ArchiveError::Random)?;
    let id = format!("{now_ms:013}-{}", hex::encode(random));
    validate_id(&id)?;
    Ok(id)
}

fn random_nonce() -> Result<[u8; NONCE_BYTES], ArchiveError> {
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| ArchiveError::Random)?;
    Ok(nonce)
}

fn audio_aad(id: &str, index: u64) -> Vec<u8> {
    [AUDIO_AAD, id.as_bytes(), &index.to_le_bytes()].concat()
}

pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn remove_if_present(path: &Path) -> Result<bool, ArchiveError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
