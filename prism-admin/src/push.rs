use prism_router::config::types::PrismConfig;

use crate::cluster::{ProxyInstance, ProxyStatus};

pub struct ConfigPusher;

impl ConfigPusher {
    /// Push config to a single proxy instance via HTTP POST.
    pub async fn push_to_proxy(proxy: &ProxyInstance, config: &PrismConfig) -> Result<(), String> {
        let client = reqwest::Client::new();
        let url = format!("http://{}/admin/config/apply", proxy.address);
        client
            .post(&url)
            .json(config)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("push to {}: {}", proxy.instance_id, e))?;
        Ok(())
    }

    /// Push config to all online proxies in a cluster.
    pub async fn push_to_cluster(
        proxies: &[ProxyInstance],
        config: &PrismConfig,
    ) -> Vec<(String, Result<(), String>)> {
        let mut results = Vec::new();
        for proxy in proxies {
            if proxy.status != ProxyStatus::Online {
                continue;
            }
            let result = Self::push_to_proxy(proxy, config).await;
            results.push((proxy.instance_id.clone(), result));
        }
        results
    }
}
