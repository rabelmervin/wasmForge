use crate::config::KubeConfig;
use crate::error::{InstallerError, InstallerResult};
use kube::Client;
use std::env;
use tracing::{debug, info, warn};

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

        match Client::try_default().await {
            Ok(client) => {
                info!("Successfully connected to Kubernetes cluster at: {}", cluster_url);
                Ok(Self {
                    client,
                    cluster_url,
                })
            }
            Err(e) => {
                let error_msg = e.to_string();
                
                if error_msg.contains("unable to run auth exec") || error_msg.contains("No such file or directory") {
                    warn!("Exec authentication is configured in kubeconfig but the auth tool is not available");
                    return Err(InstallerError::KubeConnectionError(
                        format!(
                            "Failed to create Kubernetes client: {}\n\n\
                            Your kubeconfig uses exec-based authentication. The authentication tool is missing or not in PATH.\n\n\
                            Possible solutions:\n\
                            - For Azure (Gardener/AKS): Install Azure CLI: https://docs.microsoft.com/cli/azure/install-azure-cli\n\
                            - For AWS (EKS): Install aws-iam-authenticator: https://docs.aws.amazon.com/eks/latest/userguide/install-aws-iam-authenticator.html\n\
                            - For GCP (GKE): Install gcloud SDK: https://cloud.google.com/sdk/docs/install\n\
                            - Run 'kubectl get nodes' first to test your authentication setup",
                            error_msg
                        )
                    ));
                }
                
                Err(InstallerError::KubeConnectionError(
                    format!("Failed to create Kubernetes client: {}", error_msg)
                ))
            }
        }
    }

    /// Create a client with explicit kubeconfig path
    pub async fn with_kubeconfig_path(path: &str) -> InstallerResult<Self> {
        debug!("Creating Kubernetes client with explicit kubeconfig: {}", path);

        env::set_var("KUBECONFIG", path);

        match Client::try_default().await {
            Ok(client) => {
                Ok(Self {
                    client,
                    cluster_url: "unknown".to_string(),
                })
            }
            Err(e) => {
                let error_msg = e.to_string();
                
                // Provide helpful suggestions for common auth errors
                if error_msg.contains("unable to run auth exec") || error_msg.contains("No such file or directory") {
                    return Err(InstallerError::KubeConnectionError(
                        format!(
                            "Failed to create Kubernetes client with kubeconfig {}: {}\n\n\
                            This error typically occurs when the kubeconfig uses exec authentication (e.g., for Azure, AWS, GCP)\n\
                            and the required authentication tool is not installed or not in PATH.\n\n\
                            Solutions:\n\
                            1. Install the required authentication tool:\n\
                               - For Azure: Install Azure CLI (az) - https://docs.microsoft.com/cli/azure/install-azure-cli\n\
                               - For AWS: Install AWS IAM Authenticator - https://docs.aws.amazon.com/eks/latest/userguide/install-aws-iam-authenticator.html\n\
                               - For GCP: Install Google Cloud SDK (gcloud) - https://cloud.google.com/sdk/docs/install\n\
                            2. Ensure the tool is in your PATH: which az / which aws-iam-authenticator / which gcloud\n\
                            3. Try running the exec command manually to diagnose the issue\n\
                            4. Check the kubeconfig file for the exact exec command being used",
                            path, error_msg
                        )
                    ));
                }
                
                Err(InstallerError::KubeConnectionError(
                    format!("Failed to create Kubernetes client with kubeconfig {}: {}", path, error_msg)
                ))
            }
        }
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
