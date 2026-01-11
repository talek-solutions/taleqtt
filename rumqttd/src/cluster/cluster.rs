use crate::{Filter, SegmentConfig, Strategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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

impl Cluster {
    pub async fn connect(
        own_port: u16,
        node_1_host: Ipv4Addr,
        node_2_host: Ipv4Addr,
    ) -> Result<(), std::io::Error> {
        let mut stream_node_1 = TcpStream::connect(node_1_host.to_string()).await?;
        let mut stream_node_2 = TcpStream::connect(node_2_host.to_string()).await?;

        let listener = TcpListener::bind(("0.0.0.0", own_port)).await?;

        tokio::spawn(async move {
            loop {
                match Self::read_frame(&mut stream_node_1).await {
                    Ok(frame) => {
                        println!("{:?}", frame);
                    }
                    Err(e) => {
                        eprintln!("{:?}", e);
                    }
                }
            }
        });

        tokio::spawn(async move {
            loop {
                match Self::read_frame(&mut stream_node_2).await {
                    Ok(frame) => {
                        println!("{:?}", frame);
                    }
                    Err(e) => {
                        eprintln!("{:?}", e);
                    }
                }
            }
        });

        Ok(())
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
