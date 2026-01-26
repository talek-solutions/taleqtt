use crate::cluster::replication::ping::PingReplicationMessage;
use crate::cluster::replication::pong::PongReplicationMessage;
use crate::{Filter, SegmentConfig, Strategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub name: String,
    pub nodes: Vec<ClusterNodeSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNodeSettings {
    pub node_id: u16,
    pub service_bus_address: Ipv4Addr,
    pub service_bus_port: u16,
    pub mode: ClusterNodeModeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ClusterNodeModeConfig {
    Master,
    Slave,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClusterInitializationStatus {
    INITIALIZED,
    PENDING,
    FAILED,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    intitialization_status: ClusterInitializationStatus,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ExtendedRouterConfig {
    pub max_connections: usize,
    pub max_outgoing_packet_count: u64,
    pub max_segment_size: usize,
    pub max_segment_count: usize,
    pub custom_segment: Option<HashMap<String, SegmentConfig>>,
    pub initialized_filters: Option<Vec<Filter>>,
    // defaults to Round Robin
    #[serde(default)]
    pub shared_subscriptions_strategy: Strategy,
    pub next_connection_delay_ms: Option<u64>,
}

pub enum ClusterNodeMode {
    MASTER,
    SLAVE,
}

pub struct ClusterNode {
    pub node_id: u16,
    pub host: Ipv4Addr,
    pub service_bus_address: Ipv4Addr,
    pub service_bus_port: u16,
    pub mode: ClusterNodeMode,
}

pub struct ClusterConnectionConfig {
    pub master_id: u16,
    pub nodes: Vec<ClusterNode>,
}

impl TryFrom<ClusterConfig> for ClusterConnectionConfig {
    type Error = &'static str;

    fn try_from(config: ClusterConfig) -> Result<Self, Self::Error> {
        let masters: Vec<_> = config
            .nodes
            .iter()
            .filter(|n| n.mode == ClusterNodeModeConfig::Master)
            .collect();

        if masters.is_empty() {
            return Err("Cluster config must have exactly one master node");
        }
        if masters.len() > 1 {
            return Err("Cluster config must have exactly one master node, found multiple");
        }

        let master_id = masters[0].node_id;

        let nodes = config
            .nodes
            .into_iter()
            .map(|settings| ClusterNode {
                node_id: settings.node_id,
                host: settings.service_bus_address,
                service_bus_address: settings.service_bus_address,
                service_bus_port: settings.service_bus_port,
                mode: match settings.mode {
                    ClusterNodeModeConfig::Master => ClusterNodeMode::MASTER,
                    ClusterNodeModeConfig::Slave => ClusterNodeMode::SLAVE,
                },
            })
            .collect();

        Ok(ClusterConnectionConfig { master_id, nodes })
    }
}

impl Cluster {
    pub fn connect(config: ClusterConnectionConfig) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();
        let ping_message = PingReplicationMessage::new(config.master_id);

        let slave_nodes = config
            .nodes
            .into_iter()
            .filter(|node| matches!(node.mode, ClusterNodeMode::SLAVE));

        for node in slave_nodes {
            let addr = format!("{}:{}", node.service_bus_address, node.service_bus_port);
            let ping_message = ping_message.clone();

            let handle = thread::spawn(move || {
                let mut stream = match TcpStream::connect(&addr) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Failed to connect to {}: {:?}", addr, e);
                        return;
                    }
                };

                if let Err(e) = Self::write_frame(&mut stream, &ping_message) {
                    eprintln!("Failed to write ping frame: {:?}", e);
                    return;
                }

                loop {
                    match Self::read_frame(&mut stream) {
                        Ok(frame) => println!(
                            "Replica ID {:?}",
                            PongReplicationMessage::replica_id_from_message(&frame)
                        ),
                        Err(e) => {
                            eprintln!("read error: {:?}", e);
                            break;
                        }
                    }
                }
            });

            handles.push(handle);
        }

        handles
    }

    fn write_frame(stream: &mut TcpStream, payload: &str) -> Result<(), std::io::Error> {
        let payload_len = payload.len() as u32;
        stream.write_all(&payload_len.to_be_bytes())?;
        stream.write_all(payload.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    fn read_frame(stream: &mut TcpStream) -> Result<String, std::io::Error> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let payload_len = u32::from_be_bytes(len_buf) as usize;

        let mut buf = vec![0u8; payload_len];
        stream.read_exact(&mut buf)?;
        String::from_utf8(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
