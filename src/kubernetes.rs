use crate::config::KubeConfig;
use crate::error::{InstallerError, InstallerResult};
use kube::Client;
use std::env;
use tracing::{debug, info};

/// Kubernetes client wrapper
pub struct KubeClient {
    client: Client,
    cluster_url: String,
}

impl KubeClient {
    /// Create a new Kubernetes client from kubeconfig
    pub async fn new(config: &KubeConfig) -> InstallerResult<Self> {
        debug!("Creating Kubernetes client");

        let cluster = config.get_current_cluster()?;
        let cluster_url = cluster.cluster.server.clone();
        let current_context = config.current_context.clone();

        debug!("Cluster URL: {}", cluster_url);
        debug!("Current context: {}", current_context);

        let client = Client::try_default()
            .await
            .map_err(|e| InstallerError::KubeConnectionError(
                format!("Failed to create Kubernetes client: {}", e)
            ))?;

        info!("Successfully connected to Kubernetes cluster at: {}", cluster_url);

        Ok(Self {
            client,
            cluster_url,
        })
    }

    /// Create a client with explicit kubeconfig path
    pub async fn with_kubeconfig_path(path: &str) -> InstallerResult<Self> {
        debug!("Creating Kubernetes client with explicit kubeconfig: {}", path);

        env::set_var("KUBECONFIG", path);

        let client = Client::try_default()
            .await
            .map_err(|e| InstallerError::KubeConnectionError(
                format!("Failed to create Kubernetes client with kubeconfig {}: {}", path, e)
            ))?;

        Ok(Self {
            client,
            cluster_url: "unknown".to_string(),
        })
    }

    /// Get the underlying kube Client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get cluster URL
    pub fn cluster_url(&self) -> &str {
        &self.cluster_url
    }

    /// Test connection by getting cluster info
    pub async fn test_connection(&self) -> InstallerResult<()> {
        debug!("Testing connection to Kubernetes cluster");
        
        match self.client.apiserver_version().await {
            Ok(version) => {
                info!(
                    "Connected to Kubernetes cluster version: {}.{}",
                    version.major, version.minor
                );
                Ok(())
            }
            Err(e) => Err(InstallerError::KubeConnectionError(
                format!("Failed to connect to cluster: {}", e)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests would require a running Kubernetes cluster
    // Skip in CI/CD environments
}
