//! Owner-local IPC for [`crate::external_surface`] messages.
//!
//! A datagram preserves the association between one JSON envelope and its
//! ancillary descriptors. This is important for DMA-BUF planes and sync files:
//! a byte-stream protocol can accidentally attach a descriptor to the wrong
//! frame after partial reads. The receiving process owns every returned fd.

use crate::external_surface::{
    ExternalSurfaceHostEvent, ExternalSurfaceInput, ExternalSurfaceMessage,
    ExternalSurfaceVisibility,
};
use std::{
    io,
    mem::{size_of, zeroed},
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixDatagram,
    },
    ptr,
};

pub const MAX_EXTERNAL_SURFACE_MESSAGE: usize = 64 * 1024;
pub const MAX_EXTERNAL_SURFACE_HANDLES: usize = 8;

pub fn send_acceptance(
    socket: &UnixDatagram,
    receipt: &crate::external_surface::ExternalSurfaceAcceptance,
) -> io::Result<()> {
    receipt.validate().map_err(invalid)?;
    let payload = serde_json::to_vec(receipt).map_err(invalid)?;
    if socket.send(&payload)? != payload.len() {
        return Err(invalid("truncated external-surface acceptance"));
    }
    Ok(())
}

pub fn receive_acceptance(
    socket: &UnixDatagram,
) -> io::Result<crate::external_surface::ExternalSurfaceAcceptance> {
    let mut payload = vec![0; MAX_EXTERNAL_SURFACE_MESSAGE + 1];
    let read = socket.recv(&mut payload)?;
    if read > MAX_EXTERNAL_SURFACE_MESSAGE {
        return Err(invalid("external-surface acceptance is too large"));
    }
    let receipt: crate::external_surface::ExternalSurfaceAcceptance =
        serde_json::from_slice(&payload[..read]).map_err(invalid)?;
    receipt.validate().map_err(invalid)?;
    Ok(receipt)
}

pub fn send(
    socket: &UnixDatagram,
    message: &ExternalSurfaceMessage,
    handles: &[BorrowedFd<'_>],
) -> io::Result<()> {
    if handles.len() > MAX_EXTERNAL_SURFACE_HANDLES {
        return Err(invalid("too many external surface handles"));
    }
    let payload = serde_json::to_vec(message).map_err(invalid)?;
    if payload.len() > MAX_EXTERNAL_SURFACE_MESSAGE {
        return Err(invalid("external surface message is too large"));
    }

    let mut iov = libc::iovec {
        iov_base: payload.as_ptr().cast_mut().cast(),
        iov_len: payload.len(),
    };
    let descriptor_bytes = handles.len() * size_of::<RawFd>();
    let mut control = if handles.is_empty() {
        Vec::new()
    } else {
        vec![0_u8; unsafe { libc::CMSG_SPACE(descriptor_bytes as _) as usize }]
    };
    let mut header: libc::msghdr = unsafe { zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    if !control.is_empty() {
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = control.len() as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&header) };
        if cmsg.is_null() {
            return Err(invalid("failed to allocate descriptor control message"));
        }
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(descriptor_bytes as _) as _;
            let raw: Vec<RawFd> = handles.iter().map(AsRawFd::as_raw_fd).collect();
            ptr::copy_nonoverlapping(raw.as_ptr(), libc::CMSG_DATA(cmsg).cast(), raw.len());
        }
    }

    let written = unsafe { libc::sendmsg(socket.as_raw_fd(), &header, libc::MSG_NOSIGNAL) };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    if written as usize != payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "external surface datagram was truncated",
        ));
    }
    Ok(())
}

pub fn receive(socket: &UnixDatagram) -> io::Result<(ExternalSurfaceMessage, Vec<OwnedFd>)> {
    let mut payload = vec![0_u8; MAX_EXTERNAL_SURFACE_MESSAGE];
    let descriptor_bytes = MAX_EXTERNAL_SURFACE_HANDLES * size_of::<RawFd>();
    let mut control = vec![0_u8; unsafe { libc::CMSG_SPACE(descriptor_bytes as _) as usize }];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    let mut header: libc::msghdr = unsafe { zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = control.len() as _;

    let read = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut header, 0) };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    if header.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(invalid(
            "external surface datagram or handles were truncated",
        ));
    }
    payload.truncate(read as usize);

    let mut handles = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&header) };
    while !cmsg.is_null() {
        let current = unsafe { &*cmsg };
        if current.cmsg_level == libc::SOL_SOCKET && current.cmsg_type == libc::SCM_RIGHTS {
            let header_len = unsafe { libc::CMSG_LEN(0) as usize };
            let message_len = current.cmsg_len as usize;
            if message_len < header_len {
                return Err(invalid("invalid external surface descriptor message"));
            }
            let count = (message_len - header_len) / size_of::<RawFd>();
            if handles.len() + count > MAX_EXTERNAL_SURFACE_HANDLES {
                return Err(invalid("too many external surface handles received"));
            }
            let descriptors =
                unsafe { std::slice::from_raw_parts(libc::CMSG_DATA(cmsg).cast::<RawFd>(), count) };
            for fd in descriptors.iter().copied() {
                let owned = unsafe { OwnedFd::from_raw_fd(fd) };
                let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
                if flags < 0
                    || unsafe {
                        libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC)
                    } < 0
                {
                    return Err(io::Error::last_os_error());
                }
                handles.push(owned);
            }
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&header, cmsg) };
    }

    let message: ExternalSurfaceMessage = serde_json::from_slice(&payload).map_err(invalid)?;
    message.validate(handles.len()).map_err(invalid)?;
    Ok((message, handles))
}

/// Sends one already-bounded interaction event over the producer's dedicated
/// return socket. Input never shares the frame registration queue.
pub fn send_input(socket: &UnixDatagram, input: &ExternalSurfaceInput) -> io::Result<()> {
    input.validate().map_err(invalid)?;
    let payload = serde_json::to_vec(input).map_err(invalid)?;
    if payload.len() > MAX_EXTERNAL_SURFACE_MESSAGE {
        return Err(invalid("external surface input message is too large"));
    }
    let written = socket.send(&payload)?;
    if written != payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "external surface input datagram was truncated",
        ));
    }
    Ok(())
}

pub fn receive_input(socket: &UnixDatagram) -> io::Result<ExternalSurfaceInput> {
    let mut payload = vec![0_u8; MAX_EXTERNAL_SURFACE_MESSAGE];
    let read = socket.recv(&mut payload)?;
    payload.truncate(read);
    let input: ExternalSurfaceInput = serde_json::from_slice(&payload).map_err(invalid)?;
    input.validate().map_err(invalid)?;
    Ok(input)
}

pub fn send_visibility(
    socket: &UnixDatagram,
    visibility: &ExternalSurfaceVisibility,
) -> io::Result<()> {
    visibility.validate().map_err(invalid)?;
    send_host_event(
        socket,
        &ExternalSurfaceHostEvent::Visibility(visibility.clone()),
    )
}

pub fn receive_host_event(socket: &UnixDatagram) -> io::Result<ExternalSurfaceHostEvent> {
    let mut payload = vec![0_u8; MAX_EXTERNAL_SURFACE_MESSAGE];
    let read = socket.recv(&mut payload)?;
    payload.truncate(read);
    let event: ExternalSurfaceHostEvent = serde_json::from_slice(&payload).map_err(invalid)?;
    event.validate().map_err(invalid)?;
    Ok(event)
}

fn send_host_event(socket: &UnixDatagram, event: &ExternalSurfaceHostEvent) -> io::Result<()> {
    event.validate().map_err(invalid)?;
    let payload = serde_json::to_vec(event).map_err(invalid)?;
    if payload.len() > MAX_EXTERNAL_SURFACE_MESSAGE {
        return Err(invalid("external surface host event is too large"));
    }
    let written = socket.send(&payload)?;
    if written != payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "external surface host event datagram was truncated",
        ));
    }
    Ok(())
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_surface::{
        AlphaMode, ColorSpace, EXTERNAL_SURFACE_VERSION, ExternalSurfaceDescriptor, PixelFormat,
        SurfaceTransport,
    };
    use std::os::fd::AsFd;

    #[test]
    fn one_datagram_keeps_the_envelope_and_descriptor_together() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let message = ExternalSurfaceMessage::Register {
            surface: ExternalSurfaceDescriptor {
                schema_version: EXTERNAL_SURFACE_VERSION,
                source_id: "security/door".into(),
                generation: 4,
                width: 16,
                height: 16,
                format: PixelFormat::Bgra8Unorm,
                color_space: ColorSpace::Srgb,
                alpha_mode: AlphaMode::Opaque,
                transport: SurfaceTransport::SharedMemory {
                    handle_index: 0,
                    byte_len: 16 * 16 * 4,
                    stride: 16 * 4,
                    slots: 1,
                },
                input: None,
                acceptance: None,
                streams: Vec::new(),
            },
        };
        send(&sender, &message, &[file.as_fd()]).unwrap();
        let (received, handles) = receive(&receiver).unwrap();
        assert_eq!(received, message);
        assert_eq!(handles.len(), 1);
    }

    #[test]
    fn validation_rejects_an_envelope_missing_its_handle() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        let message = ExternalSurfaceMessage::Register {
            surface: ExternalSurfaceDescriptor {
                schema_version: EXTERNAL_SURFACE_VERSION,
                source_id: "speedy/2048".into(),
                generation: 1,
                width: 16,
                height: 16,
                format: PixelFormat::Rgba8Unorm,
                color_space: ColorSpace::Srgb,
                alpha_mode: AlphaMode::Opaque,
                transport: SurfaceTransport::SharedMemory {
                    handle_index: 0,
                    byte_len: 16 * 16 * 4,
                    stride: 16 * 4,
                    slots: 1,
                },
                input: None,
                acceptance: None,
                streams: Vec::new(),
            },
        };
        send(&sender, &message, &[]).unwrap();
        assert_eq!(
            receive(&receiver).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn normalized_input_round_trips_on_its_dedicated_channel() {
        use crate::external_surface::{ExternalSurfaceInput, ExternalSurfaceInputPhase};

        let (canvas, producer) = UnixDatagram::pair().unwrap();
        let input = ExternalSurfaceInput {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "synesthesia/mix-overview".into(),
            generation: 7,
            phase: ExternalSurfaceInputPhase::Move,
            x: 0.25,
            y: 0.75,
        };
        send_input(&canvas, &input).unwrap();
        assert_eq!(receive_input(&producer).unwrap(), input);
    }

    #[test]
    fn visibility_round_trips_with_descriptor_owned_streams() {
        use crate::external_surface::{
            ExternalSurfaceHostEvent, ExternalSurfaceVisibility, SurfaceStreamDirection,
            SurfaceStreamImportance, SurfaceStreamRequirement,
        };

        let (canvas, producer) = UnixDatagram::pair().unwrap();
        let visibility = ExternalSurfaceVisibility {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "synesthesia/mix-overview".into(),
            generation: 7,
            visible: true,
            streams: vec![SurfaceStreamRequirement {
                stream_id: "synesthesia/transport".into(),
                contract: "synesthesia.transport.v1".into(),
                direction: SurfaceStreamDirection::Consume,
                importance: SurfaceStreamImportance::Needed,
            }],
        };
        send_visibility(&canvas, &visibility).unwrap();
        assert_eq!(
            receive_host_event(&producer).unwrap(),
            ExternalSurfaceHostEvent::Visibility(visibility)
        );
    }
}
#[test]
fn acceptance_is_bounded_versioned_and_generation_bound() {
    let (sender, receiver) = UnixDatagram::pair().unwrap();
    let mut receipt = crate::external_surface::ExternalSurfaceAcceptance {
        schema_version: 1,
        source_id: "native/simulator".into(),
        generation: 42,
    };
    send_acceptance(&sender, &receipt).unwrap();
    assert_eq!(receive_acceptance(&receiver).unwrap(), receipt);
    receipt.generation = 0;
    assert!(send_acceptance(&sender, &receipt).is_err());
    receipt.generation = 42;
    receipt.schema_version = 99;
    assert!(send_acceptance(&sender, &receipt).is_err());
    match sender.send(&vec![b' '; MAX_EXTERNAL_SURFACE_MESSAGE + 1]) {
        Ok(_) => assert!(receive_acceptance(&receiver).is_err()),
        // macOS may enforce a smaller kernel datagram ceiling first.
        Err(error) => assert_eq!(error.raw_os_error(), Some(libc::EMSGSIZE)),
    }
}
