//! The recording host behind `--rootless-record`: its logs and frame files.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use yserver_core::resources::ROOT_WINDOW;

use super::Fixture;
use crate::rootless::record::RecordHost;

const W: u32 = 0x0010_0001;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yserver-rootless-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The fields of the last `frames.log` line, once one exists.
fn last_frame(dir: &std::path::Path) -> Option<std::collections::HashMap<String, String>> {
    let log = std::fs::read_to_string(dir.join("frames.log")).ok()?;
    let line = log.lines().last()?;
    Some(
        line.split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    )
}

/// Each delivered frame is a file of the backing's bytes (oracle:
/// `get_image` of the backing) with a header line naming the X window; the
/// event log names each mapped lifetime.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn record_host_writes_frames_and_events() {
    let dir = temp_dir("frames");
    let Some(mut f) = Fixture::with_host_or_skip(|link| {
        Box::new(RecordHost::new(link, Some(dir.clone())).expect("record dir"))
    }) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 2);
    f.map(W);
    f.fill(W, 0x00ff_0000, (4, 4, 10, 10));
    let deadline = Instant::now() + Duration::from_secs(2);
    let frame = loop {
        f.tail();
        if let Some(frame) = last_frame(&dir) {
            break frame;
        }
        assert!(Instant::now() < deadline, "no frame recorded");
    };
    assert_eq!(frame["window"], format!("0x{W:x}"));
    assert_eq!(
        (frame["width"].as_str(), frame["height"].as_str()),
        ("44", "34")
    );
    assert_eq!(frame["bw"], "2");
    let bytes = std::fs::read(dir.join(&frame["file"])).expect("frame file");
    assert_eq!(bytes, f.full_readback(W));
    f.unmap(W);
    f.tail();
    let events = std::fs::read_to_string(dir.join("events.log")).expect("events.log");
    let mut lines = events.lines();
    assert!(
        lines
            .next()
            .is_some_and(|l| l.starts_with(&format!("mapped id=1 window=0x{W:x} kind=managed"))),
        "{events}"
    );
    assert!(events.lines().any(|l| l == "unmapped id=1"), "{events}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A frame the recorder fails to write is reported Dropped, not Delivered,
/// so its damage stays unacked and the presenter retries it (spec 4.5.1).
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_frame_that_fails_to_write_is_retried() {
    let dir = temp_dir("dropped");
    let Some(mut f) = Fixture::with_host_or_skip(|link| {
        Box::new(RecordHost::new(link, Some(dir.clone())).expect("record dir"))
    }) else {
        return;
    };
    // Every frame write fails while the directory is gone.
    std::fs::remove_dir_all(&dir).expect("remove record dir");
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.fill(W, 0x00ff_0000, (4, 4, 10, 10));
    let settle = Instant::now() + Duration::from_millis(300);
    while Instant::now() < settle {
        f.tail();
    }
    std::fs::create_dir_all(&dir).expect("recreate record dir");
    let deadline = Instant::now() + Duration::from_secs(2);
    let file = loop {
        f.tail();
        let written = std::fs::read_dir(&dir)
            .expect("record dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "bgra"));
        if let Some(file) = written {
            break file;
        }
        assert!(
            Instant::now() < deadline,
            "the lost frame was acked and never retried"
        );
    };
    assert_eq!(std::fs::read(file).expect("frame file"), f.full_readback(W));
    let _ = std::fs::remove_dir_all(&dir);
}
