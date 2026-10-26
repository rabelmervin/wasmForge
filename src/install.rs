use crate::crds::{CrdManager, HOST_CONFIG_CRD};
use crate::error::{InstallerError, InstallerResult};
use colored::*;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use crate::manifests::{
    operator_pod_selector, Manifests, API_SERVICE_NAME, DELEGATOR_BINDING_NAME, OPERATOR_NAME, OPERATOR_PORT,
};
use k8s_openapi::api::{
    apps::v1::{Deployment, ReplicaSet},
    core::v1::{Event, Namespace, Pod, Service, ServiceAccount},
    rbac::v1::{ClusterRole, ClusterRoleBinding},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams},
    runtime::{
        wait::{await_condition, conditions},
        watcher::{self, watcher},
        WatchStreamExt,
    },
    Client, Resource,
};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Field manager used for server-side apply
const FIELD_MANAGER: &str = "wasmForge";

/// Container waiting reasons that won't resolve without intervention
const FATAL_WAITING_REASONS: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "InvalidImageName",
    "ErrImageNeverPull",
    "CreateContainerConfigError",
    "CreateContainerError",
];

#[derive(Debug, Clone, Serialize)]
pub struct InstallationStatus {
    pub namespace_exists: bool,
    pub namespace_name: String,
    pub deployment_exists: bool,
    pub deployment_ready: bool,
    pub deployment_replicas: Option<Replicas>,
    pub service_exists: bool,
    pub service_account_exists: bool,
    pub cluster_role_exists: bool,
    pub cluster_role_binding_exists: bool,
    pub crds_installed: Vec<String>,
    pub crds_missing: Vec<String>,
    pub pod_status: Vec<String>,
    pub overall_status: OverallStatus,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Replicas {
    pub ready: i32,
    pub desired: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallStatus {
    NotInstalled,
    PartiallyInstalled,
    InstalledNotReady,
    InstalledAndReady,
}

impl InstallationStatus {
    pub fn print_status_report(&self) {
        use colored::*;

        println!("{}", format!("📊 Installation Status for namespace: {}", self.namespace_name).bold());
        println!("{}", "=".repeat(50));
        println!();

        // Overall status
        let status_icon = match self.overall_status {
            OverallStatus::NotInstalled => "✗",
            OverallStatus::PartiallyInstalled => "⚠️",
            OverallStatus::InstalledNotReady => "🔄",
            OverallStatus::InstalledAndReady => "✓",
        };

        let status_text = match self.overall_status {
            OverallStatus::NotInstalled => "Not Installed".red(),
            OverallStatus::PartiallyInstalled => "Partially Installed".yellow(),
            OverallStatus::InstalledNotReady => "Installed (Not Ready)".yellow(),
            OverallStatus::InstalledAndReady => "Installed and Ready".green(),
        };

        println!("{} Overall Status: {}", status_icon, status_text.bold());
        println!();

        // Namespace status
        let ns_icon = if self.namespace_exists { "✓" } else { "✗" };
        println!("{} Namespace: {}", ns_icon, self.namespace_name);

        // Deployment status
        let deploy_icon = if self.deployment_exists {
            if self.deployment_ready { "✓" } else { "🔄" }
        } else { "✗" };

        let deploy_status = if self.deployment_exists {
            if let Some(replicas) = self.deployment_replicas {
                format!("wasmcloud-operator ({}/{})", replicas.ready, replicas.desired)
            } else {
                "wasmcloud-operator (status unknown)".to_string()
            }
        } else {
            "Not found".to_string()
        };
        println!("{} Deployment: {}", deploy_icon, deploy_status);

        // Pod status
        if !self.pod_status.is_empty() {
            for pod in &self.pod_status {
                println!("   └─ Pod: {}", pod);
            }
        }

        // Service status
        let svc_icon = if self.service_exists { "✓" } else { "✗" };
        println!("{} Service: {}", svc_icon, if self.service_exists { "wasmcloud-operator" } else { "Not found" });

        // RBAC status
        println!();
        println!("RBAC Resources:");
        let sa_icon = if self.service_account_exists { "✓" } else { "✗" };
        println!("   {} ServiceAccount: {}", sa_icon, if self.service_account_exists { "wasmcloud-operator" } else { "Not found" });

        let cr_icon = if self.cluster_role_exists { "✓" } else { "✗" };
        println!("   {} ClusterRole: {}", cr_icon, if self.cluster_role_exists { "wasmcloud-operator" } else { "Not found" });

        let crb_icon = if self.cluster_role_binding_exists { "✓" } else { "✗" };
        println!("   {} ClusterRoleBinding: {}", crb_icon, if self.cluster_role_binding_exists { "wasmcloud-operator" } else { "Not found" });

        // CRDs status
        println!();
        println!("Custom Resource Definitions:");
        if !self.crds_installed.is_empty() {
            for crd in &self.crds_installed {
                println!("   ✓ {}", crd);
            }
        }
        if !self.crds_missing.is_empty() {
            for crd in &self.crds_missing {
                println!("   ✗ {} (missing)", crd);
            }
        }

        println!();

        // Recommendations
        match self.overall_status {
            OverallStatus::NotInstalled => {
                println!("{}", "💡 Recommendations:".bold());
                println!("   • Run installation: wasmForge --namespace {}", self.namespace_name);
            }
            OverallStatus::PartiallyInstalled => {
                println!("{}", "💡 Recommendations:".bold());
                println!("   • Complete installation: wasmForge --upgrade --namespace {}", self.namespace_name);
                println!("   • Or clean up: wasmForge --uninstall --namespace {}", self.namespace_name);
            }
            OverallStatus::InstalledNotReady => {
                println!("{}", "💡 Recommendations:".bold());
                println!("   • Check pod logs: kubectl logs -n {} deployment/wasmcloud-operator", self.namespace_name);
                println!("   • Try upgrade: wasmForge --upgrade --namespace {}", self.namespace_name);
            }
            OverallStatus::InstalledAndReady => {
                println!("{}", "🎉 Installation is healthy and ready!".green().bold());
            }
        }
    }
}

/// Wasmcloud operator installer
pub struct WasmcloudInstaller {
    client: Client,
    namespace: String,
    timeout: Duration,
    operator_image: String,
    manifests: Manifests,
}

impl WasmcloudInstaller {
    /// Create a new installer instance
    pub fn new(client: &Client, namespace: &str, timeout_seconds: u64, operator_image: &str) -> Self {
        Self {
            client: client.clone(),
            namespace: namespace.to_string(),
            timeout: Duration::from_secs(timeout_seconds),
            operator_image: operator_image.to_string(),
            manifests: Manifests::new(namespace, operator_image),
        }
    }

    /// Run a dry-run installation
    pub async fn dry_run(&self) -> InstallerResult<()> {
        info!("Running dry-run installation for Wasmcloud operator");

        println!("   {} Applying namespace: {}", "→".blue(), self.namespace.yellow());
        println!("   {} Applying service account: {}", "→".blue(), OPERATOR_NAME);
        println!("   {} Applying cluster role: {}", "→".blue(), OPERATOR_NAME);
        println!("   {} Applying cluster role bindings: {}, {}", "→".blue(), OPERATOR_NAME, DELEGATOR_BINDING_NAME);
        println!("   {} Applying service: {} (port {})", "→".blue(), OPERATOR_NAME, OPERATOR_PORT);
        println!("   {} Applying deployment: {} ({})", "→".blue(), OPERATOR_NAME, self.operator_image.yellow());
        println!("   {} Operator then registers CRD {} and APIService {}", "→".blue(), HOST_CONFIG_CRD, API_SERVICE_NAME);

        // Validate manifests without applying them
        let _namespace = self.manifests.namespace();
        let _service_account = self.manifests.service_account();
        let _cluster_role = self.manifests.cluster_role();
        let _cluster_role_binding = self.manifests.cluster_role_binding();
        let _delegator_binding = self.manifests.delegator_binding();
        let _service = self.manifests.service();
        let _deployment = self.manifests.deployment();

        println!("   {} All manifests validated successfully", "✓".green());

        Ok(())
    }

    /// Install the Wasmcloud operator
    pub async fn install(&self) -> InstallerResult<()> {
        info!("Installing Wasmcloud operator to namespace: {}", self.namespace);
        self.apply_resources("Installation complete").await
    }

    /// Upgrade the Wasmcloud operator. Server-side apply makes this the same
    /// path as install: every object is brought to the desired state.
    pub async fn upgrade(&self) -> InstallerResult<()> {
        info!("Upgrading Wasmcloud operator in namespace: {}", self.namespace);
        self.apply_resources("Upgrade complete").await
    }

    /// Server-side apply every operator resource. Safe to re-run.
    async fn apply_resources(&self, done_message: &'static str) -> InstallerResult<()> {
        let pb = ProgressBar::new(7);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("   {spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        pb.set_message("Applying namespace...");
        self.apply(Api::all(self.client.clone()), &self.manifests.namespace()).await?;
        pb.inc(1);

        pb.set_message("Applying service account...");
        self.apply(Api::namespaced(self.client.clone(), &self.namespace), &self.manifests.service_account()).await?;
        pb.inc(1);

        pb.set_message("Applying cluster role...");
        self.apply(Api::all(self.client.clone()), &self.manifests.cluster_role()).await?;
        pb.inc(1);

        pb.set_message("Applying cluster role binding...");
        self.apply(Api::all(self.client.clone()), &self.manifests.cluster_role_binding()).await?;
        pb.inc(1);

        pb.set_message("Applying auth-delegator binding...");
        self.apply(Api::all(self.client.clone()), &self.manifests.delegator_binding()).await?;
        pb.inc(1);

        pb.set_message("Applying service...");
        self.apply(Api::namespaced(self.client.clone(), &self.namespace), &self.manifests.service()).await?;
        pb.inc(1);

        pb.set_message("Applying deployment...");
        self.apply(Api::namespaced(self.client.clone(), &self.namespace), &self.manifests.deployment()).await?;
        pb.inc(1);

        pb.finish_with_message(done_message);

        Ok(())
    }

    /// Server-side apply a single object, taking ownership of the fields we set
    async fn apply<K>(&self, api: Api<K>, obj: &K) -> InstallerResult<()>
    where
        K: Resource<DynamicType = ()> + Clone + Debug + Serialize + DeserializeOwned,
    {
        let name = obj.meta().name.clone().unwrap_or_default();
        let params = PatchParams::apply(FIELD_MANAGER).force();

        match api.patch(&name, &params, &Patch::Apply(obj)).await {
            Ok(_) => {
                debug!("Applied {} {}", K::kind(&()), name);
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to apply {} {}: {}", K::kind(&()), name, e)
            )),
        }
    }

    /// Wait until the operator is fully up: Deployment rolled out, CRD Established,
    /// and (best effort) its aggregated API reporting Available.
    pub async fn wait_for_ready(&self) -> InstallerResult<()> {
        info!("Waiting for Wasmcloud operator to be ready");

        let deadline = Instant::now() + self.timeout;

        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("   {spinner:.green} {msg}")
                .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));

        pb.set_message("Waiting for operator rollout...");
        if let Err(e) = self.wait_for_rollout(deadline).await {
            pb.finish_and_clear();
            self.print_failure_diagnostics().await;
            return Err(e);
        }

        pb.set_message(format!("Waiting for operator to register {}...", HOST_CONFIG_CRD));
        let crd_manager = CrdManager::new(&self.client);
        if let Err(e) = crd_manager.wait_established(remaining(deadline)).await {
            pb.finish_and_clear();
            self.print_failure_diagnostics().await;
            return Err(e);
        }

        // The aggregated API needs auth delegation to work end to end. Not fatal:
        // the controller itself is running, only `kubectl get applications` is affected.
        pb.set_message(format!("Waiting for APIService {}...", API_SERVICE_NAME));
        let api_wait = remaining(deadline).min(Duration::from_secs(60));
        let api_available = self.wait_for_api_service(api_wait).await;

        pb.finish_and_clear();
        if !api_available {
            println!(
                "   {} APIService {} not Available yet. Check: kubectl get apiservice {}",
                "⚠".yellow(), API_SERVICE_NAME, API_SERVICE_NAME
            );
        }

        info!("Wasmcloud operator is running and ready");
        Ok(())
    }

    /// Watch the Deployment until its rollout completes, failing fast if a pod
    /// hits a state it won't recover from (crash loop, image pull failure, ...).
    async fn wait_for_rollout(&self, deadline: Instant) -> InstallerResult<()> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let rollout = await_condition(deployments, OPERATOR_NAME, |obj: Option<&Deployment>| {
            obj.is_some_and(|d| rollout_complete(d) || progress_deadline_exceeded(d))
        });

        tokio::select! {
            result = tokio::time::timeout(remaining(deadline), rollout) => match result {
                Ok(Ok(Some(deployment))) if progress_deadline_exceeded(&deployment) => {
                    Err(InstallerError::InstallationError(
                        "Operator deployment exceeded its progress deadline".to_string()
                    ))
                }
                Ok(Ok(_)) => {
                    debug!("Deployment {} rolled out", OPERATOR_NAME);
                    Ok(())
                }
                Ok(Err(e)) => Err(InstallerError::InstallationError(
                    format!("Failed to watch deployment {}: {}", OPERATOR_NAME, e)
                )),
                Err(_) => Err(InstallerError::TimeoutError(
                    format!("Operator not ready within {}s", self.timeout.as_secs())
                )),
            },
            reason = self.watch_for_fatal_pod_state() => {
                Err(InstallerError::InstallationError(format!("Operator pod failed: {}", reason)))
            }
        }
    }

    /// Resolves only when an operator pod enters a fatal waiting state
    async fn watch_for_fatal_pod_state(&self) -> String {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let config = watcher::Config::default().labels(&operator_pod_selector());
        let mut stream = watcher(pods, config).default_backoff().applied_objects().boxed();

        while let Some(event) = stream.next().await {
            match event {
                Ok(pod) => {
                    if let Some(reason) = fatal_pod_reason(&pod) {
                        // During an upgrade the previous revision's pods may still be failing
                        if self.is_current_revision(&pod).await {
                            return reason;
                        }
                    }
                }
                Err(e) => debug!("Pod watch error (retrying): {}", e),
            }
        }

        futures::future::pending().await
    }

    /// Whether a pod belongs to the Deployment's latest ReplicaSet. Returns false while the
    /// controller hasn't observed the latest spec yet, since revisions aren't settled.
    async fn is_current_revision(&self, pod: &Pod) -> bool {
        let Some(rs_name) = pod.metadata.owner_references.iter().flatten()
            .find(|o| o.kind == "ReplicaSet")
            .map(|o| o.name.clone())
        else {
            return true;
        };

        let replica_sets: Api<ReplicaSet> = Api::namespaced(self.client.clone(), &self.namespace);
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let (Ok(rs), Ok(deployment)) = (replica_sets.get(&rs_name).await, deployments.get(OPERATOR_NAME).await) else {
            return true;
        };

        let observed = deployment.status.as_ref()
            .is_some_and(|s| s.observed_generation >= deployment.metadata.generation);
        observed && revision(&rs.metadata) == revision(&deployment.metadata)
    }

    /// Wait for the operator's APIService to report Available
    async fn wait_for_api_service(&self, timeout: Duration) -> bool {
        let api_services: Api<APIService> = Api::all(self.client.clone());
        let available = await_condition(api_services, API_SERVICE_NAME, |obj: Option<&APIService>| {
            obj.is_some_and(api_service_available)
        });

        matches!(tokio::time::timeout(timeout, available).await, Ok(Ok(_)))
    }

    /// Print why the operator isn't coming up: container state, recent events and log tail per pod
    async fn print_failure_diagnostics(&self) {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let events: Api<Event> = Api::namespaced(self.client.clone(), &self.namespace);

        println!();
        println!("{}", "🩺 Diagnostics".red().bold());

        let pod_list = match pods.list(&ListParams::default().labels(&operator_pod_selector())).await {
            Ok(list) => list.items,
            Err(e) => {
                println!("   Failed to list operator pods: {}", e);
                return;
            }
        };

        if pod_list.is_empty() {
            // No pods usually means the ReplicaSet couldn't create them (quota, missing SA, admission)
            println!("   No operator pods found. Recent warnings in namespace {}:", self.namespace);
            match events.list(&ListParams::default().fields("type=Warning")).await {
                Ok(list) => print_events(list.items),
                Err(e) => println!("      Failed to list events: {}", e),
            }
            return;
        }

        for pod in &pod_list {
            let pod_name = pod.metadata.name.as_deref().unwrap_or("unknown");
            let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref()).unwrap_or("Unknown");
            println!();
            println!("   Pod {} ({})", pod_name.cyan(), phase);

            if let Some(state) = container_state_summary(pod) {
                println!("      State: {}", state.yellow());
            }

            println!("      Recent events:");
            match events.list(&ListParams::default().fields(&format!("involvedObject.name={}", pod_name))).await {
                Ok(list) => print_events(list.items),
                Err(e) => println!("      Failed to list events: {}", e),
            }

            // After a crash the useful output is in the previous container's log
            let restarted = pod.status.as_ref()
                .and_then(|s| s.container_statuses.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.restart_count > 0));
            let log_params = LogParams {
                tail_lines: Some(20),
                previous: restarted,
                ..Default::default()
            };

            println!("      Last log lines{}:", if restarted { " (previous container)" } else { "" });
            match pods.logs(pod_name, &log_params).await {
                Ok(logs) if !logs.trim().is_empty() => {
                    for line in logs.lines() {
                        println!("         {}", line.dimmed());
                    }
                }
                Ok(_) => println!("         {}", "(empty)".dimmed()),
                Err(_) => println!("         {}", "(no logs available - container never started)".dimmed()),
            }
        }

        println!();
    }

    /// Print installation summary showing what was created and what's running
    pub async fn print_installation_summary(&self) -> InstallerResult<()> {
        println!();
        println!("{}", " Installation Summary".cyan().bold());
        println!("{}", "=====================".cyan());
        println!();

        // Check namespace
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        match namespaces.get(&self.namespace).await {
            Ok(_) => println!("   {} Namespace: {}", "✓".green(), self.namespace.yellow()),
            Err(_) => println!("   {} Namespace: {}", "✗".red(), self.namespace.yellow()),
        }

        // Check CRD (registered by the operator)
        let crd_ready = CrdManager::new(&self.client).is_established().await;
        if crd_ready {
            println!("   {} CRD: {}", "✓".green(), "WasmCloudHostConfig".yellow());
        } else {
            println!("   {} CRD: {} (not established)", "⚠".yellow(), "WasmCloudHostConfig".yellow());
        }

        // Check ServiceAccount
        let service_accounts: Api<ServiceAccount> = Api::namespaced(self.client.clone(), &self.namespace);
        match service_accounts.get(OPERATOR_NAME).await {
            Ok(_) => println!("   {} ServiceAccount: wasmcloud-operator", "✓".green()),
            Err(_) => println!("   {} ServiceAccount: wasmcloud-operator", "✗".red()),
        }

        // Check ClusterRole
        let cluster_roles: Api<ClusterRole> = Api::all(self.client.clone());
        match cluster_roles.get(OPERATOR_NAME).await {
            Ok(_) => println!("   {} ClusterRole: wasmcloud-operator", "✓".green()),
            Err(_) => println!("   {} ClusterRole: wasmcloud-operator", "✗".red()),
        }

        // Check ClusterRoleBindings
        let cluster_role_bindings: Api<ClusterRoleBinding> = Api::all(self.client.clone());
        for binding in [OPERATOR_NAME, DELEGATOR_BINDING_NAME] {
            match cluster_role_bindings.get(binding).await {
                Ok(_) => println!("   {} ClusterRoleBinding: {}", "✓".green(), binding),
                Err(_) => println!("   {} ClusterRoleBinding: {}", "✗".red(), binding),
            }
        }

        // Check Deployment
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let mut deployment_ready = false;
        match deployments.get(OPERATOR_NAME).await {
            Ok(deployment) => {
                if let Some(status) = &deployment.status {
                    let desired = status.replicas.unwrap_or(0);
                    let ready = status.ready_replicas.unwrap_or(0);
                    deployment_ready = desired > 0 && ready == desired;

                    if deployment_ready {
                        println!("   {} Deployment: wasmcloud-operator ({}/{})", "✓".green(), ready, desired);
                    } else {
                        println!("   {} Deployment: wasmcloud-operator ({}/{}) - not ready", "⚠".yellow(), ready, desired);
                    }
                } else {
                    println!("   {} Deployment: wasmcloud-operator (status unknown)", "⚠".yellow());
                }
            }
            Err(_) => println!("   {} Deployment: wasmcloud-operator", "✗".red()),
        }

        // Check Pods
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let list_params = ListParams::default().labels(&operator_pod_selector());
        match pods.list(&list_params).await {
            Ok(pod_list) => {
                if pod_list.items.is_empty() {
                    println!("   {} Pods: No pods found", "⚠".yellow());
                } else {
                    for pod in &pod_list.items {
                        let pod_name = pod.metadata.name.as_deref().unwrap_or("unknown");
                        if let Some(status) = &pod.status {
                            let phase = status.phase.as_deref().unwrap_or("Unknown");
                            match phase {
                                "Running" => println!("   {} Pod: {} (Running)", "✓".green(), pod_name.cyan()),
                                "Pending" => println!("   {} Pod: {} (Pending)", "⚠".yellow(), pod_name.cyan()),
                                "Failed" => println!("   {} Pod: {} (Failed)", "✗".red(), pod_name.cyan()),
                                _ => println!("   {} Pod: {} ({})", "•".blue(), pod_name.cyan(), phase),
                            }
                        } else {
                            println!("   {} Pod: {} (status unknown)", "•".blue(), pod_name.cyan());
                        }
                    }
                }
            }
            Err(e) => println!("   {} Pods: Failed to list - {}", "✗".red(), e),
        }

        // Check Service
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        match services.get(OPERATOR_NAME).await {
            Ok(_) => println!("   {} Service: wasmcloud-operator", "✓".green()),
            Err(_) => println!("   {} Service: wasmcloud-operator", "✗".red()),
        }

        // Check APIService (registered by the operator)
        let api_services: Api<APIService> = Api::all(self.client.clone());
        match api_services.get_opt(API_SERVICE_NAME).await {
            Ok(Some(api)) if api_service_available(&api) => {
                println!("   {} APIService: {}", "✓".green(), API_SERVICE_NAME)
            }
            Ok(Some(_)) => println!("   {} APIService: {} (not available)", "⚠".yellow(), API_SERVICE_NAME),
            _ => println!("   {} APIService: {}", "✗".red(), API_SERVICE_NAME),
        }

        println!();

        // Final status assessment
        if deployment_ready && crd_ready {
            println!("{}", "✓ All components are ready and running!".green().bold());
        } else {
            println!("{}", " Some components are not ready yet. This may take a few moments.".yellow().bold());
            println!();
            println!("{}", "Troubleshooting:".cyan().bold());
            println!("   Check pod status: kubectl get pods -n {}", self.namespace);
            println!("   View logs: kubectl logs -n {} -l app=wasmcloud-operator", self.namespace);
            println!("   Describe deployment: kubectl describe deployment wasmcloud-operator -n {}", self.namespace);
        }

        println!();
        println!("{}", "Next: the operator needs NATS (with JetStream) and wadm to manage hosts.".dimmed());
        println!("{}", "📚 Learn more: https://github.com/wasmCloud/wasmcloud-operator".dimmed());

        Ok(())
    }

    /// Uninstall the Wasmcloud operator and all related resources
    pub async fn uninstall(&self) -> InstallerResult<()> {
        info!("Uninstalling Wasmcloud operator from namespace: {}", self.namespace);

        // Create progress bar (10 steps)
        let pb = ProgressBar::new(10);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("   {spinner:.red} [{bar:40.red/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        // Step 1: Delete host configs while the operator is still running, so it can tear down
        // the hosts and release its finalizers. Otherwise the CRD deletion below hangs forever.
        pb.set_message("Deleting WasmCloudHostConfigs and their hosts...");
        let removed = CrdManager::new(&self.client).delete_host_configs(Duration::from_secs(60)).await?;
        if removed > 0 {
            debug!("Deleted {} WasmCloudHostConfig(s)", removed);
        }
        pb.inc(1);

        // Step 2: Delete deployment so the operator stops re-registering its CRD and APIService
        pb.set_message("Deleting deployment and waiting for pods to stop...");
        self.delete_deployment_and_wait().await?;
        pb.inc(1);

        // Step 3: Delete the APIService, otherwise API discovery breaks once the Service is gone
        pb.set_message("Deleting APIService...");
        self.delete_api_service().await?;
        pb.inc(1);

        // Step 4: Delete service
        pb.set_message("Deleting service...");
        self.delete_named::<Service>(Api::namespaced(self.client.clone(), &self.namespace), OPERATOR_NAME).await?;
        pb.inc(1);

        // Step 5-6: Delete cluster role bindings
        pb.set_message("Deleting cluster role bindings...");
        self.delete_named::<ClusterRoleBinding>(Api::all(self.client.clone()), OPERATOR_NAME).await?;
        pb.inc(1);
        self.delete_named::<ClusterRoleBinding>(Api::all(self.client.clone()), DELEGATOR_BINDING_NAME).await?;
        pb.inc(1);

        // Step 7: Delete cluster role
        pb.set_message("Deleting cluster role...");
        self.delete_named::<ClusterRole>(Api::all(self.client.clone()), OPERATOR_NAME).await?;
        pb.inc(1);

        // Step 8: Delete service account
        pb.set_message("Deleting service account...");
        self.delete_named::<ServiceAccount>(Api::namespaced(self.client.clone(), &self.namespace), OPERATOR_NAME).await?;
        pb.inc(1);

        // Step 9: Delete CRDs
        pb.set_message("Deleting Custom Resource Definitions...");
        CrdManager::new(&self.client).delete_crds().await?;
        pb.inc(1);

        // Step 10: Delete namespace (optional - only if empty)
        pb.set_message("Checking namespace cleanup...");
        self.cleanup_namespace().await?;
        pb.inc(1);

        pb.finish_with_message("Uninstallation complete");

        Ok(())
    }

    /// Delete the operator Deployment in the foreground and wait until it and its
    /// pods are gone, so the namespace emptiness check afterwards is accurate
    async fn delete_deployment_and_wait(&self) -> InstallerResult<()> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);

        let uid = match deployments.delete(OPERATOR_NAME, &DeleteParams::foreground()).await {
            Ok(result) => result.left().and_then(|d| d.metadata.uid),
            Err(kube::Error::Api(err)) if err.code == 404 => {
                debug!("Deployment {} not found, skipping", OPERATOR_NAME);
                return Ok(());
            }
            Err(e) => {
                return Err(InstallerError::InstallationError(
                    format!("Failed to delete Deployment {}: {}", OPERATOR_NAME, e)
                ))
            }
        };

        if let Some(uid) = uid {
            let deleted = await_condition(deployments, OPERATOR_NAME, conditions::is_deleted(&uid));
            if tokio::time::timeout(self.timeout, deleted).await.is_err() {
                warn!("Deployment {} still terminating after {}s", OPERATOR_NAME, self.timeout.as_secs());
            }
        }

        Ok(())
    }

    /// Delete a named object, treating "not found" as success
    async fn delete_named<K>(&self, api: Api<K>, name: &str) -> InstallerResult<()>
    where
        K: Resource<DynamicType = ()> + Clone + Debug + DeserializeOwned,
    {
        match api.delete(name, &DeleteParams::default()).await {
            Ok(_) => {
                debug!("Deleted {} {}", K::kind(&()), name);
                Ok(())
            }
            Err(kube::Error::Api(err)) if err.code == 404 => {
                debug!("{} {} not found, skipping", K::kind(&()), name);
                Ok(())
            }
            Err(e) => Err(InstallerError::InstallationError(
                format!("Failed to delete {} {}: {}", K::kind(&()), name, e)
            )),
        }
    }

    /// Delete the operator's APIService, but only if it points at our Service.
    /// Another installation may own core.oam.dev.
    async fn delete_api_service(&self) -> InstallerResult<()> {
        let api_services: Api<APIService> = Api::all(self.client.clone());

        let Some(api_service) = api_services.get_opt(API_SERVICE_NAME).await? else {
            debug!("APIService {} not found, skipping", API_SERVICE_NAME);
            return Ok(());
        };

        let service = api_service.spec.as_ref().and_then(|s| s.service.as_ref());
        let ours = service.is_some_and(|s| {
            s.name.as_deref() == Some(OPERATOR_NAME) && s.namespace.as_deref() == Some(self.namespace.as_str())
        });

        if ours {
            self.delete_named::<APIService>(api_services, API_SERVICE_NAME).await
        } else {
            warn!("APIService {} points at a different service, leaving it in place", API_SERVICE_NAME);
            Ok(())
        }
    }

    /// Cleanup namespace if it's empty and was created by us
    async fn cleanup_namespace(&self) -> InstallerResult<()> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());

        // Check if namespace has our label (indicating we created it)
        match namespaces.get(&self.namespace).await {
            Ok(ns) => {
                if let Some(labels) = &ns.metadata.labels {
                    if labels.get("app.kubernetes.io/managed-by") == Some(&"wasmForge".to_string()) {
                        // Check if namespace is empty (no other resources)
                        if self.is_namespace_empty().await? {
                            match namespaces.delete(&self.namespace, &DeleteParams::default()).await {
                                Ok(_) => {
                                    debug!("Deleted namespace: {}", self.namespace);
                                }
                                Err(e) => {
                                    warn!("Failed to delete namespace {}: {}", self.namespace, e);
                                }
                            }
                        } else {
                            debug!("Namespace {} contains other resources, keeping it", self.namespace);
                        }
                    } else {
                        debug!("Namespace {} not created by installer, keeping it", self.namespace);
                    }
                }
            }
            Err(_) => {
                debug!("Namespace {} not found or not accessible", self.namespace);
            }
        }

        Ok(())
    }

    /// Check if namespace is empty (contains no resources other than defaults)
    async fn is_namespace_empty(&self) -> InstallerResult<bool> {
        // Check for pods
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        if pods.list(&ListParams::default()).await?.items.len() > 0 {
            return Ok(false);
        }

        // Check for services (excluding default kubernetes service)
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let service_list = services.list(&ListParams::default()).await?;
        let non_default_services = service_list.items.iter()
            .filter(|s| s.metadata.name.as_ref() != Some(&"kubernetes".to_string()))
            .count();
        if non_default_services > 0 {
            return Ok(false);
        }

        // Check for deployments
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        if deployments.list(&ListParams::default()).await?.items.len() > 0 {
            return Ok(false);
        }

        Ok(true)
    }

    /// Check the current installation status of wasmCloud operator
    pub async fn check_installation_status(&self) -> InstallerResult<InstallationStatus> {
        info!("Checking wasmCloud operator installation status in namespace: {}", self.namespace);

        let mut status = InstallationStatus {
            namespace_exists: false,
            namespace_name: self.namespace.clone(),
            deployment_exists: false,
            deployment_ready: false,
            deployment_replicas: None,
            service_exists: false,
            service_account_exists: false,
            cluster_role_exists: false,
            cluster_role_binding_exists: false,
            crds_installed: Vec::new(),
            crds_missing: Vec::new(),
            pod_status: Vec::new(),
            overall_status: OverallStatus::NotInstalled,
        };

        // Check namespace
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        match namespaces.get(&self.namespace).await {
            Ok(_) => {
                status.namespace_exists = true;
                debug!("Namespace {} exists", self.namespace);
            }
            Err(_) => {
                debug!("Namespace {} does not exist", self.namespace);
            }
        }

        // Check deployment
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        match deployments.get(OPERATOR_NAME).await {
            Ok(deployment) => {
                status.deployment_exists = true;
                debug!("Deployment wasmcloud-operator exists");

                if let Some(deploy_status) = &deployment.status {
                    let ready_replicas = deploy_status.ready_replicas.unwrap_or(0);
                    let desired_replicas = deployment.spec.as_ref()
                        .and_then(|spec| spec.replicas)
                        .unwrap_or(1);

                    status.deployment_replicas = Some(Replicas { ready: ready_replicas, desired: desired_replicas });
                    status.deployment_ready = ready_replicas > 0 && ready_replicas == desired_replicas;
                }

                // Check pod status
                let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
                if let Ok(pod_list) = pods.list(&ListParams::default().labels(&operator_pod_selector())).await {
                    for pod in pod_list.items {
                        let phase = pod.status.as_ref()
                            .and_then(|s| s.phase.clone())
                            .unwrap_or_else(|| "Unknown".to_string());
                        let detail = container_state_summary(&pod)
                            .map(|s| format!(" - {}", s))
                            .unwrap_or_default();
                        let pod_name = pod.metadata.name.unwrap_or_else(|| "unknown".to_string());
                        status.pod_status.push(format!("{} ({}){}", pod_name, phase, detail));
                    }
                }
            }
            Err(_) => {
                debug!("Deployment wasmcloud-operator does not exist");
            }
        }

        // Check service
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        match services.get(OPERATOR_NAME).await {
            Ok(_) => {
                status.service_exists = true;
                debug!("Service wasmcloud-operator exists");
            }
            Err(_) => {
                debug!("Service wasmcloud-operator does not exist");
            }
        }

        // Check service account
        let service_accounts: Api<ServiceAccount> = Api::namespaced(self.client.clone(), &self.namespace);
        match service_accounts.get(OPERATOR_NAME).await {
            Ok(_) => {
                status.service_account_exists = true;
                debug!("ServiceAccount wasmcloud-operator exists");
            }
            Err(_) => {
                debug!("ServiceAccount wasmcloud-operator does not exist");
            }
        }

        // Check cluster role
        let cluster_roles: Api<ClusterRole> = Api::all(self.client.clone());
        match cluster_roles.get(OPERATOR_NAME).await {
            Ok(_) => {
                status.cluster_role_exists = true;
                debug!("ClusterRole wasmcloud-operator exists");
            }
            Err(_) => {
                debug!("ClusterRole wasmcloud-operator does not exist");
            }
        }

        // Check cluster role binding
        let cluster_role_bindings: Api<ClusterRoleBinding> = Api::all(self.client.clone());
        match cluster_role_bindings.get(OPERATOR_NAME).await {
            Ok(_) => {
                status.cluster_role_binding_exists = true;
                debug!("ClusterRoleBinding wasmcloud-operator exists");
            }
            Err(_) => {
                debug!("ClusterRoleBinding wasmcloud-operator does not exist");
            }
        }

        // Check CRD (registered by the operator)
        if CrdManager::new(&self.client).is_established().await {
            status.crds_installed.push(HOST_CONFIG_CRD.to_string());
        } else {
            status.crds_missing.push(HOST_CONFIG_CRD.to_string());
        }

        // Determine overall status
        status.overall_status = if !status.namespace_exists ||
                                  (!status.deployment_exists && !status.service_exists &&
                                   !status.service_account_exists && status.crds_installed.is_empty()) {
            OverallStatus::NotInstalled
        } else if status.deployment_exists && status.deployment_ready &&
                  status.service_exists && status.service_account_exists &&
                  status.cluster_role_exists && status.cluster_role_binding_exists &&
                  status.crds_missing.is_empty() {
            OverallStatus::InstalledAndReady
        } else if status.deployment_exists && status.service_exists {
            OverallStatus::InstalledNotReady
        } else {
            OverallStatus::PartiallyInstalled
        };

        Ok(status)
    }

}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Same criteria as `kubectl rollout status`: the controller has seen the
/// latest spec and every replica is updated and available.
pub(crate) fn rollout_complete(deployment: &Deployment) -> bool {
    let (Some(spec), Some(status)) = (&deployment.spec, &deployment.status) else {
        return false;
    };
    let desired = spec.replicas.unwrap_or(1);

    status.observed_generation >= deployment.metadata.generation
        && status.updated_replicas.unwrap_or(0) == desired
        && status.replicas.unwrap_or(0) == desired
        && status.available_replicas.unwrap_or(0) == desired
}

pub(crate) fn progress_deadline_exceeded(deployment: &Deployment) -> bool {
    deployment.status.as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|conditions| conditions.iter().any(|c| {
            c.type_ == "Progressing" && c.reason.as_deref() == Some("ProgressDeadlineExceeded")
        }))
}

fn revision(meta: &ObjectMeta) -> Option<&str> {
    meta.annotations.as_ref()?.get("deployment.kubernetes.io/revision").map(String::as_str)
}

pub(crate) fn api_service_available(api_service: &APIService) -> bool {
    api_service.status.as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|conditions| conditions.iter().any(|c| c.type_ == "Available" && c.status == "True"))
}

fn fatal_pod_reason(pod: &Pod) -> Option<String> {
    let statuses = pod.status.as_ref()?.container_statuses.as_ref()?;
    statuses.iter().find_map(|c| {
        let waiting = c.state.as_ref()?.waiting.as_ref()?;
        let reason = waiting.reason.as_deref()?;
        FATAL_WAITING_REASONS.contains(&reason).then(|| {
            format!("container '{}' is {}: {}", c.name, reason, waiting.message.as_deref().unwrap_or("no message"))
        })
    })
}

/// Describe the first container that is waiting or has exited non-zero
pub(crate) fn container_state_summary(pod: &Pod) -> Option<String> {
    let statuses = pod.status.as_ref()?.container_statuses.as_ref()?;
    statuses.iter().find_map(|c| {
        let state = c.state.as_ref()?;
        if let Some(waiting) = &state.waiting {
            let reason = waiting.reason.as_deref().unwrap_or("Waiting");
            return Some(match &waiting.message {
                Some(message) => format!("{}: {}", reason, message),
                None => reason.to_string(),
            });
        }
        let terminated = state.terminated.as_ref().filter(|t| t.exit_code != 0)?;
        Some(format!(
            "exited with code {}{}",
            terminated.exit_code,
            terminated.reason.as_deref().map(|r| format!(" ({})", r)).unwrap_or_default()
        ))
    })
}

fn print_events(mut events: Vec<Event>) {
    if events.is_empty() {
        println!("         {}", "(none)".dimmed());
        return;
    }
    events.sort_by_key(|e| e.last_timestamp.clone().or_else(|| e.first_timestamp.clone()));
    for event in events.iter().rev().take(10).rev() {
        println!(
            "         {} {}: {}",
            event.type_.as_deref().unwrap_or("Normal"),
            event.reason.as_deref().unwrap_or(""),
            event.message.as_deref().unwrap_or("").trim()
        );
    }
}
