use super::require_crtc_in_vblank_event;
use std::io;

#[test]
// Mutation: continue when the capability is not one, or discard the query error context/cause.
fn crtc_event_capability_requires_one_and_preserves_query_errors() {
    assert!(require_crtc_in_vblank_event("/dev/dri/card7", Ok(1)).is_ok());

    for value in [0, 2, u64::MAX] {
        let error = require_crtc_in_vblank_event("/dev/dri/card7", Ok(value))
            .expect_err("unsupported capability value must reject setup");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(message.contains("/dev/dri/card7"));
        assert!(message.contains("DRM_CAP_CRTC_IN_VBLANK_EVENT"));
    }

    let error = require_crtc_in_vblank_event(
        "/dev/dri/card7",
        Err(io::Error::from_raw_os_error(libc::EACCES)),
    )
    .expect_err("query failure must reject setup");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("/dev/dri/card7"));
    assert!(error.to_string().contains("DRM_CAP_CRTC_IN_VBLANK_EVENT"));
    let cause = error
        .get_ref()
        .and_then(|context| context.source())
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("query error remains in the source chain");
    assert_eq!(cause.raw_os_error(), Some(libc::EACCES));
}
