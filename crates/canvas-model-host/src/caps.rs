//! Executable capability intersection for wasm app manifests.
//!
//! The rule is the one in the host design docs: an app's effective capabilities
//! are `manifest caps ∩ principal grants ∩ host policy`. Intersection, never
//! union; default deny. A capability the app did not ask for, or a grant the app
//! did not request, has no effect.
//!
//! This module is deliberately only that decision. It is a pure function over
//! three inputs — no wasmtime, no I/O, no environment reads — so the same unit
//! can be reused by the loader, a widget linker, and a test harness. Signing and
//! sigchain verification, the wasmtime linker that only exposes granted imports,
//! per-call authority/scope/quota checks, leases, revocation, and the audit
//! stream are all outside this boundary. They consume an [`EffectiveCaps`]; they
//! do not belong here.
//!
//! Limits have two directions that are easy to confuse:
//!
//! - a manifest *above the host maximum* is rejected ([`Denial::LimitAboveHostMax`]),
//!   because the host cannot honour it; it is not silently clamped;
//! - a manifest *above a principal grant maximum* is clamped down to the grant,
//!   because a principal may only reduce what it delegates, never widen it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// The capabilities this host build understands. A manifest naming anything
/// else is rejected rather than having the unknown entry dropped: silently
/// dropping would turn a typo into a quiet loss of privilege.
pub const KNOWN_CAPS: &[&str] = &[
    "soil:gpu",
    "soil:audio",
    "soil:storage.read",
    "soil:storage.write",
    "soil:net",
    "soil:screen",
    // BLE is three caps, not one: observing a link, owning the radio, and
    // mutating a characteristic are separate authorities (wit/soil-ble.wit).
    "soil:ble.observe",
    "soil:ble.connect",
    "soil:ble.write",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub memory_mb: u32,
    pub fuel: u64,
    pub epoch_ms: u32,
}

impl Limits {
    /// The first field on which `self` exceeds `max`, in stable field order.
    fn first_above(&self, max: &Limits) -> Option<&'static str> {
        if self.memory_mb > max.memory_mb {
            Some("memory_mb")
        } else if self.fuel > max.fuel {
            Some("fuel")
        } else if self.epoch_ms > max.epoch_ms {
            Some("epoch_ms")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    pub id: String,
    pub version: String,
    pub caps: BTreeSet<String>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalGrants {
    pub principal: String,
    pub caps: BTreeSet<String>,
    /// A principal may delegate at most this much; delegation only reduces.
    pub max_limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPolicy {
    pub allowed: BTreeSet<String>,
    pub denied: BTreeSet<String>,
    pub max_limits: Limits,
    pub min_memory_mb: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCaps {
    pub caps: BTreeSet<String>,
    pub limits: Limits,
    /// Limit fields reduced to a principal's grant maximum, in stable order:
    /// `memory_mb`, `fuel`, `epoch_ms`.
    pub clamped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    UnknownCap(String),
    NotGranted(String),
    DeniedByPolicy(String),
    InvalidManifest(String),
    LimitAboveHostMax(String),
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCap(cap) => write!(f, "unknown capability: {cap}"),
            Self::NotGranted(cap) => write!(f, "capability not granted: {cap}"),
            Self::DeniedByPolicy(cap) => write!(f, "capability denied by host policy: {cap}"),
            Self::InvalidManifest(field) => write!(f, "invalid manifest field: {field}"),
            Self::LimitAboveHostMax(field) => {
                write!(f, "manifest limit above host maximum: {field}")
            }
        }
    }
}

impl std::error::Error for Denial {}

fn is_known(cap: &str) -> bool {
    KNOWN_CAPS.contains(&cap)
}

/// Decide which caps and limits an app actually runs with.
///
/// Checks run in a fixed order so the same inputs always produce the same
/// denial: manifest shape (caps, then limit sanity), then host maximums, then
/// per-cap authority, then the principal clamp. No check is advisory.
pub fn effective_caps(
    manifest: &AppManifest,
    grants: &PrincipalGrants,
    policy: &HostPolicy,
) -> Result<EffectiveCaps, Denial> {
    // 1. Every requested cap must name something this host understands. Empty
    //    strings are rejected too; they would otherwise be a cap-shaped hole.
    for cap in &manifest.caps {
        if cap.is_empty() || !is_known(cap) {
            return Err(Denial::UnknownCap(cap.clone()));
        }
    }

    // 2. A manifest below the host's memory floor, or with a zero fuel/epoch
    //    budget, is not a runnable request.
    if manifest.limits.memory_mb < policy.min_memory_mb {
        return Err(Denial::InvalidManifest("memory_mb".into()));
    }
    if manifest.limits.fuel == 0 {
        return Err(Denial::InvalidManifest("fuel".into()));
    }
    if manifest.limits.epoch_ms == 0 {
        return Err(Denial::InvalidManifest("epoch_ms".into()));
    }

    // 3. Above the host maximum is a rejection, not a silent clamp: the host
    //    cannot honour the request, so pretending it did would be a lie.
    if let Some(field) = manifest.limits.first_above(&policy.max_limits) {
        return Err(Denial::LimitAboveHostMax(field.into()));
    }

    // 4. Authority is per requested cap: not granted fails closed, and an
    //    explicitly denied or not-allowed cap fails closed. A cap that is
    //    granted and allowed but not requested is simply not effective.
    let mut caps = BTreeSet::new();
    for cap in &manifest.caps {
        if !grants.caps.contains(cap) {
            return Err(Denial::NotGranted(cap.clone()));
        }
        if policy.denied.contains(cap) || !policy.allowed.contains(cap) {
            return Err(Denial::DeniedByPolicy(cap.clone()));
        }
        caps.insert(cap.clone());
    }

    // 5. A principal may only reduce the delegated envelope. Clamp down to the
    //    grant maximum and name the fields, in stable order, for the audit.
    let mut limits = manifest.limits;
    let mut clamped = Vec::new();
    if limits.memory_mb > grants.max_limits.memory_mb {
        limits.memory_mb = grants.max_limits.memory_mb;
        clamped.push("memory_mb".to_string());
    }
    if limits.fuel > grants.max_limits.fuel {
        limits.fuel = grants.max_limits.fuel;
        clamped.push("fuel".to_string());
    }
    if limits.epoch_ms > grants.max_limits.epoch_ms {
        limits.epoch_ms = grants.max_limits.epoch_ms;
        clamped.push("epoch_ms".to_string());
    }

    Ok(EffectiveCaps {
        caps,
        limits,
        clamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(caps: &[&str]) -> BTreeSet<String> {
        caps.iter().map(|cap| (*cap).to_string()).collect()
    }

    fn limits(memory_mb: u32, fuel: u64, epoch_ms: u32) -> Limits {
        Limits {
            memory_mb,
            fuel,
            epoch_ms,
        }
    }

    fn manifest(caps: &[&str], limits: Limits) -> AppManifest {
        AppManifest {
            id: "org.fpl.clock".into(),
            version: "1.2.0".into(),
            caps: set(caps),
            limits,
        }
    }

    fn grants(caps: &[&str], max_limits: Limits) -> PrincipalGrants {
        PrincipalGrants {
            principal: "buddy:ajmwagar".into(),
            caps: set(caps),
            max_limits,
        }
    }

    fn policy(allowed: &[&str], denied: &[&str], max_limits: Limits) -> HostPolicy {
        HostPolicy {
            allowed: set(allowed),
            denied: set(denied),
            max_limits,
            min_memory_mb: 8,
        }
    }

    fn permissive() -> (PrincipalGrants, HostPolicy) {
        (
            grants(KNOWN_CAPS, limits(4_096, 10_000_000_000, 60_000)),
            policy(KNOWN_CAPS, &[], limits(4_096, 10_000_000_000, 60_000)),
        )
    }

    /// The whole point of the module, as a truth table. `in_manifest` means the
    /// app requested the cap; a cap it did not request is not an error, it is
    /// simply absent. Every requested cap that misses any of the other three
    /// conditions fails with the specific denial, never with a silent drop.
    #[test]
    fn truth_table_over_request_grant_allow_and_deny() {
        let cap = "soil:gpu";
        for in_manifest in [false, true] {
            for in_grants in [false, true] {
                for in_allowed in [false, true] {
                    for in_denied in [false, true] {
                        let requested = if in_manifest { &[cap][..] } else { &[][..] };
                        let granted = if in_grants { &[cap][..] } else { &[][..] };
                        let allowed = if in_allowed { &[cap][..] } else { &[][..] };
                        let denied = if in_denied { &[cap][..] } else { &[][..] };
                        let result = effective_caps(
                            &manifest(requested, limits(64, 1_000, 2_000)),
                            &grants(granted, limits(64, 1_000, 2_000)),
                            &policy(allowed, denied, limits(64, 1_000, 2_000)),
                        );
                        let cell = format!(
                            "manifest={in_manifest} grants={in_grants} \
                             allowed={in_allowed} denied={in_denied}"
                        );
                        match result {
                            Ok(effective) if !in_manifest => {
                                assert!(
                                    effective.caps.is_empty(),
                                    "unrequested caps never become effective: {cell}"
                                );
                            }
                            Ok(effective) if in_grants && in_allowed && !in_denied => {
                                assert_eq!(
                                    effective.caps,
                                    set(&[cap]),
                                    "only the fully-permitted request is effective: {cell}"
                                );
                            }
                            Ok(_) => panic!("expected a denial: {cell}"),
                            Err(Denial::NotGranted(denied_cap)) if in_manifest && !in_grants => {
                                assert_eq!(denied_cap, cap, "NotGranted for: {cell}");
                            }
                            Err(Denial::DeniedByPolicy(denied_cap))
                                if in_manifest && in_grants && (in_denied || !in_allowed) =>
                            {
                                assert_eq!(denied_cap, cap, "DeniedByPolicy for: {cell}");
                            }
                            Err(other) => panic!("unexpected denial {other:?} for: {cell}"),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn empty_manifest_caps_is_ok_with_no_effective_caps() {
        let (grants, policy) = permissive();
        let effective =
            effective_caps(&manifest(&[], limits(64, 1_000, 2_000)), &grants, &policy).unwrap();
        assert!(effective.caps.is_empty());
        assert!(effective.clamped.is_empty());
    }

    #[test]
    fn unknown_and_empty_caps_reject_the_manifest() {
        let (grants, policy) = permissive();
        let unknown = effective_caps(
            &manifest(&["soil:telepathy"], limits(64, 1_000, 2_000)),
            &grants,
            &policy,
        );
        assert_eq!(unknown, Err(Denial::UnknownCap("soil:telepathy".into())));

        let empty = effective_caps(&manifest(&[""], limits(64, 1_000, 2_000)), &grants, &policy);
        assert_eq!(empty, Err(Denial::UnknownCap(String::new())));
    }

    /// Manifest limits above the host maximum are rejected per field, not
    /// clamped. `grants` is generous here so only the host ceiling applies.
    #[test]
    fn manifest_above_host_maximum_is_rejected() {
        let (grants, policy) = permissive();
        let cases = [
            (
                limits(4_097, 1_000, 2_000),
                Denial::LimitAboveHostMax("memory_mb".into()),
            ),
            (
                limits(64, 10_000_000_001, 2_000),
                Denial::LimitAboveHostMax("fuel".into()),
            ),
            (
                limits(64, 1_000, 60_001),
                Denial::LimitAboveHostMax("epoch_ms".into()),
            ),
        ];
        for (requested, expected) in cases {
            let result = effective_caps(&manifest(&["soil:gpu"], requested), &grants, &policy);
            assert_eq!(result, Err(expected));
        }
    }

    /// A principal can only reduce. The manifest is within the host ceiling but
    /// above the grant ceiling, so every field clamps and is named in stable
    /// order.
    #[test]
    fn principal_limits_clamp_down_and_name_the_fields() {
        let effective = effective_caps(
            &manifest(&["soil:gpu"], limits(512, 5_000, 9_000)),
            &grants(&["soil:gpu"], limits(256, 4_000, 3_000)),
            &policy(&["soil:gpu"], &[], limits(1_024, 9_000, 10_000)),
        )
        .unwrap();
        assert_eq!(effective.limits, limits(256, 4_000, 3_000));
        assert_eq!(
            effective.clamped,
            vec![
                "memory_mb".to_string(),
                "fuel".to_string(),
                "epoch_ms".to_string()
            ]
        );
    }

    #[test]
    fn only_the_reduced_fields_are_reported_as_clamped() {
        let effective = effective_caps(
            &manifest(&["soil:audio"], limits(512, 1_000, 2_000)),
            &grants(&["soil:audio"], limits(256, 1_000, 2_000)),
            &policy(&["soil:audio"], &[], limits(1_024, 9_000, 10_000)),
        )
        .unwrap();
        assert_eq!(effective.limits, limits(256, 1_000, 2_000));
        assert_eq!(effective.clamped, vec!["memory_mb".to_string()]);
    }

    #[test]
    fn memory_below_the_host_floor_is_invalid() {
        let mut policy = permissive().1;
        policy.min_memory_mb = 32;
        let result = effective_caps(
            &manifest(&["soil:gpu"], limits(16, 1_000, 2_000)),
            &permissive().0,
            &policy,
        );
        assert_eq!(result, Err(Denial::InvalidManifest("memory_mb".into())));
    }

    #[test]
    fn zero_fuel_or_epoch_is_invalid() {
        let (grants, policy) = permissive();
        assert_eq!(
            effective_caps(
                &manifest(&["soil:gpu"], limits(64, 0, 2_000)),
                &grants,
                &policy
            ),
            Err(Denial::InvalidManifest("fuel".into()))
        );
        assert_eq!(
            effective_caps(
                &manifest(&["soil:gpu"], limits(64, 1_000, 0)),
                &grants,
                &policy
            ),
            Err(Denial::InvalidManifest("epoch_ms".into()))
        );
    }

    /// Intersection, not union: authority on one side alone grants nothing.
    #[test]
    fn granted_but_unrequested_or_allowed_but_unrequested_is_not_effective() {
        let effective = effective_caps(
            &manifest(&["soil:gpu"], limits(64, 1_000, 2_000)),
            &grants(KNOWN_CAPS, limits(64, 1_000, 2_000)),
            &policy(KNOWN_CAPS, &[], limits(64, 1_000, 2_000)),
        )
        .unwrap();
        assert_eq!(effective.caps, set(&["soil:gpu"]));
    }

    #[test]
    fn manifest_round_trips_and_rejects_unknown_fields() {
        let source = serde_json::json!({
            "id": "org.fpl.clock",
            "version": "1.2.0",
            "caps": ["soil:audio", "soil:gpu"],
            "limits": {"memory_mb": 64, "fuel": 500000000_u64, "epoch_ms": 2000}
        });
        let parsed: AppManifest = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(parsed.caps, set(&["soil:gpu", "soil:audio"]));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), source);

        let mut extra = source.clone();
        extra["signer"] = serde_json::json!("buddy:ajmwagar");
        assert!(serde_json::from_value::<AppManifest>(extra).is_err());

        let mut bad_limit = source;
        bad_limit["limits"]["messages_per_sec"] = serde_json::json!(10);
        assert!(serde_json::from_value::<AppManifest>(bad_limit).is_err());
    }

    /// Same inputs, same result: sets iterate in sorted order and clamped field
    /// names are emitted in a fixed order rather than hash order.
    #[test]
    fn evaluation_is_deterministic() {
        let (base_grants, base_policy) = permissive();
        let app = manifest(
            &["soil:screen", "soil:gpu", "soil:storage.read", "soil:audio"],
            limits(64, 1_000, 2_000),
        );
        let first = effective_caps(&app, &base_grants, &base_policy).unwrap();
        let second = effective_caps(&app, &base_grants, &base_policy).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.caps.iter().cloned().collect::<Vec<_>>(),
            vec!["soil:audio", "soil:gpu", "soil:screen", "soil:storage.read"]
        );

        let clamping = effective_caps(
            &manifest(&["soil:gpu"], limits(512, 5_000, 9_000)),
            &grants(&["soil:gpu"], limits(256, 4_000, 3_000)),
            &policy(&["soil:gpu"], &[], limits(1_024, 9_000, 10_000)),
        )
        .unwrap();
        assert_eq!(
            clamping.clamped,
            vec!["memory_mb", "fuel", "epoch_ms"],
            "clamped fields keep a stable order"
        );
    }
}
