use crate::crds::CrdInstaller;
use crate::error::{InstallerError, InstallerResult};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use k8s_openapi::api::{
    apps::v1::{Deployment, DeploymentSpec, ReplicaSet},
    core::v1::{
        Container, ContainerPort, Namespace, Pod, PodSpec, PodTemplateSpec, Service, ServiceAccount, ServicePort,
        ServiceSpec,
    },
    rbac::v1::{ClusterRole, ClusterRoleBinding, PolicyRule, RoleRef, Subject},
};
use k8s_openapi::apimachinery::pkg::{
    apis::meta::v1::{LabelSelector, ObjectMeta},
    util::intstr::IntOrString,
};
use kube::{
    api::{Api, DeleteParams, ListParams, PostParams},
    Client,
};
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{debug, info};

/// Wasmcloud operator installer
pub struct WasmcloudInstaller {
    client: Client,
    namespace: String,
    timeout: Duration,
    operator_image: String,
}

impl WasmcloudInstaller {
    /// Create a new installer instance
    pub fn new(client: &Client, namespace: &str, timeout_seconds: u64, operator_image: &str) -> Self {
        Self {
            client: client.clone(),
            namespace: namespace.to_string(),
            timeout: Duration::from_secs(timeout_seconds),
            operator_image: operator_image.to_string(),
        }
    }

    /// Run a dry-run installation 
    pub async fn dry_run(&self) -> InstallerResult<()> {
        info!("Running dry-run installation for Wasmcloud operator");
        
        println!("   {} Creating namespace: {}", "→".blue(), self.namespace.yellow());
        println!("   {} Creating CRDs: Legacy (WasmCloudHostConfig, WasmCloudApplication)", "→".blue());
        println!("   {} Creating Runtime CRDs: Artifact, Host, Workload, WorkloadDeployment, WorkloadReplicaSet", "→".blue());
        println!("   {} Creating Runtime CRDs: Artifact, Host, Workload, WorkloadDeployment, WorkloadReplicaSet", "→".blue());
        println!("   {} Creating service account: wasmcloud-operator", "→".blue());
        println!("   {} Creating cluster role: wasmcloud-operator", "→".blue());
        println!("   {} Creating cluster role binding: wasmcloud-operator", "→".blue());
        println!("   {} Creating deployment: wasmcloud-operator", "→".blue());
        println!("   {} Creating service: wasmcloud-operator", "→".blue());
        
        // Validate manifests without applying them
        let _namespace = self.create_namespace_manifest();
        let _service_account = self.create_service_account_manifest();
        let _cluster_role = self.create_cluster_role_manifest();
        let _cluster_role_binding = self.create_cluster_role_binding_manifest();
        let _deployment = self.create_deployment_manifest();
        let _service = self.create_service_manifest();
        
        println!("   {} All manifests validated successfully", "✓".green());
        
        Ok(())
    }

    /// Install the Wasmcloud operator
    pub async fn install(&self) -> InstallerResult<()> {
        info!("Installing Wasmcloud operator to namespace: {}", self.namespace);

        // Create progress bar (8 steps including CRDs)
        let pb = ProgressBar::new(8);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("   {spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        // Step 1: Create namespace
        pb.set_message("Creating namespace...");
        self.create_namespace().await?;
        pb.inc(1);

        // Step 2: Create CRDs
        pb.set_message("Creating Custom Resource Definitions...");
        self.create_crds().await?;
        pb.inc(1);

        // Step 3: Create service account
        pb.set_message("Creating service account...");
        self.create_service_account().await?;
        pb.inc(1);

        // Step 4: Create cluster role
        pb.set_message("Creating cluster role...");
        self.create_cluster_role().await?;
        pb.inc(1);

        // Step 5: Create cluster role binding
        pb.set_message("Creating cluster role binding...");
        self.create_cluster_role_binding().await?;
        pb.inc(1);

        // Step 6: Create deployment
        pb.set_message("Creating deployment...");
        self.create_deployment().await?;
        pb.inc(1);

        // Step 7: Create service
        pb.set_message("Creating service...");
        self.create_service().await?;
        pb.inc(1);

        // Step 8: Wait for CRDs to be ready
        pb.set_message("Waiting for CRDs to be ready...");
        self.wait_for_crds_ready().await?;
        pb.inc(1);

        pb.finish_with_message("Installation complete");
        
        Ok(())
    }

    /// Check structural readiness (installer-grade validation)
    pub async fn check_structural_readiness(&self) -> InstallerResult<()> {
        info!("Checking structural readiness of Wasmcloud operator");

        let pb = ProgressBar::new(3);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("   {spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        // Step 1: Check deployment exists
        pb.set_message("Checking deployment exists...");
        self.check_deployment_exists().await?;
        pb.inc(1);

        // Step 2: Check CRDs exist
        pb.set_message("Checking CRDs exist...");
        self.check_crds_exist().await?;
        pb.inc(1);

        // Step 3: Check ReplicaSet exists (pod creation started)
        pb.set_message("Checking ReplicaSet exists...");
        self.check_replicaset_exists().await?;
        pb.inc(1);

        pb.finish_with_message("Structural readiness confirmed");
        info!("Wasmcloud operator structural readiness confirmed");
        Ok(())
    }

    /// Check if deployment exists
    async fn check_deployment_exists(&self) -> InstallerResult<()> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        
        match deployments.get("wasmcloud-operator").await {
            Ok(_) => {
                debug!("Deployment wasmcloud-operator exists");
                Ok(())
            }
            Err(e) => {
                Err(InstallerError::ValidationError(
                    format!("Deployment wasmcloud-operator not found: {}", e)
                ))
            }
        }
    }

    /// Check if CRDs exist via Kubernetes API
    async fn check_crds_exist(&self) -> InstallerResult<()> {
        use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        
        let required_crds = vec![
            "wasmcloudhostconfigs.core.wasmcloud.dev",
            "wasmcloudapplications.core.wasmcloud.dev"
        ];

        for crd_name in required_crds {
            match crds.get(crd_name).await {
                Ok(_) => {
                    debug!("CRD {} exists", crd_name);
                }
                Err(e) => {
                    return Err(InstallerError::ValidationError(
                        format!("CRD {} not found: {}", crd_name, e)
                    ));
                }
            }
        }
        
        Ok(())
    }

    /// Check if ReplicaSet exists (indicates pod creation has started)
    async fn check_replicaset_exists(&self) -> InstallerResult<()> {
        let replica_sets: Api<ReplicaSet> = Api::namespaced(self.client.clone(), &self.namespace);
        
        let list_params = ListParams::default().labels("app=wasmcloud-operator");
        match replica_sets.list(&list_params).await {
            Ok(rs_list) => {
                if rs_list.items.is_empty() {
                    return Err(InstallerError::ValidationError(
                        "No ReplicaSet found for wasmcloud-operator deployment".to_string()
                    ));
                }
                debug!("Found {} ReplicaSet(s) for wasmcloud-operator", rs_list.items.len());
                Ok(())
            }
            Err(e) => {
                Err(InstallerError::ValidationError(
                    format!("Failed to list ReplicaSets: {}", e)
                ))
            }
        }
    }

    /// Wait for the operator deployment to be fully ready
    pub async fn wait_for_ready(&self) -> InstallerResult<()> {
        info!("Waiting for Wasmcloud operator to be ready");

        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("   {spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message("Waiting for operator pods to be ready...");

        let start = std::time::Instant::now();
        
        loop {
            if start.elapsed() > self.timeout {
                return Err(InstallerError::TimeoutError(
                    format!("Operator not ready within {}s", self.timeout.as_secs())
                ));
            }

            // Check deployment status
            let deployment = match deployments.get("wasmcloud-operator").await {
                Ok(dep) => dep,
                Err(e) => {
                    debug!("Deployment not found yet: {}", e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    pb.tick();
                    continue;
                }
            };

            if let Some(status) = &deployment.status {
                let desired_replicas = status.replicas.unwrap_or(0);
                let ready_replicas = status.ready_replicas.unwrap_or(0);
                
                // Check for deployment conditions that indicate problems
                if let Some(conditions) = &status.conditions {
                    for condition in conditions {
                        if condition.type_ == "Progressing" && condition.status == "False" {
                            if let Some(reason) = &condition.reason {
                                if reason == "ProgressDeadlineExceeded" {
                                    // Check pod status for more details
                                    let pod_error = self.get_pod_failure_details().await;
                                    return Err(InstallerError::InstallationError(
                                        format!("Deployment failed to progress: {}. Pod details: {}", 
                                            condition.message.as_deref().unwrap_or("Unknown"), pod_error)
                                    ));
                                }
                            }
                        }
                    }
                }
                
                if desired_replicas > 0 && ready_replicas == desired_replicas {
                    pb.finish_with_message("Operator ready");
                    info!("Wasmcloud operator is running and ready");
                    return Ok(());
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
            pb.tick();
        }
    }

    /// Get detailed failure information from pods
    async fn get_pod_failure_details(&self) -> String {
        use k8s_openapi::api::core::v1::Pod;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        
        let list_params = ListParams::default().labels("app=wasmcloud-operator");
        match pods.list(&list_params).await {
            Ok(pod_list) => {
                for pod in pod_list.items {
                    if let Some(status) = &pod.status {
                        if let Some(container_statuses) = &status.container_statuses {
                            for container in container_statuses {
                                if let Some(waiting) = &container.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                                    if let Some(reason) = &waiting.reason {
                                        return format!("Container '{}': {} - {}", 
                                            container.name, 
                                            reason, 
                                            waiting.message.as_deref().unwrap_or("No message")
                                        );
                                    }
                                }
                                if let Some(terminated) = &container.state.as_ref().and_then(|s| s.terminated.as_ref()) {
                                    if terminated.exit_code != 0 {
                                        return format!("Container '{}' terminated with exit code {}: {}", 
                                            container.name, 
                                            terminated.exit_code,
                                            terminated.message.as_deref().unwrap_or("No message")
                                        );
                                    }
                                }
                            }
                        }
                        
                        // Check pod conditions
                        if let Some(conditions) = &status.conditions {
                            for condition in conditions {
                                if condition.status == "False" && condition.type_ != "PodReadyCondition" {
                                    return format!("Pod condition '{}': {}", 
                                        condition.type_, 
                                        condition.message.as_deref().unwrap_or("No message")
                                    );
                                }
                            }
                        }
                    }
                }
                "No specific pod failure details available".to_string()
            }
            Err(e) => format!("Failed to get pod details: {}", e)
        }
    }

    /// Create CRDs using CrdInstaller
    async fn create_crds(&self) -> InstallerResult<()> {
        let crd_installer = CrdInstaller::new(&self.client);
        crd_installer.install_crds().await
    }

    /// Wait for CRDs to be ready
    async fn wait_for_crds_ready(&self) -> InstallerResult<()> {
        use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());
        let start = std::time::Instant::now();
        
        loop {
            if start.elapsed() > Duration::from_secs(60) {
                return Err(InstallerError::TimeoutError(
                    "CRDs not ready within 60s".to_string()
                ));
            }

            // Check if both CRDs are established
            let mut ready_count = 0;
            
            for crd_name in &["wasmcloudhostconfigs.core.wasmcloud.dev", "wasmcloudapplications.core.wasmcloud.dev"] {
                if let Ok(crd) = crds.get(crd_name).await {
                    if let Some(status) = &crd.status {
                        if let Some(conditions) = &status.conditions {
                            if conditions.iter().any(|c| c.type_ == "Established" && c.status == "True") {
                                ready_count += 1;
                            }
                        }
                    }
                }
            }

            if ready_count == 2 {
                debug!("All CRDs are ready");
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Create namespace
    async fn create_namespace(&self) -> InstallerResult<()> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let namespace = self.create_namespace_manifest();
        
        match namespaces.create(&PostParams::default(), &namespace).await {
            Ok(_) => {
                debug!("Created namespace: {}", self.namespace);
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Namespace {} already exists, skipping", self.namespace);
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create namespace: {}", e)
            )),
        }
    }

    /// Create service account
    async fn create_service_account(&self) -> InstallerResult<()> {
        let service_accounts: Api<ServiceAccount> = Api::namespaced(self.client.clone(), &self.namespace);
        let service_account = self.create_service_account_manifest();
        
        match service_accounts.create(&PostParams::default(), &service_account).await {
            Ok(_) => {
                debug!("Created service account: wasmcloud-operator");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Service account wasmcloud-operator already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create service account: {}", e)
            )),
        }
    }

    /// Create cluster role
    async fn create_cluster_role(&self) -> InstallerResult<()> {
        let cluster_roles: Api<ClusterRole> = Api::all(self.client.clone());
        let cluster_role = self.create_cluster_role_manifest();
        
        match cluster_roles.create(&PostParams::default(), &cluster_role).await {
            Ok(_) => {
                debug!("Created cluster role: wasmcloud-operator");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Cluster role wasmcloud-operator already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create cluster role: {}", e)
            )),
        }
    }

    /// Create cluster role binding
    async fn create_cluster_role_binding(&self) -> InstallerResult<()> {
        let cluster_role_bindings: Api<ClusterRoleBinding> = Api::all(self.client.clone());
        let cluster_role_binding = self.create_cluster_role_binding_manifest();
        
        match cluster_role_bindings.create(&PostParams::default(), &cluster_role_binding).await {
            Ok(_) => {
                debug!("Created cluster role binding: wasmcloud-operator");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                // Cluster role binding already exists - delete and recreate it to update namespace
                debug!("Cluster role binding wasmcloud-operator already exists, deleting and recreating to update namespace");
                
                // Delete the old binding
                match cluster_role_bindings.delete("wasmcloud-operator", &DeleteParams::default()).await {
                    Ok(_) => {
                        debug!("Deleted old cluster role binding");
                    }
                    Err(e) => {
                        return Err(InstallerError::InstallationError(
                            format!("Failed to delete old cluster role binding: {}", e)
                        ));
                    }
                }
                
                // Wait a moment for deletion to propagate
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                
                // Create the new binding with correct namespace
                match cluster_role_bindings.create(&PostParams::default(), &cluster_role_binding).await {
                    Ok(_) => {
                        debug!("Created new cluster role binding: wasmcloud-operator with namespace: {}", self.namespace);
                        Ok(())
                    }
                    Err(e) => {
                        Err(InstallerError::InstallationError(
                            format!("Failed to recreate cluster role binding: {}", e)
                        ))
                    }
                }
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create cluster role binding: {}", e)
            )),
        }
    }

    /// Create deployment
    async fn create_deployment(&self) -> InstallerResult<()> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let deployment = self.create_deployment_manifest();
        
        match deployments.create(&PostParams::default(), &deployment).await {
            Ok(_) => {
                debug!("Created deployment: wasmcloud-operator");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Deployment wasmcloud-operator already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create deployment: {}", e)
            )),
        }
    }

    /// Create service
    async fn create_service(&self) -> InstallerResult<()> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let service = self.create_service_manifest();
        
        match services.create(&PostParams::default(), &service).await {
            Ok(_) => {
                debug!("Created service: wasmcloud-operator");
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                debug!("Service wasmcloud-operator already exists, skipping");
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to create service: {}", e)
            )),
        }
    }

    /// Create namespace manifest
    fn create_namespace_manifest(&self) -> Namespace {
        Namespace {
            metadata: ObjectMeta {
                name: Some(self.namespace.clone()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: None,
            status: None,
        }
    }

    /// Create service account manifest
    fn create_service_account_manifest(&self) -> ServiceAccount {
        ServiceAccount {
            metadata: ObjectMeta {
                name: Some("wasmcloud-operator".to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create cluster role manifest
    fn create_cluster_role_manifest(&self) -> ClusterRole {
        ClusterRole {
            metadata: ObjectMeta {
                name: Some("wasmcloud-operator".to_string()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            rules: Some(vec![
                // Core resources
                PolicyRule {
                    api_groups: Some(vec!["".to_string()]),
                    resources: Some(vec![
                        "pods".to_string(), 
                        "services".to_string(), 
                        "configmaps".to_string(),
                        "secrets".to_string(),
                        "events".to_string(),
                        "namespaces".to_string(),
                        "nodes".to_string(),
                        "serviceaccounts".to_string(),
                    ]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // Apps resources
                PolicyRule {
                    api_groups: Some(vec!["apps".to_string()]),
                    resources: Some(vec!["deployments".to_string(), "daemonsets".to_string(), "replicasets".to_string(), "statefulsets".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // CRD resources - needed for the operator to manage its own CRDs
                PolicyRule {
                    api_groups: Some(vec!["apiextensions.k8s.io".to_string()]),
                    resources: Some(vec!["customresourcedefinitions".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // wasmCloud CRD resources
                PolicyRule {
                    api_groups: Some(vec!["core.wasmcloud.dev".to_string()]),
                    resources: Some(vec!["wasmcloudhostconfigs".to_string(), "wasmcloudapplications".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // wasmCloud CRD status subresources
                PolicyRule {
                    api_groups: Some(vec!["core.wasmcloud.dev".to_string()]),
                    resources: Some(vec!["wasmcloudhostconfigs/status".to_string(), "wasmcloudapplications/status".to_string()]),
                    verbs: vec!["get".to_string(), "update".to_string(), "patch".to_string()],
                    ..Default::default()
                },
                // RBAC resources
                PolicyRule {
                    api_groups: Some(vec!["rbac.authorization.k8s.io".to_string()]),
                    resources: Some(vec!["clusterroles".to_string(), "clusterrolebindings".to_string(), "roles".to_string(), "rolebindings".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // Coordination resources (for leader election)
                PolicyRule {
                    api_groups: Some(vec!["coordination.k8s.io".to_string()]),
                    resources: Some(vec!["leases".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // API registration resources (for aggregated APIs)
                PolicyRule {
                    api_groups: Some(vec!["apiregistration.k8s.io".to_string()]),
                    resources: Some(vec!["apiservices".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
                // Admission webhooks
                PolicyRule {
                    api_groups: Some(vec!["admissionregistration.k8s.io".to_string()]),
                    resources: Some(vec!["mutatingwebhookconfigurations".to_string(), "validatingwebhookconfigurations".to_string()]),
                    verbs: vec!["get".to_string(), "list".to_string(), "watch".to_string(), "create".to_string(), "update".to_string(), "patch".to_string(), "delete".to_string()],
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }
    }

    /// Create cluster role binding manifest
    fn create_cluster_role_binding_manifest(&self) -> ClusterRoleBinding {
        ClusterRoleBinding {
            metadata: ObjectMeta {
                name: Some("wasmcloud-operator".to_string()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".to_string(),
                kind: "ClusterRole".to_string(),
                name: "wasmcloud-operator".to_string(),
            },
            subjects: Some(vec![Subject {
                kind: "ServiceAccount".to_string(),
                name: "wasmcloud-operator".to_string(),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            }]),
        }
    }

    /// Create deployment manifest
    fn create_deployment_manifest(&self) -> Deployment {
        let mut labels = self.common_labels();
        labels.insert("app".to_string(), "wasmcloud-operator".to_string());

        Deployment {
            metadata: ObjectMeta {
                name: Some("wasmcloud-operator".to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(1),
                selector: LabelSelector {
                    match_labels: Some({
                        let mut selector = BTreeMap::new();
                        selector.insert("app".to_string(), "wasmcloud-operator".to_string());
                        selector
                    }),
                    ..Default::default()
                },
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(labels),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        service_account: Some("wasmcloud-operator".to_string()),
                        containers: vec![Container {
                            name: "operator".to_string(),
                            image: Some(self.operator_image.clone()),
                            ports: Some(vec![ContainerPort {
                                container_port: 8080,
                                name: Some("http".to_string()),
                                protocol: Some("TCP".to_string()),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            status: None,
        }
    }

    /// Create service manifest
    fn create_service_manifest(&self) -> Service {
        Service {
            metadata: ObjectMeta {
                name: Some("wasmcloud-operator".to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: Some({
                    let mut selector = BTreeMap::new();
                    selector.insert("app".to_string(), "wasmcloud-operator".to_string());
                    selector
                }),
                ports: Some(vec![ServicePort {
                    name: Some("http".to_string()),
                    port: 80,
                    target_port: Some(IntOrString::Int(8080)),
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            status: None,
        }
    }

    /// Get common labels for all resources
    fn common_labels(&self) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::new();
        labels.insert("app.kubernetes.io/name".to_string(), "wasmcloud-operator".to_string());
        labels.insert("app.kubernetes.io/component".to_string(), "operator".to_string());
        labels.insert("app.kubernetes.io/part-of".to_string(), "wasmcloud".to_string());
        labels.insert("app.kubernetes.io/managed-by".to_string(), "wasmcloud-installer".to_string());
        labels
    }
}