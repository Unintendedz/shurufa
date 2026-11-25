use super::*;

const PAGE_SIZES: [u8; 2] = [5, 6];

fn reference_order(sources: &[u8], page_size: u8) -> Vec<usize> {
    if sources.len() < 2 || !sources.iter().any(|source| matches!(source, 2 | 3 | 12)) {
        return (0..sources.len()).collect();
    }
    let group = |source| {
        sources
            .iter()
            .enumerate()
            .filter(|(_, value)| **value == source)
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };
    let local: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| !matches!(source, 2 | 3 | 4 | 6 | 7 | 12))
        .map(|(index, _)| index)
        .collect();
    let cloud: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| matches!(source, 2 | 15))
        .map(|(index, _)| index)
        .collect();
    let ai: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| matches!(source, 3 | 12))
        .map(|(index, _)| index)
        .collect();
    let mut order: Vec<usize> = if cloud.is_empty() {
        (0..sources.len())
            .filter(|index| !matches!(sources[*index], 3 | 12))
            .collect()
    } else {
        let english = group(4);
        let emoji = group(6);
        let kaomoji = group(7);
        let promoted_english = !local.is_empty() && english.first() == Some(&0);
        let mut order = Vec::new();
        if promoted_english {
            order.push(0);
        }
        order.extend(local.iter().take(1).copied());
        order.extend(&cloud);
        if !promoted_english {
            order.extend(english.first().copied());
        }
        order.extend(emoji.first().copied());
        order.extend(kaomoji.first().copied());
        order.extend(local.iter().skip(1).copied());
        for group in [&english, &emoji, &kaomoji] {
            order.extend(group.iter().skip(1).copied());
        }
        order
    };
    if let Some(&first) = ai.first() {
        let seat = usize::from(page_size - 1).max(1).min(order.len());
        order.insert(seat, first);
    }
    order.extend(ai.iter().skip(1).copied());
    order
}

fn check(sources: Vec<u8>) {
    for page_size in PAGE_SIZES {
        check_page(sources.clone(), page_size);
    }
}

fn check_page(sources: Vec<u8>, page_size: u8) {
    let rows = sources.len();
    let expected = reference_order(&sources, page_size);
    let mut runtime = Runtime::new(
        Fixture {
            words: (0..rows).map(|index| format!("合成候选{index}")).collect(),
            codes: (0..rows).map(|index| format!("k{index}")).collect(),
            sources: sources.clone(),
            text: "a".into(),
            ..Fixture::default()
        },
        page_size,
    )
    .unwrap();
    runtime.refresh().unwrap();
    for (seat, &index) in expected.iter().enumerate() {
        let entry = &runtime.cached.candidates[seat];
        assert_eq!(entry.text, format!("合成候选{index}"));
        assert_eq!(entry.code, format!("k{index}"));
        assert_eq!(entry.annotation, format!("({index})"));
        assert_eq!(entry.source, sources[index]);
        assert!(!entry.corrected);
        assert!(entry.answers_key);
        assert_eq!(entry.origin, msime_engine::CandidateOrigin::EngineRow(index));
    }
    assert_eq!(runtime.cached.candidates.len(), expected.len());
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
}

#[test]
fn online_slots_match_group_append_rules_across_sources() {
    const SOURCES: [u8; 7] = [0, 2, 3, 4, 6, 7, 12];
    for rows in 0..=4 {
        for mut value in 0..SOURCES.len().pow(rows as u32) {
            let sources: Vec<u8> = (0..rows)
                .map(|_| {
                    let source = SOURCES[value % SOURCES.len()];
                    value /= SOURCES.len();
                    source
                })
                .collect();
            check(sources);
        }
    }
    let mut state = 0x7541_9812_5612_3819_u64;
    for rows in [8, 32, 128, 129, 1024] {
        for _ in 0..64 {
            let sources: Vec<u8> = (0..rows)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    SOURCES[(state >> 32) as usize % SOURCES.len()]
                })
                .collect();
            check(sources);
        }
    }
}
