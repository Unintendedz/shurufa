use super::*;

const KEY: [u8; 32] = [7; 32];
const HOUR: u64 = 3_600_000;

fn archive() -> (tempfile::TempDir, Archive) {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::open(dir.path().join("voice-archive"), &KEY).unwrap();
    (dir, archive)
}

fn tone(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|index| ((index % 100) as f32 / 100.0) - 0.5)
        .collect()
}

fn as_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

#[test]
fn a_finished_recording_round_trips_audio_and_record() {
    let (_dir, archive) = archive();
    let mut recording = archive
        .begin(1_000, "com.example.editor", "textstream")
        .unwrap();
    let id = recording.id().to_owned();
    let audio = tone(SAMPLE_RATE as usize * 2 + 123);
    for chunk in audio.chunks(341) {
        recording.append(chunk).unwrap();
    }
    let record = archive.end_capture(recording, 3_500).unwrap();
    assert_eq!(record.capture_ended_at_ms, Some(3_500));
    assert_eq!(record.audio_ms, (audio.len() as u64) * 1000 / 16_000);
    assert_eq!(archive.samples(&id).unwrap(), as_i16(&audio));
    let wav = archive.wav(&id).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(wav.len(), 44 + audio.len() * 2);
    assert_eq!(archive.pcm_f32(&id).unwrap().len(), audio.len() * 4);

    let record = archive
        .update(&id, |record| record.segments.push("第一段".into()))
        .unwrap();
    assert_eq!(record.status(false), Status::Recognized);
    let listing = archive.list().unwrap();
    assert_eq!(listing.unreadable, 0);
    assert_eq!(listing.records, vec![record]);
    assert_eq!(listing.records[0].app, "com.example.editor");
}

#[test]
fn files_hold_no_plaintext() {
    let (_dir, archive) = archive();
    let mut recording = archive
        .begin(1_000, "com.example.secret-app", "doubao")
        .unwrap();
    let id = recording.id().to_owned();
    recording.append(&[0.25; 16_000]).unwrap();
    archive.end_capture(recording, 2_000).unwrap();
    archive
        .update(&id, |record| {
            record.segments.push("synthetic-transcript-marker".into())
        })
        .unwrap();
    let plain_pcm = 0.25f32.mul_add(32767.0, 0.0).round() as i16;
    for extension in ["audio", "record"] {
        let bytes = fs::read(archive.dir().join(format!("{id}.{extension}"))).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("synthetic-transcript-marker"));
        assert!(!text.contains("com.example.secret-app"));
        assert!(!bytes
            .windows(8)
            .any(|window| window == [plain_pcm.to_le_bytes(); 4].concat().as_slice()));
    }
}

#[test]
fn an_interrupted_recording_keeps_every_complete_chunk() {
    let (_dir, archive) = archive();
    let mut recording = archive.begin(1_000, "", "textstream").unwrap();
    let id = recording.id().to_owned();
    let audio = tone(FLUSH_SAMPLES * 3);
    recording.append(&audio).unwrap();
    // 不调用 end_capture：模拟进程在录音中途退出。再截掉最后一块的一部分，模拟写到一半。
    drop(recording);
    let path = archive.dir().join(format!("{id}.audio"));
    let length = fs::metadata(&path).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(length - 10)
        .unwrap();
    assert_eq!(
        archive.samples(&id).unwrap(),
        as_i16(&audio[..FLUSH_SAMPLES * 2])
    );
    let record = archive.record(&id).unwrap();
    assert_eq!(record.status(false), Status::Interrupted);
    assert_eq!(record.status(true), Status::Recording);
}

#[test]
fn a_wrong_key_or_tampering_is_rejected() {
    let (dir, archive) = archive();
    let mut recording = archive.begin(1_000, "", "").unwrap();
    let id = recording.id().to_owned();
    recording.append(&tone(FLUSH_SAMPLES)).unwrap();
    archive.end_capture(recording, 2_000).unwrap();

    let other = Archive::open(dir.path().join("voice-archive"), &[9; 32]).unwrap();
    assert!(matches!(other.samples(&id), Err(ArchiveError::Corrupt)));
    assert!(matches!(other.record(&id), Err(ArchiveError::Corrupt)));
    assert_eq!(other.list().unwrap().unreadable, 1);

    let path = archive.dir().join(format!("{id}.record"));
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(archive.record(&id), Err(ArchiveError::Corrupt)));
}

#[test]
fn a_record_cannot_be_moved_to_another_id() {
    let (_dir, archive) = archive();
    let first = archive.begin(1_000, "", "").unwrap();
    let second = archive.begin(2_000, "", "").unwrap();
    let (first, second) = (first.id().to_owned(), second.id().to_owned());
    fs::copy(
        archive.dir().join(format!("{first}.record")),
        archive.dir().join(format!("{second}.record")),
    )
    .unwrap();
    assert!(matches!(
        archive.record(&second),
        Err(ArchiveError::Corrupt)
    ));
}

#[test]
fn status_follows_the_recorded_outcome() {
    let base = Record {
        id: "0000000001000-00000000000000000000000000000000".into(),
        started_at_ms: 1_000,
        capture_ended_at_ms: Some(2_000),
        audio_ms: 1_000,
        ..Record::default()
    };
    assert_eq!(base.status(false), Status::NoResult);
    let failed = Record {
        failure: Some("provider".into()),
        ..base.clone()
    };
    assert_eq!(failed.status(false), Status::Failed);
    let cancelled = Record {
        cancelled: true,
        ..base.clone()
    };
    assert_eq!(cancelled.status(false), Status::Cancelled);
    let recognized = Record {
        segments: vec!["文字".into()],
        failure: Some("polish".into()),
        ..base
    };
    assert_eq!(recognized.status(false), Status::Recognized);
}

#[test]
fn purge_applies_separate_audio_and_record_limits() {
    let (_dir, archive) = archive();
    let now = 300 * HOUR;
    let mut ids = Vec::new();
    for started in [now - 30 * HOUR, now - 2 * HOUR, now - 200 * HOUR] {
        let mut recording = archive.begin(started, "", "").unwrap();
        recording.append(&tone(100)).unwrap();
        ids.push(recording.id().to_owned());
        archive.end_capture(recording, started + 1).unwrap();
    }
    let report = archive.purge(now, Some(24 * HOUR), None, None).unwrap();
    assert_eq!(
        report,
        PurgeReport {
            audio_removed: 2,
            records_removed: 0
        }
    );
    assert!(!archive.has_audio(&ids[0]));
    assert!(archive.has_audio(&ids[1]));
    assert_eq!(archive.list().unwrap().records.len(), 3);

    let report = archive
        .purge(now, Some(24 * HOUR), Some(100 * HOUR), None)
        .unwrap();
    assert_eq!(
        report,
        PurgeReport {
            audio_removed: 0,
            records_removed: 1
        }
    );
    assert!(matches!(archive.record(&ids[2]), Err(ArchiveError::Io(_))));
}

#[test]
fn purge_spares_the_live_recording_and_foreign_files() {
    let (_dir, archive) = archive();
    let recording = archive.begin(1_000, "", "").unwrap();
    let id = recording.id().to_owned();
    fs::write(archive.dir().join("notes.txt"), b"keep").unwrap();
    let report = archive
        .purge(10 * HOUR, Some(HOUR), Some(HOUR), Some(&id))
        .unwrap();
    assert_eq!(report, PurgeReport::default());
    assert!(archive.has_audio(&id));
    assert!(archive.dir().join("notes.txt").exists());
}

#[test]
fn shorter_record_limit_also_removes_audio() {
    let (_dir, archive) = archive();
    let mut recording = archive.begin(1_000, "", "").unwrap();
    recording.append(&tone(100)).unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 1_001).unwrap();
    let report = archive
        .purge(10 * HOUR, Some(24 * HOUR), Some(HOUR), None)
        .unwrap();
    assert_eq!(
        report,
        PurgeReport {
            audio_removed: 1,
            records_removed: 1
        }
    );
    assert!(!archive.has_audio(&id));
}

#[test]
fn ids_are_validated_before_touching_the_filesystem() {
    let (_dir, archive) = archive();
    for id in [
        "",
        "../../etc/passwd",
        "0000000001000-0000000000000000000000000000000",
        "0000000001000-0000000000000000000000000000000G",
        "000000000100a-00000000000000000000000000000000",
        "0000000001000_00000000000000000000000000000000",
    ] {
        assert!(
            matches!(archive.record(id), Err(ArchiveError::InvalidId)),
            "{id}"
        );
        assert!(
            matches!(archive.delete(id), Err(ArchiveError::InvalidId)),
            "{id}"
        );
    }
    validate_id("0000000001000-0123456789abcdef0123456789abcdef").unwrap();
}

#[test]
fn delete_removes_both_files() {
    let (_dir, archive) = archive();
    let recording = archive.begin(1_000, "", "").unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 1_001).unwrap();
    archive.delete(&id).unwrap();
    assert!(archive.list().unwrap().records.is_empty());
    assert!(!archive.has_audio(&id));
}

#[cfg(unix)]
#[test]
fn files_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_dir, archive) = archive();
    let recording = archive.begin(1_000, "", "").unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 1_001).unwrap();
    let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(archive.dir().to_owned()), 0o700);
    assert_eq!(mode(archive.dir().join(format!("{id}.audio"))), 0o600);
    assert_eq!(mode(archive.dir().join(format!("{id}.record"))), 0o600);
}

#[test]
fn legacy_record_json_deserializes_losslessly_and_omits_empty_metadata() {
    let (_dir, archive) = archive();
    let id = "0000000012345-0123456789abcdef0123456789abcdef";
    // 构造不含任何新增元数据字段的旧版明文 JSON，验证加密落盘后的旧记录无损读取。
    let legacy_json = serde_json::json!({
        "id": id,
        "started_at_ms": 12_345,
        "capture_ended_at_ms": 14_500,
        "audio_ms": 2_155,
        "app": "com.example.legacy",
        "provider": "textstream",
        "segments": ["旧版记录文本"],
        "failure": null,
        "cancelled": false,
        "retranscription": null
    });
    let plain = serde_json::to_vec(&legacy_json).unwrap();
    let nonce = [0x2au8; NONCE_BYTES];
    let aad = [RECORD_AAD, id.as_bytes()].concat();
    let cipher = XChaCha20Poly1305::new(&Key::from(KEY));
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plain,
                aad: &aad,
            },
        )
        .unwrap();
    let mut sealed = Vec::new();
    sealed.extend_from_slice(RECORD_MAGIC);
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ciphertext);
    fs::write(archive.dir().join(format!("{id}.record")), sealed).unwrap();

    let record = archive.record(id).unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.segments, vec!["旧版记录文本"]);
    assert_eq!(record.status(false), Status::Recognized);
    assert!(record.raw_segments.is_empty());
    assert_eq!(record.mode, None);
    assert_eq!(record.app_category, None);
    assert_eq!(record.polish_provider, None);
    assert_eq!(record.polish_model, None);
    assert_eq!(record.polish_prompt_id, None);
    assert_eq!(record.polish_failed, None);
    assert_eq!(record.polish_rejected_reason, None);
    assert_eq!(record.asr_ms, None);
    assert_eq!(record.polish_ms, None);
    assert_eq!(record.dismissed_reason, None);

    // 未写入新元数据时，序列化产物不出现新键，避免旧版读取方看到多余字段。
    let serialized = serde_json::to_value(&record).unwrap();
    let object = serialized.as_object().unwrap();
    for key in [
        "raw_segments",
        "mode",
        "app_category",
        "polish_provider",
        "polish_model",
        "polish_prompt_id",
        "polish_failed",
        "polish_rejected_reason",
        "asr_ms",
        "polish_ms",
        "dismissed_reason",
    ] {
        assert!(!object.contains_key(key), "不应序列化空字段 {key}");
    }
}

#[test]
fn note_text_with_metadata_round_trips_and_preserves_existing_fields() {
    let (_dir, archive) = archive();
    let recording = archive
        .begin(10_000, "com.example.ide", "textstream")
        .unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 12_000).unwrap();

    let first = archive
        .note_text_with_metadata(
            &id,
            "请检查 API 响应结构。\n\n然后更新测试。",
            TextNoteMetadata {
                raw_text: Some("嗯请检查api响应结构然后更新测试".into()),
                mode: Some("dictation".into()),
                app_category: Some("terminal_ide".into()),
                polish_provider: Some("local".into()),
                polish_model: Some("synthetic-qwen-9b".into()),
                polish_prompt_id: Some("cleanup".into()),
                polish_failed: Some(false),
                polish_rejected_reason: None,
                asr_ms: Some(320),
                polish_ms: Some(890),
            },
        )
        .unwrap();
    assert_eq!(first.status(false), Status::Recognized);
    assert_eq!(
        first.segments,
        vec!["请检查 API 响应结构。\n\n然后更新测试。"]
    );
    assert_eq!(first.raw_segments, vec!["嗯请检查api响应结构然后更新测试"]);
    assert_eq!(first.mode.as_deref(), Some("dictation"));
    assert_eq!(first.app_category.as_deref(), Some("terminal_ide"));
    assert_eq!(first.polish_provider.as_deref(), Some("local"));
    assert_eq!(first.polish_model.as_deref(), Some("synthetic-qwen-9b"));
    assert_eq!(first.polish_prompt_id.as_deref(), Some("cleanup"));
    assert_eq!(first.polish_failed, Some(false));
    assert_eq!(first.polish_rejected_reason, None);
    assert_eq!(first.asr_ms, Some(320));
    assert_eq!(first.polish_ms, Some(890));

    // 确认新增元数据在磁盘密文中不以明文出现。
    let disk = fs::read(archive.dir().join(format!("{id}.record"))).unwrap();
    let lossy = String::from_utf8_lossy(&disk);
    assert!(!lossy.contains("synthetic-qwen-9b"));
    assert!(!lossy.contains("terminal_ide"));

    // 第二次增量更新仅提供部分字段：分段继续追加，未提供字段保持原值。
    let second = archive
        .note_text_with_metadata(
            &id,
            "第二段合成文本",
            TextNoteMetadata {
                raw_text: Some("第二段原始合成文本".into()),
                polish_failed: Some(true),
                polish_rejected_reason: Some("length_bloat".into()),
                polish_ms: Some(1_450),
                ..TextNoteMetadata::default()
            },
        )
        .unwrap();
    assert_eq!(
        second.segments,
        vec!["请检查 API 响应结构。\n\n然后更新测试。", "第二段合成文本",]
    );
    assert_eq!(
        second.raw_segments,
        vec!["嗯请检查api响应结构然后更新测试", "第二段原始合成文本"]
    );
    assert_eq!(second.mode.as_deref(), Some("dictation"));
    assert_eq!(second.app_category.as_deref(), Some("terminal_ide"));
    assert_eq!(second.polish_provider.as_deref(), Some("local"));
    assert_eq!(second.polish_failed, Some(true));
    assert_eq!(
        second.polish_rejected_reason.as_deref(),
        Some("length_bloat")
    );
    assert_eq!(second.asr_ms, Some(320));
    assert_eq!(second.polish_ms, Some(1_450));

    // 第三次写入明确标记 polish_failed = false 且未提供拒收原因时，清空残留的旧拒收原因。
    let recovered = archive
        .note_text_with_metadata(
            &id,
            "第三段恢复正常",
            TextNoteMetadata {
                polish_failed: Some(false),
                polish_ms: Some(610),
                ..TextNoteMetadata::default()
            },
        )
        .unwrap();
    assert_eq!(recovered.polish_failed, Some(false));
    assert_eq!(recovered.polish_rejected_reason, None);
    assert_eq!(recovered.polish_ms, Some(610));

    // 空文本加空元数据为空操作，记录保持不变。
    let unchanged = archive
        .note_text_with_metadata(&id, "", TextNoteMetadata::default())
        .unwrap();
    assert_eq!(unchanged, recovered);
    assert_eq!(archive.record(&id).unwrap(), recovered);
}

#[test]
fn note_dismissed_records_reason_and_keeps_no_result_status() {
    let (_dir, archive) = archive();
    let recording = archive
        .begin(20_000, "com.example.chat", "textstream")
        .unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 21_000).unwrap();

    let dismissed = archive
        .note_dismissed(&id, "silence_hallucination")
        .unwrap();
    assert_eq!(
        dismissed.dismissed_reason.as_deref(),
        Some("silence_hallucination")
    );
    assert!(dismissed.segments.is_empty());
    // 丢弃原因不破坏现有状态枚举，仍为 NoResult。
    assert_eq!(dismissed.status(false), Status::NoResult);
    let listed = archive.list().unwrap();
    assert_eq!(listed.records, vec![dismissed]);
}

#[test]
fn metadata_and_dismissed_reason_enforce_length_and_control_char_bounds() {
    let (_dir, archive) = archive();
    let recording = archive.begin(30_000, "", "textstream").unwrap();
    let id = recording.id().to_owned();
    archive.end_capture(recording, 31_000).unwrap();

    // 恰好 128 字节的 ASCII 与多字节 UTF-8（42 * 3 = 126 字节）允许写入。
    let exact_128 = "a".repeat(MAX_REASON_BYTES);
    archive.note_dismissed(&id, &exact_128).unwrap();
    let utf8_126 = "静".repeat(42);
    archive.note_dismissed(&id, &utf8_126).unwrap();
    assert_eq!(
        archive.record(&id).unwrap().dismissed_reason.as_deref(),
        Some(utf8_126.as_str())
    );

    // 超过 128 字节、空串、纯空白或含控制字符的丢弃原因全部拒绝。
    let over_128_ascii = "a".repeat(MAX_REASON_BYTES + 1);
    let over_128_utf8 = "静".repeat(43);
    for bad in [
        "",
        "   ",
        "bad\nreason",
        "bad\0reason",
        "bad\x7f",
        &over_128_ascii,
        &over_128_utf8,
    ] {
        assert!(
            matches!(
                archive.note_dismissed(&id, bad),
                Err(ArchiveError::InvalidMetadata)
            ),
            "预期拒绝非法丢弃原因：{bad:?}"
        );
    }

    // 短元数据标签超过 128 字节、polish_model 超过 512 字节或含控制字符全部拒绝，且不改动磁盘上的旧值。
    let over_tag = "m".repeat(MAX_METADATA_TAG_BYTES + 1);
    let over_model = "m".repeat(MAX_MODEL_NAME_BYTES + 1);
    for metadata in [
        TextNoteMetadata {
            mode: Some(over_tag.clone()),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            app_category: Some("im\n".into()),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            polish_provider: Some(over_tag.clone()),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            polish_model: Some("model\0name".into()),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            polish_model: Some(over_model),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            polish_prompt_id: Some(over_tag.clone()),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            polish_rejected_reason: Some(over_tag),
            ..TextNoteMetadata::default()
        },
        TextNoteMetadata {
            raw_text: Some("x".repeat(MAX_TEXT_BYTES + 1)),
            ..TextNoteMetadata::default()
        },
    ] {
        assert!(matches!(
            archive.note_text_with_metadata(&id, "合法文本", metadata),
            Err(ArchiveError::InvalidMetadata)
        ));
    }

    // `text` 本身超过 1 MiB 同样拒绝。
    let oversized_text = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(matches!(
        archive.note_text_with_metadata(&id, &oversized_text, TextNoteMetadata::default()),
        Err(ArchiveError::InvalidMetadata)
    ));
    // 失败调用不得产生部分写入。
    assert!(archive.record(&id).unwrap().segments.is_empty());

    // 恰好 512 字节的 polish_model 允许写入（对齐 voice::provider 的模型名上限）。
    let exact_512_model = "q".repeat(MAX_MODEL_NAME_BYTES);
    let saved = archive
        .note_text_with_metadata(
            &id,
            "长模型名测试",
            TextNoteMetadata {
                polish_model: Some(exact_512_model.clone()),
                ..TextNoteMetadata::default()
            },
        )
        .unwrap();
    assert_eq!(
        saved.polish_model.as_deref(),
        Some(exact_512_model.as_str())
    );
}
