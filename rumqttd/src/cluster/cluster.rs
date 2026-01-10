use crate::{Filter, RouterConfig, SegmentConfig, Strategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    routerConfig: RouterConfig,
    clusterConfig: ClusterConfig,
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
    pub fn new(cluster_config: ClusterConfig, router_config: ExtendedRouterConfig) -> Self {
        todo!()
    }
}
