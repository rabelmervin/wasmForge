use crate::crds::{crd_is_established, host_config_resource, HOST_CONFIG_CRD, LEGACY_CRD_SELECTOR};
use crate::install::{api_service_available, container_state_summary, progress_deadline_exceeded, rollout_complete};
use crate::manifests::{
    operator_pod_selector, Manifests, API_SERVICE_NAME, DELEGATOR_BINDING_NAME, DELEGATOR_CLUSTER_ROLE, OPERATOR_NAME,
    OPERATOR_PORT,
};
use colored::*;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Endpoints, Event, Namespace, Pod, Service, ServiceAccount},
    rbac::v1::{ClusterRole, ClusterRoleBinding, PolicyRule},
};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::{
    api::{Api, DynamicObject, ListParams, LogParams},
    Client, ResourceExt,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::IpAddr;

/// Defaults the operator applies when a WasmCloudHostConfig leaves these unset
const DEFAULT_NATS_ADDRESS: &str = "nats://nats.default.svc.cluster.local";
const DEFAULT_NATS_CLIENT_PORT: i64 = 4222;
const DEFAULT_NATS_LEAFNODE_PORT: i64 = 7422;

const V2_CRD_GROUP: &str = "runtime.wasmcloud.dev";
const WADM_SELECTOR: &str = "app.kubernetes.io/name=wadm";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub category: &'static str,
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub namespace: String,
    pub healthy: bool,
    pub summary: Summary,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn print(&self) {
        println!("{}", format!("🩺 forge doctor - namespace {}", self.namespace).bold());
        println!("{}", "=".repeat(50));

        let mut current_category = "";
        for check in &self.checks {
            if check.category != current_category {
                current_category = check.category;
                println!();
                println!("{}", current_category.bold());
            }

            let icon = match check.status {
                CheckStatus::Pass => "✓".green(),
                CheckStatus::Warn => "⚠".yellow(),
                CheckStatus::Fail => "✗".red(),
            };
            println!("   {} {}: {}", icon, check.name, check.message);
            for detail in &check.details {
                println!("        {}", detail.dimmed());
            }
            if let Some(hint) = &check.hint {
                println!("        {} {}", "→".cyan(), hint.cyan());
            }
        }

        println!();
        let summary = format!(
            "{} passed, {} warnings, {} failed",
            self.summary.pass, self.summary.warn, self.summary.fail
        );
        if self.healthy {
            println!("{}", summary.green().bold());
        } else {
            println!("{}", summary.red().bold());
        }
    }
}

/// Diagnoses a wasmCloud operator installation without changing anything
pub struct Doctor {
    client: Client,
    namespace: String,
    image: String,
    checks: Vec<Check>,
}

impl Doctor {
    pub fn new(client: &Client, namespace: &str, image: &str) -> Self {
        Self {
            client: client.clone(),
            namespace: namespace.to_string(),
            image: image.to_string(),
            checks: Vec::new(),
        }
    }

    pub async fn run(mut self) -> DoctorReport {
        // Without the namespace nothing is installed, so per-component failures would only be noise
        if self.check_cluster().await {
            if self.check_operator().await {
                self.check_pods().await;
            }
            self.check_service().await;
            self.check_rbac().await;
            self.check_crds().await;
            self.check_api_service().await;
        }
        self.check_dependencies().await;

        let count = |status| self.checks.iter().filter(|c| c.status == status).count();
        let summary = Summary {
            pass: count(CheckStatus::Pass),
            warn: count(CheckStatus::Warn),
            fail: count(CheckStatus::Fail),
        };

        DoctorReport {
            namespace: self.namespace,
            healthy: summary.fail == 0,
            summary,
            checks: self.checks,
        }
    }

    fn record(&mut self, category: &'static str, name: &str, status: CheckStatus, message: impl Into<String>) -> &mut Check {
        self.checks.push(Check {
            category,
            name: name.to_string(),
            status,
            message: message.into(),
            hint: None,
            details: Vec::new(),
        });
        self.checks.last_mut().unwrap()
    }

    /// Returns whether the install namespace exists
    async fn check_cluster(&mut self) -> bool {
        const CAT: &str = "Cluster";
        match self.client.apiserver_version().await {
            Ok(version) => {
                self.record(CAT, "API server", CheckStatus::Pass, format!("reachable, Kubernetes v{}.{}", version.major, version.minor));
            }
            Err(e) => {
                self.record(CAT, "API server", CheckStatus::Fail, format!("not reachable: {}", e));
            }
        }

        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        match namespaces.get_opt(&self.namespace).await {
            Ok(Some(_)) => {
                self.record(CAT, "Namespace", CheckStatus::Pass, format!("{} exists", self.namespace));
                true
            }
            Ok(None) => {
                let namespace = self.namespace.clone();
                self.record(CAT, "Namespace", CheckStatus::Fail, format!("{} not found, the operator is not installed", namespace))
                    .hint = Some(format!("Install with: forge --namespace {}", namespace));
                false
            }
            Err(e) => {
                self.record(CAT, "Namespace", CheckStatus::Fail, format!("could not read: {}", e));
                false
            }
        }
    }

    /// Returns whether the Deployment exists, so pod checks can be skipped otherwise
    async fn check_operator(&mut self) -> bool {
        const CAT: &str = "Operator";
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);

        let deployment = match deployments.get_opt(OPERATOR_NAME).await {
            Ok(Some(deployment)) => deployment,
            Ok(None) => {
                let namespace = self.namespace.clone();
                self.record(CAT, "Deployment", CheckStatus::Fail, format!("{} not found", OPERATOR_NAME)).hint =
                    Some(format!("Install with: forge --namespace {}", namespace));
                return false;
            }
            Err(e) => {
                self.record(CAT, "Deployment", CheckStatus::Fail, format!("could not read: {}", e));
                return false;
            }
        };

        let images: Vec<String> = deployment.spec.as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .map(|s| s.containers.iter().filter_map(|c| c.image.clone()).collect())
            .unwrap_or_default();
        if images.contains(&self.image) {
            let image = self.image.clone();
            self.record(CAT, "Image", CheckStatus::Pass, image);
        } else {
            let message = format!("running {}, forge pins {}", images.join(", "), self.image);
            self.record(CAT, "Image", CheckStatus::Warn, message).hint =
                Some(format!("Align with: forge --upgrade --namespace {}", self.namespace));
        }

        let status = deployment.status.clone().unwrap_or_default();
        let ready = status.ready_replicas.unwrap_or(0);
        let desired = deployment.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
        if progress_deadline_exceeded(&deployment) {
            self.record(CAT, "Rollout", CheckStatus::Fail, format!("progress deadline exceeded ({}/{} ready)", ready, desired));
        } else if rollout_complete(&deployment) {
            self.record(CAT, "Rollout", CheckStatus::Pass, format!("complete ({}/{} ready)", ready, desired));
        } else {
            self.record(CAT, "Rollout", CheckStatus::Warn, format!("in progress ({}/{} ready)", ready, desired));
        }

        true
    }

    async fn check_pods(&mut self) {
        const CAT: &str = "Operator";
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        // Pods already being deleted (e.g. the previous revision after a rollout) don't reflect current health
        let pod_list: Vec<Pod> = match pods.list(&ListParams::default().labels(&operator_pod_selector())).await {
            Ok(list) => list.items.into_iter().filter(|p| p.metadata.deletion_timestamp.is_none()).collect(),
            Err(e) => {
                self.record(CAT, "Pods", CheckStatus::Fail, format!("could not list: {}", e));
                return;
            }
        };

        if pod_list.is_empty() {
            let details = self.recent_warnings().await;
            self.record(CAT, "Pods", CheckStatus::Fail, "no operator pods exist").details = details;
            return;
        }

        for pod in &pod_list {
            let name = format!("Pod {}", pod.name_any());
            let statuses = pod.status.as_ref().and_then(|s| s.container_statuses.clone()).unwrap_or_default();
            let restarts: i32 = statuses.iter().map(|c| c.restart_count).sum();
            let ready = !statuses.is_empty() && statuses.iter().all(|c| c.ready);

            if let Some(state) = container_state_summary(pod) {
                let details = self.log_tail(pod, restarts > 0).await;
                let hint = format!("Inspect with: kubectl describe pod -n {} {}", self.namespace, pod.name_any());
                let check = self.record(CAT, &name, CheckStatus::Fail, state);
                check.details = details;
                check.hint = Some(hint);
            } else if restarts > 0 {
                let last_exit = statuses.iter()
                    .filter_map(|c| c.last_state.as_ref()?.terminated.as_ref())
                    .map(|t| format!("{} (exit {})", t.reason.as_deref().unwrap_or("Terminated"), t.exit_code))
                    .next()
                    .unwrap_or_else(|| "unknown".to_string());
                let details = self.log_tail(pod, true).await;
                self.record(CAT, &name, CheckStatus::Warn, format!("running, restarted {} times, last exit: {}", restarts, last_exit))
                    .details = details;
            } else if ready {
                self.record(CAT, &name, CheckStatus::Pass, "running and ready");
            } else {
                let phase = pod.status.as_ref().and_then(|s| s.phase.clone()).unwrap_or_else(|| "Unknown".to_string());
                self.record(CAT, &name, CheckStatus::Warn, format!("{}, not ready", phase));
            }
        }
    }

    async fn check_service(&mut self) {
        const CAT: &str = "Operator";
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);

        let service = match services.get_opt(OPERATOR_NAME).await {
            Ok(Some(service)) => service,
            Ok(None) => {
                self.record(CAT, "Service", CheckStatus::Fail, format!("{} not found", OPERATOR_NAME));
                return;
            }
            Err(e) => {
                self.record(CAT, "Service", CheckStatus::Fail, format!("could not read: {}", e));
                return;
            }
        };

        let ports = service_ports(&service);
        if !ports.contains(&(OPERATOR_PORT as i64)) {
            self.record(CAT, "Service", CheckStatus::Fail, format!("exposes {:?}, the operator's APIService expects {}", ports, OPERATOR_PORT))
                .hint = Some(format!("Fix with: forge --upgrade --namespace {}", self.namespace));
            return;
        }

        match self.ready_endpoints(OPERATOR_NAME, &self.namespace.clone()).await {
            0 => {
                self.record(CAT, "Service", CheckStatus::Fail, "port 8443 has no ready endpoints").hint =
                    Some("The operator pod is not ready or the selector doesn't match it".to_string());
            }
            n => {
                self.record(CAT, "Service", CheckStatus::Pass, format!("port {} with {} ready endpoint(s)", OPERATOR_PORT, n));
            }
        }
    }

    async fn check_rbac(&mut self) {
        const CAT: &str = "RBAC";
        let expected = Manifests::new(&self.namespace, &self.image);

        let service_accounts: Api<ServiceAccount> = Api::namespaced(self.client.clone(), &self.namespace);
        match service_accounts.get_opt(OPERATOR_NAME).await {
            Ok(Some(_)) => {
                self.record(CAT, "ServiceAccount", CheckStatus::Pass, format!("{} exists", OPERATOR_NAME));
            }
            _ => {
                self.record(CAT, "ServiceAccount", CheckStatus::Fail, format!("{} not found", OPERATOR_NAME));
            }
        }

        let cluster_roles: Api<ClusterRole> = Api::all(self.client.clone());
        match cluster_roles.get_opt(OPERATOR_NAME).await {
            Ok(Some(actual)) => {
                let wanted = expected.cluster_role().rules.unwrap_or_default();
                let granted = actual.rules.unwrap_or_default();
                let missing = missing_permissions(&wanted, &granted);
                if missing.is_empty() {
                    self.record(CAT, "ClusterRole", CheckStatus::Pass, "grants every permission the operator needs");
                } else {
                    let namespace = self.namespace.clone();
                    let check = self.record(CAT, "ClusterRole", CheckStatus::Fail, format!("missing {} permission(s)", missing.len()));
                    check.details = missing;
                    check.hint = Some(format!("Restore with: forge --upgrade --namespace {}", namespace));
                }
            }
            _ => {
                self.record(CAT, "ClusterRole", CheckStatus::Fail, format!("{} not found", OPERATOR_NAME));
            }
        }

        let bindings: Api<ClusterRoleBinding> = Api::all(self.client.clone());
        for (name, role) in [(OPERATOR_NAME, OPERATOR_NAME), (DELEGATOR_BINDING_NAME, DELEGATOR_CLUSTER_ROLE)] {
            let label = format!("ClusterRoleBinding {}", name);
            let binding = match bindings.get_opt(name).await {
                Ok(Some(binding)) => binding,
                _ => {
                    let namespace = self.namespace.clone();
                    self.record(CAT, &label, CheckStatus::Fail, "not found").hint =
                        Some(format!("Restore with: forge --upgrade --namespace {}", namespace));
                    continue;
                }
            };

            let bound_namespaces: Vec<String> = binding.subjects.iter().flatten()
                .filter(|s| s.kind == "ServiceAccount" && s.name == OPERATOR_NAME)
                .filter_map(|s| s.namespace.clone())
                .collect();

            if binding.role_ref.name != role {
                self.record(CAT, &label, CheckStatus::Fail, format!("references role {}, expected {}", binding.role_ref.name, role));
            } else if !bound_namespaces.contains(&self.namespace) {
                let message = format!(
                    "binds the ServiceAccount in [{}], not {}",
                    bound_namespaces.join(", "), self.namespace
                );
                let namespace = self.namespace.clone();
                self.record(CAT, &label, CheckStatus::Fail, message).hint =
                    Some(format!("Installed into another namespace? Rebind with: forge --upgrade --namespace {}", namespace));
            } else {
                self.record(CAT, &label, CheckStatus::Pass, format!("{} → {}/{}", role, self.namespace, OPERATOR_NAME));
            }
        }
    }

    async fn check_crds(&mut self) {
        const CAT: &str = "CRDs";
        let crds: Api<CustomResourceDefinition> = Api::all(self.client.clone());

        match crds.get_opt(HOST_CONFIG_CRD).await {
            Ok(Some(crd)) => {
                let version = crd.spec.versions.iter().find(|v| v.name == "v1alpha1");
                let stored = crd.status.as_ref().and_then(|s| s.stored_versions.clone()).unwrap_or_default();
                if !crd_is_established(&crd) {
                    self.record(CAT, "WasmCloudHostConfig", CheckStatus::Fail, "registered but not Established");
                } else if !version.is_some_and(|v| v.served && v.storage) {
                    let versions: Vec<_> = crd.spec.versions.iter().map(|v| v.name.as_str()).collect();
                    self.record(CAT, "WasmCloudHostConfig", CheckStatus::Warn, format!(
                        "v1alpha1 is not the served storage version (versions: {})", versions.join(", ")
                    ));
                } else if stored.iter().any(|v| v != "v1alpha1") {
                    self.record(CAT, "WasmCloudHostConfig", CheckStatus::Warn, format!(
                        "objects stored in older versions: {}", stored.join(", ")
                    ));
                } else {
                    self.record(CAT, "WasmCloudHostConfig", CheckStatus::Pass, "v1alpha1 served and stored, Established");
                }
            }
            Ok(None) => {
                self.record(CAT, "WasmCloudHostConfig", CheckStatus::Fail, format!("{} not registered", HOST_CONFIG_CRD)).hint =
                    Some("The operator registers it on startup. Check the operator pod and its CRD permissions above".to_string());
            }
            Err(e) => {
                self.record(CAT, "WasmCloudHostConfig", CheckStatus::Fail, format!("could not read: {}", e));
            }
        }

        let all = match crds.list(&ListParams::default()).await {
            Ok(list) => list.items,
            Err(_) => return,
        };
        let legacy_label = LEGACY_CRD_SELECTOR.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()));
        let is_legacy = |crd: &CustomResourceDefinition| {
            legacy_label.as_ref().is_some_and(|(k, v)| crd.labels().get(k) == Some(v))
        };

        let leftovers: Vec<String> = all.iter().filter(|c| is_legacy(c)).map(|c| c.name_any()).collect();
        if !leftovers.is_empty() {
            let namespace = self.namespace.clone();
            let check = self.record(CAT, "Leftovers", CheckStatus::Warn, format!("{} CRD(s) from an older forge version", leftovers.len()));
            check.details = leftovers;
            check.hint = Some(format!(
                "Clean up with: forge --uninstall --namespace {0} && forge --namespace {0}", namespace
            ));
        }

        let v2: Vec<String> = all.iter()
            .filter(|c| c.spec.group == V2_CRD_GROUP && !is_legacy(c))
            .map(|c| c.name_any())
            .collect();
        if !v2.is_empty() {
            let check = self.record(CAT, "wasmCloud v2", CheckStatus::Warn, "runtime-operator CRDs are also installed");
            check.details = v2;
            check.hint = Some("forge manages the v1 operator. Running v1 and v2 side by side is untested".to_string());
        }
    }

    async fn check_api_service(&mut self) {
        const CAT: &str = "Aggregated API";
        let api_services: Api<APIService> = Api::all(self.client.clone());

        let api_service = match api_services.get_opt(API_SERVICE_NAME).await {
            Ok(Some(api_service)) => api_service,
            Ok(None) => {
                self.record(CAT, API_SERVICE_NAME, CheckStatus::Fail, "not registered").hint =
                    Some("The operator registers it on startup. Check the operator pod above".to_string());
                return;
            }
            Err(e) => {
                self.record(CAT, API_SERVICE_NAME, CheckStatus::Fail, format!("could not read: {}", e));
                return;
            }
        };

        let target = api_service.spec.as_ref().and_then(|s| s.service.as_ref());
        let target_name = target.and_then(|t| t.name.clone()).unwrap_or_default();
        let target_namespace = target.and_then(|t| t.namespace.clone()).unwrap_or_default();
        let target_port = target.and_then(|t| t.port).unwrap_or(443);
        if target_name != OPERATOR_NAME || target_namespace != self.namespace || target_port != OPERATOR_PORT {
            self.record(CAT, API_SERVICE_NAME, CheckStatus::Fail, format!(
                "points at {}/{}:{}, expected {}/{}:{}",
                target_namespace, target_name, target_port, self.namespace, OPERATOR_NAME, OPERATOR_PORT
            )).hint = Some("Another operator install may own core.oam.dev".to_string());
            return;
        }

        if api_service_available(&api_service) {
            self.record(CAT, API_SERVICE_NAME, CheckStatus::Pass, "Available");
        } else {
            let condition = api_service.status.as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|c| c.iter().find(|c| c.type_ == "Available"));
            let reason = condition
                .map(|c| format!("{}: {}", c.reason.as_deref().unwrap_or("Unavailable"), c.message.as_deref().unwrap_or("")))
                .unwrap_or_else(|| "no Available condition yet".to_string());
            self.record(CAT, API_SERVICE_NAME, CheckStatus::Fail, format!("not Available - {}", reason)).hint =
                Some(format!("Check the Service endpoints and the {} binding above", DELEGATOR_BINDING_NAME));
        }
    }

    async fn check_dependencies(&mut self) {
        const CAT: &str = "Dependencies";

        // NATS endpoints the operator's hosts will connect to, keyed by address
        let mut targets: BTreeMap<NatsTarget, Vec<String>> = BTreeMap::new();
        let host_configs = self.host_configs().await;
        for host_config in &host_configs {
            let namespace = host_config.namespace().unwrap_or_else(|| "default".to_string());
            let spec = &host_config.data["spec"];
            let address = spec["natsAddress"].as_str().unwrap_or(DEFAULT_NATS_ADDRESS);
            let target = NatsTarget {
                address: address.to_string(),
                default_namespace: namespace.clone(),
                client_port: spec["natsClientPort"].as_i64().unwrap_or(DEFAULT_NATS_CLIENT_PORT),
                leafnode_port: spec["natsLeafnodePort"].as_i64().unwrap_or(DEFAULT_NATS_LEAFNODE_PORT),
            };
            targets.entry(target).or_default().push(format!("{}/{}", namespace, host_config.name_any()));
        }

        // No hosts yet: check the address a new WasmCloudHostConfig would use by default
        let referenced = !targets.is_empty();
        if !referenced {
            targets.insert(NatsTarget {
                address: DEFAULT_NATS_ADDRESS.to_string(),
                default_namespace: "default".to_string(),
                client_port: DEFAULT_NATS_CLIENT_PORT,
                leafnode_port: DEFAULT_NATS_LEAFNODE_PORT,
            }, Vec::new());
        }

        for (target, users) in targets {
            self.check_nats_target(&target, &users, referenced).await;
        }

        let deployments: Api<Deployment> = Api::all(self.client.clone());
        match deployments.list(&ListParams::default().labels(WADM_SELECTOR)).await {
            Ok(list) if list.items.iter().any(|d| d.status.as_ref().and_then(|s| s.ready_replicas).unwrap_or(0) > 0) => {
                let found: Vec<_> = list.items.iter()
                    .map(|d| format!("{}/{}", d.namespace().unwrap_or_default(), d.name_any()))
                    .collect();
                self.record(CAT, "wadm", CheckStatus::Pass, format!("running ({})", found.join(", ")));
            }
            Ok(list) if !list.items.is_empty() => {
                self.record(CAT, "wadm", CheckStatus::Warn, "deployed but no replicas are ready");
            }
            _ => {
                self.record(CAT, "wadm", CheckStatus::Warn, format!("no Deployment labelled {} found", WADM_SELECTOR)).hint =
                    Some("Install with: forge --with-deps, or helm install wadm oci://ghcr.io/wasmcloud/charts/wadm".to_string());
            }
        }
    }

    async fn check_nats_target(&mut self, target: &NatsTarget, users: &[String], referenced: bool) {
        const CAT: &str = "Dependencies";
        let name = format!("NATS {}", target.address);
        // A missing NATS only breaks things once a host config points at it
        let missing_status = if referenced { CheckStatus::Fail } else { CheckStatus::Warn };
        let used_by = if users.is_empty() {
            vec!["default address for new WasmCloudHostConfigs".to_string()]
        } else {
            users.iter().map(|u| format!("used by {}", u)).collect()
        };

        let Some((service_name, namespace)) = target.cluster_service() else {
            let check = self.record(CAT, &name, CheckStatus::Warn, "outside the cluster, can't be verified from Kubernetes objects");
            check.details = used_by;
            return;
        };

        let services: Api<Service> = Api::namespaced(self.client.clone(), &namespace);
        let service = match services.get_opt(&service_name).await {
            Ok(Some(service)) => service,
            _ => {
                let check = self.record(CAT, &name, missing_status, format!("Service {}/{} not found", namespace, service_name));
                check.details = used_by;
                check.hint = Some("Install with: forge --with-deps, or helm upgrade --install nats nats/nats with JetStream and leafnodes enabled".to_string());
                return;
            }
        };

        let ports = service_ports(&service);
        let missing_ports: Vec<String> = [("client", target.client_port), ("leafnode", target.leafnode_port)]
            .iter()
            .filter(|(_, port)| !ports.contains(port))
            .map(|(kind, port)| format!("{} {}", kind, port))
            .collect();
        if !missing_ports.is_empty() {
            let check = self.record(CAT, &name, missing_status, format!("Service doesn't expose {}", missing_ports.join(", ")));
            check.details = used_by;
            check.hint = Some("Hosts connect through a NATS leaf node, so the leafnode port must be enabled".to_string());
            return;
        }

        let ready = self.ready_endpoints(&service_name, &namespace).await;
        if ready == 0 {
            let check = self.record(CAT, &name, missing_status, format!("Service {}/{} has no ready endpoints", namespace, service_name));
            check.details = used_by;
        } else {
            let check = self.record(CAT, &name, CheckStatus::Pass, format!(
                "{}/{} exposes {}/{} with {} ready endpoint(s)",
                namespace, service_name, target.client_port, target.leafnode_port, ready
            ));
            check.details = used_by;
            check.details.push("JetStream is not verified (requires a NATS connection)".to_string());
        }
    }

    async fn host_configs(&self) -> Vec<DynamicObject> {
        let api: Api<DynamicObject> = Api::all_with(self.client.clone(), &host_config_resource());
        api.list(&ListParams::default()).await.map(|l| l.items).unwrap_or_default()
    }

    async fn ready_endpoints(&self, service: &str, namespace: &str) -> usize {
        let endpoints: Api<Endpoints> = Api::namespaced(self.client.clone(), namespace);
        match endpoints.get_opt(service).await {
            Ok(Some(ep)) => ep.subsets.iter().flatten()
                .map(|s| s.addresses.as_ref().map_or(0, Vec::len))
                .sum(),
            _ => 0,
        }
    }

    async fn log_tail(&self, pod: &Pod, previous: bool) -> Vec<String> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let params = LogParams {
            tail_lines: Some(10),
            previous,
            ..Default::default()
        };
        match pods.logs(&pod.name_any(), &params).await {
            Ok(logs) if !logs.trim().is_empty() => {
                let mut lines = vec![format!("last log lines{}:", if previous { " (previous container)" } else { "" })];
                lines.extend(logs.lines().map(|l| format!("  {}", l)));
                lines
            }
            _ => Vec::new(),
        }
    }

    async fn recent_warnings(&self) -> Vec<String> {
        let events: Api<Event> = Api::namespaced(self.client.clone(), &self.namespace);
        let mut list = events.list(&ListParams::default().fields("type=Warning")).await
            .map(|l| l.items)
            .unwrap_or_default();
        list.sort_by_key(|e| e.last_timestamp.clone());
        list.iter().rev().take(5)
            .map(|e| format!("{}: {}", e.reason.as_deref().unwrap_or(""), e.message.as_deref().unwrap_or("").trim()))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NatsTarget {
    address: String,
    /// Namespace used when the address is a bare service name
    default_namespace: String,
    client_port: i64,
    leafnode_port: i64,
}

impl NatsTarget {
    /// Resolve a cluster-local address like nats://nats.default.svc.cluster.local to (service, namespace)
    fn cluster_service(&self) -> Option<(String, String)> {
        let without_scheme = self.address.rsplit("://").next().unwrap_or(&self.address);
        let host = without_scheme.split([':', '/']).next().unwrap_or(without_scheme);
        if host.is_empty() || host.parse::<IpAddr>().is_ok() {
            return None;
        }

        let parts: Vec<&str> = host.split('.').collect();
        match parts.as_slice() {
            [service] => Some((service.to_string(), self.default_namespace.clone())),
            [service, namespace] => Some((service.to_string(), namespace.to_string())),
            [service, namespace, "svc", ..] => Some((service.to_string(), namespace.to_string())),
            _ => None,
        }
    }
}

fn service_ports(service: &Service) -> Vec<i64> {
    service.spec.as_ref()
        .and_then(|s| s.ports.as_ref())
        .map(|ports| ports.iter().map(|p| p.port as i64).collect())
        .unwrap_or_default()
}

/// Every (verb, resource, group) in `wanted` that no rule in `granted` covers
fn missing_permissions(wanted: &[PolicyRule], granted: &[PolicyRule]) -> Vec<String> {
    let covers = |list: &Option<Vec<String>>, value: &str| {
        list.as_ref().is_some_and(|l| l.iter().any(|v| v == "*" || v == value))
    };

    let mut missing = Vec::new();
    for rule in wanted {
        for group in rule.api_groups.iter().flatten() {
            for resource in rule.resources.iter().flatten() {
                for verb in &rule.verbs {
                    let allowed = granted.iter().any(|g| {
                        covers(&g.api_groups, group)
                            && covers(&g.resources, resource)
                            && g.verbs.iter().any(|v| v == "*" || v == verb)
                    });
                    if !allowed {
                        let group = if group.is_empty() { "core" } else { group };
                        missing.push(format!("{} {} ({})", verb, resource, group));
                    }
                }
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(address: &str) -> NatsTarget {
        NatsTarget {
            address: address.to_string(),
            default_namespace: "apps".to_string(),
            client_port: DEFAULT_NATS_CLIENT_PORT,
            leafnode_port: DEFAULT_NATS_LEAFNODE_PORT,
        }
    }

    #[test]
    fn resolves_cluster_local_nats_addresses() {
        let resolve = |a: &str| target(a).cluster_service();
        assert_eq!(resolve(DEFAULT_NATS_ADDRESS), Some(("nats".into(), "default".into())));
        assert_eq!(resolve("nats://nats.messaging:4222"), Some(("nats".into(), "messaging".into())));
        assert_eq!(resolve("nats"), Some(("nats".into(), "apps".into())));
        assert_eq!(resolve("tls://10.0.0.5:4222"), None);
        assert_eq!(resolve("nats://connect.ngs.global"), None);
    }

    #[test]
    fn reports_permissions_the_cluster_role_lacks() {
        let rule = |groups: &[&str], resources: &[&str], verbs: &[&str]| PolicyRule {
            api_groups: Some(groups.iter().map(|s| s.to_string()).collect()),
            resources: Some(resources.iter().map(|s| s.to_string()).collect()),
            verbs: verbs.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let wanted = [rule(&["apps"], &["deployments"], &["get", "create"]), rule(&[""], &["pods"], &["list"])];

        assert!(missing_permissions(&wanted, &[rule(&["*"], &["*"], &["*"])]).is_empty());
        assert_eq!(
            missing_permissions(&wanted, &[rule(&["apps"], &["deployments"], &["get"])]),
            vec!["create deployments (apps)", "list pods (core)"]
        );
    }
}
