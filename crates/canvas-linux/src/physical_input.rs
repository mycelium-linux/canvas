//! Local seat discovery and accelerated input. No device grabs or permission changes.
use input::Libinput;
use smithay::backend::{
    libinput::LibinputSessionInterface,
    session::{Session, libseat::LibSeatSession},
};

pub fn open(session: LibSeatSession) -> Result<Libinput, String> {
    let seat = session.seat();
    let mut context = Libinput::new_with_udev(LibinputSessionInterface::from(session));
    context
        .udev_assign_seat(&seat)
        .map_err(|()| format!("physical seat: could not assign {seat}"))?;
    Ok(context)
}
