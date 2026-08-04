//! Deny-by-default authorization boundary for every interactive OpticCode action.

mod approval;
mod audit;
mod engine;
mod model;
mod paths;

pub use approval::{
    ApprovalBinding, ApprovalError, ApprovalFileBinding, ApprovalGrant, ApprovalState,
    ApprovalStore, NativeConfirmation, DEFAULT_APPROVAL_TTL_SECONDS, MAX_APPROVAL_TTL_SECONDS,
};
pub use audit::{AuditEvent, AuditQuery, AuditStore, AuditStoreReport};
pub use engine::{PolicyEngine, PolicyError, PolicyPreflight};
pub use model::*;
pub use paths::{PathFingerprint, PathSafetyReport};

pub const POLICY_PROTOCOL_ID: &str = "opticcode.policy";
pub const POLICY_SCHEMA_VERSION: u32 = 1;
pub const POLICY_VERSION: &str = "opticcode.default.v1";
