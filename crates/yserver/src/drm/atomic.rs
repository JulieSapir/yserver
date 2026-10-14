//! Atomic commit request assembly, timing, and cursor latency telemetry.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io,
    rc::Rc,
    time::{Duration, Instant},
};

use drm::control::{
    AtomicCommitFlags, Device as ControlDevice, RawResourceHandle, ResourceHandle,
    atomic::AtomicModeReq, crtc, plane, property,
};

use crate::drm::Device;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // the commit producers arrive in Tasks 2–3
pub(crate) enum AtomicCommitKind {
    CursorOnly,
    SceneWithCursor,
    Scene,
}

impl AtomicCommitKind {
    const fn name(self) -> &'static str {
        match self {
            Self::CursorOnly => "cursor-only",
            Self::SceneWithCursor => "scene-with-cursor",
            Self::Scene => "scene",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::CursorOnly => 0,
            Self::SceneWithCursor => 1,
            Self::Scene => 2,
        }
    }
}

/// One cursor plane's CRTC-local position. `motion_timestamp` is captured
/// when the pointer event producing this position is handled and travels with
/// the position until an accepted commit and its retiring flip are observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlanePosition {
    /// DRM node that owns the following CRTC/plane handles. Raw DRM handles
    /// are only unique within one device.
    pub(crate) device_key: crate::platform::drm::DrmDeviceKey,
    pub(crate) crtc: crtc::Handle,
    pub(crate) plane: plane::Handle,
    pub(crate) crtc_x: property::Handle,
    pub(crate) crtc_y: property::Handle,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) motion_timestamp: Option<Instant>,
}

impl CursorPlanePosition {
    /// Append this cursor's `CRTC_X` and `CRTC_Y` values to an atomic request.
    #[allow(dead_code)] // consumed by the scene commit path in Task 2
    pub(crate) fn add_to(&self, req: &mut AtomicModeReq) {
        req.add_raw_property(self.plane.into(), self.crtc_x, i64::from(self.x) as u64);
        req.add_raw_property(self.plane.into(), self.crtc_y, i64::from(self.y) as u64);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AtomicPropertyUpdate {
    pub(crate) object: RawResourceHandle,
    pub(crate) property: property::Handle,
    pub(crate) value: u64,
}

#[allow(dead_code)] // request assembly starts in Task 2
impl AtomicPropertyUpdate {
    pub(crate) fn new<H: ResourceHandle>(
        object: H,
        property: property::Handle,
        value: u64,
    ) -> Self {
        Self {
            object: object.into(),
            property,
            value,
        }
    }
}

/// An atomic request paired with its flattened updates. The flattened form is
/// retained for the test recorder; production submits the `AtomicModeReq`.
#[allow(dead_code)] // scene and direct submitters start using this in Task 2
pub(crate) struct AtomicCommitRequest {
    request: AtomicModeReq,
    properties: Vec<AtomicPropertyUpdate>,
    cursor_properties: Vec<(RawResourceHandle, property::Handle)>,
}

#[allow(dead_code)] // request assembly starts in Task 2
impl AtomicCommitRequest {
    pub(crate) fn new() -> Self {
        Self {
            request: AtomicModeReq::new(),
            properties: Vec::new(),
            cursor_properties: Vec::new(),
        }
    }

    pub(crate) fn add_raw_property(
        &mut self,
        object: RawResourceHandle,
        property: property::Handle,
        value: u64,
    ) {
        self.request.add_raw_property(object, property, value);
        self.record_property(AtomicPropertyUpdate {
            object,
            property,
            value,
        });
    }

    pub(crate) fn add_cursor_position(&mut self, position: CursorPlanePosition) {
        position.add_to(&mut self.request);
        for property in [position.crtc_x, position.crtc_y] {
            self.cursor_properties
                .push((position.plane.into(), property));
        }
        self.record_property(AtomicPropertyUpdate::new(
            position.plane,
            position.crtc_x,
            i64::from(position.x) as u64,
        ));
        self.record_property(AtomicPropertyUpdate::new(
            position.plane,
            position.crtc_y,
            i64::from(position.y) as u64,
        ));
    }

    fn record_property(&mut self, update: AtomicPropertyUpdate) {
        if let Some(previous) = self.properties.iter_mut().find(|previous| {
            previous.object == update.object && previous.property == update.property
        }) {
            previous.value = update.value;
        } else {
            self.properties.push(update);
        }
    }

    fn into_request(self) -> AtomicModeReq {
        self.request
    }

    fn attribution_snapshot(&self) -> CommitAttributionSnapshot {
        CommitAttributionSnapshot {
            properties: self.properties.clone(),
            cursor_properties: self.cursor_properties.clone(),
        }
    }
}

/// Flattened copy of a cursor-carrying scene request, kept so a rejection can
/// be re-tested with and without the cursor fields.
struct CommitAttributionSnapshot {
    properties: Vec<AtomicPropertyUpdate>,
    cursor_properties: Vec<(RawResourceHandle, property::Handle)>,
}

impl CommitAttributionSnapshot {
    fn request(&self, with_cursor: bool) -> AtomicCommitRequest {
        let mut request = AtomicCommitRequest::new();
        for update in &self.properties {
            if with_cursor
                || !self
                    .cursor_properties
                    .contains(&(update.object, update.property))
            {
                request.add_raw_property(update.object, update.property, update.value);
            }
        }
        request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AtomicCommitEvent {
    pub(crate) crtc: crtc::Handle,
    pub(crate) motion_timestamp: Option<Instant>,
}

/// Injectable executor. The production executor calls the device's
/// `atomic_commit`; tests provide a recorder that accepts scripted results.
pub(crate) trait AtomicCommitExecutor {
    fn atomic_commit(
        &self,
        device: &Device,
        flags: AtomicCommitFlags,
        kind: AtomicCommitKind,
        request: AtomicCommitRequest,
        events: &[AtomicCommitEvent],
    ) -> io::Result<()>;
}

#[derive(Default)]
pub(crate) struct DrmAtomicCommitExecutor;

impl AtomicCommitExecutor for DrmAtomicCommitExecutor {
    fn atomic_commit(
        &self,
        device: &Device,
        flags: AtomicCommitFlags,
        _kind: AtomicCommitKind,
        request: AtomicCommitRequest,
        _events: &[AtomicCommitEvent],
    ) -> io::Result<()> {
        device.atomic_commit(flags, request.into_request())
    }
}

/// Per-device commit wrapper. It owns the injectable executor and aggregates
/// ioctl and pointer latency samples for the existing loop telemetry.
pub(crate) struct AtomicCommitSink {
    executor: Rc<dyn AtomicCommitExecutor>,
    metrics: RefCell<CommitWindow>,
    last_rejection_was_cursor: Cell<bool>,
}

impl AtomicCommitSink {
    /// `executor` is normally `DrmAtomicCommitExecutor`; tests can inject the
    /// cfg(test) request recorder through the same constructor.
    pub(crate) fn new(executor: Rc<dyn AtomicCommitExecutor>) -> Self {
        Self {
            executor,
            metrics: RefCell::new(CommitWindow::new(Instant::now())),
            last_rejection_was_cursor: Cell::new(false),
        }
    }

    /// Submit an atomic request and record one diagnostic row per target CRTC.
    #[allow(dead_code)] // no cursor commit producer exists until Tasks 2–3
    pub(crate) fn submit(
        &self,
        device: &Device,
        flags: AtomicCommitFlags,
        kind: AtomicCommitKind,
        request: AtomicCommitRequest,
        events: &[AtomicCommitEvent],
    ) -> io::Result<()> {
        let attribution =
            (kind == AtomicCommitKind::SceneWithCursor).then(|| request.attribution_snapshot());
        let started = Instant::now();
        let result = self
            .executor
            .atomic_commit(device, flags, kind, request, events);
        let finished = Instant::now();
        let cursor_rejected = match (&result, attribution) {
            (Ok(()), _) => false,
            (Err(_), None) => kind == AtomicCommitKind::CursorOnly,
            (Err(error), Some(snapshot)) => self.cursor_caused_rejection(device, error, &snapshot),
        };
        self.last_rejection_was_cursor.set(cursor_rejected);
        let elapsed = finished.saturating_duration_since(started);
        self.metrics.borrow_mut().record_commit(
            kind,
            events,
            elapsed,
            result.as_ref().err(),
            finished,
        );

        let result_text = result
            .as_ref()
            .map_or_else(|error| format!("error:{error}"), |_| "ok".to_owned());
        for event in events {
            log::debug!(
                target: "yserver::kms::render::backend::kms",
                "atomic commit kind={} crtc={:?} ioctl_us={} result={result_text}",
                kind.name(),
                event.crtc,
                elapsed.as_micros(),
            );
        }
        if result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(libc::EBUSY))
        {
            log::warn!(
                target: "yserver::kms::render::backend::kms",
                "atomic commit returned EBUSY; the CRTC already has a pending commit",
            );
        }
        self.emit_telemetry_if_due();
        result
    }

    /// Validate a request with `TEST_ONLY`; nothing is committed, no event is
    /// requested, and the commit telemetry is untouched.
    pub(crate) fn test_only(
        &self,
        device: &Device,
        kind: AtomicCommitKind,
        request: AtomicCommitRequest,
    ) -> io::Result<()> {
        self.executor
            .atomic_commit(device, AtomicCommitFlags::TEST_ONLY, kind, request, &[])
    }

    /// A failed scene request carrying cursor fields is the cursor's fault only
    /// if `TEST_ONLY` rejects it as sent and accepts it without those fields.
    /// EBUSY, or a failure `TEST_ONLY` does not reproduce, is a commit-phase
    /// error of the scene producer.
    fn cursor_caused_rejection(
        &self,
        device: &Device,
        error: &io::Error,
        snapshot: &CommitAttributionSnapshot,
    ) -> bool {
        let cursor_rejected = error.raw_os_error() != Some(libc::EBUSY)
            && self
                .test_only(
                    device,
                    AtomicCommitKind::SceneWithCursor,
                    snapshot.request(true),
                )
                .is_err()
            && self
                .test_only(device, AtomicCommitKind::Scene, snapshot.request(false))
                .is_ok();
        log::debug!(
            target: "yserver::kms::render::backend::kms",
            "atomic scene-with-cursor commit failed ({error}); attributed to {}",
            if cursor_rejected { "the cursor" } else { "the scene" },
        );
        cursor_rejected
    }

    /// Record the legacy path's motion-to-return latency. Both telemetry
    /// latency measures intentionally use this same return time.
    pub(crate) fn record_legacy_move(&self, motion_timestamp: Instant, returned_at: Instant) {
        self.metrics
            .borrow_mut()
            .record_legacy_move(motion_timestamp, returned_at);
        self.emit_telemetry_if_due();
    }

    /// Complete motion-to-visible timing when the corresponding accepted
    /// commit's page-flip event retires.
    pub(crate) fn note_cursor_commit_retired(&self, crtc: crtc::Handle, retired_at: Instant) {
        self.metrics
            .borrow_mut()
            .record_cursor_retirement(crtc, retired_at);
        self.emit_telemetry_if_due();
    }

    pub(crate) fn emit_telemetry_if_due(&self) {
        if std::env::var_os("YSERVER_LOOP_TELEMETRY").is_none() {
            return;
        }
        let now = Instant::now();
        let Some(snapshot) = self.metrics.borrow_mut().take_if_due(now) else {
            return;
        };
        let counts = snapshot.ioctl_us.each_ref().map(Vec::len);
        let summaries = snapshot
            .ioctl_us
            .each_ref()
            .map(|values| percentile(values));
        let accept = percentile(&snapshot.motion_to_accept_us);
        let visible = percentile(&snapshot.motion_to_visible_us);
        log::info!(
            target: "yserver::kms::render::backend::kms",
            "atomic commits/s cursor_only={} scene_with_cursor={} scene={} ebusy={} ioctl_us_max/p99 cursor_only={}/{} scene_with_cursor={}/{} scene={}/{} motion_to_accept_us_max/p99={}/{} motion_to_visible_us_max/p99={}/{}",
            counts[0],
            counts[1],
            counts[2],
            snapshot.ebusy,
            summaries[0].0,
            summaries[0].1,
            summaries[1].0,
            summaries[1].1,
            summaries[2].0,
            summaries[2].1,
            accept.0,
            accept.1,
            visible.0,
            visible.1,
        );
    }

    /// Whether the most recent rejected ioctl was caused by its cursor fields:
    /// any cursor-only failure, or a scene failure `cursor_caused_rejection`
    /// pins on the cursor. Other failures leave the cursor state untouched.
    pub(crate) fn take_last_cursor_rejection(&self) -> bool {
        self.last_rejection_was_cursor.replace(false)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_motion_to_accept_sample_count(&self) -> usize {
        self.metrics.borrow().motion_to_accept_us.len()
    }

    #[cfg(test)]
    pub(crate) fn for_tests_motion_to_visible_sample_count(&self) -> usize {
        self.metrics.borrow().motion_to_visible_us.len()
    }

    #[cfg(test)]
    pub(crate) fn for_tests_ebusy_count(&self) -> u64 {
        self.metrics.borrow().ebusy
    }
}

struct CommitWindow {
    started: Instant,
    ioctl_us: [Vec<u64>; 3],
    ebusy: u64,
    motion_to_accept_us: Vec<u64>,
    motion_to_visible_us: Vec<u64>,
    pending_motion_to_visible: HashMap<crtc::Handle, Instant>,
}

struct CommitWindowSnapshot {
    ioctl_us: [Vec<u64>; 3],
    ebusy: u64,
    motion_to_accept_us: Vec<u64>,
    motion_to_visible_us: Vec<u64>,
}

impl CommitWindow {
    fn new(started: Instant) -> Self {
        Self {
            started,
            ioctl_us: std::array::from_fn(|_| Vec::new()),
            ebusy: 0,
            motion_to_accept_us: Vec::new(),
            motion_to_visible_us: Vec::new(),
            pending_motion_to_visible: HashMap::new(),
        }
    }

    fn record_commit(
        &mut self,
        kind: AtomicCommitKind,
        events: &[AtomicCommitEvent],
        elapsed: Duration,
        error: Option<&io::Error>,
        finished: Instant,
    ) {
        let ioctl_us = duration_us(elapsed);
        self.ioctl_us[kind.index()].push(ioctl_us);
        for event in events {
            if error.is_none()
                && let Some(motion_timestamp) = event.motion_timestamp
            {
                self.motion_to_accept_us
                    .push(elapsed_us(motion_timestamp, finished));
                self.pending_motion_to_visible
                    .insert(event.crtc, motion_timestamp);
            }
        }
        if error.is_some_and(|error| error.raw_os_error() == Some(libc::EBUSY)) {
            self.ebusy = self.ebusy.saturating_add(1);
        }
    }

    fn record_legacy_move(&mut self, motion_timestamp: Instant, returned_at: Instant) {
        let latency = elapsed_us(motion_timestamp, returned_at);
        self.motion_to_accept_us.push(latency);
        self.motion_to_visible_us.push(latency);
    }

    fn record_cursor_retirement(&mut self, crtc: crtc::Handle, retired_at: Instant) {
        if let Some(motion_timestamp) = self.pending_motion_to_visible.remove(&crtc) {
            self.motion_to_visible_us
                .push(elapsed_us(motion_timestamp, retired_at));
        }
    }

    fn take_if_due(&mut self, now: Instant) -> Option<CommitWindowSnapshot> {
        if now.saturating_duration_since(self.started) < Duration::from_secs(1) {
            return None;
        }
        self.started = now;
        Some(CommitWindowSnapshot {
            ioctl_us: std::mem::replace(&mut self.ioctl_us, std::array::from_fn(|_| Vec::new())),
            ebusy: std::mem::take(&mut self.ebusy),
            motion_to_accept_us: std::mem::take(&mut self.motion_to_accept_us),
            motion_to_visible_us: std::mem::take(&mut self.motion_to_visible_us),
        })
    }
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn elapsed_us(started: Instant, finished: Instant) -> u64 {
    duration_us(finished.saturating_duration_since(started))
}

fn percentile(values: &[u64]) -> (u64, u64) {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let max = sorted.last().copied().unwrap_or(0);
    let p99_index = sorted
        .len()
        .saturating_mul(99)
        .div_ceil(100)
        .saturating_sub(1);
    (max, sorted.get(p99_index).copied().unwrap_or(0))
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod tests_support {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum ScriptedResult {
        Ok,
        Error {
            kind: io::ErrorKind,
            raw_os_error: Option<i32>,
        },
    }

    impl ScriptedResult {
        fn from_result(result: io::Result<()>) -> Self {
            match result {
                Ok(()) => Self::Ok,
                Err(error) => Self::Error {
                    kind: error.kind(),
                    raw_os_error: error.raw_os_error(),
                },
            }
        }

        fn into_result(self) -> io::Result<()> {
            match self {
                Self::Ok => Ok(()),
                Self::Error {
                    kind: _,
                    raw_os_error: Some(raw_os_error),
                } => Err(io::Error::from_raw_os_error(raw_os_error)),
                Self::Error {
                    kind,
                    raw_os_error: None,
                } => Err(io::Error::from(kind)),
            }
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) struct AtomicCommitRecord {
        pub(crate) flags: AtomicCommitFlags,
        pub(crate) kind: AtomicCommitKind,
        pub(crate) properties: Vec<AtomicPropertyUpdate>,
        pub(crate) events: Vec<AtomicCommitEvent>,
        pub(crate) result: ScriptedResult,
    }

    #[derive(Default)]
    pub(crate) struct RecordingAtomicCommitExecutor {
        records: RefCell<Vec<AtomicCommitRecord>>,
        scripted_results: RefCell<VecDeque<ScriptedResult>>,
    }

    impl RecordingAtomicCommitExecutor {
        pub(crate) fn records(&self) -> Vec<AtomicCommitRecord> {
            self.records.borrow().clone()
        }

        pub(crate) fn script_result(&self, result: io::Result<()>) {
            self.scripted_results
                .borrow_mut()
                .push_back(ScriptedResult::from_result(result));
        }
    }

    impl AtomicCommitExecutor for RecordingAtomicCommitExecutor {
        fn atomic_commit(
            &self,
            _device: &Device,
            flags: AtomicCommitFlags,
            kind: AtomicCommitKind,
            request: AtomicCommitRequest,
            events: &[AtomicCommitEvent],
        ) -> io::Result<()> {
            let result = self
                .scripted_results
                .borrow_mut()
                .pop_front()
                .unwrap_or(ScriptedResult::Ok);
            self.records.borrow_mut().push(AtomicCommitRecord {
                flags,
                kind,
                properties: request.properties,
                events: events.to_vec(),
                result,
            });
            result.into_result()
        }
    }
}
