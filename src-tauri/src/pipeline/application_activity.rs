//! Observed application work, not a timer-driven estimate or an engine process.
use super::*;
use std::time::Duration;

pub(super) struct PreparationClock {
    pub(super) activity: Instant,
    pub(super) logged: Instant,
    pub(super) notice: Option<Instant>,
}

impl PreparationClock {
    pub(super) fn quiet_notice(&mut self, now: Instant) -> bool {
        if now.duration_since(self.activity) < Duration::from_secs(15)
            || self
                .notice
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(30))
        {
            return false;
        }
        self.notice = Some(now);
        true
    }
}

pub(super) struct ApplicationActivity {
    events: EventSink,
    stage: PipelineStage,
    label: &'static str,
    clock: Arc<std::sync::Mutex<PreparationClock>>,
    counts: std::sync::Mutex<Option<(u64, u64)>>,
    cancel: tokio_util::sync::CancellationToken,
    timer: tokio::task::JoinHandle<()>,
    progress_events: bool,
}

impl ApplicationActivity {
    /// Legacy callers already established their lifecycle. Bind rather than
    /// moving it; their counters remain log-only for snapshot compatibility.
    pub(super) fn new(
        events: &EventSink,
        stage: PipelineStage,
        cancel: &tokio_util::sync::CancellationToken,
        label: &'static str,
        total: Option<u64>,
    ) -> Self {
        let mut scoped = events.clone();
        scoped.bound_operation = events
            .context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .current
            .clone();
        let value = Self::create(scoped, stage, cancel, label, false);
        value.emit(EventKind::Log, total.map(|n| (0, n)));
        value
    }

    /// Explicit application lifecycle: a new source/step owns one operation.
    pub(super) fn begin(
        events: &EventSink,
        stage: PipelineStage,
        cancel: &tokio_util::sync::CancellationToken,
        label: &'static str,
        total: Option<u64>,
    ) -> Self {
        let value = Self::create(events.begin_application(stage), stage, cancel, label, true);
        value.emit(EventKind::Stage, total.map(|n| (0, n)));
        value
    }

    fn create(
        events: EventSink,
        stage: PipelineStage,
        cancel: &tokio_util::sync::CancellationToken,
        label: &'static str,
        progress_events: bool,
    ) -> Self {
        let now = Instant::now();
        let clock = Arc::new(std::sync::Mutex::new(PreparationClock {
            activity: now,
            logged: now,
            notice: None,
        }));
        let timer_clock = clock.clone();
        let timer_events = events.clone();
        let timer_cancel = cancel.clone();
        let timer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = timer_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        // A later process/step owns its own quiet notices.
                        if timer_events.context.lock().unwrap_or_else(|p| p.into_inner()).current.as_ref().map(|op| op.id)
                            != timer_events.bound_operation.as_ref().map(|op| op.id) { break; }
                        if timer_clock.lock().unwrap_or_else(|p| p.into_inner()).quiet_notice(Instant::now()) {
                            timer_events.send(stage, Some(PipelineEngine::System), EventKind::Log, EventLevel::Info,
                                None, true, format!("{label}。此步骤可能耗时较长，请耐心等待"), None, None, None);
                        }
                    }
                }
            }
        });
        Self {
            events,
            stage,
            label,
            clock,
            counts: Default::default(),
            cancel: cancel.clone(),
            timer,
            progress_events,
        }
    }

    fn emit(&self, kind: EventKind, count: Option<(u64, u64)>) {
        if self.cancel.is_cancelled() {
            return;
        }
        let wire_count = count.filter(|_| self.progress_events);
        self.events.send(
            self.stage,
            Some(PipelineEngine::System),
            kind,
            EventLevel::Info,
            None,
            true,
            count.map_or_else(
                || self.label.to_owned(),
                |(current, total)| format!("{}：{current}/{total}", self.label),
            ),
            wire_count.map(|v| v.0),
            wire_count.map(|v| v.1),
            wire_count.map(|_| "files"),
        );
    }

    pub(super) fn update(&self, current: u64, total: u64) {
        if total == 0 || current > total {
            return;
        }
        let mut counts = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        if counts
            .as_ref()
            .is_some_and(|last| *last == (current, total))
        {
            return;
        }
        *counts = Some((current, total));
        drop(counts);
        let now = Instant::now();
        let mut clock = self.clock.lock().unwrap_or_else(|p| p.into_inner());
        clock.activity = now;
        clock.notice = None;
        let log = current == total || now.duration_since(clock.logged) >= Duration::from_secs(1);
        if log {
            clock.logged = now;
        }
        drop(clock);
        if log {
            self.emit(
                if self.progress_events {
                    EventKind::Progress
                } else {
                    EventKind::Log
                },
                Some((current, total)),
            );
        }
    }

    pub(super) fn touch(&self) {
        let mut clock = self.clock.lock().unwrap_or_else(|p| p.into_inner());
        clock.activity = Instant::now();
        clock.notice = None;
    }

    #[cfg(test)]
    pub(super) fn log(&self, message: String) {
        if !self.cancel.is_cancelled() {
            self.events.send(
                self.stage,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                None,
                true,
                message,
                None,
                None,
                None,
            );
        }
    }

    pub(super) fn finish(&self) {
        self.timer.abort();
        let counts = *self.counts.lock().unwrap_or_else(|p| p.into_inner());
        // The final real count was already emitted; do not duplicate it.
        if !counts.is_some_and(|(current, total)| current == total) {
            self.emit(EventKind::Log, counts);
        }
    }
}

impl Drop for ApplicationActivity {
    fn drop(&mut self) {
        self.timer.abort();
    }
}

impl PipelineRunner {
    pub(super) fn observed_activity(&self, label: &'static str) -> ApplicationActivity {
        let stage = self
            .events
            .context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stage
            .unwrap_or(PipelineStage::Created);
        ApplicationActivity::new(
            &self.events,
            stage,
            &self.process_manager.child_token(),
            label,
            None,
        )
    }

    pub(super) fn application_activity(
        &self,
        stage: PipelineStage,
        label: &'static str,
        total: Option<u64>,
    ) -> ApplicationActivity {
        ApplicationActivity::begin(
            &self.events,
            stage,
            &self.process_manager.child_token(),
            label,
            total,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_notices_are_fifteen_then_thirty_seconds_and_activity_resets_them() {
        let now = Instant::now();
        let mut clock = PreparationClock {
            activity: now,
            logged: now,
            notice: None,
        };
        assert!(!clock.quiet_notice(now + Duration::from_secs(14)));
        assert!(clock.quiet_notice(now + Duration::from_secs(15)));
        assert!(!clock.quiet_notice(now + Duration::from_secs(44)));
        assert!(clock.quiet_notice(now + Duration::from_secs(45)));
        clock.activity = now + Duration::from_secs(46);
        clock.notice = None;
        assert!(!clock.quiet_notice(now + Duration::from_secs(60)));
        assert!(clock.quiet_notice(now + Duration::from_secs(61)));
    }

    #[tokio::test]
    async fn quiet_application_notice_stops_on_cancel_without_moving_progress() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(EnginePaths::from_root("not-used"), move |event| {
            output.lock().unwrap().push(event)
        });
        let cancel = runner.process_manager.child_token();
        let activity = ApplicationActivity::begin(
            &runner.events,
            PipelineStage::TrainingSplats,
            &cancel,
            "正在校验训练图片和相机数据",
            None,
        );
        activity.clock.lock().unwrap().activity = Instant::now() - Duration::from_secs(16);
        tokio::time::sleep(Duration::from_millis(5)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let count = {
            let events = captured.lock().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.message.ends_with("此步骤可能耗时较长，请耐心等待"))
                    .count(),
                1
            );
            assert!(events.iter().all(|event| event.stage_progress.is_none()));
            events.len()
        };
        cancel.cancel();
        activity.update(1, 1);
        activity.finish();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(captured.lock().unwrap().len(), count);
    }

    #[tokio::test]
    async fn application_counts_do_not_fake_percentages_or_cross_sources() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(EnginePaths::from_root("not-used"), move |event| {
            output.lock().unwrap().push(event)
        });
        runner.events.source(Some("first"));
        let first = runner.application_activity(
            PipelineStage::ExtractingFrames,
            "正在准备训练图片",
            Some(100),
        );
        for n in 1..=100 {
            first.update(n, 100);
        }
        first.update(100, 100);
        let before = runner
            .events
            .last_progress_milli_percent
            .load(Ordering::Relaxed);
        runner.events.source(Some("second"));
        let second = runner.application_activity(
            PipelineStage::ExtractingFrames,
            "正在准备训练图片",
            Some(2),
        );
        let new_id = captured
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .operation
            .as_ref()
            .unwrap()
            .id;
        first.update(101, 101);
        second.update(2, 2);
        second.finish();
        let events = captured.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.message.ends_with("100/100"))
                .count(),
            1
        );
        assert!(events
            .iter()
            .filter(|event| event.operation.as_ref().is_some_and(|op| op.id >= new_id))
            .all(|event| event.operation.as_ref().unwrap().source_id.as_deref() == Some("second")));
        assert!(events.iter().all(|event| event.stage_progress.is_none()));
        assert_eq!(
            runner
                .events
                .last_progress_milli_percent
                .load(Ordering::Relaxed),
            before
        );
    }
}
