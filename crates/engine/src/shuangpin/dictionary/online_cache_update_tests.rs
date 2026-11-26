use super::*;
use crate::ime::personal_rerank::allocations::count;
use crate::quanpin::fixture::Fixture;

fn cached_rows(count: usize, source: CandidateSource) -> Vec<WordItem> {
    let mut rows = Vec::with_capacity(count + 1);
    for index in 0..count {
        let mut row = WordItem::new(
            "ni",
            format!("合成本地{index}"),
            index as i64,
            CandidateSource::Database,
            "ni",
        );
        row.corrected_from = "合成纠错".into();
        row.sentence_association = true;
        row.sentence_words = vec!["合成词甲".to_owned(), "合成词乙".to_owned()];
        rows.push(row);
    }
    rows.push(WordItem::new("ni", "合成旧在线", 1, source, ""));
    rows
}

fn batches() -> Vec<(Vec<String>, CandidateSource)> {
    vec![
        (vec!["合成新在线".to_owned()], CandidateSource::AiSuggestion),
        (
            vec!["合成新在线".to_owned()],
            CandidateSource::CloudSuggestion,
        ),
        (vec![], CandidateSource::AiSuggestion),
        (vec!["坏\n".to_owned()], CandidateSource::AiSuggestion),
        (
            (0..11).map(|index| format!("超额{index}")).collect(),
            CandidateSource::AiSuggestion,
        ),
        (vec!["不合法来源".to_owned()], CandidateSource::Database),
    ]
}

impl ShuangpinDictionary {
    fn frozen_insert_word_to_series_cache(
        &mut self,
        raw: &str,
        words: &[String],
        source: CandidateSource,
    ) -> bool {
        if raw.is_empty() {
            return false;
        }
        let key = self.sentence_cache_key(raw);
        let mut list = self.series_cache.get(&key).unwrap_or_default();
        if !replace_online_candidate_batch(&mut list, raw, words, source) {
            return false;
        }
        self.series_cache.insert(key, list);
        true
    }
}

fn dictionary(fixture: &Fixture) -> ShuangpinDictionary {
    ShuangpinDictionary::new(
        crate::shuangpin::profile::profile(crate::types::ShuangpinProfileKind::Xiaohe).unwrap(),
        &fixture.paths,
    )
}

#[test]
fn cached_online_update_avoids_page_clones() {
    let fixture = Fixture::new();
    for width in [0, 1, 63, 64, 128] {
        for source in [
            CandidateSource::AiSuggestion,
            CandidateSource::CloudSuggestion,
        ] {
            let original = cached_rows(width, source);
            let (_, clone_cost) = count(|| original.clone());
            for (batch, batch_source) in batches() {
                let mut before = dictionary(&fixture);
                let mut after = dictionary(&fixture);
                let key = before.sentence_cache_key("ni");
                before.series_cache.insert(key.clone(), original.clone());
                after.series_cache.insert(key.clone(), original.clone());
                let (expected, old_count) =
                    count(|| before.frozen_insert_word_to_series_cache("ni", &batch, batch_source));
                let (actual, new_count) =
                    count(|| after.insert_word_to_series_cache("ni", &batch, batch_source));
                assert_eq!(actual, expected);
                assert_eq!(
                    after.series_cache.get_ref(&key),
                    before.series_cache.get_ref(&key)
                );
                if batch_source == source || !actual {
                    assert_eq!(new_count + clone_cost, old_count);
                }
            }
        }
    }
}
