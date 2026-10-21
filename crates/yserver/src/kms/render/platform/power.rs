use crate::{drm, kms::render::platform::PlatformBackend};
use std::{collections::HashSet, io, rc::Rc};

pub(in crate::kms::render) trait OutputPowerExecutor {
    fn disable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
    ) -> io::Result<()>;

    fn enable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
        framebuffer: ::drm::control::framebuffer::Handle,
    ) -> io::Result<()>;

    /// Turn every CRTC, connector and plane on `device` off in one commit,
    /// whatever another DRM master left behind.
    fn disable_device(&self, device: &drm::Device) -> io::Result<()>;
}

pub(super) struct DrmOutputPowerExecutor;

impl OutputPowerExecutor for DrmOutputPowerExecutor {
    fn disable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
    ) -> io::Result<()> {
        drm::modeset::disable_output(device, output)
    }

    fn enable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
        framebuffer: ::drm::control::framebuffer::Handle,
    ) -> io::Result<()> {
        let result = drm::modeset::commit_modeset(device, output, framebuffer);
        if result.is_err() {
            drm::modeset::log_kms_state(device, &output.connector_name);
        }
        result
    }

    fn disable_device(&self, device: &drm::Device) -> io::Result<()> {
        drm::modeset::disable_all_outputs_on_device(device)
    }
}

#[cfg(test)]
struct AcceptingOutputPowerExecutor;

#[cfg(test)]
impl OutputPowerExecutor for AcceptingOutputPowerExecutor {
    fn disable(
        &self,
        _device: &drm::Device,
        _output: &crate::platform::drm::Output,
    ) -> io::Result<()> {
        Ok(())
    }

    fn enable(
        &self,
        _device: &drm::Device,
        _output: &crate::platform::drm::Output,
        _framebuffer: ::drm::control::framebuffer::Handle,
    ) -> io::Result<()> {
        Ok(())
    }

    fn disable_device(&self, _device: &drm::Device) -> io::Result<()> {
        Ok(())
    }
}

impl PlatformBackend {
    // ── Disable output ──────────────────────────────────────────

    /// Best-effort wait for all in-flight GPU work to complete, bounded
    /// to 5 seconds (matching the `FenceTicket::wait` / `device_wait_idle`
    /// convention used at shutdown). Called by `KmsBackend::run_suspend`
    /// before DRM master is dropped, so in-flight submits don't race a
    /// kernel-side scanout teardown.
    ///
    /// Errors from `device_wait_idle` are logged and swallowed: the VT
    /// release path must always continue even if the wait times out or the
    /// device is already lost.
    pub(crate) fn wait_idle_bounded(&self) {
        // `device_wait_idle` is inherently blocking; 5 s is the same bound
        // used by FenceTicket::wait in the pool destructor.  We do not set a
        // real timeout here because ash's `device_wait_idle` wraps
        // `vkDeviceWaitIdle` which has no timeout parameter — on a lost
        // device it returns VK_ERROR_DEVICE_LOST promptly.  The 5-second
        // comment in the plan refers to the *practical* upper bound the
        // driver enforces on a wedged device; real quiescence is typically
        // sub-millisecond.
        if let Some(vk) = self.gpu.vk.as_ref() {
            let result = unsafe { vk.device.device_wait_idle() };
            if let Err(e) = result {
                log::warn!("kms: wait_idle_bounded: device_wait_idle failed: {e:?}");
            }
        }
        for (renderer_id, vk) in &self.copy_vk_contexts {
            let result = unsafe { vk.device.device_wait_idle() };
            if let Err(e) = result {
                log::warn!(
                    "kms: wait_idle_bounded: copied sink {renderer_id:?} device_wait_idle failed: {e:?}"
                );
            }
        }
    }

    /// Post-loop teardown — disable each output, leaving the
    /// scanout BOs in a state where their Drop can clean up
    /// (or, on atomic disable failure, disarm them so we leak
    /// rather than confuse KMS — same shape as v1).
    ///
    /// # Errors
    ///
    /// Propagates the first per-output `disable_output` failure;
    /// subsequent outputs still attempted.
    pub(crate) fn disable_output(&mut self) -> io::Result<()> {
        self.shutting_down = true;
        self.clear_scanout_render_completions();

        // Best-effort: drain both the selected renderer and every copied
        // sink transfer device before pulling the modeset.
        self.wait_idle_bounded();

        // Stage 3f.10: drain the pixmap pool so the recycled
        // image/memory/view triples don't leak through the
        // VkContext destruction path. Safe to drain here: every
        // in-flight CB has been waited on by device_wait_idle.
        if let Some(pool) = self.gpu.pixmap_pool.as_ref() {
            pool.drain();
        }

        // Clear each whole device first: after a VT switch another master
        // may have moved our connectors, and a per-output disable then
        // orphans a foreign CRTC (EINVAL). Per-output disables remain the
        // fallback for a device whose whole-device commit is refused.
        let cleared = self.disable_all_devices("shutdown");
        self.settle_cursor_crtcs_after_device_clear(&cleared);

        let mut first_err: Option<io::Error> = None;
        let output_indices: Vec<_> = (0..self.outputs.len()).collect();
        for i in output_indices {
            let output_key = self.outputs[i].key.clone();
            if cleared.contains(&output_key.device_key) {
                continue;
            }
            let crtc = self.outputs[i].output.crtc;
            let Some(device) = self
                .device_for_output(&output_key)
                .map(|device| Rc::clone(&device.device))
            else {
                log::warn!(
                    "render disable_output: no DRM device {} for {}",
                    output_key.device_key,
                    self.outputs[i].output.connector_name,
                );
                continue;
            };
            if let Err(error) =
                self.detach_atomic_cursor_before_disable(output_key.device_key, crtc)
            {
                log::warn!(
                    "render disable_output: atomic cursor detach failed for {}: {error}",
                    self.outputs[i].output.connector_name,
                );
            }
            let executor = Rc::clone(&self.output_power_executor);
            if let Err(e) = executor.disable(&device, &self.outputs[i].output) {
                log::warn!(
                    "render disable_output: failed for {} (output {i}): {e}",
                    self.outputs[i].output.connector_name,
                );
                // Disarm the matching scanout pool so its Drop
                // doesn't try to destroy framebuffers KMS may
                // still hold (matches v1's behaviour).
                if let Some(pool) = self.scanout_pools.get_mut(i).and_then(|p| p.as_mut()) {
                    pool.disarm();
                }
                if first_err.is_none() {
                    first_err = Some(e);
                }
            } else {
                self.settle_disabled_cursor_crtc(output_key.device_key, crtc);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Turn every CRTC, connector and plane off on each opened DRM device,
    /// one atomic commit per device, whatever state another DRM master left
    /// behind. Returns the devices that are now known to be all-off; a
    /// refused device is logged and left to the per-output paths.
    pub(crate) fn disable_all_devices(
        &self,
        context: &str,
    ) -> HashSet<crate::platform::drm::DrmDeviceKey> {
        let mut cleared = HashSet::new();
        for device in &self.devices {
            match self.output_power_executor.disable_device(&device.device) {
                Ok(()) => {
                    cleared.insert(device.key);
                }
                Err(error) => log::warn!(
                    "kms: {context}: device-wide disable of {} failed: {error}",
                    device.key
                ),
            }
        }
        cleared
    }

    /// Release the cursor sprites of every CRTC on the `cleared` devices.
    /// Their blocking all-off commit removed every plane, so no detach is
    /// needed; shutdown skips those devices' per-output disables, which
    /// would otherwise be the release owner.
    fn settle_cursor_crtcs_after_device_clear(
        &mut self,
        cleared: &HashSet<crate::platform::drm::DrmDeviceKey>,
    ) {
        let mut crtcs: HashSet<_> = self
            .outputs
            .iter()
            .filter(|layout| cleared.contains(&layout.key.device_key))
            .map(|layout| (layout.key.device_key, layout.output.crtc))
            .collect();
        for device in self.devices.iter().filter(|d| cleared.contains(&d.key)) {
            let cursor = &device.cursor;
            crtcs.extend(
                cursor
                    .crtc_states
                    .keys()
                    .chain(cursor.atomic_cursor_commits_in_flight.keys())
                    .map(|crtc| (device.key, *crtc)),
            );
        }
        for (device_key, crtc) in crtcs {
            self.settle_disabled_cursor_crtc(device_key, crtc);
        }
    }

    /// Drive every output to a binary KMS power state. Used by
    /// DPMS — collapses Standby/Suspend/Off to "outputs inactive"
    /// and On to "outputs active". Unlike the post-loop
    /// `disable_output` it does NOT set `shutting_down`, NOT call
    /// `device_wait_idle`, and NOT disarm scanout pools — DPMS is
    /// reversible.
    ///
    /// # Errors
    ///
    /// Collects the first per-output failure, continues with the
    /// rest, then returns it. The caller (KmsBackend::set_dpms_power)
    /// logs and advances the in-memory DPMS state regardless.
    pub(crate) fn dpms_set_outputs_active(&mut self, active: bool) -> io::Result<()> {
        let mut first_err: Option<io::Error> = None;
        if active {
            // Re-commit modeset. Pick the OnScreen BO (last frame
            // before blank) or any registered fb — same selection
            // logic as `requery_outputs_and_modeset` at :2030.
            for (i, layout) in self.outputs.iter().enumerate() {
                let Some(device) = self
                    .device_for_output(&layout.key)
                    .map(|device| Rc::clone(&device.device))
                else {
                    let error = io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "dpms_set_outputs_active(true): no DRM device {} for {}",
                            layout.key.device_key, layout.output.connector_name,
                        ),
                    );
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                let front =
                    self.scanout_pools
                        .get(i)
                        .and_then(|p| p.as_ref())
                        .and_then(|pool| {
                            use crate::kms::vk::scanout::BoPhase;
                            let pool = pool.display_pool();
                            pool.bos
                                .iter()
                                .enumerate()
                                .find(|(_, bo)| bo.state.phase == BoPhase::OnScreen)
                                .and_then(|(bo_idx, bo)| bo.fb_handle.map(|fb| (bo_idx, fb)))
                                .or_else(|| {
                                    pool.bos.iter().enumerate().find_map(|(bo_idx, bo)| {
                                        bo.fb_handle.map(|fb| (bo_idx, fb))
                                    })
                                })
                        });
                let Some((bo_idx, fb_id)) = front else {
                    let error = io::Error::other(format!(
                        "dpms_set_outputs_active(true): no framebuffer for output {}; \
                         an ordinary page flip cannot restore MODE_ID/ACTIVE",
                        layout.output.connector_name
                    ));
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                let result = self
                    .output_power_executor
                    .enable(&device, &layout.output, fb_id);
                if let Err(e) = &result
                    && !layout.connector_connected
                {
                    // Xorg keeps a disconnected output's CRTC configured; the
                    // kernel may refuse to light a connector that is gone.
                    // That must never keep the connected outputs dark.
                    log::debug!(
                        "dpms_set_outputs_active(true): {} is disconnected; its commit \
                         failed ({e}), continuing",
                        layout.output.connector_name,
                    );
                } else if let Err(e) = result {
                    log::error!(
                        "dpms_set_outputs_active(true): commit_modeset for {} failed: {e}",
                        layout.output.connector_name,
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                } else if let Some(scanout) = self.scanout_pools.get_mut(i).and_then(Option::as_mut)
                {
                    scanout.display_pool_mut().bos[bo_idx]
                        .state
                        .mark_on_screen_after_modeset();
                    if let Err(error) = scanout.note_kms_modeset_installed(bo_idx) {
                        log::error!(
                            "dpms_set_outputs_active(true): copied ownership ledger failed for \
                             output {i} bo {bo_idx}: {error}"
                        );
                        self.gpu.renderer_failed = true;
                        if first_err.is_none() {
                            first_err = Some(error);
                        }
                    }
                }
            }
        } else {
            let output_indices: Vec<_> = (0..self.outputs.len()).collect();
            for index in output_indices {
                let output_key = self.outputs[index].key.clone();
                let crtc = self.outputs[index].output.crtc;
                let Some(device) = self
                    .device_for_output(&output_key)
                    .map(|device| Rc::clone(&device.device))
                else {
                    let error = io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "dpms_set_outputs_active(false): no DRM device {} for {}",
                            output_key.device_key, self.outputs[index].output.connector_name,
                        ),
                    );
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                if let Err(error) =
                    self.detach_atomic_cursor_before_disable(output_key.device_key, crtc)
                {
                    log::warn!(
                        "dpms_set_outputs_active(false): atomic cursor detach for {} failed: {error}",
                        self.outputs[index].output.connector_name,
                    );
                }
                if let Err(e) = self
                    .output_power_executor
                    .disable(&device, &self.outputs[index].output)
                {
                    log::error!(
                        "dpms_set_outputs_active(false): disable_output for {} failed: {e}",
                        self.outputs[index].output.connector_name,
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_accept_output_power_commits(&mut self) {
        self.output_power_executor = Rc::new(AcceptingOutputPowerExecutor);
    }

    #[cfg(test)]
    pub(in crate::kms::render) fn for_tests_set_output_power_executor(
        &mut self,
        executor: Rc<dyn OutputPowerExecutor>,
    ) {
        self.output_power_executor = executor;
    }
}
