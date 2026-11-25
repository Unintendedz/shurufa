//! 无状态纯函数槽位分配器（`SlotAllocator`）：负责在全部解码、逐键神经重排与异步停顿精排完成后，
//! 将附加扩展候选（首批为精选内联 Emoji）以声明式配额投影到最终展示页。
//!
//! 设计不变量（由本模块单测锁死）：
//! 1. **中文首位神圣不可侵犯**：扩展候选永不占据下标 0（空格永远提交主首选）。
//! 2. **置顶首位硬约束不可破坏**：用户显式置顶（`pinned_first = true`）的候选必须稳坐最终下标 0，
//!    投影 Emoji 主动绕开首位，未置顶候选沿空余席位顺流，绝不把置顶候选挤离首位。
//! 3. **固定低侵入槽位与首页限额**：主推 Emoji 默认落在下标 2（第 3 位，若该席被固定则顺延至首页最近空余席），首页至多 1 格；
//!    同词其余备选 Emoji 放第 2 页开头（下标 `page_size` 起的首个空余席），方便偶尔点选后由本地 MRU 升至主推席。
//! 4. **不撕裂英文补全区块**：当英文自动补全处于首页连续席位时，Emoji 绝不楔入英文词与其补全之间；
//!    英文/字面原文居首时直接静默。
//! 5. **未固定主序零扰动**：剔除投影行与固定席位后，所有未固定主候选保持输入 `base` 的相对次序，零丢失、零重复。

use crate::local::inline_emoji::lookup_inline_emojis;
use crate::types::CandidateSource;

/// 参与槽位分配的候选大类（脱离原始行号与零散布尔位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotCandidateKind {
    PrimaryChinese,
    English,
    Online,
    Other,
}

impl SlotCandidateKind {
    /// 从 `CandidateSource` 序号映射为槽位大类。
    pub fn from_source(source: u8) -> Self {
        if source == CandidateSource::EnglishDictionary as u8 {
            Self::English
        } else if source == CandidateSource::CloudSuggestion as u8
            || source == CandidateSource::CloudPending as u8
            || source == CandidateSource::AiSuggestion as u8
            || source == CandidateSource::AiPending as u8
        {
            Self::Online
        } else if source == CandidateSource::Database as u8
            || source == CandidateSource::UserDatabase as u8
            || source == CandidateSource::Generated as u8
            || source == CandidateSource::NeuralKeyboard as u8
            || source == CandidateSource::Collocation as u8
            || source == CandidateSource::NeuralV2 as u8
            || source == CandidateSource::NeuralConstrained as u8
        {
            Self::PrimaryChinese
        } else {
            Self::Other
        }
    }
}

/// 传入 `SlotAllocator` 的单条干净主候选描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotCandidate<'a> {
    pub id: usize,
    pub text: &'a str,
    pub kind: SlotCandidateKind,
    pub answers_key: bool,
    /// 用户显式置顶到第 1 位（下标 0）的硬约束。
    pub pinned_first: bool,
}

/// `SlotAllocator` 产出的展示席位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocatedSlot {
    /// 指向输入 `base` 中的主候选 `id`。
    Base(usize),
    /// 由指定中文锚定词投影出的内联 Emoji。
    ProjectedEmoji {
        anchor_word: String,
        emoji: &'static str,
    },
}

/// 槽位预算配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotBudget {
    /// 每页可见候选数（Shurufa 桌面默认为 6）。
    pub page_size: usize,
    /// 主推 Emoji 的期望下标（默认 2，即第 3 位）。
    pub primary_emoji_slot: usize,
    /// 检查前几位完整中文候选作为 Emoji 锚点（默认 2，即 Top-1 / Top-2）。
    pub max_anchor_rank: usize,
    /// 首页外（第 2 页开头）最多额外放置几个同词备选 Emoji，用于支持 MRU 偏好切换。
    pub max_secondary_emojis: usize,
}

impl Default for SlotBudget {
    fn default() -> Self {
        Self {
            page_size: 6,
            primary_emoji_slot: 2,
            max_anchor_rank: 2,
            max_secondary_emojis: 2,
        }
    }
}

/// 无状态纯函数：基于最终排好的主候选列表与本地偏好回调，产出包含投影 Emoji 的展示席位。
///
/// 仅在调用方确认处于干净中文组字态（非本地模式、非英文模式、非光标前缀、非辅助码、非笔画筛选、非整句改字）
/// 时开启 `inline_emoji_enabled = true`。
pub fn allocate_slots<'p, F>(
    base: &[SlotCandidate<'_>],
    budget: SlotBudget,
    inline_emoji_enabled: bool,
    preferred_emoji: F,
) -> Vec<AllocatedSlot>
where
    F: Fn(&str) -> Option<&'p str>,
{
    let identity = || {
        base.iter()
            .map(|item| AllocatedSlot::Base(item.id))
            .collect()
    };
    if !inline_emoji_enabled || base.is_empty() {
        return identity();
    }
    // 英文/字面原文居首时（如 `Cla`、`inter`、`conf`），绝不插入中文 Emoji 干扰英文补全。
    if matches!(
        base[0].kind,
        SlotCandidateKind::English | SlotCandidateKind::Other
    ) {
        return identity();
    }

    let anchor_window = base.len().min(budget.max_anchor_rank.max(1));
    let Some((anchor_word, raw_emojis)) = base[..anchor_window].iter().find_map(|candidate| {
        if candidate.kind != SlotCandidateKind::PrimaryChinese || !candidate.answers_key {
            return None;
        }
        let emojis = lookup_inline_emojis(candidate.text);
        if emojis.is_empty() {
            None
        } else {
            Some((candidate.text, emojis))
        }
    }) else {
        return identity();
    };

    // 过滤已存在于主候选中的同文本项，并将本地 MRU 偏好项提到首位。
    let mut emojis: Vec<&'static str> = raw_emojis
        .iter()
        .copied()
        .filter(|&emoji| !base.iter().any(|candidate| candidate.text == emoji))
        .collect();
    if emojis.is_empty() {
        return identity();
    }
    if let Some(preferred) = preferred_emoji(anchor_word) {
        if let Some(pos) = emojis.iter().position(|&e| e == preferred) {
            if pos > 0 {
                emojis[..=pos].rotate_right(1);
            }
        }
    }

    let page_size = budget.page_size.max(2);
    let extra_count = emojis
        .len()
        .saturating_sub(1)
        .min(budget.max_secondary_emojis);
    let total_len = base.len() + 1 + extra_count;

    // 1. 仅锁定置顶到第 1 位（下标 0）的候选（`pinned_first = true`），废除 2..9 任意固定席位；
    //    若出现多条 `pinned_first`，仅首位见到的锁在下标 0，其余顺入未固定流。
    let mut reserved_pinned: Vec<Option<usize>> = vec![None; total_len];
    let mut unpinned_ids: Vec<usize> = Vec::with_capacity(base.len());
    for candidate in base {
        if candidate.pinned_first && reserved_pinned[0].is_none() {
            reserved_pinned[0] = Some(candidate.id);
        } else {
            unpinned_ids.push(candidate.id);
        }
    }

    // 2. 计算主推 Emoji 的安全下标：永不为 0，默认落在第 3 位（下标 2），不楔入连续英文补全块，且绝不覆盖固定席位。
    let mut primary_slot = budget.primary_emoji_slot.max(1).min(base.len());
    if primary_slot < base.len()
        && base[primary_slot].kind == SlotCandidateKind::English
        && primary_slot > 0
        && base[primary_slot - 1].kind == SlotCandidateKind::English
    {
        let mut after_english = primary_slot;
        while after_english < base.len() && base[after_english].kind == SlotCandidateKind::English {
            after_english += 1;
        }
        primary_slot = after_english
            .min(page_size.saturating_sub(1))
            .min(base.len());
    }
    if reserved_pinned[primary_slot].is_some() {
        let page_end = page_size.min(total_len);
        let fallback = (primary_slot + 1..page_end)
            .find(|&s| reserved_pinned[s].is_none())
            .or_else(|| {
                (1..primary_slot)
                    .rev()
                    .find(|&s| reserved_pinned[s].is_none())
            })
            .or_else(|| (page_end..total_len).find(|&s| reserved_pinned[s].is_none()));
        if let Some(free) = fallback {
            primary_slot = free;
        }
    }

    let mut reserved_emojis: Vec<Option<&'static str>> = vec![None; total_len];
    if reserved_pinned[primary_slot].is_none() {
        reserved_emojis[primary_slot] = Some(emojis[0]);
    }

    // 3. 同词其余备选 Emoji 放在第 2 页开头起的空余席位（不抢占首页任何一格，也不覆盖固定席位）。
    if extra_count > 0 {
        let mut cursor = page_size.min(total_len.saturating_sub(1));
        for &emoji in &emojis[1..=extra_count] {
            while cursor < total_len
                && (reserved_pinned[cursor].is_some() || reserved_emojis[cursor].is_some())
            {
                cursor += 1;
            }
            if cursor >= total_len {
                if let Some(free) = (1..total_len)
                    .find(|&s| reserved_pinned[s].is_none() && reserved_emojis[s].is_none())
                {
                    cursor = free;
                } else {
                    break;
                }
            }
            reserved_emojis[cursor] = Some(emoji);
            cursor += 1;
        }
    }

    // 4. 合并产出最终展示席位：固定席位原地锁死、Emoji 落入专属空位、其余未固定主候选按原相对顺序顺流填入。
    let mut unpinned_iter = unpinned_ids.into_iter();
    let mut result: Vec<AllocatedSlot> = Vec::with_capacity(total_len);
    for seat in 0..total_len {
        if let Some(id) = reserved_pinned[seat] {
            result.push(AllocatedSlot::Base(id));
        } else if let Some(emoji) = reserved_emojis[seat] {
            result.push(AllocatedSlot::ProjectedEmoji {
                anchor_word: anchor_word.to_owned(),
                emoji,
            });
        } else if let Some(id) = unpinned_iter.next() {
            result.push(AllocatedSlot::Base(id));
        }
    }
    for id in unpinned_iter {
        result.push(AllocatedSlot::Base(id));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh(id: usize, text: &str, answers_key: bool) -> SlotCandidate<'_> {
        SlotCandidate {
            id,
            text,
            kind: SlotCandidateKind::PrimaryChinese,
            answers_key,
            pinned_first: false,
        }
    }

    fn en(id: usize, text: &str) -> SlotCandidate<'_> {
        SlotCandidate {
            id,
            text,
            kind: SlotCandidateKind::English,
            answers_key: false,
            pinned_first: false,
        }
    }

    #[test]
    fn emoji_lands_at_slot_three_and_preserves_base_order() {
        let base = [
            zh(0, "思考", true),
            zh(1, "思靠", true),
            zh(2, "私考", true),
            zh(3, "思", false),
        ];
        let allocated = allocate_slots(&base, SlotBudget::default(), true, |_| None);
        assert_eq!(
            allocated,
            vec![
                AllocatedSlot::Base(0),
                AllocatedSlot::Base(1),
                AllocatedSlot::ProjectedEmoji {
                    anchor_word: "思考".into(),
                    emoji: "🤔",
                },
                AllocatedSlot::Base(2),
                AllocatedSlot::Base(3),
            ]
        );
        // 性质：剔除投影 Emoji 后与 base 逐条一致。
        let stripped: Vec<usize> = allocated
            .iter()
            .filter_map(|slot| match slot {
                AllocatedSlot::Base(id) => Some(*id),
                AllocatedSlot::ProjectedEmoji { .. } => None,
            })
            .collect();
        assert_eq!(stripped, vec![0, 1, 2, 3]);
    }

    #[test]
    fn emoji_never_occupies_slot_zero_even_with_single_candidate() {
        let base = [zh(0, "思考", true)];
        let allocated = allocate_slots(&base, SlotBudget::default(), true, |_| None);
        assert_eq!(allocated[0], AllocatedSlot::Base(0));
        assert_eq!(
            allocated[1],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "思考".into(),
                emoji: "🤔",
            }
        );
    }

    #[test]
    fn top2_anchor_also_projects_at_slot_three_and_secondary_goes_to_page2() {
        let base = [
            zh(0, "大小", true),
            zh(1, "大笑", true),
            zh(2, "打小", true),
            zh(3, "大", false),
            zh(4, "打", false),
            zh(5, "达", false),
        ];
        let allocated = allocate_slots(&base, SlotBudget::default(), true, |_| None);
        // 首页前 6 格只有 1 个主推 Emoji（下标 2）
        let page1_emojis: Vec<_> = allocated[..6]
            .iter()
            .filter(|s| matches!(s, AllocatedSlot::ProjectedEmoji { .. }))
            .collect();
        assert_eq!(page1_emojis.len(), 1);
        assert_eq!(
            allocated[2],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "大笑".into(),
                emoji: "😂",
            }
        );
        // 备选 Emoji 落在第 2 页开头（下标 6、7）
        assert_eq!(
            allocated[6],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "大笑".into(),
                emoji: "🤣",
            }
        );
    }

    #[test]
    fn local_mru_promotes_preferred_emoji_to_slot_three() {
        let base = [zh(0, "大笑", true), zh(1, "大小", true), zh(2, "大", false)];
        let allocated = allocate_slots(&base, SlotBudget::default(), true, |word| {
            (word == "大笑").then_some("🤣")
        });
        assert_eq!(
            allocated[2],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "大笑".into(),
                emoji: "🤣",
            }
        );
    }

    #[test]
    fn emoji_never_wedges_into_english_completion_block_or_english_led_list() {
        // 场景 1：英文/字面领衔（如 `Cla`、`inter`）直接静默
        let english_led = [en(0, "inter"), en(1, "international"), zh(2, "思考", true)];
        let allocated = allocate_slots(&english_led, SlotBudget::default(), true, |_| None);
        assert_eq!(
            allocated,
            vec![
                AllocatedSlot::Base(0),
                AllocatedSlot::Base(1),
                AllocatedSlot::Base(2),
            ]
        );

        // 场景 2：中文第 0 位 + 英文补全占第 1、2 位，Emoji 放到英文块之后（下标 3），绝不劈开英文补全
        let with_english_block = [
            zh(0, "思考", true),
            en(1, "sik"),
            en(2, "sika"),
            zh(3, "思靠", true),
            zh(4, "思", false),
        ];
        let allocated = allocate_slots(&with_english_block, SlotBudget::default(), true, |_| None);
        assert_eq!(allocated[0], AllocatedSlot::Base(0));
        assert_eq!(allocated[1], AllocatedSlot::Base(1));
        assert_eq!(allocated[2], AllocatedSlot::Base(2));
        assert_eq!(
            allocated[3],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "思考".into(),
                emoji: "🤔",
            }
        );
    }

    #[test]
    fn post_settled_rerank_reprojects_cleanly_without_stale_anchor_drift() {
        // 停顿前：开心在第 1 位、思考在第 3 位 -> 投 😄
        let before = [
            zh(0, "开心", true),
            zh(1, "开新", true),
            zh(2, "思考", true),
        ];
        let alloc_before = allocate_slots(&before, SlotBudget::default(), true, |_| None);
        assert_eq!(
            alloc_before[2],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "开心".into(),
                emoji: "😄",
            }
        );
        // 停顿后：思考升至第 0 位 -> 用同一纯函数重新投影，立刻切到 🤔，绝不残留旧 😄
        let after = [
            zh(2, "思考", true),
            zh(1, "开新", true),
            zh(0, "开心", true),
        ];
        let alloc_after = allocate_slots(&after, SlotBudget::default(), true, |_| None);
        assert_eq!(
            alloc_after[2],
            AllocatedSlot::ProjectedEmoji {
                anchor_word: "思考".into(),
                emoji: "🤔",
            }
        );
        assert!(!alloc_after
            .iter()
            .any(|slot| matches!(slot, AllocatedSlot::ProjectedEmoji { emoji: "😄", .. })));
    }

    #[test]
    fn pinned_first_holds_slot_zero_while_emojis_route_around_it() {
        // 场景：`嘛` 原本位于下标 3，显式置顶（`pinned_first = true`），`马` 投影 🐴/🐎：
        // `嘛` 稳坐下标 0、`吗` 顺延下标 1、`🐴` 落下标 2、`🐎` 落第 2 页开头下标 6。
        let mut ma_pinned = zh(3, "嘛", true);
        ma_pinned.pinned_first = true;
        let base = [
            zh(0, "吗", true),
            zh(1, "马", true),
            zh(2, "码", true),
            ma_pinned,
            zh(4, "麻", true),
            zh(5, "妈", true),
            zh(6, "骂", true),
        ];
        let allocated = allocate_slots(&base, SlotBudget::default(), true, |_| None);
        assert_eq!(allocated[0], AllocatedSlot::Base(3));
        assert_eq!(allocated[1], AllocatedSlot::Base(0));
        assert!(matches!(
            allocated[2],
            AllocatedSlot::ProjectedEmoji { ref anchor_word, .. } if anchor_word == "马"
        ));
        assert_eq!(allocated[3], AllocatedSlot::Base(1));
        assert_eq!(allocated[4], AllocatedSlot::Base(2));
        assert_eq!(allocated[5], AllocatedSlot::Base(4));
        assert!(matches!(
            allocated[6],
            AllocatedSlot::ProjectedEmoji { ref anchor_word, .. } if anchor_word == "马"
        ));
    }
}
