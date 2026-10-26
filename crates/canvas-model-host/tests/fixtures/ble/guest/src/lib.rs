wit_bindgen::generate!({
    path: "../../../../wit/soil-ble.wit",
    world: "soil-ble-guest",
    with: { "soil:ble/types@1.0.0": generate, "soil:ble/ble@1.0.0": generate },
    inline: r#"
        package canvas:soil-ble-guest@0.1.0;
        world soil-ble-guest {
            import soil:ble/ble@1.0.0;
            import soil:ble/types@1.0.0;
            export run-scan: func(timeout-ms: u32) -> u32;
            export run-connect: func(device: u32) -> u32;
            export run-disconnect: func() -> u32;
            export run-services: func() -> u32;
            export run-read: func(char-uuid: string, max-bytes: u32) -> u32;
            export run-write: func(char-uuid: string, data: list<u8>) -> u32;
            export run-subscribe: func(char-uuid: string) -> u32;
            export run-next-event: func(timeout-ms: u32) -> u32;
            export run-all: func(timeout-ms: u32) -> u64;
        }
    "#,
});

use soil::ble::ble::{self, Error};
use soil::ble::types::Event;

fn code(error: Error) -> u32 {
    match error {
        Error::Denied => 100,
        Error::NotFound => 101,
        Error::NotConnected => 102,
        Error::Leased => 103,
        Error::Timeout => 104,
        Error::OverBudget => 105,
        Error::NotReady => 106,
        Error::Invalid => 107,
        Error::Io => 108,
    }
}

struct Component;

impl Guest for Component {
    fn run_scan(timeout_ms: u32) -> u32 {
        match ble::scan(timeout_ms) {
            Ok(devices) => devices.len() as u32,
            Err(error) => code(error),
        }
    }
    fn run_connect(device: u32) -> u32 {
        ble::connect(device).map(|_| 0).unwrap_or_else(code)
    }
    fn run_disconnect() -> u32 {
        ble::disconnect().map(|_| 0).unwrap_or_else(code)
    }
    fn run_services() -> u32 {
        ble::services().map(|services| services.len() as u32).unwrap_or_else(code)
    }
    fn run_read(char_uuid: String, max_bytes: u32) -> u32 {
        ble::read(&char_uuid, max_bytes).map(|data| data.len() as u32).unwrap_or_else(code)
    }
    fn run_write(char_uuid: String, data: Vec<u8>) -> u32 {
        ble::write(&char_uuid, &data).map(|_| 0).unwrap_or_else(code)
    }
    fn run_subscribe(char_uuid: String) -> u32 {
        ble::subscribe(&char_uuid).map(|_| 0).unwrap_or_else(code)
    }
    fn run_next_event(timeout_ms: u32) -> u32 {
        match ble::next_event(timeout_ms) {
            Ok(None) => 0,
            Ok(Some(Event::Notification(_))) => 1,
            Ok(Some(Event::Disconnected(_))) => 2,
            Err(error) => code(error),
        }
    }
    fn run_all(timeout_ms: u32) -> u64 {
        // One component entry exercises all eight verbs in order. wasmtime 24
        // cannot re-enter the same component instance between separate host
        // calls, so the proof table lives inside one guest call.
        let scan = match ble::scan(timeout_ms) {
            Ok(devices) => devices.len() as u32,
            Err(error) => code(error),
        };
        let connect = ble::connect(7).map(|_| 0).unwrap_or_else(code);
        let services = ble::services()
            .map(|services| services.len() as u32)
            .unwrap_or_else(code);
        let read = ble::read("cmd", 16)
            .map(|data| data.len() as u32)
            .unwrap_or_else(code);
        let write = ble::write("cmd", &[4, 5]).map(|_| 0).unwrap_or_else(code);
        let subscribe = ble::subscribe("cmd").map(|_| 0).unwrap_or_else(code);
        let next = match ble::next_event(250) {
            Ok(None) => 0,
            Ok(Some(Event::Notification(_))) => 1,
            Ok(Some(Event::Disconnected(_))) => 2,
            Err(error) => code(error),
        };
        let disconnect = ble::disconnect().map(|_| 0).unwrap_or_else(code);
        u64::from(scan)
            | (u64::from(connect) << 8)
            | (u64::from(services) << 16)
            | (u64::from(read) << 24)
            | (u64::from(write) << 32)
            | (u64::from(subscribe) << 40)
            | (u64::from(next) << 48)
            | (u64::from(disconnect) << 56)
    }
}

export!(Component);
