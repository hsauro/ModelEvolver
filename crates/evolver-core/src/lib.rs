//! Core of ModelEvolver: reaction networks, diffsol BDF simulation,
//! objectives and the genetic algorithm. No UI and no threads.

pub mod compile;
pub mod config;
pub mod export;
pub mod ga;
pub mod network;
pub mod objective;
pub mod osc;
pub mod patterns;
pub mod rng;
pub mod samples;
pub mod simulate;
