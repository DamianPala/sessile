//! The engine behind the sessile mod. `sessile schema` describes every
//! command; `README.md` explains the contract.

pub mod cli;
pub mod error;
pub mod export;
pub mod interrupt;
pub mod introspect;
pub mod live;
pub mod ops;
pub mod output;
pub mod paths;
pub mod pins;
pub mod scan;
pub mod search;
pub mod session;
pub mod turns;
