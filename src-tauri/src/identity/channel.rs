//! A physical input is a suggestion, not proof of identity. All durations share one clock.
use crate::state::AudioChannelProvenanceKind;

pub fn suggests_self(
    source: AudioChannelProvenanceKind,
    cluster_ms: u64,
    mic_speech_ms: u64,
    system_overlap_ms: u64,
) -> bool {
    matches!(source, AudioChannelProvenanceKind::Physical)
        && cluster_ms > 0
        && cluster_ms <= mic_speech_ms
        && u128::from(cluster_ms) * 5 >= u128::from(mic_speech_ms) * 4
        && u128::from(system_overlap_ms) * 5 < u128::from(cluster_ms) * 3
}

type Interval = (u64, u64);
fn union(intervals: &mut Vec<Interval>) {
    intervals.sort_unstable();
    let mut n = 0;
    for i in 0..intervals.len() {
        let (start, end) = intervals[i];
        if n > 0 && intervals[n - 1].1 >= start {
            intervals[n - 1].1 = intervals[n - 1].1.max(end)
        } else {
            intervals[n] = (start, end);
            n += 1;
        }
    }
    intervals.truncate(n);
}
fn duration(intervals: &[Interval]) -> u64 {
    intervals.iter().map(|(a, b)| b - a).sum()
}
fn overlap(a: &[Interval], b: &[Interval]) -> u64 {
    let (mut i, mut j, mut total) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        let end = a[i].1.min(b[j].1);
        total += end.saturating_sub(a[i].0.max(b[j].0));
        if a[i].1 <= b[j].1 { i += 1 } else { j += 1 }
    }
    total
}

/// Union overlapping windows before measuring speech. Counting spans instead of time
/// would double-count the ASR-anchored observation of a clustering window.
pub(crate) fn self_candidates(
    timeline: &crate::projections::SpeakerTimeline,
    sources: &std::collections::HashMap<String, AudioChannelProvenanceKind>,
) -> std::collections::BTreeSet<super::ClusterRef> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut remote = Vec::new();
    let mut mic: BTreeMap<&str, Vec<Interval>> = BTreeMap::new();
    let mut cluster_times: BTreeMap<super::ClusterRef, Vec<Interval>> = BTreeMap::new();
    for s in &timeline.latest_spans {
        let Some(source) = s.source_id.as_deref() else {
            continue;
        };
        let interval = (
            (s.start_time * 1000.).round() as u64,
            (s.end_time * 1000.).round() as u64,
        );
        if interval.1 <= interval.0 {
            continue;
        }
        match sources.get(source) {
            Some(AudioChannelProvenanceKind::Physical) => {
                mic.entry(source).or_default().push(interval);
                if s.identity.is_none()
                    && s.timeline_id == source
                    && let Some(c) = super::ClusterRef::origin(s)
                {
                    cluster_times.entry(c).or_default().push(interval);
                }
            }
            Some(
                AudioChannelProvenanceKind::Mixed
                | AudioChannelProvenanceKind::AppProcessDerived
                | AudioChannelProvenanceKind::VirtualMeetingLane,
            ) => remote.push(interval),
            _ => {}
        }
    }
    union(&mut remote);
    let totals: BTreeMap<_, _> = mic
        .into_iter()
        .map(|(s, mut v)| {
            union(&mut v);
            (s, duration(&v))
        })
        .collect();
    let mut winners: BTreeMap<String, Option<super::ClusterRef>> = BTreeMap::new();
    for (c, mut v) in cluster_times {
        union(&mut v);
        if suggests_self(
            AudioChannelProvenanceKind::Physical,
            duration(&v),
            totals[c.timeline_id.as_str()],
            overlap(&v, &remote),
        ) {
            // Ambiguous overlapping speakers or two engines observing the same mic: abstain.
            winners
                .entry(c.timeline_id.clone())
                .and_modify(|w| *w = None)
                .or_insert(Some(c));
        }
    }
    winners.into_values().flatten().collect::<BTreeSet<_>>()
}
#[cfg(test)]
mod tests {
    use super::*;
    use AudioChannelProvenanceKind::*;
    #[test]
    fn dominance_and_bleed_thresholds_are_inclusive() {
        assert!(suggests_self(Physical, 80, 100, 47));
        assert!(!suggests_self(Physical, 80, 100, 48));
        assert!(!suggests_self(Physical, 79, 100, 0));
        assert!(!suggests_self(Physical, 0, 0, 0));
        assert!(!suggests_self(Mixed, 100, 100, 0));
        assert!(!suggests_self(AppProcessDerived, 100, 100, 0));
    }
    #[test]
    fn source_local_duration_unions_abstain_on_overlap_and_echo() {
        use crate::identity::{ClusterRef, tests::span};
        use crate::projections::SpeakerTimeline;
        let mic = span("a", "mic");
        let mut duplicate = mic.clone();
        duplicate.span_id = "duplicate".into();
        let mut other = span("b", "mic");
        other.speaker_id = Some("other".into());
        other.start_time = 1.;
        other.end_time = 1.25;
        let mut remote = span("r", "system");
        remote.end_time = 0.59;
        let sources = [("mic".into(), Physical), ("system".into(), Mixed)]
            .into_iter()
            .collect();
        let mut t =
            SpeakerTimeline::replay("s", [mic.clone(), duplicate, other, remote.clone()]).unwrap();
        assert_eq!(
            self_candidates(&t, &sources),
            [ClusterRef::origin(&mic).unwrap()].into_iter().collect()
        );
        remote.end_time = 0.60;
        remote.revision_number = 2;
        remote.supersedes = Some("r@rev1".into());
        t.apply_event(remote).unwrap();
        assert!(self_candidates(&t, &sources).is_empty());
        let mut competing = mic.clone();
        competing.span_id = "competing".into();
        competing.speaker_id = Some("second-engine".into());
        let t = SpeakerTimeline::replay("s", [mic, competing]).unwrap();
        assert!(self_candidates(&t, &sources).is_empty());
    }
}
