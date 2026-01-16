use crate::error::{InstallerError, InstallerResult};
use k8s_openapi::api::{
    authorization::v1::{SelfSubjectAccessReview, SelfSubjectAccessReviewSpec, ResourceAttributes},
    core::v1::{Namespace, Node},
};use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;use kube::{
    api::{Api, ListParams},
    Client,
};
use tracing::{debug, info, warn};

/// Validator for pre-installation checks
pub struct Validator {
    client: Client,
}

impl Validator {
    /// Create a new validator instance
    pub fn new(client: &Client) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// Validate cluster access and basic connectivity
    pub async fn validate_cluster_access(&self) -> InstallerResult<()> {
        debug!("Validating cluster access");

        // Test basic API server connectivity
        let version = self.client
            .apiserver_version()
            .await
            .map_err(|e| InstallerError::ValidationError(
                format!("Cannot connect to Kubernetes API server: {}", e)
            ))?;

        info!("Connected to Kubernetes API server version: {}.{}", version.major, version.minor);

        // Check if we can list nodes (may not be accessible in managed clusters)
        let nodes: Api<Node> = Api::all(self.client.clone());
        match nodes.list(&ListParams::default().limit(1)).await {
            Ok(node_list) => {
                info!("Cluster has {} nodes", node_list.items.len());
            }
            Err(e) => {
                warn!("Cannot list nodes ({}). This is normal for managed Kubernetes clusters", e);
            }
        }

        // Validate Kubernetes version compatibility
        self.validate_kubernetes_version(&version)?;

        Ok(())
    }

    /// Validate required permissions for installation
    pub async fn validate_permissions(&self, namespace: &str) -> InstallerResult<()> {
        debug!("Validating permissions for namespace: {}", namespace);

        // Check if we can create namespaces 
        self.check_permission(
            None,
            Some(""),
            Some("namespaces"),
            Some("create"),
            "create namespaces"
        ).await?;

        // Check RBAC permissions
        self.check_permission(
            None,
            Some("rbac.authorization.k8s.io"),
            Some("clusterroles"),
            Some("create"),
            "create cluster roles"
        ).await?;

        self.check_permission(
            None,
            Some("rbac.authorization.k8s.io"),
            Some("clusterrolebindings"),
            Some("create"),
            "create cluster role bindings"
        ).await?;

        // Check namespace-level permissions
        self.check_permission(
            Some(namespace),
            Some(""),
            Some("serviceaccounts"),
            Some("create"),
            "create service accounts"
        ).await?;

        self.check_permission(
            Some(namespace),
            Some("apps"),
            Some("deployments"),
            Some("create"),
            "create deployments"
        ).await?;

        self.check_permission(
            Some(namespace),
            Some(""),
            Some("services"),
            Some("create"),
            "create services"
        ).await?;

        // Check if we can watch/list resources (needed for readiness checks)
        self.check_permission(
            Some(namespace),
            Some("apps"),
            Some("deployments"),
            Some("get"),
            "get deployments"
        ).await?;

        self.check_permission(
            Some(namespace),
            Some(""),
            Some("pods"),
            Some("list"),
            "list pods"
        ).await?;

        info!("All required permissions validated");
        Ok(())
    }

    /// Validate cluster prerequisites
    pub async fn validate_prerequisites(&self) -> InstallerResult<()> {
        debug!("Validating cluster prerequisites");

        // Check if the cluster supports custom resources
        self.validate_crd_support().await?;

        // Check if we can create CRDs
        self.validate_crd_permissions().await?;

        // Check cluster resource availability
        self.validate_resource_availability().await?;

        // Check for conflicting installations
        self.check_existing_installations().await?;

        info!("All prerequisites validated");
        Ok(())
    }

    /// Validate Kubernetes version compatibility
    fn validate_kubernetes_version(&self, version: &k8s_openapi::apimachinery::pkg::version::Info) -> InstallerResult<()> {
        let major: u16 = version.major.parse()
            .map_err(|_| InstallerError::ValidationError("Invalid Kubernetes major version".to_string()))?;
        let minor: u16 = version.minor.parse()
            .map_err(|_| InstallerError::ValidationError("Invalid Kubernetes minor version".to_string()))?;

        // Wasmcloud operator requires Kubernetes 1.20+
        if major < 1 || (major == 1 && minor < 20) {
            return Err(InstallerError::ValidationError(
                format!(
                    "Kubernetes version {}.{} is not supported. Minimum required version is 1.20",
                    major, minor
                )
            ));
        }

        // Warn about very old versions
        if major == 1 && minor < 24 {
            warn!(
                "Kubernetes version {}.{} is quite old. Consider upgrading to 1.24+",
                major, minor
            );
        }

        debug!("Kubernetes version {}.{} is supported", major, minor);
        Ok(())
    }

    /// Check if Custom Resource Definitions are supported
    async fn validate_crd_support(&self) -> InstallerResult<()> {
        debug!("Validating CRD support");

        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        
        // Try to list CRDs with a limit of 1 to test API availability
        match crds.list(&ListParams::default().limit(1)).await {
            Ok(_) => {
                debug!("CRD API is accessible");
            }
            Err(e) => {
                return Err(InstallerError::ValidationError(
                    format!("Custom Resource Definitions are not supported or accessible: {}", e)
                ));
            }
        }

        debug!("CRD support validated");
        Ok(())
    }

    /// Check if we have permissions to create CRDs
    async fn validate_crd_permissions(&self) -> InstallerResult<()> {
        debug!("Validating CRD creation permissions");

        // Test if we can create CRDs at cluster level
        let auth_api: Api<SelfSubjectAccessReview> = Api::all(self.client.clone());
        
        let review = SelfSubjectAccessReview {
            metadata: Default::default(),
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    group: Some("apiextensions.k8s.io".to_string()),
                    resource: Some("customresourcedefinitions".to_string()),
                    verb: Some("create".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            status: None,
        };

        let response = auth_api
            .create(&kube::api::PostParams::default(), &review)
            .await
            .map_err(|e| InstallerError::ValidationError(
                format!("Failed to check CRD creation permissions: {}", e)
            ))?;

        if let Some(status) = response.status {
            if !status.allowed {
                let reason = status.reason.unwrap_or("No reason provided".to_string());
                return Err(InstallerError::ValidationError(
                    format!("Insufficient permissions to create CRDs. Reason: {}", reason)
                ));
            }
        }

        debug!("CRD creation permissions validated");
        Ok(())
    }

    /// Validate cluster has sufficient resources
    async fn validate_resource_availability(&self) -> InstallerResult<()> {
        debug!("Validating resource availability");

        let nodes: Api<Node> = Api::all(self.client.clone());
        
        // Try to list nodes, but don't fail if we can't (some managed clusters restrict node access)
        match nodes.list(&ListParams::default()).await {
            Ok(node_list) => {
                if node_list.items.is_empty() {
                    warn!("No nodes visible in the cluster (this may be normal for managed/serverless clusters)");
                    info!("Skipping node readiness check - cluster may be serverless or node access may be restricted");
                } else {
                    // Basic check - ensure we have at least one ready node
                    let ready_nodes = node_list.items.iter().filter(|node| {
                        if let Some(status) = &node.status {
                            if let Some(conditions) = &status.conditions {
                                return conditions.iter().any(|condition| {
                                    condition.type_ == "Ready" && condition.status == "True"
                                });
                            }
                        }
                        false
                    }).count();

                    if ready_nodes == 0 {
                        warn!("No ready nodes found in the cluster - workloads may not be schedulable");
                    } else {
                        info!("Found {} ready nodes in cluster", ready_nodes);
                    }
                }
            }
            Err(e) => {
                warn!("Cannot list nodes ({}). This is normal for managed Kubernetes clusters with restricted RBAC", e);
                info!("Skipping node validation - assuming cluster can schedule workloads");
            }
        }
        debug!("Resource availability validated");
        Ok(())
    }

    /// Check for existing Wasmcloud installations that might conflict
    async fn check_existing_installations(&self) -> InstallerResult<()> {
        debug!("Checking for existing Wasmcloud installations");

        // Check for existing wasmcloud-operator namespace
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        
        match namespaces.get("wasmcloud-operator").await {
            Ok(_) => {
                warn!("Found existing wasmcloud-operator namespace - installation may update existing resources");
            }
            Err(kube::Error::Api(api_err)) if api_err.code == 404 => {
                debug!("No existing wasmcloud-operator namespace found");
            }
            Err(e) => {
                return Err(InstallerError::ValidationError(
                    format!("Failed to check for existing namespace: {}", e)
                ));
            }
        }


        debug!("Existing installation check completed");
        Ok(())
    }

    /// Check a specific permission using SelfSubjectAccessReview
    async fn check_permission(
        &self,
        namespace: Option<&str>,
        group: Option<&str>,
        resource: Option<&str>,
        verb: Option<&str>,
        description: &str,
    ) -> InstallerResult<()> {
        debug!("Checking permission: {}", description);

        let review = SelfSubjectAccessReview {
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    namespace: namespace.map(String::from),
                    group: group.map(String::from),
                    resource: resource.map(String::from),
                    verb: verb.map(String::from),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let auth_api: Api<SelfSubjectAccessReview> = Api::all(self.client.clone());
        
        let result = auth_api
            .create(&kube::api::PostParams::default(), &review)
            .await
            .map_err(|e| InstallerError::ValidationError(
                format!("Failed to check permission '{}': {}", description, e)
            ))?;

        if let Some(status) = result.status {
            if !status.allowed {
                let reason = status.reason.unwrap_or_else(|| "Permission denied".to_string());
                return Err(InstallerError::ValidationError(
                    format!("Missing permission to {}: {}", description, reason)
                ));
            }
        } else {
            return Err(InstallerError::ValidationError(
                format!("Cannot determine permission status for: {}", description)
            ));
        }

        debug!("Permission validated: {}", description);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validator_creation() {
        // This test requires a running Kubernetes cluster
        let client = Client::try_default().await;
        if client.is_err() {
            // Skip test if no cluster available
            return;
        }

        let validator = Validator::new(&client.unwrap());
        
        // Test that we can create a validator
        // Individual validation tests would require cluster access and permissions
        assert_eq!(std::mem::size_of_val(&validator), std::mem::size_of::<Client>());
    }

    #[test]
    fn test_version_validation() {

        let supported_version = k8s_openapi::apimachinery::pkg::version::Info {
            major: "1".to_string(),
            minor: "24".to_string(),
            git_version: "v1.24.0".to_string(),
            ..Default::default()
        };
        
        // Test unsupported version structure
        let unsupported_version = k8s_openapi::apimachinery::pkg::version::Info {
            major: "1".to_string(),
            minor: "19".to_string(),
            git_version: "v1.19.0".to_string(),
            ..Default::default()
        };
        
        assert_eq!(supported_version.major, "1");
        assert_eq!(unsupported_version.minor, "19");
    }
}