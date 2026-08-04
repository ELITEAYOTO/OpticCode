//! Versioned, bounded edit proposals and their verified transactional lifecycle.

mod apply;
mod build;
mod diff;
mod generation;
mod policy_adapter;
mod rollback;
mod runtime;
mod schema;
mod store;
mod transaction;
mod validation;
mod verification;

pub use apply::*;
pub use diff::*;
pub use generation::*;
pub use policy_adapter::{inspect_edit_workspace, EditWorkspaceObservation};
pub use rollback::*;
pub use runtime::*;
pub use schema::*;
pub use store::*;
pub use validation::*;
pub use verification::*;

pub const EDIT_PLAN_SCHEMA_VERSION: u32 = 1;
pub const PROPOSAL_STORE_SCHEMA_VERSION: u32 = 1;
pub const CHAT_EDIT_REPORT_SCHEMA_VERSION: u32 = 1;
