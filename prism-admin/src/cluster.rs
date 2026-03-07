use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInstance {
    pub instance_id: String,
    pub cluster_id: String,
    pub address: String,
    pub version: String,
    pub status: ProxyStatus,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub registered_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub config_version: u64,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyStatus {
    Online,
    Offline,
    Degraded,
}

/// Config scope — which clusters a config change applies to.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    #[default]
    Global,
    Clusters(Vec<String>),
}

/// A versioned config snapshot for a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigVersion {
    pub version: u64,
    pub cluster_id: String,
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
