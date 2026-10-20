//! A host that records: every lifecycle event as a line of `events.log`, every
//! delivered CPU frame as `<id>-<seq>.bgra` plus a line of `frames.log`
//! (`--headless --rootless --rootless-record DIR`, plan RL1 Task 9). It
//! answers at once on the link: setup done, delivered, ready, released.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Write as _},
    path::PathBuf,
    sync::Arc,
};

use ash::vk;
use yserver_core::rootless::{RootlessEvent, ToplevelId, ToplevelKind};

use super::host::{CpuFrame, FrameEvent, HandoffKind, RootlessHost};
use crate::{kms::vk::device::VkContext, surface::link::ShellLink};

pub(crate) struct RecordHost {
    link: Arc<ShellLink>,
    dir: Option<PathBuf>,
    events: Option<File>,
    frames: Option<File>,
    /// The X window and border of each live id, for the frame lines.
    windows: HashMap<ToplevelId, u32>,
    borders: HashMap<ToplevelId, u16>,
}

impl RecordHost {
    /// Record into `dir` (created), or only answer with `None`.
    ///
    /// # Errors
    ///
    /// The directory or its logs could not be created.
    pub(crate) fn new(link: Arc<ShellLink>, dir: Option<PathBuf>) -> io::Result<Self> {
        let (events, frames) = match &dir {
            Some(dir) => {
                fs::create_dir_all(dir)?;
                (
                    Some(File::create(dir.join("events.log"))?),
                    Some(File::create(dir.join("frames.log"))?),
                )
            }
            None => (None, None),
        };
        Ok(Self {
            link,
            dir,
            events,
            frames,
            windows: HashMap::new(),
            borders: HashMap::new(),
        })
    }

    fn log_event(&mut self, line: &str) {
        log::info!("rootless host: {line}");
        if let Some(events) = self.events.as_mut()
            && let Err(e) = writeln!(events, "{line}")
        {
            log::warn!("rootless record: events.log: {e}");
        }
    }

    fn write_frame(&mut self, frame: &CpuFrame) -> io::Result<()> {
        let (Some(dir), Some(frames)) = (&self.dir, self.frames.as_mut()) else {
            return Ok(());
        };
        let file = format!("{}-{}.bgra", frame.id.get(), frame.seq.0);
        fs::write(dir.join(&file), &frame.pixels)?;
        let window = self.windows.get(&frame.id).copied().unwrap_or(0);
        let bw = self.borders.get(&frame.id).copied().unwrap_or(0);
        writeln!(
            frames,
            "id={} window=0x{window:x} seq={} gen={} width={} height={} stride={} depth={} bw={bw} \
             damage={} file={file}",
            frame.id.get(),
            frame.seq.0,
            frame.backing_gen,
            frame.width,
            frame.height,
            frame.stride,
            frame.depth,
            frame.damage.len(),
        )
    }
}

impl RootlessHost for RecordHost {
    fn handoff(&self, _id: ToplevelId, _depth: u8) -> HandoffKind {
        HandoffKind::Cpu
    }

    fn create_surface(&mut self, _id: ToplevelId, _vk: &VkContext) -> io::Result<vk::SurfaceKHR> {
        Err(io::Error::other("the recording host takes CPU frames"))
    }

    fn submit_cpu(&mut self, frame: CpuFrame) {
        let (id, seq) = (frame.id, frame.seq);
        // A frame lost on disk is Dropped, so its damage stays unacked.
        if let Err(e) = self.write_frame(&frame) {
            log::warn!("rootless record: frame {seq:?}: {e}");
            self.link.post_frame(FrameEvent::Dropped { id, seq });
        } else {
            self.link.post_frame(FrameEvent::Delivered { id, seq });
        }
        self.link.post_frame(FrameEvent::Ready { id });
        self.link.post_frame(FrameEvent::Released {
            id,
            seq,
            pixels: frame.pixels,
        });
    }

    fn first_frame(&mut self, id: ToplevelId) {
        self.log_event(&format!("first-frame id={}", id.get()));
    }

    fn on_events(&mut self, events: &[RootlessEvent]) {
        for event in events {
            let line = match event {
                RootlessEvent::Mapped {
                    id,
                    window,
                    kind,
                    geometry: g,
                    depth,
                    ..
                } => {
                    self.windows.insert(*id, window.0);
                    self.borders.insert(*id, g.border);
                    self.link.post_frame(FrameEvent::SetupDone { id: *id });
                    let kind = match kind {
                        ToplevelKind::Managed => "managed".to_owned(),
                        ToplevelKind::Popup { parent } => {
                            format!("popup parent={}", parent.map_or(0, ToplevelId::get))
                        }
                    };
                    format!(
                        "mapped id={} window=0x{:x} kind={kind} x={} y={} width={} height={} bw={} depth={depth}",
                        id.get(),
                        window.0,
                        g.x,
                        g.y,
                        g.width,
                        g.height,
                        g.border
                    )
                }
                RootlessEvent::Unmapped { id } => {
                    self.windows.remove(id);
                    self.borders.remove(id);
                    format!("unmapped id={}", id.get())
                }
                RootlessEvent::Reshaped {
                    id,
                    bounding,
                    input,
                } => format!(
                    "reshaped id={} bounding={} input={}",
                    id.get(),
                    bounding.as_ref().map_or(-1, |r| r.len() as i64),
                    input.as_ref().map_or(-1, |r| r.len() as i64)
                ),
                RootlessEvent::Configured {
                    id, geometry: g, ..
                } => {
                    self.borders.insert(*id, g.border);
                    format!(
                        "configured id={} x={} y={} width={} height={} bw={}",
                        id.get(),
                        g.x,
                        g.y,
                        g.width,
                        g.height,
                        g.border
                    )
                }
                other => format!("{other:?}"),
            };
            self.log_event(&line);
        }
    }

    fn frame_wanted(&mut self, id: ToplevelId) {
        self.link.post_frame(FrameEvent::Ready { id });
    }
}
