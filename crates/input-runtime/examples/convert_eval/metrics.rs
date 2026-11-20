//! Metric accumulation for pinyin-to-text conversion, kept free of Engine types so it can be
//! unit-tested without a dictionary.

use std::collections::BTreeMap;

/// Where a candidate came from. Mirrors the Engine's CandidateSource discriminants, which the
/// runtime passes through untouched as `Candidate::source`.
pub fn source_name(source: u8) -> &'static str {
    // Mirrors msime_engine::CandidateSource, in declaration order.
    match source {
        0 => "database",
        1 => "user-database",
        2 => "cloud",
        3 => "ai",
        4 => "english",
        5 => "quick-phrase",
        6 => "emoji",
        7 => "kaomoji",
        8 => "generated",
        9 => "fallback",
        10 => "neural-desktop",
        11 => "neural-keyboard",
        17 => "collocation",
        _ => "unknown",
    }
}

/// 分阶段失败归因类别：定位目标词是在词表、搜索还是呈现排座环节掉出首屏（前 6 格）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionStage {
    Top1,
    Page1NotTop1,
    PresentationDemotedOffPage1,
    EngineRankGt6,
    SearchPruned,
    DictOov,
}

#[derive(Default, Clone)]
pub struct AttributionSummary {
    pub cases: usize,
    pub top1: usize,
    pub page1_not_top1: usize,
    pub presentation_demoted_off_page1: usize,
    pub engine_rank_gt6: usize,
    pub search_pruned: usize,
    pub dict_oov: usize,
    pub immediate_top1: usize,
    pub immediate_top6: usize,
    pub settled_top6: usize,
    pub settled_top1_flips: usize,
    pub settled_top1_rescues: usize,
    pub settled_top1_regressions: usize,
    pub settled_top1_lateral: usize,
    /// 已找到用例的名次样本，用于计算 p50 / p90 / p95 / max。
    pub found_ranks: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankPercentiles {
    pub p50: usize,
    pub p90: usize,
    pub p95: usize,
    pub max: usize,
}

impl AttributionSummary {
    pub fn observe(
        &mut self,
        gold: &str,
        immediate: &[(String, u8)],
        settled: &[(String, u8)],
        raw_engine: &[(String, u8)],
        gold_in_dictionary: bool,
    ) -> AttributionStage {
        self.cases += 1;
        let immediate_top1_text = immediate.first().map(|(t, _)| t.as_str()).unwrap_or("");
        let settled_top1_text = settled.first().map(|(t, _)| t.as_str()).unwrap_or("");
        let immediate_rank = immediate
            .iter()
            .position(|(text, _)| text == gold)
            .map(|i| i + 1);
        let settled_rank = settled
            .iter()
            .position(|(text, _)| text == gold)
            .map(|i| i + 1);
        let raw_rank = raw_engine
            .iter()
            .position(|(text, _)| text == gold)
            .map(|i| i + 1);

        if immediate_rank == Some(1) {
            self.immediate_top1 += 1;
        }
        if immediate_rank.is_some_and(|r| r <= 6) {
            self.immediate_top6 += 1;
        }
        if settled_rank.is_some_and(|r| r <= 6) {
            self.settled_top6 += 1;
        }
        if let Some(rank) = settled_rank {
            self.found_ranks.push(rank);
        }
        if immediate_top1_text != settled_top1_text {
            self.settled_top1_flips += 1;
            match (immediate_top1_text == gold, settled_top1_text == gold) {
                (false, true) => self.settled_top1_rescues += 1,
                (true, false) => self.settled_top1_regressions += 1,
                _ => self.settled_top1_lateral += 1,
            }
        }

        match settled_rank {
            Some(1) => {
                self.top1 += 1;
                AttributionStage::Top1
            }
            Some(2..=6) => {
                self.page1_not_top1 += 1;
                AttributionStage::Page1NotTop1
            }
            Some(_) => {
                if raw_rank.is_some_and(|r| r <= 6) {
                    self.presentation_demoted_off_page1 += 1;
                    AttributionStage::PresentationDemotedOffPage1
                } else {
                    self.engine_rank_gt6 += 1;
                    AttributionStage::EngineRankGt6
                }
            }
            None => {
                if gold_in_dictionary {
                    self.search_pruned += 1;
                    AttributionStage::SearchPruned
                } else {
                    self.dict_oov += 1;
                    AttributionStage::DictOov
                }
            }
        }
    }

    pub fn percentiles(&self) -> Option<RankPercentiles> {
        if self.found_ranks.is_empty() {
            return None;
        }
        let mut sorted = self.found_ranks.clone();
        sorted.sort_unstable();
        let at = |pct: usize| -> usize {
            let idx = (sorted.len() * pct).div_ceil(100).saturating_sub(1);
            sorted[idx.min(sorted.len() - 1)]
        };
        Some(RankPercentiles {
            p50: at(50),
            p90: at(90),
            p95: at(95),
            max: *sorted.last().unwrap(),
        })
    }
}

#[derive(Default, Clone)]
pub struct Bucket {
    pub cases: usize,
    pub top1: usize,
    pub top5: usize,
    pub top9: usize,
    pub found: usize,
    /// Summed 1/rank over cases where the gold answer appears at all.
    pub reciprocal_rank: f64,
    /// Summed longest-common-prefix character ratio against the gold answer for the top-1 text.
    pub top1_char_prefix: f64,
}

impl Bucket {
    fn observe(&mut self, rank: Option<usize>, top1_prefix: f64) {
        self.cases += 1;
        self.top1_char_prefix += top1_prefix;
        if let Some(rank) = rank {
            self.found += 1;
            self.reciprocal_rank += 1.0 / (rank as f64);
            if rank == 1 {
                self.top1 += 1;
            }
            if rank <= 5 {
                self.top5 += 1;
            }
            if rank <= 9 {
                self.top9 += 1;
            }
        }
    }

    pub fn rate(hits: usize, cases: usize) -> f64 {
        if cases == 0 {
            0.0
        } else {
            hits as f64 / cases as f64
        }
    }
}

#[derive(Default)]
pub struct Report {
    pub overall: Bucket,
    /// Keyed by tag, then by syllable count, then by the source of the gold-matching candidate.
    pub by_tag: BTreeMap<String, Bucket>,
    pub by_syllables: BTreeMap<usize, Bucket>,
    /// How often the gold answer, when found at all, came from each candidate source.
    ///
    /// This is the measurement that matters for lattice work. For three or more complete syllables
    /// the Engine adds one lattice path and then, *if the Google-Pinyin fallback produced a
    /// sentence*, moves that fallback row to index 0. Whether a lattice change can move top-1
    /// therefore depends on which source actually occupies position 1, which varies by input —
    /// so both are recorded rather than assumed.
    pub gold_source: BTreeMap<&'static str, usize>,
    /// What sat at position 1, by source, whether or not it was correct.
    pub top1_source: BTreeMap<&'static str, usize>,
    pub failures: Vec<Failure>,
    pub attribution: AttributionSummary,
}

pub struct Failure {
    pub id: String,
    pub input: String,
    pub gold: String,
    pub got: String,
    pub rank: Option<usize>,
}

/// One case as the report sees it. The fields travel together, so they are one value rather than
/// eight positional arguments that are easy to transpose at the call site.
pub struct Observation<'a> {
    pub id: &'a str,
    pub input: &'a str,
    pub gold: &'a str,
    pub tags: &'a [String],
    pub syllables: usize,
    /// The full ordered candidate list as (text, source), position 1 first.
    pub candidates: &'a [(String, u8)],
    pub keep_failure: bool,
}

impl Report {
    pub fn observe(&mut self, case: Observation<'_>) {
        let Observation {
            id,
            input,
            gold,
            tags,
            syllables,
            candidates,
            keep_failure,
        } = case;
        let rank = candidates
            .iter()
            .position(|(text, _)| text == gold)
            .map(|i| i + 1);
        let top1_text = candidates.first().map(|(t, _)| t.as_str()).unwrap_or("");
        let prefix = char_prefix_ratio(top1_text, gold);

        self.overall.observe(rank, prefix);
        self.by_syllables
            .entry(syllables)
            .or_default()
            .observe(rank, prefix);
        for tag in tags {
            self.by_tag
                .entry(tag.clone())
                .or_default()
                .observe(rank, prefix);
        }
        if let Some((_, source)) = candidates.first() {
            *self.top1_source.entry(source_name(*source)).or_default() += 1;
        }
        if let Some(rank) = rank {
            let (_, source) = &candidates[rank - 1];
            *self.gold_source.entry(source_name(*source)).or_default() += 1;
        } else if keep_failure {
            self.failures.push(Failure {
                id: id.to_string(),
                input: input.to_string(),
                gold: gold.to_string(),
                got: top1_text.to_string(),
                rank,
            });
        }
        if keep_failure && rank.is_some_and(|r| r > 1) {
            self.failures.push(Failure {
                id: id.to_string(),
                input: input.to_string(),
                gold: gold.to_string(),
                got: top1_text.to_string(),
                rank,
            });
        }
    }
}

/// Fraction of the gold answer's characters that the produced text reproduces from the start.
/// A whole-sentence decoder that gets the first half right is meaningfully closer than one that
/// gets nothing right, and top-1 exact match alone cannot see that.
pub fn char_prefix_ratio(got: &str, gold: &str) -> f64 {
    let gold_chars: Vec<char> = gold.chars().collect();
    if gold_chars.is_empty() {
        return 0.0;
    }
    let shared = got
        .chars()
        .zip(gold_chars.iter().copied())
        .take_while(|(a, b)| a == b)
        .count();
    shared as f64 / gold_chars.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ratio_is_the_shared_leading_fraction() {
        assert_eq!(char_prefix_ratio("你好", "你好"), 1.0);
        assert_eq!(char_prefix_ratio("你号", "你好"), 0.5);
        assert_eq!(char_prefix_ratio("拟好", "你好"), 0.0);
        assert_eq!(char_prefix_ratio("", "你好"), 0.0);
        assert_eq!(char_prefix_ratio("你好", ""), 0.0);
        // A longer wrong answer still only scores its shared prefix.
        assert_eq!(char_prefix_ratio("你好吗", "你好"), 1.0);
    }

    #[test]
    fn rank_drives_every_threshold_and_reciprocal_rank() {
        let mut report = Report::default();
        let candidates: Vec<(String, u8)> = (1..=9).map(|i| (format!("c{i}"), 0u8)).collect();
        report.observe(Observation {
            id: "a",
            input: "x",
            gold: "c1",
            tags: &[],
            syllables: 2,
            candidates: &candidates,
            keep_failure: false,
        });
        report.observe(Observation {
            id: "b",
            input: "x",
            gold: "c5",
            tags: &[],
            syllables: 2,
            candidates: &candidates,
            keep_failure: false,
        });
        report.observe(Observation {
            id: "c",
            input: "x",
            gold: "zz",
            tags: &[],
            syllables: 2,
            candidates: &candidates,
            keep_failure: false,
        });
        assert_eq!(report.overall.cases, 3);
        assert_eq!(report.overall.top1, 1);
        assert_eq!(report.overall.top5, 2);
        assert_eq!(report.overall.found, 2);
        assert!((report.overall.reciprocal_rank - (1.0 + 0.2)).abs() < 1e-9);
    }

    #[test]
    fn gold_source_records_where_the_correct_answer_came_from() {
        let mut report = Report::default();
        // Position 1 is a fallback sentence; the correct answer is the lattice row behind it.
        let candidates = vec![("错的".to_string(), 9u8), ("对的".to_string(), 8u8)];
        report.observe(Observation {
            id: "a",
            input: "x",
            gold: "对的",
            tags: &[],
            syllables: 2,
            candidates: &candidates,
            keep_failure: false,
        });
        assert_eq!(report.gold_source.get("generated"), Some(&1));
        assert_eq!(report.top1_source.get("fallback"), Some(&1));
        assert_eq!(report.overall.top1, 0);
        assert_eq!(report.overall.top5, 1);
    }

    #[test]
    fn attribution_classifies_stages_percentiles_and_settled_flips() {
        let mut summary = AttributionSummary::default();
        let row = |text: &str| (text.to_string(), 0u8);
        // 1. 停顿精排把错误首位救回正确首位
        assert_eq!(
            summary.observe(
                "正解",
                &[row("误选"), row("正解")],
                &[row("正解"), row("误选")],
                &[row("误选"), row("正解")],
                true,
            ),
            AttributionStage::Top1
        );
        // 2. 引擎原本排在首屏第 2 位，却被呈现排座挤到第 7 位
        let mut demoted: Vec<(String, u8)> = (1..=6).map(|i| row(&format!("干扰{i}"))).collect();
        demoted.push(row("掉位"));
        assert_eq!(
            summary.observe(
                "掉位",
                &demoted,
                &demoted,
                &[row("干扰1"), row("掉位")],
                true,
            ),
            AttributionStage::PresentationDemotedOffPage1
        );
        // 3. 词表可拼出但被搜索剪掉
        assert_eq!(
            summary.observe("被剪", &[row("甲")], &[row("甲")], &[row("甲")], true),
            AttributionStage::SearchPruned
        );
        // 4. 词表缺失
        assert_eq!(
            summary.observe("缺词", &[row("甲")], &[row("甲")], &[row("甲")], false),
            AttributionStage::DictOov
        );
        assert_eq!(summary.cases, 4);
        assert_eq!(summary.top1, 1);
        assert_eq!(summary.presentation_demoted_off_page1, 1);
        assert_eq!(summary.search_pruned, 1);
        assert_eq!(summary.dict_oov, 1);
        assert_eq!(summary.settled_top1_flips, 1);
        assert_eq!(summary.settled_top1_rescues, 1);
        assert_eq!(summary.settled_top1_regressions, 0);
        let pct = summary.percentiles().unwrap();
        assert_eq!(pct.p50, 1);
        assert_eq!(pct.max, 7);
    }
}
