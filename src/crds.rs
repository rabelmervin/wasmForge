use crate::error::{InstallerError, InstallerResult};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::{
    api::{Api, ApiResource, DeleteParams, DynamicObject, GroupVersionKind, ListParams, Patch, PatchParams},
    runtime::wait::{await_condition, conditions},
    Client, ResourceExt,
};
use std::time::Duration;
use tracing::{debug, warn};

/// The only CRD the wasmCloud v1 operator defines. The operator registers it
/// itself on startup, so the installer never writes its schema and can't drift
/// from the operator version being deployed.
pub const HOST_CONFIG_CRD: &str = "wasmcloudhostconfigs.k8s.wasmcloud.dev";

/// Earlier wasmForge versions hand-wrote CRDs and labelled them with this.
/// Uninstall removes them so old installs get cleaned up too.
pub const LEGACY_CRD_SELECTOR: &str = "app.kubernetes.io/managed-by=wasmForge";

/// Typed handle for WasmCloudHostConfig objects, which have no Rust type here
pub fn host_config_resource() -> ApiResource {
    let gvk = GroupVersionKind::gvk("k8s.wasmcloud.dev", "v1alpha1", "WasmCloudHostConfig");
    ApiResource::from_gvk_with_plural(&gvk, "wasmcloudhostconfigs")
}

/// Tracks and removes the CRDs owned by the wasmCloud operator
pub struct CrdManager {
    client: Client,
}

impl CrdManager {
    pub fn new(client: &Client) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// Wait until the operator has registered its CRD and the API server reports it Established
    pub async fn wait_established(&self, timeout: Duration) -> InstallerResult<()> {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let established = await_condition(crds, HOST_CONFIG_CRD, conditions::is_crd_established());

        match tokio::time::timeout(timeout, established).await {
            Ok(Ok(_)) => {
                debug!("CRD {} is established", HOST_CONFIG_CRD);
                Ok(())
            }
            Ok(Err(e)) => Err(InstallerError::InstallationError(
                format!("Failed to watch CRD {}: {}", HOST_CONFIG_CRD, e)
            )),
            Err(_) => Err(InstallerError::TimeoutError(
                format!("Operator did not register CRD {} within {}s", HOST_CONFIG_CRD, timeout.as_secs())
            )),
        }
    }

    /// Whether the operator's CRD exists and is Established
    pub async fn is_established(&self) -> bool {
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        match crds.get_opt(HOST_CONFIG_CRD).await {
            Ok(Some(crd)) => crd_is_established(&crd),
            _ => false,
        }
    }

    /// Delete every WasmCloudHostConfig and wait for the operator to tear down its hosts.
    /// Must run while the operator is still up: each object carries an operator finalizer,
    /// and without the operator the CRD deletion would hang forever. If the operator doesn't
    /// finish in time (e.g. it is crash looping), the finalizers are removed so uninstall can proceed.
    /// Returns how many host configs were deleted.
    pub async fn delete_host_configs(&self, timeout: Duration) -> InstallerResult<usize> {
        let resource = host_config_resource();
        let all: Api<DynamicObject> = Api::all_with(self.client.clone(), &resource);

        let host_configs = match all.list(&ListParams::default()).await {
            Ok(list) => list.items,
            // CRD not registered: nothing to clean up
            Err(kube::Error::Api(err)) if err.code == 404 => return Ok(0),
            Err(e) => return Err(e.into()),
        };

        let mut waits = Vec::new();
        for host_config in &host_configs {
            let (Some(namespace), Some(uid)) = (host_config.namespace(), host_config.uid()) else {
                continue;
            };
            let name = host_config.name_any();
            let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), &namespace, &resource);

            match api.delete(&name, &DeleteParams::default()).await {
                Ok(_) => {}
                Err(kube::Error::Api(err)) if err.code == 404 => continue,
                Err(e) => {
                    return Err(InstallerError::InstallationError(
                        format!("Failed to delete WasmCloudHostConfig {}/{}: {}", namespace, name, e)
                    ))
                }
            }
            waits.push(async move { await_condition(api, &name, conditions::is_deleted(&uid)).await });
        }

        if tokio::time::timeout(timeout, futures::future::join_all(waits)).await.is_err() {
            for host_config in all.list(&ListParams::default()).await?.items {
                let Some(namespace) = host_config.namespace() else { continue };
                let name = host_config.name_any();
                warn!(
                    "Operator did not finalize WasmCloudHostConfig {}/{} within {}s, removing its finalizer. Its host workloads may need manual cleanup",
                    namespace, name, timeout.as_secs()
                );
                let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), &namespace, &resource);
                let patch = serde_json::json!({ "metadata": { "finalizers": null } });
                api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await?;
            }
        }

        Ok(host_configs.len())
    }

    /// Delete the operator's CRD plus any CRDs left behind by older wasmForge versions
    pub async fn delete_crds(&self) -> InstallerResult<()> {
        debug!("Deleting wasmCloud CRDs");

        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());

        let mut crd_names = vec![HOST_CONFIG_CRD.to_string()];
        let legacy = crds.list(&ListParams::default().labels(LEGACY_CRD_SELECTOR)).await?;
        crd_names.extend(legacy.items.into_iter().filter_map(|crd| crd.metadata.name));

        for crd_name in crd_names {
            match crds.delete(&crd_name, &DeleteParams::default()).await {
                Ok(result) => {
                    // CRD removal finishes asynchronously once its custom resources are cleaned up
                    if let Some(uid) = result.left().and_then(|crd| crd.metadata.uid) {
                        let deleted = await_condition(crds.clone(), &crd_name, conditions::is_deleted(&uid));
                        if tokio::time::timeout(Duration::from_secs(60), deleted).await.is_err() {
                            warn!("CRD {} still terminating after 60s", crd_name);
                        }
                    }
                    debug!("Deleted CRD: {}", crd_name);
                }
                Err(kube::Error::Api(err)) if err.code == 404 => {
                    debug!("CRD {} not found, skipping", crd_name);
                }
                Err(e) => {
                    return Err(InstallerError::InstallationError(
                        format!("Failed to delete CRD {}: {}", crd_name, e)
                    ));
                }
            }
        }

        Ok(())
    }
}

pub fn crd_is_established(crd: &CustomResourceDefinition) -> bool {
    crd.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map(|conditions| conditions.iter().any(|c| c.type_ == "Established" && c.status == "True"))
        .unwrap_or(false)
}
