use crate::cluster::replication::ping::PingReplicationMessage;
use crate::cluster::replication::pong::PongReplicationMessage;
use crate::{Filter, SegmentConfig, Strategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::either::Either;
use x509_parser::nom::HexDisplay;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub name: String,
    pub cluster_balancer_port: usize,
    pub nodes: Vec<ClusterNodeSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNodeSettings {
    pub cluster_node_port: usize,
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
    pub host: Ipv4Addr,
    pub service_bus_address: Ipv4Addr,
    pub mode: ClusterNodeMode,
}

pub struct ClusterConnectionConfig {
    mode: ClusterNodeMode,
    port: u16,
    nodes: Vec<ClusterNode>,
}

impl Cluster {
    pub async fn connect(config: ClusterConnectionConfig) {
        for node in &config.nodes {
            let addr = node.service_bus_address.clone();

            tokio::spawn(async move {
                let mut stream = TcpStream::connect(addr.to_string()).await.unwrap();
                let stream_rc = Arc::new(Mutex::new(stream));
                
                Self::write_frame(&stream_rc.clone(), &PingReplicationMessage::new(1212)).await;

                loop {
                    match Self::read_frame(&mut stream).await {
                        Ok(frame) => println!(
                            "Replica ID {:?}",
                            PongReplicationMessage::replica_id_from_message(&*frame)
                        ),
                        Err(e) => {
                            eprintln!("read error: {:?}", e);
                            break; // or reconnect, etc.
                        }
                    }
                }
            });
        }

        Ok(()).expect("TODO: panic message");
    }

    async fn write_frame(stream: &mut TcpStream, payload: &str) {
        let payload_len = payload.len();
        stream.write_u32(payload_len as u32).await.unwrap();
        stream.write_all(payload.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    }

    async fn read_frame(stream: &mut TcpStream) -> Result<String, std::io::Error> {
        let payload_len = stream.read_u32().await?;
        let mut buf = vec![0u8; payload_len as usize];
        stream.read_exact(&mut buf).await?;

        Ok(buf.to_hex(payload_len as usize))
    }

    fn get_greeting_message() -> &'static str {
        "test"
    }
}
