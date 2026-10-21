//! Atomic full-state cursor-plane operations and lease retirement support.

use std::{io, rc::Rc};

use ::drm::control::AtomicCommitFlags;

use crate::{
    drm::atomic::{AtomicCommitEvent, AtomicCommitKind, AtomicCommitRequest},
    kms::{
        cursor_plane::{CursorShowError, sprites::SpriteBufferLease},
        render::platform::{
            CursorHideRecovery, CursorMoveOutcome, KmsCursorState, PlatformBackend,
            cursor_commit::{prepare_cursor_commit, request_for_cursor_commit},
            cursor_state::{CursorBinding, InFlightCursorCommit},
        },
    },
};

impl PlatformBackend {
    /// Release the distinct sprite users carried by one ownership transition.
    /// Several cursor records can refer to the same lease, but they represent
    /// one owner for a CRTC and must only decrement the pool once.
    pub(super) fn release_cursor_bindings_once(
        &mut self,
        device_idx: usize,
        bindings: impl IntoIterator<Item = Option<CursorBinding>>,
        owner: &str,
    ) {
        let mut leases: Vec<SpriteBufferLease> = Vec::new();
        for binding in bindings.into_iter().flatten() {
            if !leases.contains(&binding.lease) {
                leases.push(binding.lease);
            }
        }
        if let Some(plane) = self.devices[device_idx].cursor.plane.as_mut() {
            for lease in leases {
                if !plane.sprites_mut().release_user(lease) {
                    log::error!("render cursor: {owner} found no user for sprite lease");
                }
            }
        }
    }

    pub(super) fn atomic_show(
        &mut self,
        output_idx: usize,
        hot_x: u16,
        hot_y: u16,
        x: i32,
        y: i32,
    ) -> Result<(), CursorShowError> {
        let (device_idx, crtc, layout_x, layout_y) = self
            .cursor_output_route(output_idx)
            .map_err(CursorShowError::Unbound)?;
        let (cx, cy) =
            super::cursor::cursor_root_to_crtc_local(x, y, layout_x, layout_y, hot_x, hot_y);
        if !self.devices[device_idx].cursor.available_on(crtc) {
            return Err(CursorShowError::Unbound(io::Error::other(
                "cursor plane unavailable for output",
            )));
        }

        let (candidate, framebuffer, properties, width, height, prior, valid) = {
            let cursor = &mut self.devices[device_idx].cursor;
            if cursor.atomic_cursor_commits_in_flight.contains_key(&crtc)
                || cursor.cursor_retry_waits_for_flip.contains(&crtc)
            {
                cursor.cursor_retry_waits_for_flip.insert(crtc);
                return Err(deferred_show(cursor, crtc));
            }
            let Some(plane) = cursor.plane.as_ref() else {
                return Err(CursorShowError::Unbound(io::Error::other(
                    "cursor plane unavailable for output",
                )));
            };
            let Some(candidate) = plane.sprites().candidate_lease() else {
                return Err(deferred_show(cursor, crtc));
            };
            if !candidate.matches(candidate.version(), hot_x, hot_y) {
                return Err(deferred_show(cursor, crtc));
            }
            let Some(framebuffer) = plane.sprites().framebuffer_for_lease(candidate) else {
                return Err(deferred_show(cursor, crtc));
            };
            let Some(properties) = plane.properties_for(crtc) else {
                return Err(CursorShowError::Unbound(io::Error::other(
                    "atomic cursor plane properties unavailable",
                )));
            };
            let state = cursor.crtc_states.get(&crtc).cloned().unwrap_or_default();
            (
                candidate,
                framebuffer,
                properties,
                plane.width(),
                plane.height(),
                state.applied,
                state.valid,
            )
        };
        if !candidate.matches(candidate.version(), hot_x, hot_y) {
            return Err(deferred_show(&self.devices[device_idx].cursor, crtc));
        }
        let binding = CursorBinding {
            properties,
            lease: candidate,
            framebuffer,
            x: cx,
            y: cy,
            hot_x,
            hot_y,
        };
        let prepared =
            prepare_cursor_commit(properties, crtc, prior, valid, Some(binding), width, height);
        if prepared.updates.is_empty() {
            self.cache_atomic_show_position(device_idx, crtc, cx, cy);
            return Ok(());
        }

        let reservation_taken = prepared.prior.is_none_or(|old| old.lease != candidate);
        if reservation_taken
            && !self.devices[device_idx]
                .cursor
                .plane
                .as_mut()
                .is_some_and(|plane| plane.sprites_mut().reserve(candidate))
        {
            return Err(CursorShowError::Unbound(io::Error::other(
                "cursor sprite lease became stale before submit",
            )));
        }

        let device = Rc::clone(&self.devices[device_idx].device);
        let sink = Rc::clone(&self.devices[device_idx].atomic_commit_sink);
        let request = request_for_cursor_commit(&prepared);
        let submit = sink.submit_with_commit_id(
            &device,
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            AtomicCommitKind::CursorOnly,
            request,
            &[AtomicCommitEvent {
                crtc,
                motion_timestamp: None,
            }],
        );
        let commit_id = match submit {
            Ok(commit_id) => commit_id,
            Err(error) => {
                if reservation_taken
                    && let Some(plane) = self.devices[device_idx].cursor.plane.as_mut()
                {
                    plane.sprites_mut().reject_reserved(candidate);
                }
                let _ = sink.take_last_cursor_rejection();
                if error.raw_os_error() == Some(libc::EBUSY) {
                    self.devices[device_idx]
                        .cursor
                        .cursor_retry_waits_for_flip
                        .insert(crtc);
                    return Err(deferred_show(&self.devices[device_idx].cursor, crtc));
                }
                let rollback = self.atomic_rollback(device_idx, crtc, &error, true);
                return Err(match rollback {
                    Ok(()) => CursorShowError::Unbound(error),
                    Err(rollback_error) => CursorShowError::StillVisible {
                        operation_error: error,
                        rollback_error: Some(rollback_error),
                    },
                });
            }
        };

        if reservation_taken {
            let can_accept = self.devices[device_idx]
                .cursor
                .plane
                .as_ref()
                .is_some_and(|plane| plane.sprites().can_accept_reserved(candidate));
            let accepted = can_accept
                && self.devices[device_idx]
                    .cursor
                    .plane
                    .as_mut()
                    .is_some_and(|plane| plane.sprites_mut().accept_reserved(candidate));
            if !accepted {
                log::error!(
                    "render cursor: accepted commit {commit_id} lost its reserved sprite lease on CRTC {crtc:?}"
                );
                let state = self.devices[device_idx]
                    .cursor
                    .crtc_states
                    .entry(crtc)
                    .or_default();
                state.valid = false;
                self.devices[device_idx]
                    .cursor
                    .atomic_cursor_commits_in_flight
                    .insert(
                        crtc,
                        InFlightCursorCommit {
                            commit_id: Some(commit_id),
                            prepared: Some(prepared),
                        },
                    );
                return Err(CursorShowError::Unbound(io::Error::other(
                    "accepted cursor commit lost its reserved sprite lease",
                )));
            }
        }

        let cursor = &mut self.devices[device_idx].cursor;
        let state = cursor.crtc_states.entry(crtc).or_default();
        state.applied = Some(binding);
        state.valid = true;
        state.resume_visible = false;
        cursor.atomic_cursor_commits_in_flight.insert(
            crtc,
            InFlightCursorCommit {
                commit_id: Some(commit_id),
                prepared: Some(prepared),
            },
        );
        cursor.cursor_retry_waits_for_flip.remove(&crtc);
        cursor.note_cursor_success(crtc);
        self.cache_atomic_show_position(device_idx, crtc, cx, cy);
        Ok(())
    }

    pub(super) fn atomic_hide(&mut self, output_idx: usize) -> io::Result<()> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx)?;
        let (properties, prior, valid) = {
            let cursor = &self.devices[device_idx].cursor;
            let state = cursor.crtc_states.get(&crtc).cloned().unwrap_or_default();
            if cursor.atomic_cursor_commits_in_flight.contains_key(&crtc) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "cursor hide waits for the prior CRTC event",
                ));
            }
            if state.applied.is_none() && state.valid {
                return Ok(());
            }
            if cursor.permanently_disabled && state.applied.is_none() {
                return Ok(());
            }
            let properties = cursor
                .plane
                .as_ref()
                .and_then(|plane| plane.properties_for(crtc))
                .ok_or_else(|| io::Error::other("atomic cursor plane unavailable"))?;
            (properties, state.applied, state.valid)
        };
        let prepared = prepare_cursor_commit(properties, crtc, prior, valid, None, 0, 0);
        if prepared.updates.is_empty() {
            return Ok(());
        }

        let device = Rc::clone(&self.devices[device_idx].device);
        let sink = Rc::clone(&self.devices[device_idx].atomic_commit_sink);
        let request = request_for_cursor_commit(&prepared);
        let submit = sink.submit_with_commit_id(
            &device,
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            AtomicCommitKind::CursorOnly,
            request,
            &[AtomicCommitEvent {
                crtc,
                motion_timestamp: None,
            }],
        );
        let commit_id = match submit {
            Ok(commit_id) => commit_id,
            Err(error) => {
                let _ = sink.take_last_cursor_rejection();
                if error.raw_os_error() == Some(libc::EBUSY) {
                    self.devices[device_idx]
                        .cursor
                        .cursor_retry_waits_for_flip
                        .insert(crtc);
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "cursor hide waits for the prior CRTC event",
                    ));
                }
                let _ = self.atomic_rollback(device_idx, crtc, &error, true);
                return Err(error);
            }
        };

        let cursor = &mut self.devices[device_idx].cursor;
        let state = cursor.crtc_states.entry(crtc).or_default();
        state.applied = None;
        state.valid = true;
        cursor.atomic_cursor_commits_in_flight.insert(
            crtc,
            InFlightCursorCommit {
                commit_id: Some(commit_id),
                prepared: Some(prepared),
            },
        );
        cursor.cursor_retry_waits_for_flip.remove(&crtc);
        cursor.note_cursor_success(crtc);
        cursor.clear_atomic_position(crtc);
        Ok(())
    }

    pub(super) fn detach_cursor_blocking(
        &mut self,
        device_idx: usize,
        crtc: ::drm::control::crtc::Handle,
    ) -> io::Result<()> {
        let (properties, applied, flight_prior, had_binding) = {
            let cursor = &self.devices[device_idx].cursor;
            let state = cursor.crtc_states.get(&crtc).cloned().unwrap_or_default();
            let flight_prior = cursor
                .atomic_cursor_commits_in_flight
                .get(&crtc)
                .and_then(|commit| commit.prepared.as_ref())
                .and_then(|prepared| prepared.prior);
            let selected = state.applied.or(flight_prior);
            let properties = if let Some(binding) = selected {
                Some(binding.properties)
            } else if !state.valid {
                cursor
                    .plane
                    .as_ref()
                    .and_then(|plane| plane.properties_for(crtc))
            } else {
                None
            };
            let Some(properties) = properties else {
                return Ok(());
            };
            (
                properties,
                state.applied,
                flight_prior,
                state.applied.is_some() || flight_prior.is_some(),
            )
        };

        let mut request = AtomicCommitRequest::new();
        request.add_cursor_property(properties.plane.into(), properties.fb_id, 0);
        request.add_cursor_property(properties.plane.into(), properties.crtc_id, 0);
        let device = Rc::clone(&self.devices[device_idx].device);
        let sink = Rc::clone(&self.devices[device_idx].atomic_commit_sink);
        let result = sink.submit(
            &device,
            AtomicCommitFlags::empty(),
            AtomicCommitKind::CursorOnly,
            request,
            &[],
        );
        let _ = sink.take_last_cursor_rejection();
        if let Err(error) = result {
            self.devices[device_idx]
                .cursor
                .crtc_states
                .entry(crtc)
                .or_default()
                .valid = false;
            return Err(error);
        }

        self.release_cursor_bindings_once(device_idx, [applied, flight_prior], "blocking detach");
        if let Some(commit) = self.devices[device_idx]
            .cursor
            .atomic_cursor_commits_in_flight
            .get_mut(&crtc)
            && let Some(prepared) = commit.prepared.as_mut()
        {
            prepared.prior = None;
        }
        let cursor = &mut self.devices[device_idx].cursor;
        let state = cursor.crtc_states.entry(crtc).or_default();
        state.applied = None;
        state.valid = true;
        if had_binding {
            state.resume_visible = true;
        }
        cursor.clear_atomic_position(crtc);
        Ok(())
    }

    pub(super) fn atomic_rollback(
        &mut self,
        device_idx: usize,
        crtc: ::drm::control::crtc::Handle,
        error: &io::Error,
        defer_outcome: bool,
    ) -> io::Result<()> {
        let device_key = self.devices[device_idx].key;
        let had_applied = self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| state.applied.is_some());
        {
            let cursor = &mut self.devices[device_idx].cursor;
            cursor.clear_atomic_position(crtc);
            cursor.cursor_retry_waits_for_flip.remove(&crtc);
        }
        let disposition = super::cursor::apply_cursor_operation_error(
            &mut self.devices[device_idx].cursor,
            crtc,
            error,
        );
        let rollback = if had_applied {
            self.detach_cursor_blocking(device_idx, crtc)
        } else {
            Ok(())
        };
        let mut outcome = CursorMoveOutcome::default();
        match &rollback {
            Ok(()) => {
                let cursor = &mut self.devices[device_idx].cursor;
                cursor.cursor_hide_recovery.remove(&crtc);
                if disposition == crate::kms::render::platform::CursorFailureDisposition::Unchanged
                {
                    cursor.note_einval(crtc);
                }
                outcome.fallback_changed = true;
                log::warn!(
                    "render cursor atomic commit: device {device_key} CRTC {crtc:?} rejected cursor fields ({error}); detached the plane and entered transient software fallback"
                );
            }
            Err(hide_error) => {
                self.devices[device_idx]
                    .cursor
                    .cursor_hide_recovery
                    .insert(crtc, CursorHideRecovery::default());
                outcome.retry_required = true;
                log::warn!(
                    "render cursor atomic commit: device {device_key} CRTC {crtc:?} rejected cursor fields ({error}); atomic detach failed ({hide_error}), retaining hardware ownership for motion recovery"
                );
            }
        }
        if defer_outcome {
            self.deferred_cursor_move_outcome.merge(outcome);
        }
        rollback
    }

    fn cache_atomic_show_position(
        &mut self,
        device_idx: usize,
        crtc: ::drm::control::crtc::Handle,
        x: i32,
        y: i32,
    ) {
        let device_key = self.devices[device_idx].key;
        let cursor = &mut self.devices[device_idx].cursor;
        if let Some((plane, crtc_x, crtc_y)) = cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.atomic_position_properties(crtc))
        {
            let position = crate::drm::atomic::CursorPlanePosition {
                device_key,
                crtc,
                plane,
                crtc_x,
                crtc_y,
                x,
                y,
                motion_timestamp: None,
            };
            cursor.atomic_positions.insert(crtc, position);
        }
        cursor.atomic_pending_moves.remove(&crtc);
    }
}

fn deferred_show(cursor: &KmsCursorState, crtc: ::drm::control::crtc::Handle) -> CursorShowError {
    CursorShowError::Deferred {
        remains_visible: cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| state.applied.is_some()),
        reason: io::Error::new(
            io::ErrorKind::WouldBlock,
            "cursor show waits for a prepared image or prior CRTC event",
        ),
    }
}
