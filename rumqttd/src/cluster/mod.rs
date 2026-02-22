//! Multi-node cluster support for rumqttd.
//!
//! Provides node discovery, health tracking, and inter-node communication
//! over TCP using a length-prefixed framing protocol. Each node maintains
//! outbound connections to all configured nodes and a listener for inbound
//! connections, exchanging periodic ping-pong messages to track health.
//!
//! # Modules
//!
//! - [`cluster`] — Core cluster state, health tracking, and networking loop.
//! - [`config`] — TOML-deserializable cluster configuration and resolved connection config.
//! - [`framing`] — Length-prefixed TCP frame encoding/decoding.
//! - [`replication`] — Ping and pong message formats for inter-node heartbeats.

pub mod cluster;
pub mod config;
pub(crate) mod framing;
mod replication;
