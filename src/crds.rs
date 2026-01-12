use crate::error::{InstallerError, InstallerResult};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionSpec, CustomResourceDefinitionVersion,
    CustomResourceDefinitionNames, CustomResourceValidation, JSONSchemaProps,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::{api::{Api, PostParams}, Client};
use std::collections::BTreeMap;
use tracing::debug;

/// CRD installer for wasmCloud
pub struct CrdInstaller {
    client: Client,
}

impl CrdInstaller {
    /// Create a new CRD installer
    pub fn new(client: &Client) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// Install all wasmCloud CRDs
    pub async fn install_crds(&self) -> InstallerResult<()> {
        // Legacy CRDs (keeping for backward compatibility)
        self.create_host_config_crd().await?;
        self.create_application_crd().await?;
        
        // Runtime CRDs (runtime.wasmcloud.dev/v1alpha1)
        self.create_artifact_crd().await?;
        self.create_host_crd().await?;
        self.create_workload_crd().await?;
        self.create_workload_deployment_crd().await?;
        self.create_workload_replica_set_crd().await?;
        
        Ok(())
    }

    /// Create WasmCloudHostConfig CRD
    async fn create_host_config_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.host_config_crd_manifest();
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created WasmCloudHostConfig CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("WasmCloudHostConfig CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create WasmCloudHostConfig CRD: {}", e)
            )),
        }
    }

    /// Create WasmCloudApplication CRD
    async fn create_application_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.application_crd_manifest();
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created WasmCloudApplication CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("WasmCloudApplication CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create WasmCloudApplication CRD: {}", e)
            )),
        }
    }

    /// Create Artifact CRD
    async fn create_artifact_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.create_runtime_crd("artifacts", "Artifact", vec!["art"]);
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created Artifact CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Artifact CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create Artifact CRD: {}", e)
            )),
        }
    }

    /// Create Host CRD
    async fn create_host_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.create_runtime_crd("hosts", "Host", vec!["whost"]);
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created Host CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Host CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create Host CRD: {}", e)
            )),
        }
    }

    /// Create Workload CRD
    async fn create_workload_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.create_runtime_crd("workloads", "Workload", vec!["wl"]);
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created Workload CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Workload CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create Workload CRD: {}", e)
            )),
        }
    }

    /// Create WorkloadDeployment CRD
    async fn create_workload_deployment_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.create_runtime_crd("workloaddeployments", "WorkloadDeployment", vec!["wldep"]);
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created WorkloadDeployment CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("WorkloadDeployment CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create WorkloadDeployment CRD: {}", e)
            )),
        }
    }

    /// Create WorkloadReplicaSet CRD
    async fn create_workload_replica_set_crd(&self) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let crd = self.create_runtime_crd("workloadreplicasets", "WorkloadReplicaSet", vec!["wlrs"]);
        
        match crds.create(&PostParams::default(), &crd).await {
            Ok(_) => {
                debug!("Created WorkloadReplicaSet CRD");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("WorkloadReplicaSet CRD already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create WorkloadReplicaSet CRD: {}", e)
            )),
        }
    }

    /// Helper to create runtime CRDs with the standard pattern
    fn create_runtime_crd(&self, plural: &str, kind: &str, short_names: Vec<&str>) -> CustomResourceDefinition {
        CustomResourceDefinition {
            metadata: ObjectMeta {
                name: Some(format!("{}.runtime.wasmcloud.dev", plural)),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: CustomResourceDefinitionSpec {
                group: "runtime.wasmcloud.dev".to_string(),
                names: CustomResourceDefinitionNames {
                    kind: kind.to_string(),
                    plural: plural.to_string(),
                    singular: Some(kind.to_lowercase()),
                    short_names: Some(short_names.iter().map(|s| s.to_string()).collect()),
                    ..Default::default()
                },
                scope: "Namespaced".to_string(),
                versions: vec![CustomResourceDefinitionVersion {
                    name: "v1alpha1".to_string(),
                    served: true,
                    storage: true,
                    schema: Some(CustomResourceValidation {
                        open_api_v3_schema: Some(self.basic_schema()),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            status: None,
        }
    }

    /// Create WasmCloudHostConfig CRD manifest
    fn host_config_crd_manifest(&self) -> CustomResourceDefinition {
        CustomResourceDefinition {
            metadata: ObjectMeta {
                name: Some("wasmcloudhostconfigs.core.wasmcloud.dev".to_string()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: CustomResourceDefinitionSpec {
                group: "core.wasmcloud.dev".to_string(),
                names: CustomResourceDefinitionNames {
                    kind: "WasmCloudHostConfig".to_string(),
                    plural: "wasmcloudhostconfigs".to_string(),
                    singular: Some("wasmcloudhostconfig".to_string()),
                    short_names: Some(vec!["wchc".to_string()]),
                    ..Default::default()
                },
                scope: "Namespaced".to_string(),
                versions: vec![CustomResourceDefinitionVersion {
                    name: "v1alpha1".to_string(),
                    served: true,
                    storage: true,
                    schema: Some(CustomResourceValidation {
                        open_api_v3_schema: Some(self.basic_schema()),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            status: None,
        }
    }

    /// Create WasmCloudApplication CRD manifest
    fn application_crd_manifest(&self) -> CustomResourceDefinition {
        CustomResourceDefinition {
            metadata: ObjectMeta {
                name: Some("wasmcloudapplications.core.wasmcloud.dev".to_string()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: CustomResourceDefinitionSpec {
                group: "core.wasmcloud.dev".to_string(),
                names: CustomResourceDefinitionNames {
                    kind: "WasmCloudApplication".to_string(),
                    plural: "wasmcloudapplications".to_string(),
                    singular: Some("wasmcloudapplication".to_string()),
                    short_names: Some(vec!["wcapp".to_string()]),
                    ..Default::default()
                },
                scope: "Namespaced".to_string(),
                versions: vec![CustomResourceDefinitionVersion {
                    name: "v1alpha1".to_string(),
                    served: true,
                    storage: true,
                    schema: Some(CustomResourceValidation {
                        open_api_v3_schema: Some(self.basic_schema()),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            status: None,
        }
    }

    /// Basic OpenAPI schema that accepts any structure
    fn basic_schema(&self) -> JSONSchemaProps {
        JSONSchemaProps {
            type_: Some("object".to_string()),
            properties: Some({
                let mut props = BTreeMap::new();
                props.insert("spec".to_string(), JSONSchemaProps {
                    type_: Some("object".to_string()),
                    x_kubernetes_preserve_unknown_fields: Some(true),
                    ..Default::default()
                });
                props.insert("status".to_string(), JSONSchemaProps {
                    type_: Some("object".to_string()),
                    x_kubernetes_preserve_unknown_fields: Some(true),
                    ..Default::default()
                });
                props
            }),
            ..Default::default()
        }
    }

    /// Get common labels for CRDs
    fn common_labels(&self) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::new();
        labels.insert("app.kubernetes.io/name".to_string(), "wasmcloud-operator".to_string());
        labels.insert("app.kubernetes.io/component".to_string(), "crd".to_string());
        labels.insert("app.kubernetes.io/part-of".to_string(), "wasmcloud".to_string());
        labels.insert("app.kubernetes.io/managed-by".to_string(), "wasmcloud-installer".to_string());
        labels
    }
}
