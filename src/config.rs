use crate::error::{InstallerError, InstallerResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::debug;

/// Represents a kubeconfig file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeConfig {
    #[serde(default)]
    pub api_version: String,

    #[serde(default)]
    pub kind: String,

    #[serde(default)]
    pub clusters: Vec<ClusterInfo>,

    #[serde(default)]
    pub users: Vec<UserInfo>,

    #[serde(default)]
    pub contexts: Vec<ContextInfo>,

    #[serde(rename = "current-context", default)]
    pub current_context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub name: String,
    pub cluster: Cluster,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub server: String,

    #[serde(rename = "certificate-authority-data", default)]
    pub certificate_authority_data: Option<String>,

    #[serde(rename = "certificate-authority", default)]
    pub certificate_authority: Option<String>,

    #[serde(default)]
    pub insecure_skip_tls_verify: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub name: String,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(rename = "client-certificate-data", default)]
    pub client_certificate_data: Option<String>,

    #[serde(rename = "client-certificate", default)]
    pub client_certificate: Option<String>,

    #[serde(rename = "client-key-data", default)]
    pub client_key_data: Option<String>,

    #[serde(rename = "client-key", default)]
    pub client_key: Option<String>,

    #[serde(default)]
    pub token: Option<String>,

    #[serde(default)]
    pub username: Option<String>,

    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInfo {
    pub name: String,
    pub context: Context,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub cluster: String,
    pub user: String,

    #[serde(default)]
    pub namespace: Option<String>,
}

impl KubeConfig {
    /// Load kubeconfig from file path
    pub fn from_file<P: AsRef<Path>>(path: P) -> InstallerResult<Self> {
        let path = path.as_ref();
        debug!("Loading kubeconfig from: {:?}", path);

        let content = std::fs::read_to_string(path).map_err(|e| {
            InstallerError::KubeconfigError(format!("Failed to read kubeconfig file: {}", e))
        })?;

        let config: KubeConfig = serde_yaml::from_str(&content).map_err(|e| {
            InstallerError::KubeconfigError(format!("Failed to parse kubeconfig YAML: {}", e))
        })?;

        config.validate()?;
        debug!("Kubeconfig loaded successfully");

        Ok(config)
    }

    /// Validate kubeconfig structure
    fn validate(&self) -> InstallerResult<()> {
        if self.current_context.is_empty() {
            return Err(InstallerError::KubeconfigError(
                "No current-context specified in kubeconfig".to_string(),
            ));
        }

        let current = self
            .contexts
            .iter()
            .find(|c| c.name == self.current_context)
            .ok_or_else(|| {
                InstallerError::KubeconfigError(format!(
                    "Current context '{}' not found in kubeconfig",
                    self.current_context
                ))
            })?;

        let cluster_name = &current.context.cluster;
        if !self.clusters.iter().any(|c| &c.name == cluster_name) {
            return Err(InstallerError::KubeconfigError(format!(
                "Cluster '{}' referenced by context not found",
                cluster_name
            )));
        }

        let user_name = &current.context.user;
        if !self.users.iter().any(|u| &u.name == user_name) {
            return Err(InstallerError::KubeconfigError(format!(
                "User '{}' referenced by context not found",
                user_name
            )));
        }

        Ok(())
    }

    /// Get the current cluster info
    pub fn get_current_cluster(&self) -> InstallerResult<&ClusterInfo> {
        let current = self
            .contexts
            .iter()
            .find(|c| c.name == self.current_context)
            .ok_or_else(|| {
                InstallerError::KubeconfigError("Current context not found".to_string())
            })?;

        self.clusters
            .iter()
            .find(|c| c.name == current.context.cluster)
            .ok_or_else(|| {
                InstallerError::KubeconfigError("Cluster not found".to_string())
            })
    }

    /// Get the current user info
    pub fn get_current_user(&self) -> InstallerResult<&UserInfo> {
        let current = self
            .contexts
            .iter()
            .find(|c| c.name == self.current_context)
            .ok_or_else(|| {
                InstallerError::KubeconfigError("Current context not found".to_string())
            })?;

        self.users
            .iter()
            .find(|u| u.name == current.context.user)
            .ok_or_else(|| {
                InstallerError::KubeconfigError("User not found".to_string())
            })
    }

    /// Get the namespace from current context
    pub fn get_namespace(&self) -> InstallerResult<String> {
        let current = self
            .contexts
            .iter()
            .find(|c| c.name == self.current_context)
            .ok_or_else(|| {
                InstallerError::KubeconfigError("Current context not found".to_string())
            })?;

        Ok(current
            .context
            .namespace
            .clone()
            .unwrap_or_else(|| "default".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kubeconfig_validation() {
        let config = KubeConfig {
            api_version: "v1".to_string(),
            kind: "Config".to_string(),
            clusters: vec![],
            users: vec![],
            contexts: vec![],
            current_context: "test".to_string(),
        };

        let result = config.validate();
        assert!(result.is_err());
    }
}
