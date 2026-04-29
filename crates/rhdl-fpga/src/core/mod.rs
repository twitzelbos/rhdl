#![warn(missing_docs)]
//! Core components (RAMs, DFF, constants, etc)
pub mod barrel_shifter;
pub mod constant;
pub mod counter;
pub mod crc;
pub mod debouncer;
pub mod delay;
pub mod dff;
pub mod divider;
pub mod edge_detector;
pub mod leading_zeros;
pub mod mac;
pub mod one_hot;
pub mod option;
pub mod popcount;
pub mod priority_encoder;
pub mod pulse_stretcher;
pub mod ram;
pub mod round_robin_arbiter;
pub mod slice;
pub mod strict_priority_arbiter;
