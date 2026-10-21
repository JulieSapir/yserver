//! DRM cursor-plane discovery and the static cursor-state path decision.

use std::collections::{HashMap, HashSet};

use drm::control::{Device as ControlDevice, PlaneType, crtc, plane, property};

use crate::drm::{CursorDeviceCaps, Device};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiscoveredCursorPlane {
    pub(super) handle: plane::Handle,
    pub(super) crtcs: HashSet<crtc::Handle>,
    pub(super) properties: HashMap<String, property::Handle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorPlaneDiscovery {
    pub(super) crtcs: Vec<crtc::Handle>,
    pub(super) planes: Vec<DiscoveredCursorPlane>,
}

pub(crate) trait CursorPlaneDiscoveryIo {
    fn discover(&self, device: &Device) -> std::io::Result<CursorPlaneDiscovery>;
}

pub(super) struct DrmCursorPlaneDiscovery;

impl CursorPlaneDiscoveryIo for DrmCursorPlaneDiscovery {
    fn discover(&self, device: &Device) -> std::io::Result<CursorPlaneDiscovery> {
        discover_cursor_planes(device)
    }
}

fn discover_cursor_planes(device: &Device) -> std::io::Result<CursorPlaneDiscovery> {
    let resources = device.resource_handles()?;
    let mut cursor_planes = Vec::new();
    for handle in device.plane_handles()? {
        let info = device.get_plane(handle)?;
        let props = device.get_properties(handle)?;
        let map = props.as_hashmap(device)?;
        let Some(type_info) = map.get("type") else {
            continue;
        };
        let raw = props
            .iter()
            .find(|(property, _)| **property == type_info.handle())
            .map(|(_, value)| *value)
            .unwrap_or(0);
        if raw != PlaneType::Cursor as u64 {
            continue;
        }
        cursor_planes.push(DiscoveredCursorPlane {
            handle,
            crtcs: resources
                .filter_crtcs(info.possible_crtcs())
                .into_iter()
                .collect(),
            properties: map
                .into_iter()
                .map(|(name, property)| (name, property.handle()))
                .collect(),
        });
    }
    Ok(CursorPlaneDiscovery {
        crtcs: resources.crtcs().to_vec(),
        planes: cursor_planes,
    })
}

/// Atomic state needed to bind, position, and size one assigned cursor plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorPlaneProperties {
    pub(crate) plane: plane::Handle,
    pub(crate) fb_id: property::Handle,
    pub(crate) crtc_id: property::Handle,
    pub(crate) crtc_x: property::Handle,
    pub(crate) crtc_y: property::Handle,
    pub(crate) crtc_w: property::Handle,
    pub(crate) crtc_h: property::Handle,
    pub(crate) src_x: property::Handle,
    pub(crate) src_y: property::Handle,
    pub(crate) src_w: property::Handle,
    pub(crate) src_h: property::Handle,
    pub(crate) hotspot_x: Option<property::Handle>,
    pub(crate) hotspot_y: Option<property::Handle>,
}

impl DiscoveredCursorPlane {
    pub(super) fn cursor_properties(&self) -> Option<CursorPlaneProperties> {
        let get = |name: &str| self.properties.get(name).copied();
        Some(CursorPlaneProperties {
            plane: self.handle,
            fb_id: get("FB_ID")?,
            crtc_id: get("CRTC_ID")?,
            crtc_x: get("CRTC_X")?,
            crtc_y: get("CRTC_Y")?,
            crtc_w: get("CRTC_W")?,
            crtc_h: get("CRTC_H")?,
            src_x: get("SRC_X")?,
            src_y: get("SRC_Y")?,
            src_w: get("SRC_W")?,
            src_h: get("SRC_H")?,
            hotspot_x: get("HOTSPOT_X"),
            hotspot_y: get("HOTSPOT_Y"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorStatePath {
    Atomic,
    Legacy,
}

impl std::fmt::Display for CursorStatePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Atomic => f.write_str("atomic"),
            Self::Legacy => f.write_str("legacy"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorPathDecision {
    pub(crate) path: CursorStatePath,
    pub(crate) reason: CursorPathReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorPathReason {
    NoCursorPlane,
    NoDeviceCaps,
    MissingProperty(&'static str),
    HotspotSupportMissing,
    Qualified,
}

impl std::fmt::Display for CursorPathReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCursorPlane => f.write_str("no-cursor-plane"),
            Self::NoDeviceCaps => f.write_str("no-device-caps"),
            Self::MissingProperty(name) => write!(f, "missing-property:{name}"),
            Self::HotspotSupportMissing => f.write_str("hotspot-support-missing"),
            Self::Qualified => f.write_str("qualified"),
        }
    }
}

const REQUIRED_PROPERTIES: [&str; 10] = [
    "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W", "SRC_H",
];

pub(super) fn decide_cursor_state_path(
    assignment: &HashMap<crtc::Handle, plane::Handle>,
    planes: &[DiscoveredCursorPlane],
    caps: Option<&CursorDeviceCaps>,
) -> CursorPathDecision {
    let legacy = |reason| CursorPathDecision {
        path: CursorStatePath::Legacy,
        reason,
    };
    if assignment.is_empty() {
        return legacy(CursorPathReason::NoCursorPlane);
    }
    let Some(caps) = caps else {
        return legacy(CursorPathReason::NoDeviceCaps);
    };

    for discovered in planes {
        if !assignment
            .values()
            .any(|handle| *handle == discovered.handle)
        {
            continue;
        }
        for name in REQUIRED_PROPERTIES {
            if !discovered.properties.contains_key(name) {
                return legacy(CursorPathReason::MissingProperty(name));
            }
        }
    }

    if caps.requires_hotspot_support()
        && (!caps.cursor_plane_hotspot_cap_accepted()
            || assignment.values().any(|handle| {
                planes
                    .iter()
                    .find(|discovered| discovered.handle == *handle)
                    .is_none_or(|discovered| {
                        !discovered.properties.contains_key("HOTSPOT_X")
                            || !discovered.properties.contains_key("HOTSPOT_Y")
                    })
            }))
    {
        return legacy(CursorPathReason::HotspotSupportMissing);
    }

    CursorPathDecision {
        path: CursorStatePath::Atomic,
        reason: CursorPathReason::Qualified,
    }
}

#[cfg(test)]
pub(crate) struct CursorPlaneTestPlane {
    pub(crate) handle: u32,
    pub(crate) possible_crtcs: Vec<u32>,
    pub(crate) property_names: Vec<String>,
}

#[cfg(test)]
pub(crate) struct CursorPlaneTestSnapshot {
    pub(crate) crtcs: Vec<u32>,
    pub(crate) planes: Vec<CursorPlaneTestPlane>,
    pub(crate) cursor_caps: Option<CursorDeviceCaps>,
    pub(crate) has_buffer: bool,
}

#[cfg(test)]
impl CursorPlaneTestSnapshot {
    pub(crate) fn into_discovery(self) -> (CursorPlaneDiscovery, Option<CursorDeviceCaps>, bool) {
        let discovery = CursorPlaneDiscovery {
            crtcs: self
                .crtcs
                .into_iter()
                .map(|raw| ::drm::control::from_u32(raw).expect("test CRTC handle"))
                .collect(),
            planes: self
                .planes
                .into_iter()
                .map(|plane| DiscoveredCursorPlane {
                    handle: ::drm::control::from_u32(plane.handle)
                        .expect("test cursor plane handle"),
                    crtcs: plane
                        .possible_crtcs
                        .into_iter()
                        .map(|raw| ::drm::control::from_u32(raw).expect("test CRTC handle"))
                        .collect(),
                    properties: plane
                        .property_names
                        .into_iter()
                        .enumerate()
                        .map(|(index, name)| {
                            (
                                name,
                                ::drm::control::from_u32(0x9000 + index as u32)
                                    .expect("test cursor property handle"),
                            )
                        })
                        .collect(),
                })
                .collect(),
        };
        (discovery, self.cursor_caps, self.has_buffer)
    }
}

#[cfg(test)]
pub(super) struct RecordingCursorPlaneDiscovery {
    discovery: CursorPlaneDiscovery,
}

#[cfg(test)]
impl RecordingCursorPlaneDiscovery {
    pub(super) fn new(discovery: CursorPlaneDiscovery) -> Self {
        Self { discovery }
    }
}

#[cfg(test)]
impl CursorPlaneDiscoveryIo for RecordingCursorPlaneDiscovery {
    fn discover(&self, _device: &Device) -> std::io::Result<CursorPlaneDiscovery> {
        Ok(self.discovery.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: [&str; 10] = [
        "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W",
        "SRC_H",
    ];
    const HOTSPOT: [&str; 2] = ["HOTSPOT_X", "HOTSPOT_Y"];

    fn raw<H: ::drm::control::ResourceHandle>(value: u32) -> H {
        ::drm::control::from_u32(value).expect("nonzero test handle")
    }

    fn discovered(handle: u32, names: &[&str]) -> DiscoveredCursorPlane {
        DiscoveredCursorPlane {
            handle: raw(handle),
            crtcs: HashSet::from([raw(11)]),
            properties: names
                .iter()
                .enumerate()
                .map(|(index, name)| ((*name).to_owned(), raw(0x9000 + index as u32)))
                .collect(),
        }
    }

    fn decide(
        plane: Option<&DiscoveredCursorPlane>,
        caps: Option<&CursorDeviceCaps>,
    ) -> CursorPathDecision {
        let assignment = plane
            .map(|plane| HashMap::from([(raw(11), plane.handle)]))
            .unwrap_or_default();
        let planes: Vec<_> = plane.cloned().into_iter().collect();
        decide_cursor_state_path(&assignment, &planes, caps)
    }

    fn caps(driver_name: Option<&str>, accepted: bool) -> CursorDeviceCaps {
        CursorDeviceCaps::new(accepted, driver_name.map(str::to_owned))
    }

    fn assert_decision(
        actual: CursorPathDecision,
        path: CursorStatePath,
        reason: CursorPathReason,
    ) {
        assert_eq!(actual, CursorPathDecision { path, reason });
    }

    #[test]
    // Mutation: accept only CRTC_X/CRTC_Y, demote physical drivers when the
    // cap is rejected, treat absent caps as an unknown driver, accept a
    // paravirtualized plane without hotspot properties, or change the
    // lowercase path-reason display text.
    fn cursor_path_decision_table() {
        assert_decision(
            decide(None, None),
            CursorStatePath::Legacy,
            CursorPathReason::NoCursorPlane,
        );

        let complete_names: Vec<_> = REQUIRED.into_iter().chain(HOTSPOT).collect();
        let complete = discovered(21, &complete_names);
        assert_decision(
            decide(Some(&complete), None),
            CursorStatePath::Legacy,
            CursorPathReason::NoDeviceCaps,
        );

        for name in REQUIRED {
            let names: Vec<_> = REQUIRED
                .into_iter()
                .filter(|candidate| *candidate != name)
                .chain(HOTSPOT)
                .collect();
            assert_decision(
                decide(
                    Some(&discovered(21, &names)),
                    Some(&caps(Some("amdgpu"), false)),
                ),
                CursorStatePath::Legacy,
                CursorPathReason::MissingProperty(name),
            );
        }

        assert_decision(
            decide(Some(&complete), Some(&caps(Some("amdgpu"), false))),
            CursorStatePath::Atomic,
            CursorPathReason::Qualified,
        );

        for driver in ["virtio_gpu", "vmwgfx", "qxl", "vboxvideo"] {
            assert_decision(
                decide(Some(&complete), Some(&caps(Some(driver), true))),
                CursorStatePath::Atomic,
                CursorPathReason::Qualified,
            );
            assert_decision(
                decide(Some(&complete), Some(&caps(Some(driver), false))),
                CursorStatePath::Legacy,
                CursorPathReason::HotspotSupportMissing,
            );
        }

        let hotspot_y_missing_names: Vec<_> = REQUIRED.into_iter().chain(["HOTSPOT_X"]).collect();
        let hotspot_y_missing = discovered(21, &hotspot_y_missing_names);
        assert_decision(
            decide(
                Some(&hotspot_y_missing),
                Some(&caps(Some("virtio_gpu"), true)),
            ),
            CursorStatePath::Legacy,
            CursorPathReason::HotspotSupportMissing,
        );
        assert_decision(
            decide(Some(&complete), Some(&caps(None, false))),
            CursorStatePath::Legacy,
            CursorPathReason::HotspotSupportMissing,
        );

        let physical_without_hotspots = discovered(21, &REQUIRED);
        for driver in [
            "amdgpu",
            "msm",
            "xe",
            "i915",
            "nouveau",
            "nvidia-drm",
            "apple",
        ] {
            assert_decision(
                decide(
                    Some(&physical_without_hotspots),
                    Some(&caps(Some(driver), false)),
                ),
                CursorStatePath::Atomic,
                CursorPathReason::Qualified,
            );
        }
        assert_eq!(CursorStatePath::Atomic.to_string(), "atomic");
        assert_eq!(CursorStatePath::Legacy.to_string(), "legacy");
        assert_eq!(
            CursorPathReason::NoCursorPlane.to_string(),
            "no-cursor-plane"
        );
        assert_eq!(CursorPathReason::NoDeviceCaps.to_string(), "no-device-caps");
        assert_eq!(
            CursorPathReason::MissingProperty("FB_ID").to_string(),
            "missing-property:FB_ID"
        );
        assert_eq!(
            CursorPathReason::HotspotSupportMissing.to_string(),
            "hotspot-support-missing"
        );
        assert_eq!(CursorPathReason::Qualified.to_string(), "qualified");
    }
}
