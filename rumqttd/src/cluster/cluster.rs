use crate::cluster::replication::ping::PingReplicationMessage;
use crate::cluster::replication::pong::PongReplicationMessage;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread::{self, sleep, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub name: String,
    pub node_mode: ClusterNodeMode,
    pub node_port: u16,
    pub nodes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClusterNodeMode {
    #[default]
    Master,
    Slave,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClusterInitializationStatus {
    Initialized,
    Pending,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    initialization_status: ClusterInitializationStatus,
}

/// Resolved cluster connection configuration derived from `ClusterConfig`.
///
/// Contains this node's mode and port, plus a parsed list of peer addresses.
pub struct ClusterConnectionConfig {
    pub node_mode: ClusterNodeMode,
    pub node_port: u16,
    pub peers: Vec<String>,
}

impl TryFrom<ClusterConfig> for ClusterConnectionConfig {
    type Error = String;

    fn try_from(config: ClusterConfig) -> Result<Self, Self::Error> {
        let peers: Vec<String> = config
            .nodes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if peers.is_empty() {
            return Err("Cluster config must have at least one peer node".to_string());
        }

        Ok(ClusterConnectionConfig {
            node_mode: config.node_mode,
            node_port: config.node_port,
            peers,
        })
    }
}

impl Cluster {
    pub fn connect(config: ClusterConnectionConfig) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();
        let ping_message = PingReplicationMessage::new(config.node_port);

        let listener_port = config.node_port;
        let listener_handle = thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", listener_port))
                .expect("Failed to bind cluster listener");

            for incoming in listener.incoming() {
                match incoming {
                    Ok(mut stream) => {
                        println!("Accepted connection from {}", stream.peer_addr().unwrap());
                        thread::spawn(move || {
                            match Self::read_frame(&mut stream) {
                                Ok(frame) => println!("Received frame: {}", frame),
                                Err(e) => eprintln!("Read error: {:?}", e),
                            }
                        });
                    }
                    Err(e) => eprintln!("Accept error: {:?}", e),
                }
            }
        });
        handles.push(listener_handle);
        
        for addr in config.peers {
            let ping_message = ping_message.clone();

            let handle = thread::spawn(move || {
                let mut stream: TcpStream;
                let max_retries = 5;
                let mut retry = 0;
                
                let wait_sec = 5;
                
                loop {
                    if let Ok(tcp_stream) = TcpStream::connect(&addr) {
                        stream = tcp_stream;
                        println!("Connected to {} !", addr);
                        break;
                    }
                    
                    if retry < max_retries {
                        retry += 1;
                        println!("Retrying in {} seconds", wait_sec);
                        sleep(Duration::from_secs(wait_sec));
                    } else {
                        println!("Failed to connect to {}", addr);
                        return;
                    }
                }
                
                if let Err(e) = Self::write_frame(&mut stream, &ping_message) {
                    eprintln!("Failed to write ping frame: {:?}", e);
                    return;
                }

                println!("Wrote ping frame to {}", addr);
                
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
