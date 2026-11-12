//! Reconstruct only application-owned fixed progress templates. Unknown text,
//! engine output and source identifiers still use the normal privacy filter.
use crate::pipeline::{EventKind, PipelineEngine, PipelineEvent, PipelineStage};

const QUIET: &str = "。此步骤可能耗时较长，请耐心等待";
const LABELS: &[&str] = &[
    "正在准备训练图片",
    "正在准备全景图片",
    "正在准备原项目文件",
    "正在准备补拍画面",
    "正在整理补拍画面",
    "正在检查画面与蒙版",
    "正在校验训练图片和相机数据",
    "正在检查可复用结果",
    "正在保存生成检查点",
    "正在检查图片焦距信息",
    "正在准备原模型训练副本",
    "正在恢复结果发布",
    "正在校验并发布 final.ply",
    "素材分析",
    "画面准备",
    "特征提取",
    "图像匹配",
    "相机重建",
    "Splat 训练",
];

struct Number<'a> {
    text: &'a str,
    value: u64,
}

fn parse<'a>(message: &'a str, parts: &[&str]) -> Option<Vec<Number<'a>>> {
    let mut rest = message.strip_prefix(parts.first()?)?;
    let mut numbers = Vec::new();
    for suffix in &parts[1..] {
        let length = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_digit() || *byte == b',')
            .count();
        let text = rest.get(..length)?;
        if text.is_empty() {
            return None;
        }
        let groups = text.split(',').collect::<Vec<_>>();
        if groups.len() > 1
            && (!(1..=3).contains(&groups[0].len())
                || groups[1..].iter().any(|group| group.len() != 3))
        {
            return None;
        }
        let digits = text.replace(',', "");
        let value = digits.parse::<u64>().ok()?;
        numbers.push(Number { text, value });
        rest = rest.get(length..)?.strip_prefix(suffix)?;
    }
    rest.is_empty().then_some(numbers)
}

fn reconstruct(parts: &[&str], numbers: &[Number<'_>]) -> String {
    let mut output = String::new();
    for (prefix, number) in parts.iter().zip(numbers) {
        output.push_str(prefix);
        output.push_str(number.text);
    }
    output.push_str(parts.last().copied().unwrap_or_default());
    output
}

fn count_matches(event: &PipelineEvent, current: u64, total: u64) -> bool {
    match (event.current, event.total) {
        (Some(saved_current), Some(saved_total)) => {
            saved_current == current && saved_total == total
        }
        (None, None) => true, // Old journals did not attach structured counters.
        _ => false,
    }
}

fn checked(
    event: &PipelineEvent,
    message: &str,
    parts: &[&str],
    primary: usize,
    faces: bool,
) -> Option<String> {
    let numbers = parse(message, parts)?;
    for index in (0..numbers.len()).step_by(2) {
        let total = numbers.get(index + 1)?;
        if numbers[index].value > total.value {
            return None;
        }
    }
    if faces
        && (numbers[0].value != numbers[2].value / 6 || numbers[1].value != numbers[3].value / 6)
    {
        return None;
    }
    if !count_matches(
        event,
        numbers.get(primary)?.value,
        numbers.get(primary + 1)?.value,
    ) {
        return None;
    }
    Some(reconstruct(parts, &numbers))
}

fn bridge_summary(event: &PipelineEvent, message: &str) -> Option<String> {
    if event.stage != PipelineStage::ValidatingReconstruction
        || event.kind != EventKind::Log
        || event.unit.as_deref().is_some_and(|unit| unit != "frames")
    {
        return None;
    }
    let (registration, rest) = message.split_once('（')?;
    let prefix = ["Bridge Backfill 已触发：初始注册=", "/", ""];
    let initial = parse(registration, &prefix)?;
    if initial[0].value > initial[1].value {
        return None;
    }
    let (percentage, counts) = rest.split_once("%），剩余预算=")?;
    let (integer, fraction) = percentage.split_once('.')?;
    if !(1..=3).contains(&integer.len())
        || fraction.len() != 1
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let percentage_value = percentage.parse::<f64>().ok()?;
    if !percentage_value.is_finite() || !(0.0..=100.0).contains(&percentage_value) {
        return None;
    }
    let expected_percentage = if initial[1].value == 0 {
        0.0
    } else {
        initial[0].value as f64 / initial[1].value as f64 * 100.0
    };
    if (percentage_value - expected_percentage).abs() > 0.050_001 {
        return None;
    }
    let parts = ["", "，补帧=", "，内部桥接=", "，边缘延伸=", ""];
    let counts = parse(counts, &parts)?;
    let (budget, added, internal, edge) = (
        counts[0].value,
        counts[1].value,
        counts[2].value,
        counts[3].value,
    );
    if added > budget
        || internal.checked_add(edge)? != added
        || !count_matches(event, added, budget)
    {
        return None;
    }
    Some(format!(
        "{}（{percentage}%），剩余预算={}",
        reconstruct(&prefix, &initial),
        reconstruct(&parts, &counts)
    ))
}

pub(super) fn message(event: &PipelineEvent) -> Option<String> {
    if event.engine != Some(PipelineEngine::System)
        || !matches!(
            event.kind,
            EventKind::Log | EventKind::Stage | EventKind::Progress
        )
    {
        return None;
    }
    let (base, suffix) = event
        .message
        .strip_suffix(QUIET)
        .map_or((event.message.as_str(), ""), |base| (base, QUIET));
    if let Some(result) = bridge_summary(event, base) {
        return Some(format!("{result}{suffix}"));
    }
    // The application aggregates feature extraction across camera groups. Its
    // raw COLMAP logs remain untrusted and never enter this branch.
    if event.kind == EventKind::Progress
        && matches!(
            event.stage,
            PipelineStage::ExtractingFeatures | PipelineStage::ValidatingReconstruction
        )
        && event.unit.as_deref() == Some("images")
        && event.current.is_some()
        && event.total.is_some()
    {
        if let Some(result) = checked(event, base, &["正在分析画面（", "/", "）"], 0, false)
        {
            return Some(format!("{result}{suffix}"));
        }
    }
    if LABELS.contains(&base) {
        return Some(format!("{base}{suffix}"));
    }
    for label in LABELS {
        for separator in ["：", " ", " · "] {
            let prefix = format!("{label}{separator}");
            if let Some(result) = checked(event, base, &[&prefix, "/", ""], 0, false) {
                return Some(format!("{result}{suffix}"));
            }
        }
    }
    for parts in [
        ["补拍画面已加入：", "/", ""],
        ["当前模型已注册 ", "/", " 张原始画面"],
        ["当前模型已注册 ", "/", " 张原始图片"],
        ["六面训练缓存 ", "/", " 个全景时刻"],
        ["正在还原拍摄场景（", "/", "）"],
        ["正在生成高斯泼溅（", "/", "）"],
        [
            "补拍画面已加入 ",
            "/",
            " 张，部分新增画面未能找到与原场景的联系，效果可能受影响。",
        ],
        [
            "补拍画面成功加入 ",
            "/",
            "，结果已生成，但部分补拍画面未能找到与原项目的联系。",
        ],
    ] {
        if let Some(result) = checked(event, base, &parts, 0, false) {
            return Some(format!("{result}{suffix}"));
        }
    }
    for parts in [
        ["正在准备训练图片：全景图片 ", "/", "，六面图片 ", "/", ""],
        ["正在准备训练图片：全景帧 ", "/", "，六面图片 ", "/", ""],
        ["已校验并复用训练缓存：全景帧 ", "/", "，六面图片 ", "/", ""],
    ] {
        if let Some(result) = checked(event, base, &parts, 2, true) {
            return Some(format!("{result}{suffix}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(message: &str) -> PipelineEvent {
        let mut event = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.0, message);
        event.kind = EventKind::Progress;
        event
    }
    #[test]
    fn fixed_templates_keep_real_counts_and_legacy_counters_without_accepting_free_text() {
        let mut progress = event("正在准备训练图片：41/90");
        progress.current = Some(41);
        progress.total = Some(90);
        assert_eq!(
            message(&progress).as_deref(),
            Some("正在准备训练图片：41/90")
        );
        progress.current = Some(42);
        assert!(message(&progress).is_none());
        for text in [
            "正在准备训练图片：41/90",
            "正在准备原项目文件：1/10",
            "正在检查画面与蒙版：9/10。此步骤可能耗时较长，请耐心等待",
            "正在准备训练图片：全景图片 1/15，六面图片 11/90",
            "补拍画面已加入：1/10",
        ] {
            assert_eq!(message(&event(text)).as_deref(), Some(text));
        }
        for text in [
            "私人目录/照片",
            "正在准备训练图片：../private.png",
            "正在准备训练图片：41/90 secret=hunter2",
            "正在准备训练图片：41/90\nAuthorization: Bearer secret",
            "正在准备训练图片：91/90",
            "正在准备训练图片：NaN/90",
            "正在准备训练图片：全景帧 2/15，六面图片 1/90",
        ] {
            assert!(message(&event(text)).is_none(), "{text}");
        }
        let mut raw = event("正在准备训练图片：41/90");
        raw.engine = Some(PipelineEngine::Colmap);
        raw.kind = EventKind::Log;
        assert!(message(&raw).is_none());
    }

    #[test]
    fn aggregated_features_require_the_application_counter_contract() {
        let mut aggregate = event("正在分析画面（2/90）");
        aggregate.stage = PipelineStage::ExtractingFeatures;
        aggregate.unit = Some("images".into());
        aggregate.current = Some(2);
        aggregate.total = Some(90);
        assert_eq!(message(&aggregate).as_deref(), Some("正在分析画面（2/90）"));
        aggregate.stage = PipelineStage::ValidatingReconstruction;
        assert!(message(&aggregate).is_some());
        aggregate.current = Some(3);
        assert!(message(&aggregate).is_none());
        aggregate.current = Some(2);
        aggregate.unit = None;
        assert!(message(&aggregate).is_none());
        aggregate.unit = Some("images".into());
        aggregate.engine = Some(PipelineEngine::Colmap);
        assert!(message(&aggregate).is_none());
        aggregate.engine = Some(PipelineEngine::System);
        aggregate.kind = EventKind::Log;
        assert!(message(&aggregate).is_none());
    }

    #[test]
    fn bridge_summary_checks_percentages_all_counts_and_legacy_result_text() {
        let text = "Bridge Backfill 已触发：初始注册=24/45（53.3%），剩余预算=23，补帧=11，内部桥接=0，边缘延伸=11";
        let mut bridge = event(text);
        bridge.stage = PipelineStage::ValidatingReconstruction;
        bridge.kind = EventKind::Log;
        bridge.unit = Some("frames".into());
        bridge.current = Some(11);
        bridge.total = Some(23);
        assert_eq!(message(&bridge).as_deref(), Some(text));
        bridge.current = None;
        bridge.total = None;
        assert_eq!(message(&bridge).as_deref(), Some(text));
        for invalid in [
            text.replace("53.3", "NaN"),
            text.replace("53.3", "100.1"),
            text.replace("53.3", "54.0"),
            text.replace("53.3", "5.33e1"),
            text.replace("内部桥接=0", "内部桥接=1"),
            text.replace("补帧=11", "补帧=24"),
            format!("{text} secret=hidden"),
            format!("{text}\nC:\\private\\family.png"),
        ] {
            bridge.message = invalid;
            assert!(message(&bridge).is_none(), "{}", bridge.message);
        }
        bridge.message = text.into();
        bridge.current = Some(12);
        bridge.total = Some(23);
        assert!(message(&bridge).is_none());
        let legacy = "补拍画面成功加入 12/15，结果已生成，但部分补拍画面未能找到与原项目的联系。";
        assert_eq!(message(&event(legacy)).as_deref(), Some(legacy));
    }

    #[test]
    fn reader_templates_still_match_the_application_producer_literals() {
        let runner = include_str!("../pipeline/runner.rs");
        let reshoot = include_str!("../pipeline/reshoot_runner.rs");
        let dataset = include_str!("../pipeline/input_dataset.rs");
        let activity = include_str!("../pipeline/application_activity.rs");
        assert!(runner.contains("Bridge Backfill 已触发：初始注册={}/{}（{:.1}%），剩余预算={}，补帧={}，内部桥接={}，边缘延伸={}"));
        assert!(runner.contains("补拍画面成功加入 {registered_reshoot_count}/{}，结果已生成，但部分补拍画面未能找到与原项目的联系。"));
        assert!(reshoot.contains(
            "补拍画面成功加入 {}/{}，结果已生成，但部分补拍画面未能找到与原项目的联系。"
        ));
        assert!(activity.contains("{}：{current}/{total}"));
        assert!(dataset.contains("正在准备训练图片：全景帧 {}/{}，六面图片 {completed}/{total}"));
        assert!(dataset.contains("正在准备训练图片：全景图片 {}/{}，六面图片 {completed}/{total}"));
    }
}
