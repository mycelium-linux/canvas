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

/// Optional owner-local return path for bounded interaction events. The
/// descriptor is transferred with registration, so it never becomes network
/// configuration or persistent workspace state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceInputTransport {
    UnixDatagram { handle_index: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSurfaceInputPhase {
    Press,
    Move,
    Release,
}

/// How an external widget participates in a canonical Unibus stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceStreamDirection {
    Consume,
    Publish,
}

/// Whether the widget can render/usefully operate without this stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceStreamImportance {
    Needed,
    Wanted,
}

/// Transport-neutral stream declaration supplied by the widget producer.
/// Canvas can turn active `consume` declarations into renewable Unibus
/// interests without understanding the payload contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceStreamRequirement {
    pub stream_id: String,
    pub contract: String,
    pub direction: SurfaceStreamDirection,
    pub importance: SurfaceStreamImportance,
}

/// One normalized pointer event returned to the producer that registered the
/// current surface generation. Coordinates are bounded to the visible surface
/// rather than exposing Canvas's display or workspace geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSurfaceInput {
    pub schema_version: u32,
    pub source_id: String,
    pub generation: u64,
    pub phase: ExternalSurfaceInputPhase,
    pub x: f32,
    pub y: f32,
}

/// Visibility activation sent by Canvas to a producer-owned surface.
///
/// The stream list is copied from the accepted descriptor rather than authored
/// again by Canvas. A producer can therefore acquire authenticated consume
/// leases only while its exact generation is on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSurfaceVisibility {
    pub schema_version: u32,
    pub source_id: String,
    pub generation: u64,
    pub visible: bool,
    pub streams: Vec<SurfaceStreamRequirement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExternalSurfaceHostEvent {
    Pointer(ExternalSurfaceInput),
    Visibility(ExternalSurfaceVisibility),
}

impl ExternalSurfaceInput {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != EXTERNAL_SURFACE_VERSION {
            return Err(format!(
                "unsupported external surface input schema version {}",
                self.schema_version
            ));
        }
        validate_source_id(&self.source_id)?;
        if self.generation == 0 {
            return Err("external surface input generation must be positive".into());
        }
        if !self.x.is_finite()
            || !self.y.is_finite()
            || !(0.0..=1.0).contains(&self.x)
            || !(0.0..=1.0).contains(&self.y)
        {
            return Err("external surface input coordinates must be normalized".into());
        }
        Ok(())
    }
}

impl ExternalSurfaceVisibility {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != EXTERNAL_SURFACE_VERSION {
            return Err(format!(
                "unsupported external surface visibility schema version {}",
                self.schema_version
            ));
        }
        validate_source_id(&self.source_id)?;
        if self.generation == 0 {
            return Err("external surface visibility generation must be positive".into());
        }
        let mut stream_ids = std::collections::BTreeSet::new();
        for stream in &self.streams {
            if stream.direction != SurfaceStreamDirection::Consume {
                return Err("external surface visibility can activate only consume streams".into());
            }
            validate_stream_token("stream id", &stream.stream_id)?;
            validate_stream_token("stream contract", &stream.contract)?;
            if !stream_ids.insert(&stream.stream_id) {
                return Err(format!(
                    "duplicate consume stream declaration {}",
                    stream.stream_id
                ));
            }
        }
        Ok(())
    }
}

impl ExternalSurfaceHostEvent {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Pointer(input) => input.validate(),
            Self::Visibility(visibility) => visibility.validate(),
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<SurfaceInputTransport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<SurfaceStreamRequirement>,
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
        if let Some(SurfaceInputTransport::UnixDatagram { handle_index }) = self.input {
            validate_handle(handle_index, received_handles)?;
        }
        let mut stream_ids = std::collections::BTreeSet::new();
        for stream in &self.streams {
            validate_stream_token("stream id", &stream.stream_id)?;
            validate_stream_token("stream contract", &stream.contract)?;
            if !stream_ids.insert((&stream.stream_id, stream.direction)) {
                return Err(format!(
                    "duplicate {:?} stream declaration {}",
                    stream.direction, stream.stream_id
                ));
            }
        }
        Ok(())
    }
}

fn validate_stream_token(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
        })
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
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
            input: None,
            streams: Vec::new(),
        }
    }

    #[test]
    fn validates_a_bounded_double_buffer() {
        assert_eq!(shared().validate(1), Ok(()));
    }

    #[test]
    fn input_requires_its_own_received_handle() {
        let mut descriptor = shared();
        descriptor.input = Some(SurfaceInputTransport::UnixDatagram { handle_index: 1 });
        assert!(descriptor.validate(1).unwrap_err().contains("index 1"));
        assert_eq!(descriptor.validate(2), Ok(()));
    }

    #[test]
    fn pointer_input_is_normalized_and_generation_bound() {
        let mut input = ExternalSurfaceInput {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "synesthesia/mix-overview".into(),
            generation: 2,
            phase: ExternalSurfaceInputPhase::Press,
            x: 0.5,
            y: 0.75,
        };
        assert_eq!(input.validate(), Ok(()));
        input.x = 1.1;
        assert!(input.validate().is_err());
    }

    #[test]
    fn visibility_activates_only_declared_consume_streams() {
        let mut visibility = ExternalSurfaceVisibility {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "synesthesia/mix-overview".into(),
            generation: 2,
            visible: true,
            streams: vec![SurfaceStreamRequirement {
                stream_id: "synesthesia/mix/snapshot".into(),
                contract: "synesthesia.mix.snapshot.v1".into(),
                direction: SurfaceStreamDirection::Consume,
                importance: SurfaceStreamImportance::Needed,
            }],
        };
        assert_eq!(visibility.validate(), Ok(()));
        visibility.streams[0].direction = SurfaceStreamDirection::Publish;
        assert!(visibility.validate().is_err());
    }

    #[test]
    fn stream_requirements_are_canonical_and_unique_per_direction() {
        let mut descriptor = shared();
        descriptor.streams.push(SurfaceStreamRequirement {
            stream_id: "synesthesia/home-studio/mix-control".into(),
            contract: "synesthesia.mix.control.v1".into(),
            direction: SurfaceStreamDirection::Publish,
            importance: SurfaceStreamImportance::Needed,
        });
        assert_eq!(descriptor.validate(1), Ok(()));
        descriptor.streams.push(descriptor.streams[0].clone());
        assert!(descriptor.validate(1).unwrap_err().contains("duplicate"));
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
