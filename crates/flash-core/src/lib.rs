//! Chip-agnostic SPI NOR flash access.
//!
//! Works with any 25-series flash by using only industry-standard commands
//! (JEDEC `0x9F`, `READ 0x03`, `WREN 0x06`, `RDSR 0x05`, `PP 0x02`, erase opcodes
//! discovered through JESD216 SFDP) so no per-chip code is required.
#![cfg_attr(not(test), no_std)]

pub mod driver;
pub mod info;
pub mod plan;
pub mod sfdp;

pub use driver::{Error, Flash};
pub use info::{EraseType, FlashInfo, InfoSource};
pub use plan::{EraseOp, ErasePlan, PlanError};
