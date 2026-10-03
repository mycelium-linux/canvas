//! Renderer-neutral contract for producer-owned pixel surfaces.
//!
//! Workspace documents retain only a stable source id. The volatile handle,
//! synchronization primitive, and frame generation travel over an owner-local
//! socket and are never persisted. Unix file descriptors are transferred as
//! ancillary data; `handle_index` values refer to that ordered descriptor list.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const EXTERNAL_SURFACE_VERSION: u32 = 1;
pub const MAX_SURFACE_DIMENSION: u32 = 16_384;
pub const MAX_SURFACE_PLANES: usize = 4;
pub const MAX_SURFACE_SLOTS: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Rgba8Unorm,
    Bgra8Unorm,
    Rgba16Float,
    R32Float,
    Nv12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
    Rec709,
    Rec2020,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    Opaque,
    Premultiplied,
    Straight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceTransport {
    /// One descriptor containing `slots` tightly bounded frame regions.
    /// This is the portable fallback; Canvas uploads only the presented slot.
    SharedMemory {
        handle_index: u8,
        byte_len: u64,
        stride: u32,
        slots: u8,
    },
    /// Linux zero-copy image. Each plane descriptor is sent with registration.
    DmaBuf {
        fourcc: u32,
        modifier: u64,
        planes: Vec<DmaBufPlane>,
    },
    /// macOS zero-copy image. The IOSurface id is valid only for the local
    /// owner session and must be looked up by the receiving process.
    IoSurface { surface_id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmaBufPlane {
    pub handle_index: u8,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameSync {
    /// The producer follows the transport's implicit synchronization rules.
    Implicit,
    /// Linux sync-file descriptor accompanying this present message.
    SyncFile { handle_index: u8 },
    /// Value signalled on the registered Metal shared event.
    MetalSharedEvent { event_id: u64, value: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSurfaceDescriptor {
    pub schema_version: u32,
    pub source_id: String,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub color_space: ColorSpace,
    pub alpha_mode: AlphaMode,
    pub transport: SurfaceTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalSurfaceMessage {
    Register {
        surface: ExternalSurfaceDescriptor,
    },
    Present {
        source_id: String,
        generation: u64,
        slot: u8,
        sync: FrameSync,
        presented_at_unix_ns: u64,
    },
    Withdraw {
        source_id: String,
        generation: u64,
    },
}

/// Latest accepted presentation for one registered source. GPU allocations
/// and OS handles stay in the renderer adapter; this state machine owns only
/// the deterministic lifecycle shared by every adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedSurface {
    pub descriptor: ExternalSurfaceDescriptor,
    pub slot: Option<u8>,
    pub presented_at_unix_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryChange {
    Registered {
        source_id: String,
        generation: u64,
    },
    Presented {
        source_id: String,
        generation: u64,
        slot: u8,
    },
    Withdrawn {
        source_id: String,
        generation: u64,
    },
}

#[derive(Debug, Default)]
pub struct ExternalSurfaceRegistry {
    surfaces: BTreeMap<String, PresentedSurface>,
}

impl ExternalSurfaceRegistry {
    pub fn get(&self, source_id: &str) -> Option<&PresentedSurface> {
        self.surfaces.get(source_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &PresentedSurface)> {
        self.surfaces
            .iter()
            .map(|(id, surface)| (id.as_str(), surface))
    }

    pub fn apply(
        &mut self,
        message: ExternalSurfaceMessage,
        received_handles: usize,
    ) -> Result<RegistryChange, String> {
        message.validate(received_handles)?;
        match message {
            ExternalSurfaceMessage::Register { surface } => {
                if self
                    .surfaces
                    .get(&surface.source_id)
                    .is_some_and(|current| current.descriptor.generation >= surface.generation)
                {
                    return Err("external surface registration generation did not advance".into());
                }
                let source_id = surface.source_id.clone();
                let generation = surface.generation;
                self.surfaces.insert(
                    source_id.clone(),
                    PresentedSurface {
                        descriptor: surface,
                        slot: None,
                        presented_at_unix_ns: None,
                    },
                );
                Ok(RegistryChange::Registered {
                    source_id,
                    generation,
                })
            }
            ExternalSurfaceMessage::Present {
                source_id,
                generation,
                slot,
                presented_at_unix_ns,
                ..
            } => {
                let current = self
                    .surfaces
                    .get_mut(&source_id)
                    .ok_or_else(|| format!("external surface is not registered: {source_id}"))?;
                if current.descriptor.generation != generation {
                    return Err("external surface presentation generation is stale".into());
                }
                let slots = match &current.descriptor.transport {
                    SurfaceTransport::SharedMemory { slots, .. } => *slots,
                    SurfaceTransport::DmaBuf { .. } | SurfaceTransport::IoSurface { .. } => 1,
                };
                if slot >= slots {
                    return Err(format!("external surface slot {slot} is out of range"));
                }
                if current
                    .presented_at_unix_ns
                    .is_some_and(|previous| presented_at_unix_ns < previous)
                {
                    return Err("external surface presentation timestamp moved backwards".into());
                }
                current.slot = Some(slot);
                current.presented_at_unix_ns = Some(presented_at_unix_ns);
                Ok(RegistryChange::Presented {
                    source_id,
                    generation,
                    slot,
                })
            }
            ExternalSurfaceMessage::Withdraw {
                source_id,
                generation,
            } => {
                let current = self
                    .surfaces
                    .get(&source_id)
                    .ok_or_else(|| format!("external surface is not registered: {source_id}"))?;
                if current.descriptor.generation != generation {
                    return Err("external surface withdrawal generation is stale".into());
                }
                self.surfaces.remove(&source_id);
                Ok(RegistryChange::Withdrawn {
                    source_id,
                    generation,
                })
            }
        }
    }
}

impl ExternalSurfaceDescriptor {
    pub fn validate(&self, received_handles: usize) -> Result<(), String> {
        if self.schema_version != EXTERNAL_SURFACE_VERSION {
            return Err(format!(
                "unsupported external surface schema version {}",
                self.schema_version
            ));
        }
        validate_source_id(&self.source_id)?;
        if self.generation == 0 {
            return Err("external surface generation must be positive".into());
        }
        if self.width == 0
            || self.height == 0
            || self.width > MAX_SURFACE_DIMENSION
            || self.height > MAX_SURFACE_DIMENSION
        {
            return Err(format!(
                "external surface dimensions must be within 1..={MAX_SURFACE_DIMENSION}"
            ));
        }
        match &self.transport {
            SurfaceTransport::SharedMemory {
                handle_index,
                byte_len,
                stride,
                slots,
            } => {
                validate_handle(*handle_index, received_handles)?;
                if !(1..=MAX_SURFACE_SLOTS).contains(slots) {
                    return Err(format!(
                        "shared memory slots must be within 1..={MAX_SURFACE_SLOTS}"
                    ));
                }
                let minimum_stride = minimum_stride(self.format, self.width)?;
                if *stride < minimum_stride {
                    return Err(format!(
                        "shared memory stride must be at least {minimum_stride}"
                    ));
                }
                let minimum_len = u64::from(*stride)
                    .checked_mul(u64::from(self.height))
                    .and_then(|frame| frame.checked_mul(u64::from(*slots)))
                    .ok_or("shared memory allocation size overflow")?;
                if *byte_len < minimum_len {
                    return Err(format!(
                        "shared memory allocation must contain {minimum_len} bytes"
                    ));
                }
            }
            SurfaceTransport::DmaBuf { planes, .. } => {
                if planes.is_empty() || planes.len() > MAX_SURFACE_PLANES {
                    return Err(format!(
                        "DMA-BUF must contain 1..={MAX_SURFACE_PLANES} planes"
                    ));
                }
                for plane in planes {
                    validate_handle(plane.handle_index, received_handles)?;
                    if plane.stride == 0 {
                        return Err("DMA-BUF plane stride must be positive".into());
                    }
                }
            }
            SurfaceTransport::IoSurface { surface_id } if *surface_id == 0 => {
                return Err("IOSurface id must be positive".into());
            }
            SurfaceTransport::IoSurface { .. } => {}
        }
        Ok(())
    }
}

impl ExternalSurfaceMessage {
    pub fn validate(&self, received_handles: usize) -> Result<(), String> {
        match self {
            Self::Register { surface } => surface.validate(received_handles),
            Self::Present {
                source_id,
                generation,
                sync,
                ..
            } => {
                validate_source_id(source_id)?;
                if *generation == 0 {
                    return Err("external surface generation must be positive".into());
                }
                if let FrameSync::SyncFile { handle_index } = sync {
                    validate_handle(*handle_index, received_handles)?;
                }
                Ok(())
            }
            Self::Withdraw {
                source_id,
                generation,
            } => {
                validate_source_id(source_id)?;
                if *generation == 0 {
                    return Err("external surface generation must be positive".into());
                }
                Ok(())
            }
        }
    }
}

pub fn valid_source_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        })
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."))
}

fn validate_source_id(value: &str) -> Result<(), String> {
    if !valid_source_id(value) {
        return Err("external surface source id is invalid".into());
    }
    Ok(())
}

fn validate_handle(index: u8, received_handles: usize) -> Result<(), String> {
    if usize::from(index) >= received_handles {
        return Err(format!("external surface handle index {index} is missing"));
    }
    Ok(())
}

fn minimum_stride(format: PixelFormat, width: u32) -> Result<u32, String> {
    let bytes_per_pixel = match format {
        PixelFormat::Rgba8Unorm | PixelFormat::Bgra8Unorm | PixelFormat::R32Float => 4,
        PixelFormat::Rgba16Float => 8,
        PixelFormat::Nv12 => return Err("NV12 shared-memory fallback is not supported".into()),
    };
    width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "external surface stride overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> ExternalSurfaceDescriptor {
        ExternalSurfaceDescriptor {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "radioman/main".into(),
            generation: 1,
            width: 640,
            height: 480,
            format: PixelFormat::Rgba8Unorm,
            color_space: ColorSpace::Srgb,
            alpha_mode: AlphaMode::Opaque,
            transport: SurfaceTransport::SharedMemory {
                handle_index: 0,
                byte_len: 640 * 480 * 4 * 2,
                stride: 640 * 4,
                slots: 2,
            },
        }
    }

    #[test]
    fn validates_a_bounded_double_buffer() {
        assert_eq!(shared().validate(1), Ok(()));
    }

    #[test]
    fn source_ids_are_stable_names_not_paths_or_addresses() {
        assert!(valid_source_id("synesthesia/mix-overview"));
        assert!(!valid_source_id("../mix-overview"));
        assert!(!valid_source_id("https://studio.invalid/view"));
        assert!(!valid_source_id("Mix Overview"));
    }

    #[test]
    fn rejects_missing_handles_and_short_allocations() {
        assert!(shared().validate(0).unwrap_err().contains("missing"));
        let mut descriptor = shared();
        let SurfaceTransport::SharedMemory { byte_len, .. } = &mut descriptor.transport else {
            unreachable!()
        };
        *byte_len -= 1;
        assert!(descriptor.validate(1).unwrap_err().contains("contain"));
    }

    #[test]
    fn present_sync_file_must_arrive_with_its_descriptor() {
        let message = ExternalSurfaceMessage::Present {
            source_id: "speedy/freedoom".into(),
            generation: 7,
            slot: 0,
            sync: FrameSync::SyncFile { handle_index: 0 },
            presented_at_unix_ns: 1,
        };
        assert!(message.validate(0).is_err());
        assert_eq!(message.validate(1), Ok(()));
    }

    #[test]
    fn registry_rejects_stale_generations_and_invalid_slots() {
        let mut registry = ExternalSurfaceRegistry::default();
        registry
            .apply(ExternalSurfaceMessage::Register { surface: shared() }, 1)
            .unwrap();
        assert!(
            registry
                .apply(ExternalSurfaceMessage::Register { surface: shared() }, 1)
                .unwrap_err()
                .contains("advance")
        );
        let bad_slot = ExternalSurfaceMessage::Present {
            source_id: "radioman/main".into(),
            generation: 1,
            slot: 2,
            sync: FrameSync::Implicit,
            presented_at_unix_ns: 9,
        };
        assert!(registry.apply(bad_slot, 0).unwrap_err().contains("range"));
    }

    #[test]
    fn registry_presents_and_withdraws_one_generation() {
        let mut registry = ExternalSurfaceRegistry::default();
        registry
            .apply(ExternalSurfaceMessage::Register { surface: shared() }, 1)
            .unwrap();
        registry
            .apply(
                ExternalSurfaceMessage::Present {
                    source_id: "radioman/main".into(),
                    generation: 1,
                    slot: 1,
                    sync: FrameSync::Implicit,
                    presented_at_unix_ns: 10,
                },
                0,
            )
            .unwrap();
        assert_eq!(registry.get("radioman/main").unwrap().slot, Some(1));
        registry
            .apply(
                ExternalSurfaceMessage::Withdraw {
                    source_id: "radioman/main".into(),
                    generation: 1,
                },
                0,
            )
            .unwrap();
        assert!(registry.get("radioman/main").is_none());
    }
}
