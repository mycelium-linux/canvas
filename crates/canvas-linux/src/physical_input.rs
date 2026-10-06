//! Local seat discovery and accelerated input. No device grabs or permission changes.
use input::{Libinput, LibinputInterface};
use std::{
    fs::OpenOptions,
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::Path,
};

struct Interface;
impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let access = flags & libc::O_ACCMODE;
        OpenOptions::new()
            .read(access != libc::O_WRONLY)
            .write(access != libc::O_RDONLY)
            .custom_flags(flags | libc::O_CLOEXEC)
            .open(path)
            .map(Into::into)
            .map_err(|error| {
                eprintln!("physical seat: cannot open {}: {error}", path.display());
                error.raw_os_error().unwrap_or(libc::EIO)
            })
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

pub fn open() -> Result<Libinput, String> {
    let mut context = Libinput::new_with_udev(Interface);
    context
        .udev_assign_seat("seat0")
        .map_err(|()| "physical seat: could not assign seat0".to_owned())?;
    Ok(context)
}
