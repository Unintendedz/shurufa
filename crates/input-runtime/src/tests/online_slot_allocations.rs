use super::allocations;
use super::*;

fn seated_runtime(rows: usize, source: u8) -> Runtime<Fixture> {
    let mut sources = vec![0; rows];
    sources[if source == 2 { 1 } else { 4 }] = source;
    Runtime::new(
        Fixture {
            words: (0..rows).map(|index| format!("合成候选{index}")).collect(),
            codes: (0..rows).map(|index| format!("k{index}")).collect(),
            sources,
            text: "a".into(),
            ..Fixture::default()
        },
        5,
    )
    .unwrap()
}

#[test]
fn online_seating_skips_identity_storage_when_already_seated() {
    let mut measurements = Vec::new();
    for rows in [32, 1024, 4096] {
        for source in [2, 3, 12] {
            let mut runtime = seated_runtime(rows, source);
            let (snapshot, snapshot_allocations, snapshot_bytes) =
                allocations::count_bytes(|| runtime.engine.snapshot().unwrap());
            let (_, refresh_allocations, refresh_bytes) =
                allocations::count_bytes(|| runtime.refresh().unwrap());
            assert_eq!(runtime.cached.candidates, snapshot.candidates);
            measurements.push((
                rows,
                source,
                refresh_allocations.saturating_sub(snapshot_allocations),
                refresh_bytes.saturating_sub(snapshot_bytes),
            ));
        }
    }
    assert!(
        measurements
            .iter()
            .all(|(_, _, allocations, bytes)| *allocations == 0 && *bytes == 0),
        "已归位时不应构造排列存储: {measurements:?}"
    );
}

#[test]
fn online_seating_materializes_early_and_late_mismatches() {
    for rows in [8, 32, 128, 129, 4096] {
        for late in [false, true] {
            let mut sources = vec![0; rows];
            if late {
                sources[1..rows - 2].fill(2);
                sources[4] = 3;
                sources[rows - 2] = 3;
            } else {
                sources[rows - 1] = 3;
            }
            let mut runtime = Runtime::new(
                Fixture {
                    words: (0..rows).map(|index| format!("合成候选{index}")).collect(),
                    codes: (0..rows).map(|index| format!("k{index}")).collect(),
                    sources,
                    text: "a".into(),
                    ..Fixture::default()
                },
                5,
            )
            .unwrap();
            runtime.refresh().unwrap();
            let mut expected: Vec<usize> = (0..rows).collect();
            if late {
                expected.swap(rows - 2, rows - 1);
            } else {
                expected.remove(rows - 1);
                expected.insert(4, rows - 1);
            }
            let actual_order: Vec<usize> = runtime
                .cached
                .candidates
                .iter()
                .map(|entry| match entry.origin {
                    msime_engine::CandidateOrigin::EngineRow(idx) => idx,
                    _ => usize::MAX,
                })
                .collect();
            assert_eq!(actual_order, expected);
            for (seat, &row) in expected.iter().enumerate() {
                assert_eq!(
                    runtime.cached.candidates[seat].text,
                    format!("合成候选{row}")
                );
                assert_eq!(runtime.cached.candidates[seat].code, format!("k{row}"));
                assert_eq!(
                    runtime.cached.candidates[seat].annotation,
                    format!("({row})")
                );
            }
            let (snapshot, snapshot_allocations, snapshot_bytes) =
                allocations::count_bytes(|| runtime.engine.snapshot().unwrap());
            let (_, refresh_allocations, refresh_bytes) =
                allocations::count_bytes(|| runtime.refresh().unwrap());
            let cycle_scratch = if rows > 128 { rows } else { 0 };
            assert_eq!(
                refresh_allocations - snapshot_allocations,
                usize::from(cycle_scratch > 0)
            );
            assert_eq!(refresh_bytes - snapshot_bytes, cycle_scratch);
            assert_eq!(runtime.cached.candidates.len(), snapshot.candidates.len());
            let refreshed_order: Vec<usize> = runtime
                .cached
                .candidates
                .iter()
                .map(|entry| match entry.origin {
                    msime_engine::CandidateOrigin::EngineRow(idx) => idx,
                    _ => usize::MAX,
                })
                .collect();
            assert_eq!(refreshed_order, expected);
            runtime.focused = true;
            let seat = if late { rows - 2 } else { 4 };
            let id = CandidateId {
                session: runtime.session,
                generation: runtime.generation,
                index: seat,
            };
            assert_eq!(
                runtime
                    .dispatch(Action::SelectAnyCandidate(id))
                    .unwrap()
                    .commit,
                Some(format!("合成候选{}", rows - 1))
            );
        }
    }
}

#[test]
fn seated_source_groups_need_no_order_allocation() {
    let cases = [
        vec![0, 2, 2, 4, 3, 6, 7, 0, 0, 4, 6, 7, 3, 12],
        vec![0, 4, 6, 7, 3, 0, 4, 6, 7, 12, 3],
        vec![4, 0, 2, 6, 3, 7, 0, 4, 6, 7, 12],
        vec![2, 2, 4, 6, 3, 7, 4, 6, 7, 3],
    ];
    let mut measurements = Vec::new();
    for sources in cases {
        let rows = sources.len();
        let mut runtime = Runtime::new(
            Fixture {
                words: (0..rows).map(|index| format!("合成候选{index}")).collect(),
                codes: (0..rows).map(|index| format!("k{index}")).collect(),
                sources,
                text: "a".into(),
                ..Fixture::default()
            },
            5,
        )
        .unwrap();
        let epochs = (runtime.composition_epoch, runtime.candidates_epoch);
        let (snapshot, snapshot_allocations, snapshot_bytes) =
            allocations::count_bytes(|| runtime.engine.snapshot().unwrap());
        let (_, refresh_allocations, refresh_bytes) =
            allocations::count_bytes(|| runtime.refresh().unwrap());
        assert_eq!(runtime.cached.candidates, snapshot.candidates);
        assert_eq!(
            (runtime.composition_epoch, runtime.candidates_epoch),
            epochs
        );
        let actual_order: Vec<usize> = runtime
            .cached
            .candidates
            .iter()
            .map(|entry| match entry.origin {
                msime_engine::CandidateOrigin::EngineRow(idx) => idx,
                _ => usize::MAX,
            })
            .collect();
        let expected_identity: Vec<usize> = (0..rows).collect();
        assert_eq!(actual_order, expected_identity);
        measurements.push((
            rows,
            refresh_allocations - snapshot_allocations,
            refresh_bytes - snapshot_bytes,
        ));
    }
    assert!(
        measurements
            .iter()
            .all(|(_, count, bytes)| *count == 0 && *bytes == 0),
        "已归位的多来源列表不需要排列存储: {measurements:?}"
    );
}
