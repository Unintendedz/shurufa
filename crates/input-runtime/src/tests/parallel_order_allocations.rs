use super::allocations;
use super::*;

fn runtime_with_rows(rows: usize) -> Runtime<Fixture> {
    Runtime::new(
        Fixture {
            words: (0..rows).map(|index| format!("合成候选{index}")).collect(),
            codes: (0..rows).map(|index| format!("k{index}")).collect(),
            text: "a".into(),
            ..Fixture::default()
        },
        5,
    )
    .unwrap()
}

#[test]
fn parallel_candidate_order_uses_one_cycle_buffer() {
    let mut measurements = Vec::new();
    for rows in [32, 127, 128, 129, 1024, 4096] {
        for reverse in [false, true] {
            let mut runtime = runtime_with_rows(rows);
            let mut order: Vec<usize> = (0..rows).collect();
            if reverse {
                order.reverse();
            } else {
                order[1..].rotate_left(1);
            }
            let (_, count, bytes) = allocations::count_bytes(|| {
                runtime.apply_candidate_order(&order);
            });
            let scratch = if rows > 128 { rows } else { 0 };
            let allocation_budget = usize::from(scratch > 0);
            eprintln!("rows={rows} reverse={reverse}: allocations={count} requested_bytes={bytes}");
            measurements.push((rows, reverse, count, bytes, allocation_budget, scratch));
            let actual_order: Vec<usize> = runtime
                .cached
                .candidates
                .iter()
                .map(|entry| match entry.origin {
                    msime_engine::CandidateOrigin::EngineRow(idx) => idx,
                    _ => usize::MAX,
                })
                .collect();
            assert_eq!(actual_order, order);
            for (seat, &row) in order.iter().enumerate() {
                assert_eq!(runtime.cached.candidates[seat].text, format!("合成候选{row}"));
                assert_eq!(runtime.cached.candidates[seat].code, format!("k{row}"));
                assert_eq!(
                    runtime.cached.candidates[seat].annotation,
                    format!("({row})")
                );
            }
        }
    }
    assert!(
        measurements
            .iter()
            .all(|(_, _, count, bytes, allocation_budget, budget)| {
                count <= allocation_budget && bytes <= budget
            }),
        "候选条目已自带 origin，仅超过 128 行时分配一份循环访问缓冲: {measurements:?}"
    );
}

#[test]
fn parallel_candidate_order_keeps_all_metadata_and_composes_engine_seats() {
    for rows in [0, 1, 127, 128, 129, 4096] {
        let mut runtime = runtime_with_rows(rows);
        runtime.focus(true).unwrap();
        type_key(&mut runtime);
        // 使用不同的合成字段值，避免不变的布尔或来源掩盖漏交换。
        for (row, entry) in runtime.cached.candidates.iter_mut().enumerate() {
            entry.static_evidence = (row % 2 == 0).then_some(
                msime_engine::ordering::StaticEvidence::new(-(row as i64), row + 1),
            );
            entry.source = (row % 14) as u8;
            entry.corrected = row % 2 == 0;
            entry.answers_key = row % 3 == 0;
        }
        let original = runtime.cached.clone();
        let pointer = runtime.cached.candidates.as_ptr();
        let mut order: Vec<usize> = (0..rows).collect();
        if rows > 1 {
            order[1..].rotate_left(1);
        }
        runtime.apply_candidate_order(&order);
        let reverse: Vec<usize> = (0..rows).rev().collect();
        runtime.apply_candidate_order(&reverse);
        let expected: Vec<usize> = reverse.iter().map(|&seat| order[seat]).collect();
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
        assert_eq!(runtime.cached.candidates.as_ptr(), pointer);
        for (seat, &row) in expected.iter().enumerate() {
            assert_eq!(runtime.cached.candidates[seat], original.candidates[row]);
        }
        let identity: Vec<usize> = (0..rows).collect();
        let (_, allocations) = allocations::count(|| runtime.apply_candidate_order(&identity));
        assert_eq!(allocations, 0);
        let after_identity: Vec<usize> = runtime
            .cached
            .candidates
            .iter()
            .map(|entry| match entry.origin {
                msime_engine::CandidateOrigin::EngineRow(idx) => idx,
                _ => usize::MAX,
            })
            .collect();
        assert_eq!(after_identity, expected);
        if rows > 0 {
            let candidate = runtime.view().candidates[0].clone();
            assert_eq!(
                runtime
                    .dispatch(Action::Select(candidate.id))
                    .unwrap()
                    .commit,
                Some(candidate.text)
            );
        }
    }
}
