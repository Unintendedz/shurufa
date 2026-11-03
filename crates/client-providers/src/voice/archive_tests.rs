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
        app: String::new(),
        provider: String::new(),
        segments: Vec::new(),
        failure: None,
        cancelled: false,
        retranscription: None,
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
