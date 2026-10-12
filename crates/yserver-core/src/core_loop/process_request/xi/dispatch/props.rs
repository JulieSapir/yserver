use super::*;

pub(super) fn handle_xi_list_properties(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    {
        // XIListProperties (xXIListPropertiesReq, XI2proto.h:742-749).
        // body[0..2] = deviceid, body[2..4] = pad.
        if body.len() < 2 {
            return Ok(RequestOutcome::Handled);
        }
        let deviceid = u16::from_le_bytes([body[0], body[1]]);
        debug!(
            "client {} #{} XIListProperties deviceid={}",
            client_id.0, sequence.0, deviceid
        );
        let Some(device) = crate::xinput::find_device(&state.xi_devices, deviceid) else {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                XI2_FIRST_ERROR, // XI_BadDevice
                u32::from(deviceid),
                56,
                XI2_MAJOR_OPCODE,
            );
        };
        buf.extend_from_slice(&crate::xinput::encode_list_properties_reply(
            byte_order, sequence, device,
        ));
    }
    xi_write_reply(state, client_id, buf)
}

#[allow(clippy::needless_return)]
pub(super) fn handle_xi_change_property(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    // XIChangeProperty (xXIChangePropertyReq, XI2proto.h:769-780).
    // Header struct: deviceid(2), mode(1), format(1), property(4),
    // type(4), num_items(4); after stripping the 4-byte generic
    // header `body` begins at deviceid:
    //   body[0..2] deviceid, body[2] mode, body[3] format,
    //   body[4..8] property, body[8..12] type,
    //   body[12..16] num_items, body[16..] value bytes.
    if body.len() < 16 {
        return Ok(RequestOutcome::Handled);
    }
    let deviceid = u16::from_le_bytes([body[0], body[1]]);
    let mode = body[2];
    let format = body[3];
    let property = AtomId(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
    let type_atom = AtomId(u32::from_le_bytes([body[8], body[9], body[10], body[11]]));
    let num_items = u32::from_le_bytes([body[12], body[13], body[14], body[15]]) as usize;
    debug!(
        "client {} #{} XIChangeProperty deviceid={} mode={} format={} property={} type={} num_items={}",
        client_id.0, sequence.0, deviceid, mode, format, property.0, type_atom.0, num_items
    );
    // Device lookup FIRST (xserver ProcXIChangeProperty,
    // xiproperty.c:1137-1141, does dixLookupDevice before the
    // format/mode checks), so an unknown device yields BadDevice
    // even when the format is also invalid.
    if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            XI2_FIRST_ERROR, // XI_BadDevice
            u32::from(deviceid),
            57,
            XI2_MAJOR_OPCODE,
        );
    }
    // Validate mode (xiproperty.c check_change_property:325-329).
    if mode != crate::xinput::XI_PROP_MODE_REPLACE
        && mode != crate::xinput::XI_PROP_MODE_PREPEND
        && mode != crate::xinput::XI_PROP_MODE_APPEND
    {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            2, // BadValue
            u32::from(mode),
            57,
            XI2_MAJOR_OPCODE,
        );
    }
    // Format must be 8/16/32 before we compute the value length.
    if format != 8 && format != 16 && format != 32 {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            2, // BadValue
            u32::from(format),
            57,
            XI2_MAJOR_OPCODE,
        );
    }
    let value_len = num_items.saturating_mul(usize::from(format / 8));
    let Some(data) = body.get(16..16 + value_len) else {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            16, // BadLength
            0,
            57,
            XI2_MAJOR_OPCODE,
        );
    };
    let data = canonicalize_xi_property_data(byte_order, format, data);
    // T3 spec §D order is implemented by `dispatch_change_property`;
    // both write arms (XI2 minor 57 + XI1 minor 37) share the same
    // pipeline so T5/T6 only has to wire event emission in one place.
    match dispatch_change_property(
        state, backend, client_id, sequence, 57, deviceid, mode, format, property, type_atom, &data,
    ) {
        Ok(PropertyChangeOutcome::Changed(what)) => {
            let _ = emit_property_change(state, deviceid, property, what);
            return Ok(RequestOutcome::Handled);
        }
        Ok(PropertyChangeOutcome::Pending(request)) => {
            return Ok(RequestOutcome::PendingXiConfig(request));
        }
        Err(err) => {
            return emit_property_dispatch_error(state, client_id, sequence, err, 57);
        }
    }
}

#[allow(clippy::needless_return)]
pub(super) fn handle_xi_delete_property(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
) -> io::Result<RequestOutcome> {
    // XIDeleteProperty (xXIDeletePropertyReq, XI2proto.h:785-793).
    //   body[0..2] deviceid, body[2..4] pad0, body[4..8] property.
    if body.len() < 8 {
        return Ok(RequestOutcome::Handled);
    }
    let deviceid = u16::from_le_bytes([body[0], body[1]]);
    let property = AtomId(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
    debug!(
        "client {} #{} XIDeleteProperty deviceid={} property={}",
        client_id.0, sequence.0, deviceid, property.0
    );
    // T3: BadAtom guard (xserver ProcXIDeleteProperty,
    // xiproperty.c).
    if !state.atoms.exists(property) {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            5, // BadAtom
            property.0,
            58,
            XI2_MAJOR_OPCODE,
        );
    }
    if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            XI2_FIRST_ERROR, // XI_BadDevice
            u32::from(deviceid),
            58,
            XI2_MAJOR_OPCODE,
        );
    }
    if is_xtest_marker_property(state, deviceid, property) {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            10, // BadAccess: XTEST Device is protected by atom identity.
            property.0,
            58,
            XI2_MAJOR_OPCODE,
        );
    }
    if crate::xinput::find_device(&state.xi_devices, deviceid)
        .and_then(|device| device.properties.get(&property))
        .is_some_and(|property| !property.deletable)
    {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            10, // BadAccess: seeded driver properties are non-deletable.
            property.0,
            58,
            XI2_MAJOR_OPCODE,
        );
    }
    let device = crate::xinput::find_device_mut(&mut state.xi_devices, deviceid)
        .expect("device existence verified above");
    if let Some(what) = crate::xinput::apply_delete_property(device, property) {
        // Notify on a real removal only; a redundant delete of
        // an absent property must stay silent — xiproperty.c
        // does not call `send_property_event` in that case.
        let _ = emit_property_change(state, deviceid, property, what);
    }
    return Ok(RequestOutcome::Handled);
}

pub(super) fn handle_xi_get_property(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    {
        // XIGetProperty (xXIGetPropertyReq, XI2proto.h:798-814).
        // Struct: deviceid(2), delete(1), pad0(1), property(4),
        // type(4), offset(4), len(4). After the 4-byte generic header
        // `body` begins at deviceid:
        //   body[0..2] deviceid, body[2] delete, body[3] pad0,
        //   body[4..8] property, body[8..12] type,
        //   body[12..16] offset, body[16..20] len.
        if body.len() < 20 {
            return Ok(RequestOutcome::Handled);
        }
        let deviceid = u16::from_le_bytes([body[0], body[1]]);
        let delete_raw = body[2];
        let property = AtomId(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
        let req_type = AtomId(u32::from_le_bytes([body[8], body[9], body[10], body[11]]));
        let offset = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let len = u32::from_le_bytes([body[16], body[17], body[18], body[19]]);
        debug!(
            "client {} #{} XIGetProperty deviceid={} property={} type={} offset={} len={} delete={}",
            client_id.0, sequence.0, deviceid, property.0, req_type.0, offset, len, delete_raw
        );
        // T3: BadAtom guard before the device lookup so a bogus atom
        // surfaces as BadAtom (xserver ProcXIGetProperty does it the
        // same way via ValidAtom() before dixLookupDevice's payload
        // path; the relative precedence vs. BadDevice never matters
        // because clients always pair a valid atom with the deviceid
        // they just learned from XIListProperties).
        if !state.atoms.exists(property) {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                5, // BadAtom
                property.0,
                59,
                XI2_MAJOR_OPCODE,
            );
        }
        // Device lookup first (xserver dixLookupDevice precedes
        // get_property, so BadDevice outranks the BadValue below).
        if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                XI2_FIRST_ERROR, // XI_BadDevice
                u32::from(deviceid),
                59,
                XI2_MAJOR_OPCODE,
            );
        }
        // `delete` must be exactly 0 or 1; xserver's get_property
        // (xiproperty.c:252-255) rejects anything else with BadValue.
        if delete_raw > 1 {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                2, // BadValue
                u32::from(delete_raw),
                59,
                XI2_MAJOR_OPCODE,
            );
        }
        let delete = delete_raw != 0;
        // Re-acquire mutably now that both validations passed.
        let device = crate::xinput::find_device_mut(&mut state.xi_devices, deviceid)
            .expect("device existence verified above");
        let send_deleted = match crate::xinput::encode_get_property_reply(
            byte_order, sequence, device, property, req_type, offset, len, delete,
        ) {
            Ok(reply) => {
                let returned = x11::read_u32(byte_order, &reply[4..8]);
                let bytes_after = x11::read_u32(byte_order, &reply[12..16]);
                buf.extend_from_slice(&reply);
                delete && returned != 0 && bytes_after == 0
            }
            Err(crate::xinput::XiPropError::BadValue) => {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    2, // BadValue
                    offset,
                    59,
                    XI2_MAJOR_OPCODE,
                );
            }
            Err(_) => {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    2,
                    offset,
                    59,
                    XI2_MAJOR_OPCODE,
                );
            }
        };
        if send_deleted {
            let _ =
                emit_property_change(state, deviceid, property, crate::xinput::PropWhat::Deleted);
        }
    }
    xi_write_reply(state, client_id, buf)
}

// XI 1.x ListDeviceProperties (minor 36; xListDevicePropertiesReq,
// XIproto.h:1433-1440). Body after the 4-byte generic header:
//   body[0] deviceid (CARD8), body[1] pad0, body[2..4] pad1.
// MATE's settings daemon enumerates a touchpad's libinput
// properties through this XI1 path (not the XI2 XIListProperties),
// so a real reply backed by `xi_devices` is required.
pub(super) fn handle_xi1_list_device_properties(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    {
        if body.is_empty() {
            return Ok(RequestOutcome::Handled);
        }
        let deviceid = u16::from(body[0]);
        debug!(
            "client {} #{} XListDeviceProperties deviceid={}",
            client_id.0, sequence.0, deviceid
        );
        let Some(device) = crate::xinput::find_device(&state.xi_devices, deviceid) else {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                XI2_FIRST_ERROR, // XI_BadDevice
                u32::from(deviceid),
                36,
                XI2_MAJOR_OPCODE,
            );
        };
        buf.extend_from_slice(&crate::xinput::encode_xi1_list_properties_reply(
            byte_order, sequence, device,
        ));
    }
    xi_write_reply(state, client_id, buf)
}

// XI 1.x ChangeDeviceProperty (minor 37; xChangeDevicePropertyReq,
// XIproto.h:1462-1473). Body after the 4-byte generic header:
//   body[0..4] property (Atom), body[4..8] type (Atom),
//   body[8] deviceid (CARD8), body[9] format, body[10] mode,
//   body[11] pad, body[12..16] nUnits (CARD32), body[16..] value.
#[allow(clippy::needless_return)]
pub(super) fn handle_xi1_change_device_property(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    if body.len() < 16 {
        return Ok(RequestOutcome::Handled);
    }
    let property = AtomId(u32::from_le_bytes([body[0], body[1], body[2], body[3]]));
    let type_atom = AtomId(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
    let deviceid = u16::from(body[8]);
    let format = body[9];
    let mode = body[10];
    let num_items = u32::from_le_bytes([body[12], body[13], body[14], body[15]]) as usize;
    debug!(
        "client {} #{} XChangeDeviceProperty deviceid={} mode={} format={} property={} type={} num_items={}",
        client_id.0, sequence.0, deviceid, mode, format, property.0, type_atom.0, num_items
    );
    // Device lookup first, mirroring the XI2 path (xserver does
    // dixLookupDevice before the mode/format checks).
    if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            XI2_FIRST_ERROR, // XI_BadDevice
            u32::from(deviceid),
            37,
            XI2_MAJOR_OPCODE,
        );
    }
    if mode != crate::xinput::XI_PROP_MODE_REPLACE
        && mode != crate::xinput::XI_PROP_MODE_PREPEND
        && mode != crate::xinput::XI_PROP_MODE_APPEND
    {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            2, // BadValue
            u32::from(mode),
            37,
            XI2_MAJOR_OPCODE,
        );
    }
    if format != 8 && format != 16 && format != 32 {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            2, // BadValue
            u32::from(format),
            37,
            XI2_MAJOR_OPCODE,
        );
    }
    let value_len = num_items.saturating_mul(usize::from(format / 8));
    let Some(data) = body.get(16..16 + value_len) else {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            16, // BadLength
            0,
            37,
            XI2_MAJOR_OPCODE,
        );
    };
    let data = canonicalize_xi_property_data(byte_order, format, data);
    // The T3 spec §D pipeline is shared with the XI2 arm
    // (minor 57) via `dispatch_change_property`; the same
    // `emit_property_change` helper fans out
    // `XI_PropertyEvent` here too — xserver's
    // `send_property_event` fires from both XI1 and XI2 paths.
    match dispatch_change_property(
        state, backend, client_id, sequence, 37, deviceid, mode, format, property, type_atom, &data,
    ) {
        Ok(PropertyChangeOutcome::Changed(what)) => {
            let _ = emit_property_change(state, deviceid, property, what);
            return Ok(RequestOutcome::Handled);
        }
        Ok(PropertyChangeOutcome::Pending(request)) => {
            return Ok(RequestOutcome::PendingXiConfig(request));
        }
        Err(err) => {
            return emit_property_dispatch_error(state, client_id, sequence, err, 37);
        }
    }
}

// XI 1.x DeleteDeviceProperty (minor 38; xDeleteDevicePropertyReq,
// XIproto.h:1481-1489). Body after the 4-byte generic header:
//   body[0..4] property (Atom), body[4] deviceid (CARD8),
//   body[5] pad0, body[6..8] pad1.
#[allow(clippy::needless_return)]
pub(super) fn handle_xi1_delete_device_property(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
) -> io::Result<RequestOutcome> {
    if body.len() < 5 {
        return Ok(RequestOutcome::Handled);
    }
    let property = AtomId(u32::from_le_bytes([body[0], body[1], body[2], body[3]]));
    let deviceid = u16::from(body[4]);
    debug!(
        "client {} #{} XDeleteDeviceProperty deviceid={} property={}",
        client_id.0, sequence.0, deviceid, property.0
    );
    // T3: BadAtom guard (mirror of XI2 minor 58).
    if !state.atoms.exists(property) {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            5, // BadAtom
            property.0,
            38,
            XI2_MAJOR_OPCODE,
        );
    }
    if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            XI2_FIRST_ERROR, // XI_BadDevice
            u32::from(deviceid),
            38,
            XI2_MAJOR_OPCODE,
        );
    }
    if is_xtest_marker_property(state, deviceid, property) {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            10, // BadAccess: XTEST Device is protected by atom identity.
            property.0,
            38,
            XI2_MAJOR_OPCODE,
        );
    }
    if crate::xinput::find_device(&state.xi_devices, deviceid)
        .and_then(|device| device.properties.get(&property))
        .is_some_and(|property| !property.deletable)
    {
        return emit_x11_error_with_minor(
            state,
            client_id,
            sequence,
            10, // BadAccess: seeded driver properties are non-deletable.
            property.0,
            38,
            XI2_MAJOR_OPCODE,
        );
    }
    let device = crate::xinput::find_device_mut(&mut state.xi_devices, deviceid)
        .expect("device existence verified above");
    if let Some(what) = crate::xinput::apply_delete_property(device, property) {
        // Same XI2 fan-out the minor-58 arm runs.
        let _ = emit_property_change(state, deviceid, property, what);
    }
    return Ok(RequestOutcome::Handled);
}

// XI 1.x GetDeviceProperty (minor 39; xGetDevicePropertyReq,
// XIproto.h:1497-1512). Body after the 4-byte generic header:
//   body[0..4] property (Atom), body[4..8] type (Atom),
//   body[8..12] longOffset (CARD32), body[12..16] longLength
//   (CARD32), body[16] deviceid (CARD8), body[17] delete (BOOL),
//   body[18..20] pad.
// This is the request MATE's settings daemon uses to read
// "libinput Tapping Enabled" etc. The reply echoes the requested
// deviceid (byte 21), unlike the XI2 reply (Task brief: the old
// zero-stub returned device=0 and MATE rejected the touchpad).
pub(super) fn handle_xi1_get_device_property(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
) -> io::Result<RequestOutcome> {
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    {
        if body.len() < 18 {
            return Ok(RequestOutcome::Handled);
        }
        let property = AtomId(u32::from_le_bytes([body[0], body[1], body[2], body[3]]));
        let req_type = AtomId(u32::from_le_bytes([body[4], body[5], body[6], body[7]]));
        let offset = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
        let len = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let deviceid = u16::from(body[16]);
        let delete_raw = body[17];
        debug!(
            "client {} #{} XGetDeviceProperty deviceid={} property={} type={} offset={} len={} delete={}",
            client_id.0, sequence.0, deviceid, property.0, req_type.0, offset, len, delete_raw
        );
        // T3: BadAtom guard (mirror of XI2 minor 59).
        if !state.atoms.exists(property) {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                5, // BadAtom
                property.0,
                39,
                XI2_MAJOR_OPCODE,
            );
        }
        if crate::xinput::find_device(&state.xi_devices, deviceid).is_none() {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                XI2_FIRST_ERROR, // XI_BadDevice
                u32::from(deviceid),
                39,
                XI2_MAJOR_OPCODE,
            );
        }
        if delete_raw > 1 {
            return emit_x11_error_with_minor(
                state,
                client_id,
                sequence,
                2, // BadValue
                u32::from(delete_raw),
                39,
                XI2_MAJOR_OPCODE,
            );
        }
        let delete = delete_raw != 0;
        let device = crate::xinput::find_device_mut(&mut state.xi_devices, deviceid)
            .expect("device existence verified above");
        let send_deleted = match crate::xinput::encode_xi1_get_property_reply(
            byte_order, sequence, device, property, req_type, offset, len, delete,
        ) {
            Ok(reply) => {
                let bytes_after = x11::read_u32(byte_order, &reply[12..16]);
                buf.extend_from_slice(&reply);
                delete && bytes_after == 0
            }
            Err(_) => {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    2, // BadValue
                    offset,
                    39,
                    XI2_MAJOR_OPCODE,
                );
            }
        };
        if send_deleted {
            let _ =
                emit_property_change(state, deviceid, property, crate::xinput::PropWhat::Deleted);
        }
    }
    xi_write_reply(state, client_id, buf)
}
