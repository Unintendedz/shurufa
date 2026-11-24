//! 无状态纯函数槽位分配器（`SlotAllocator`）：负责在全部解码、逐键神经重排与异步停顿精排完成后，
//! 将附加扩展候选（首批为精选内联 Emoji）以声明式配额投影到最终展示页。
//!
//! 设计不变量（由本模块单测锁死）：
//! 1. **中文首位神圣不可侵犯**：扩展候选永不占据下标 0（空格永远提交主首选）。
//! 2. **固定低侵入槽位与首页限额**：主推 Emoji 默认落在下标 2（第 3 位，不足 2 项时紧跟首位后），首页至多 1 格；
//!    同词其余备选 Emoji 放第 2 页开头（下标 `page_size`），方便偶尔点选后由本地 MRU 升至主推席。
//! 3. **不撕裂英文补全区块**：当英文自动补全处于首页连续席位时，Emoji 绝不楔入英文词与其补全之间；
//!    英文/字面原文居首时直接静默。
//! 4. **主序零扰动**：剔除所有投影行后，剩余主候选与输入 `base` 逐条一致。

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
    // 计算主推 Emoji 的安全下标：永不为 0，默认落在第 3 位（下标 2），且不楔入首页连续英文补全块中间。
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

    let mut result: Vec<AllocatedSlot> = Vec::with_capacity(base.len() + emojis.len());
    result.extend(base.iter().map(|item| AllocatedSlot::Base(item.id)));
    result.insert(
        primary_slot,
        AllocatedSlot::ProjectedEmoji {
            anchor_word: anchor_word.to_owned(),
            emoji: emojis[0],
        },
    );

    // 同一词的其余备选 Emoji 放在第 2 页开头（不抢占首页任何一格），方便用户偶尔翻页点选后由 MRU 记住。
    let extra_count = emojis
        .len()
        .saturating_sub(1)
        .min(budget.max_secondary_emojis);
    if extra_count > 0 {
        let secondary_start = page_size.min(result.len());
        for (offset, &emoji) in emojis[1..=extra_count].iter().enumerate() {
            result.insert(
                secondary_start + offset,
                AllocatedSlot::ProjectedEmoji {
                    anchor_word: anchor_word.to_owned(),
                    emoji,
                },
            );
        }
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
        }
    }

    fn en(id: usize, text: &str) -> SlotCandidate<'_> {
        SlotCandidate {
            id,
            text,
            kind: SlotCandidateKind::English,
            answers_key: false,
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
}
