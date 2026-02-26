//! Core cluster state and networking.
//!
//! [`Cluster`] manages the health state of all nodes and spawns the
//! networking threads that maintain inter-node connections. Call
//! [`Cluster::connect`] to start the cluster layer — it returns thread
//! handles and an `Arc<Mutex<Cluster>>` for querying cluster health
//! via [`Cluster::info`].

use crate::cluster::config::{ClusterConnectionConfig, ClusterNodeMode};
use crate::cluster::framing::{read_frame, write_frame};
use crate::cluster::replication::ping::PingReplicationMessage;
use crate::cluster::replication::pong::PongReplicationMessage;
use crate::{Broker, Config};
use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep, JoinHandle};
use std::time::Duration;

/// Connection state and role of a single remote node.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub connected: bool,
    /// Populated after the first successful PONG response from this node.
    pub node_mode: Option<ClusterNodeMode>,
}

/// Snapshot of overall cluster health returned by [`Cluster::info`].
#[derive(Debug, Clone)]
pub struct ClusterInfo {
    /// `true` when every configured node is connected.
    pub healthy: bool,
    /// This node's own role in the cluster.
    pub node_mode: ClusterNodeMode,
    /// Per-node connection state and role, keyed by address.
    pub nodes: HashMap<String, NodeInfo>,
}

const PING_INTERVAL_SECS: u64 = 15;
const READ_TIMEOUT_SECS: u64 = 5;
const CONNECT_RETRY_MAX: u32 = 5;
const CONNECT_RETRY_WAIT_SECS: u64 = 5;

/// Tracks cluster node health and owns the networking threads.
pub struct Cluster {
    node_mode: ClusterNodeMode,
    node_port: u16,
    nodes: HashMap<String, NodeInfo>,
}

impl Cluster {
    /// Starts the cluster networking layer.
    ///
    /// Spawns a TCP listener for inbound connections and one outbound thread
    /// per configured node. Each outbound thread runs a periodic ping-pong
    /// loop, updating the shared `Cluster` state on success or failure.
    pub fn connect(config: ClusterConnectionConfig, broker_config: Config) -> (Vec<JoinHandle<()>>, Arc<Mutex<Cluster>>) {
        let mut nodes = HashMap::new();
        for node in &config.nodes {
            nodes.insert(node.clone(), NodeInfo { connected: false, node_mode: None });
        }

        let cluster = Arc::new(Mutex::new(Cluster {
            node_mode: config.node_mode.clone(),
            node_port: config.node_port,
            nodes,
        }));

        let mut handles = Vec::new();

        // Inbound: accept connections from nodes and respond to PINGs with PONGs
        let listener_port = config.node_port;
        let listener_node_mode = config.node_mode.clone();
        let listener_handle = thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", listener_port))
                .expect("Failed to bind cluster listener");

            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => {
                        let port = listener_port;
                        let mode = listener_node_mode.clone();
                        thread::spawn(move || {
                            Self::handle_inbound(stream, port, &mode);
                        });
                    }
                    Err(e) => eprintln!("Accept error: {:?}", e),
                }
            }
        });
        handles.push(listener_handle);

        // Outbound: connect to each node and run ping-pong loop
        for addr in config.nodes {
            let cluster = Arc::clone(&cluster);
            let node_port = config.node_port;

            let handle = thread::spawn(move || {
                let mut stream = match Self::try_connect(&addr) {
                    Some(s) => s,
                    None => return,
                };

                println!("Connected to {}!", addr);

                loop {
                    let ping_message = PingReplicationMessage::new(node_port);
                    if let Err(e) = write_frame(&mut stream, &ping_message) {
                        eprintln!("Failed to send PING to {}: {:?}", addr, e);
                        cluster.lock().unwrap().disconnect_node(&addr);
                        break;
                    }
                    println!("Sent PING to {}", addr);

                    stream
                        .set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
                        .ok();

                    match read_frame(&mut stream) {
                        Ok(frame) => {
                            match PongReplicationMessage::parse(&frame) {
                                Ok((replica_id, node_mode)) => {
                                    println!(
                                        "Received PONG from {} (replica_id={}, mode={:?}), node connected",
                                        addr, replica_id, node_mode
                                    );
                                    cluster.lock().unwrap().connect_node(&addr, node_mode);
                                }
                                Err(e) => {
                                    eprintln!(
                                        "Invalid PONG from {}: {} (frame: {})",
                                        addr, e, frame
                                    );
                                    cluster.lock().unwrap().disconnect_node(&addr);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("PONG timeout/error from {}: {:?}", addr, e);
                            cluster.lock().unwrap().disconnect_node(&addr);
                            break;
                        }
                    }

                    sleep(Duration::from_secs(PING_INTERVAL_SECS));
                    println!("Cluster Info: {:?}", cluster.lock().unwrap().info());
                }
            });

            handles.push(handle);
        }

        let mut broker = Broker::new(broker_config);
        broker.start().unwrap();
        
        (handles, cluster)
    }

    /// Returns a snapshot of cluster health and per-node status.
    pub fn info(&self) -> ClusterInfo {
        let healthy = self.nodes.values().all(|n| n.connected);
        ClusterInfo {
            healthy,
            node_mode: self.node_mode.clone(),
            nodes: self.nodes.clone(),
        }
    }

    /// Marks a node as connected and records its mode from the PONG response.
    fn connect_node(&mut self, node: &str, mode: ClusterNodeMode) {
        self.nodes.insert(node.to_string(), NodeInfo { connected: true, node_mode: Some(mode) });
    }

    /// Marks a node as disconnected, preserving its last known mode.
    fn disconnect_node(&mut self, node: &str) {
        if let Some(info) = self.nodes.get_mut(node) {
            info.connected = false;
        }
    }

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

    /// Attempts to establish a TCP connection to a node, retrying up to
    /// [`CONNECT_RETRY_MAX`] times with a delay between attempts.
    fn try_connect(addr: &str) -> Option<TcpStream> {
        for retry in 0..=CONNECT_RETRY_MAX {
            if let Ok(stream) = TcpStream::connect(addr) {
                return Some(stream);
            }

            if retry < CONNECT_RETRY_MAX {
                println!(
                    "Failed to connect to {}, retrying in {}s ({}/{})",
                    addr,
                    CONNECT_RETRY_WAIT_SECS,
                    retry + 1,
                    CONNECT_RETRY_MAX
                );
                sleep(Duration::from_secs(CONNECT_RETRY_WAIT_SECS));
            }
        }

        eprintln!(
            "Failed to connect to {} after {} retries",
            addr, CONNECT_RETRY_MAX
        );
        None
    }
}
