//! Core cluster state and networking.
//!
//! [`Cluster`] manages the health state of all nodes and spawns the
//! networking threads that maintain internode connections. Call
//! [`Cluster::connect`] to start the cluster layer — it returns thread
//! handles and an `Arc<Mutex<Cluster>>` for querying cluster health
//! via [`Cluster::info`].

use crate::cluster::config::{ClusterConnectionConfig, ClusterNodeMode};
use crate::cluster::framing::{read_frame, write_frame};
use crate::cluster::replication::ping::PingReplicationMessage;
use crate::cluster::replication::pong::PongReplicationMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep, JoinHandle};
use std::time::{Duration, Instant};

// ── Enums ───────────────────────────────────────────────────────────

/// Health status of a single remote node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeStatus {
    Connected,
    /// Missed too many pings but still retrying.
    Unhealthy,
    /// Gave up reconnecting or never connected.
    Disconnected,
}

/// Errors from a single ping-pong exchange.
enum PingPongError {
    SendFailed(std::io::Error),
    ReadFailed(std::io::Error),
    InvalidPong(&'static str, String),
}

// ── Structs ─────────────────────────────────────────────────────────

/// Connection state and role of a single remote node.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub status: NodeStatus,
    /// Populated after the first successful PONG response from this node.
    pub node_mode: Option<ClusterNodeMode>,
    pub consecutive_failures: u32,
    #[allow(dead_code)]
    pub last_success: Option<Instant>,
}

impl Default for NodeInfo {
    fn default() -> Self {
        NodeInfo {
            status: NodeStatus::Disconnected,
            node_mode: None,
            consecutive_failures: 0,
            last_success: None,
        }
    }
}

impl NodeInfo {
    pub fn init_nodes(addrs: &[String]) -> HashMap<String, NodeInfo> {
        addrs.iter().map(|a| (a.clone(), NodeInfo::default())).collect()
    }
}

/// Serializable snapshot of a single node for the console API.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfoSnapshot {
    pub status: NodeStatus,
    pub node_mode: Option<ClusterNodeMode>,
    pub consecutive_failures: u32,
}

impl From<&NodeInfo> for NodeInfoSnapshot {
    fn from(info: &NodeInfo) -> Self {
        NodeInfoSnapshot {
            status: info.status.clone(),
            node_mode: info.node_mode.clone(),
            consecutive_failures: info.consecutive_failures,
        }
    }
}

/// Snapshot of overall cluster health returned by [`Cluster::info`].
#[derive(Debug, Clone, Serialize)]
pub struct ClusterInfo {
    /// `true` when every configured node is connected.
    pub healthy: bool,
    /// This node's own role in the cluster.
    pub node_mode: ClusterNodeMode,
    /// Per-node connection state and role, keyed by address.
    pub nodes: HashMap<String, NodeInfoSnapshot>,
}

// ── Constants ───────────────────────────────────────────────────────

const PING_INTERVAL_SECS: u64 = 15;
const READ_TIMEOUT_SECS: u64 = 5;
const CONNECT_RETRY_WAIT_SECS: u64 = 5;

// ── Cluster ─────────────────────────────────────────────────────────

/// Tracks cluster node health and owns the networking threads.
pub struct Cluster {
    node_mode: ClusterNodeMode,
    #[allow(dead_code)]
    node_port: u16,
    nodes: HashMap<String, NodeInfo>,
    #[allow(dead_code)]
    max_missed_pings: u32,
    #[allow(dead_code)]
    max_reconnect_attempts: u32,
}

impl From<&ClusterConnectionConfig> for Cluster {
    fn from(config: &ClusterConnectionConfig) -> Self {
        Cluster {
            node_mode: config.node_mode.clone(),
            node_port: config.node_port,
            nodes: NodeInfo::init_nodes(&config.nodes),
            max_missed_pings: config.max_missed_pings,
            max_reconnect_attempts: config.max_reconnect_attempts,
        }
    }
}

impl Cluster {
    /// Starts the cluster networking layer.
    ///
    /// Spawns a TCP listener for inbound connections and one outbound thread
    /// per configured node. Each outbound thread runs a periodic ping-pong
    /// loop with retry and health tracking, updating the shared `Cluster`
    /// state on success or failure.
    pub fn connect(config: ClusterConnectionConfig) -> (Vec<JoinHandle<()>>, Arc<Mutex<Cluster>>) {
        let cluster = Arc::new(Mutex::new(Cluster::from(&config)));
        let mut handles = Vec::new();

        handles.push(Self::spawn_listener(config.node_port, config.node_mode.clone()));

        for addr in &config.nodes {
            handles.push(Self::spawn_outbound(
                addr.clone(),
                Arc::clone(&cluster),
                config.node_port,
                config.max_missed_pings,
                config.max_reconnect_attempts,
            ));
        }

        (handles, cluster)
    }

    /// Returns a snapshot of cluster health and per-node status.
    pub fn info(&self) -> ClusterInfo {
        let healthy = self
            .nodes
            .values()
            .all(|n| n.status == NodeStatus::Connected);
        ClusterInfo {
            healthy,
            node_mode: self.node_mode.clone(),
            nodes: self.nodes.iter().map(|(k, v)| (k.clone(), v.into())).collect(),
        }
    }

    /// Records a successful PONG: resets failure count, marks Connected.
    fn record_success(&mut self, node: &str, mode: ClusterNodeMode) {
        if let Some(info) = self.nodes.get_mut(node) {
            let prev_status = info.status.clone();
            info.status = NodeStatus::Connected;
            info.node_mode = Some(mode);
            info.consecutive_failures = 0;
            info.last_success = Some(Instant::now());

            if prev_status != NodeStatus::Connected {
                println!("Node {} status: {:?} -> Connected", node, prev_status);
            }
        }
    }

    /// Records a ping failure: increments counter, may transition to Unhealthy.
    fn record_failure(&mut self, node: &str, max_missed_pings: u32) {
        if let Some(info) = self.nodes.get_mut(node) {
            info.consecutive_failures += 1;
            let prev_status = info.status.clone();

            if info.consecutive_failures >= max_missed_pings
                && info.status != NodeStatus::Unhealthy
            {
                info.status = NodeStatus::Unhealthy;
                println!(
                    "Node {} status: {:?} -> Unhealthy ({} consecutive failures)",
                    node, prev_status, info.consecutive_failures
                );
            }
        }
    }

    /// Explicitly sets a node's status (used for terminal Disconnected state).
    fn set_node_status(&mut self, node: &str, status: NodeStatus) {
        if let Some(info) = self.nodes.get_mut(node) {
            let prev_status = info.status.clone();
            info.status = status.clone();
            if prev_status != status {
                println!("Node {} status: {:?} -> {:?}", node, prev_status, status);
            }
        }
    }

    // ── Thread spawners ─────────────────────────────────────────────

    /// Spawns the inbound TCP listener that accepts connections and responds
    /// to PINGs with PONGs.
    fn spawn_listener(port: u16, node_mode: ClusterNodeMode) -> JoinHandle<()> {
        thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", port))
                .expect("Failed to bind cluster listener");

            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => {
                        let mode = node_mode.clone();
                        thread::spawn(move || Self::handle_inbound(stream, port, &mode));
                    }
                    Err(e) => eprintln!("Accept error: {:?}", e),
                }
            }
        })
    }

    /// Spawns an outbound thread for a single node that runs the ping-pong
    /// health-check loop with reconnection.
    fn spawn_outbound(
        addr: String,
        cluster: Arc<Mutex<Cluster>>,
        node_port: u16,
        max_missed_pings: u32,
        max_reconnect_attempts: u32,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            Self::run_outbound(&addr, &cluster, node_port, max_missed_pings, max_reconnect_attempts);
        })
    }

    // ── Outbound loop ───────────────────────────────────────────────

    /// Main outbound loop: connect → ping → read pong → sleep → repeat.
    /// Reconnects on failure and gives up after `max_reconnect_attempts`.
    fn run_outbound(
        addr: &str,
        cluster: &Arc<Mutex<Cluster>>,
        node_port: u16,
        max_missed_pings: u32,
        max_reconnect_attempts: u32,
    ) {
        let mut stream: Option<TcpStream> = None;
        let mut reconnect_attempts: u32 = 0;

        loop {
            // Ensure we have a live connection
            if stream.is_none() {
                match TcpStream::connect(addr) {
                    Ok(s) => {
                        println!("Connected to {}!", addr);
                        stream = Some(s);
                        reconnect_attempts = 0;
                    }
                    Err(_) => {
                        reconnect_attempts += 1;
                        println!(
                            "Failed to connect to {}, attempt {}/{}",
                            addr, reconnect_attempts, max_reconnect_attempts
                        );
                        if reconnect_attempts >= max_reconnect_attempts {
                            eprintln!(
                                "Giving up on {} after {} reconnect attempts",
                                addr, max_reconnect_attempts
                            );
                            cluster.lock().unwrap().set_node_status(addr, NodeStatus::Disconnected);
                            return;
                        }
                        sleep(Duration::from_secs(CONNECT_RETRY_WAIT_SECS));
                        continue;
                    }
                }
            }

            let s = stream.as_mut().unwrap();

            match Self::ping_pong(s, addr, node_port) {
                Ok((replica_id, node_mode)) => {
                    println!(
                        "Received PONG from {} (replica_id={}, mode={:?})",
                        addr, replica_id, node_mode
                    );
                    cluster.lock().unwrap().record_success(addr, node_mode);
                }
                Err(PingPongError::SendFailed(e)) => {
                    eprintln!("Failed to send PING to {}: {:?}", addr, e);
                    stream = None;
                    cluster.lock().unwrap().record_failure(addr, max_missed_pings);
                    sleep(Duration::from_secs(CONNECT_RETRY_WAIT_SECS));
                    continue;
                }
                Err(PingPongError::InvalidPong(e, frame)) => {
                    eprintln!("Invalid PONG from {}: {} (frame: {})", addr, e, frame);
                    cluster.lock().unwrap().record_failure(addr, max_missed_pings);
                }
                Err(PingPongError::ReadFailed(e)) => {
                    eprintln!("PONG timeout/error from {}: {:?}", addr, e);
                    stream = None;
                    cluster.lock().unwrap().record_failure(addr, max_missed_pings);
                    sleep(Duration::from_secs(CONNECT_RETRY_WAIT_SECS));
                    continue;
                }
            }

            sleep(Duration::from_secs(PING_INTERVAL_SECS));
            println!("Cluster Info: {:?}", cluster.lock().unwrap().info());
        }
    }

    /// Sends a PING and reads the PONG response from a single stream.
    fn ping_pong(
        stream: &mut TcpStream,
        addr: &str,
        node_port: u16,
    ) -> Result<(u16, ClusterNodeMode), PingPongError> {
        let ping = PingReplicationMessage::new(node_port);
        write_frame(stream, &ping).map_err(PingPongError::SendFailed)?;
        println!("Sent PING to {}", addr);

        stream
            .set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
            .ok();

        let frame = read_frame(stream).map_err(PingPongError::ReadFailed)?;
        PongReplicationMessage::parse(&frame)
            .map_err(|e| PingPongError::InvalidPong(e, frame))
    }

    // ── Inbound handler ─────────────────────────────────────────────

    /// Handles an inbound connection: reads PINGs and responds with PONGs.
    fn handle_inbound(mut stream: TcpStream, node_port: u16, node_mode: &ClusterNodeMode) {
        let node = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        println!("Accepted cluster connection from {}", node);

        loop {
            match read_frame(&mut stream) {
                Ok(frame) => {
                    if PingReplicationMessage::matches(&frame) {
                        println!("Received PING from {}", node);
                        let pong = PongReplicationMessage::new(node_port, node_mode);
                        if let Err(e) = write_frame(&mut stream, &pong) {
                            eprintln!("Failed to send PONG to {}: {:?}", node, e);
                            break;
                        }
                        println!("Sent PONG to {}", node);
                    } else {
                        println!("Unknown message from {}: {}", node, frame);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                    println!("Node {} disconnected", node);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    println!("Node {} disconnected", node);
                    break;
                }
                Err(e) => {
                    eprintln!("Read error from {}: {:?}", node, e);
                    break;
                }
            }
        }
    }
}
