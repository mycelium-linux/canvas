//! Local Mach rights accompany the Unix descriptor, keyed by source/generation/id.
//! IDs alone cannot share non-global ScreenCaptureKit IOSurfaces across tasks.
use mach2::{bootstrap::*, mach_port::*, message::*, port::*, traps::mach_task_self};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::CString,
    mem::size_of,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    time::{Duration, Instant},
};

const MESSAGE_ID: i32 = 0x434e5350;
const MAX_PENDING: usize = 8;
type Key = [u8; 48];

#[derive(Debug)]
pub struct NativeSurfacePort(u32);
impl NativeSurfacePort {
    pub fn raw(&self) -> u32 {
        self.0
    }
    /// # Safety
    /// `surface` must be a live IOSurfaceRef for the duration of this call.
    pub unsafe fn from_iosurface(surface: *mut std::ffi::c_void) -> Result<Self, String> {
        #[link(name = "IOSurface", kind = "framework")]
        unsafe extern "C" {
            fn IOSurfaceCreateMachPort(surface: *mut std::ffi::c_void) -> u32;
        }
        let port = unsafe { IOSurfaceCreateMachPort(surface) };
        if port == 0 {
            return Err("IOSurfaceCreateMachPort failed".into());
        }
        Ok(Self(port))
    }
}
impl Drop for NativeSurfacePort {
    fn drop(&mut self) {
        unsafe {
            mach_port_deallocate(mach_task_self(), self.0);
        }
    }
}

#[repr(C)]
struct Message {
    header: mach_msg_header_t,
    body: mach_msg_body_t,
    port: mach_msg_port_descriptor_t,
    key: Key,
}
#[repr(C)]
struct ReceiveBuffer {
    message: Message,
    trailer: [u8; 128],
}

fn key(source: &str, generation: u64, surface_id: u32) -> Result<Key, String> {
    if !super::external_surface::valid_source_id(source) || generation == 0 || surface_id == 0 {
        return Err("invalid native surface right identity".into());
    }
    let mut key = [0; 48];
    key[..4].copy_from_slice(&1u32.to_le_bytes());
    key[4..12].copy_from_slice(&generation.to_le_bytes());
    key[12..16].copy_from_slice(&surface_id.to_le_bytes());
    key[16..].copy_from_slice(&Sha256::digest(source.as_bytes()));
    Ok(key)
}
fn name(socket: &Path) -> Result<CString, String> {
    let metadata = std::fs::symlink_metadata(socket).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("native socket belongs to another user".into());
    }
    CString::new(format!(
        "dev.fpl.canvas.native.{}.{}.{}",
        metadata.uid(),
        metadata.dev(),
        metadata.ino()
    ))
    .map_err(|e| e.to_string())
}
fn check(result: i32, operation: &str) -> Result<(), String> {
    if result == 0 {
        Ok(())
    } else {
        Err(format!("{operation} failed: Mach {result:#x}"))
    }
}

pub struct NativeSurfacePortSender(NativeSurfacePort);
impl NativeSurfacePortSender {
    pub fn connect(socket: &Path) -> Result<Self, String> {
        let name = name(socket)?;
        let mut port = 0;
        check(
            unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr().cast_mut(), &mut port) },
            "look up Canvas native rights receiver",
        )?;
        Ok(Self(NativeSurfacePort(port)))
    }
    pub fn send(
        &self,
        source: &str,
        generation: u64,
        surface_id: u32,
        port: &NativeSurfacePort,
    ) -> Result<(), String> {
        let mut message = Message {
            header: mach_msg_header_t {
                msgh_bits: MACH_MSGH_BITS_COMPLEX | MACH_MSG_TYPE_COPY_SEND,
                msgh_size: size_of::<Message>() as u32,
                msgh_remote_port: self.0.raw(),
                msgh_id: MESSAGE_ID,
                ..Default::default()
            },
            body: mach_msg_body_t {
                msgh_descriptor_count: 1,
            },
            port: mach_msg_port_descriptor_t::new(port.raw(), MACH_MSG_TYPE_COPY_SEND),
            key: key(source, generation, surface_id)?,
        };
        // COPY_SEND preserves the producer's right; the kernel creates a new
        // receiver-owned send right. A blocked receiver cannot block capture forever.
        check(
            unsafe {
                mach_msg(
                    &mut message.header,
                    MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                    size_of::<Message>() as u32,
                    0,
                    0,
                    500,
                    0,
                )
            },
            "send native surface right",
        )
    }
}

pub struct NativeSurfacePortReceiver {
    port: u32,
    pending: BTreeMap<Key, (Instant, NativeSurfacePort)>,
}
impl NativeSurfacePortReceiver {
    pub fn bind(socket: &Path) -> Result<Self, String> {
        let name = name(socket)?;
        let mut port = 0;
        check(
            unsafe { mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &mut port) },
            "allocate native receiver",
        )?;
        let receiver = Self {
            port,
            pending: BTreeMap::new(),
        };
        check(
            unsafe {
                mach_port_insert_right(mach_task_self(), port, port, MACH_MSG_TYPE_MAKE_SEND)
            },
            "insert native receiver right",
        )?;
        check(
            unsafe { bootstrap_register(bootstrap_port, name.as_ptr().cast_mut(), port) },
            "register native receiver",
        )?;
        Ok(receiver)
    }
    pub fn take(
        &mut self,
        source: &str,
        generation: u64,
        surface_id: u32,
    ) -> Result<NativeSurfacePort, String> {
        let expected = key(source, generation, surface_id)?;
        self.pending
            .retain(|_, (time, _)| time.elapsed() < Duration::from_secs(1));
        if let Some((_, port)) = self.pending.remove(&expected) {
            return Ok(port);
        }
        for _ in 0..MAX_PENDING {
            // Zero initialization covers the bounded Mach trailer too. Received
            // rights are either transferred into RAII or destroyed on rejection.
            let mut buffer: ReceiveBuffer = unsafe { std::mem::zeroed() };
            check(
                unsafe {
                    mach_msg(
                        &mut buffer.message.header,
                        MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                        0,
                        size_of::<ReceiveBuffer>() as u32,
                        self.port,
                        100,
                        0,
                    )
                },
                "receive native surface right",
            )?;
            let message = &mut buffer.message;
            if message.header.msgh_id != MESSAGE_ID
                || message.header.msgh_size != size_of::<Message>() as u32
                || message.header.msgh_bits & MACH_MSGH_BITS_COMPLEX == 0
                || message.body.msgh_descriptor_count != 1
                || message.port.type_ != MACH_MSG_PORT_DESCRIPTOR as u8
                || message.port.disposition != MACH_MSG_TYPE_MOVE_SEND as u8
                || message.port.name == 0
                || message.key[..4] != 1u32.to_le_bytes()
            {
                unsafe {
                    mach_msg_destroy(&mut message.header);
                }
                return Err("invalid native surface rights envelope".into());
            }
            let port = NativeSurfacePort(message.port.name);
            if message.key == expected {
                return Ok(port);
            }
            if self.pending.len() == MAX_PENDING {
                return Err("native surface pending-right limit exceeded".into());
            }
            self.pending.insert(message.key, (Instant::now(), port));
        }
        Err("matching native surface right was not received".into())
    }
}
impl Drop for NativeSurfacePortReceiver {
    fn drop(&mut self) {
        unsafe {
            mach_port_destroy(mach_task_self(), self.port);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rights_are_transferred_and_correlated_independently_of_registration_order() {
        let path = std::env::temp_dir().join(format!(
            "canvas-mach-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let mut receiver = NativeSurfacePortReceiver::bind(&path).unwrap();
        let sender = NativeSurfacePortSender::connect(&path).unwrap();
        // A send right to the receiver itself suffices to test the kernel's
        // rights-transfer ABI without relying on a GUI or fake IOSurface ID.
        assert_eq!(
            unsafe { mach_port_mod_refs(mach_task_self(), receiver.port, MACH_PORT_RIGHT_SEND, 1) },
            0
        );
        let right = NativeSurfacePort(receiver.port);
        sender.send("native/a", 1, 42, &right).unwrap();
        sender.send("native/b", 2, 43, &right).unwrap();
        assert_ne!(receiver.take("native/b", 2, 43).unwrap().raw(), 0);
        assert_ne!(receiver.take("native/a", 1, 42).unwrap().raw(), 0);
        assert!(sender.send("native/a", 0, 42, &right).is_err());
        drop(socket);
        std::fs::remove_file(path).unwrap();
    }
}
