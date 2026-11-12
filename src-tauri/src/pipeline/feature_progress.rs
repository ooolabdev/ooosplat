//! Display-only aggregation of real COLMAP feature-group counters. Camera
//! grouping, extraction requests, database contents and recipes are untouched.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Group {
    expected: u64,
    reported: u64,
    operation_id: Option<u64>,
}

#[derive(Default)]
struct State {
    groups: BTreeMap<String, Group>,
    total: u64,
    valid: bool,
}

#[derive(Clone, Default)]
pub(super) struct FeatureProgressScope(Arc<Mutex<State>>);

#[derive(Clone)]
pub(super) struct FeatureGroupProgress {
    scope: FeatureProgressScope,
    key: String,
}

impl FeatureProgressScope {
    pub fn new(groups: impl IntoIterator<Item = (String, u64)>) -> Self {
        let mut state = State {
            valid: true,
            ..Default::default()
        };
        for (key, expected) in groups {
            if expected == 0 || state.groups.contains_key(&key) {
                state.valid = false;
            }
            if let Some(total) = state.total.checked_add(expected) {
                state.total = total;
            } else {
                state.valid = false;
            }
            state.groups.insert(
                key,
                Group {
                    expected,
                    ..Default::default()
                },
            );
        }
        state.valid &= state.total > 0;
        Self(Arc::new(Mutex::new(state)))
    }

    pub fn group(&self, key: String) -> FeatureGroupProgress {
        FeatureGroupProgress {
            scope: self.clone(),
            key,
        }
    }

    pub fn validated_complete(&self) -> Option<(u64, u64)> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if !state.valid {
            return None;
        }
        for group in state.groups.values_mut() {
            group.reported = group.expected;
        }
        Some((state.total, state.total))
    }
}

impl FeatureGroupProgress {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn expected(&self) -> Option<u64> {
        self.scope
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .groups
            .get(&self.key)
            .map(|group| group.expected)
    }

    pub fn begin(&self, operation_id: u64) -> Option<(u64, u64)> {
        let mut state = self.scope.0.lock().unwrap_or_else(|p| p.into_inner());
        if !state.valid {
            return None;
        }
        state.groups.get_mut(&self.key)?.operation_id = Some(operation_id);
        Some((
            state.groups.values().map(|group| group.reported).sum(),
            state.total,
        ))
    }

    pub fn report(&self, operation_id: u64, current: u64, total: u64) -> Option<(u64, u64)> {
        let mut state = self.scope.0.lock().unwrap_or_else(|p| p.into_inner());
        if !state.valid {
            return None;
        }
        let group = state.groups.get_mut(&self.key)?;
        if group.operation_id != Some(operation_id) || total != group.expected || current > total {
            return None;
        }
        // A genuine retry may report the same images again. Never double count.
        group.reported = group.reported.max(current);
        Some((
            state.groups.values().map(|group| group.reported).sum(),
            state.total,
        ))
    }

    pub fn validated(&self, operation_id: u64) -> Option<(u64, u64)> {
        let mut state = self.scope.0.lock().unwrap_or_else(|p| p.into_inner());
        if !state.valid {
            return None;
        }
        let group = state.groups.get_mut(&self.key)?;
        if group.operation_id != Some(operation_id) || group.reported == group.expected {
            return None;
        }
        group.reported = group.expected;
        Some((
            state.groups.values().map(|group| group.reported).sum(),
            state.total,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_groups_accumulate_unique_raw_images_and_ignore_stale_or_untrusted_counts() {
        let scope = FeatureProgressScope::new([
            ("photo-0".into(), 1),
            ("photo-1".into(), 1),
            ("erp".into(), 4),
        ]);
        let first = scope.group("photo-0".into());
        let second = scope.group("photo-1".into());
        let erp = scope.group("erp".into());
        assert_eq!(first.begin(1), Some((0, 6)));
        assert_eq!(first.report(1, 1, 1), Some((1, 6)));
        assert_eq!(second.begin(2), Some((1, 6)));
        assert_eq!(second.report(2, 1, 1), Some((2, 6)));
        assert_eq!(erp.begin(3), Some((2, 6)));
        assert!(erp.report(3, 6, 24).is_none()); // not six training faces
        assert!(erp.report(3, 5, 4).is_none());
        assert_eq!(erp.report(3, 2, 4), Some((4, 6)));
        assert_eq!(erp.begin(4), Some((4, 6)));
        assert!(erp.report(3, 4, 4).is_none());
        assert_eq!(erp.report(4, 1, 4), Some((4, 6)));
        assert_eq!(erp.report(4, 4, 4), Some((6, 6)));
    }

    #[test]
    fn bridge_scope_starts_at_zero_and_only_explicit_validation_can_fill_unreported_groups() {
        let initial = FeatureProgressScope::new([("initial".into(), 90)]);
        assert_eq!(initial.validated_complete(), Some((90, 90)));
        let bridge = FeatureProgressScope::new([("new-a".into(), 12), ("new-b".into(), 6)]);
        assert_eq!(bridge.group("new-a".into()).begin(1), Some((0, 18)));
        assert_eq!(bridge.group("new-a".into()).report(1, 3, 12), Some((3, 18)));
        assert_eq!(bridge.group("new-b".into()).begin(2), Some((3, 18)));
        assert_eq!(bridge.validated_complete(), Some((18, 18)));
    }

    #[test]
    fn malformed_display_scopes_never_invent_counts_or_fail_generation() {
        for specs in [
            vec![],
            vec![("a".into(), 0)],
            vec![("a".into(), 1), ("a".into(), 1)],
            vec![("a".into(), u64::MAX), ("b".into(), 1)],
        ] {
            let scope = FeatureProgressScope::new(specs);
            assert!(scope.group("a".into()).begin(1).is_none());
            assert!(scope.group("a".into()).report(1, 1, 1).is_none());
            assert!(scope.validated_complete().is_none());
        }
    }
}
