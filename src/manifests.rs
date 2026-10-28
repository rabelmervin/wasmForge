use crate::error::InstallerResult;
use k8s_openapi::api::{
    apps::v1::{Deployment, DeploymentSpec},
    core::v1::{
        Container, ContainerPort, EnvVar, EnvVarSource, Namespace, ObjectFieldSelector, PodSpec, PodTemplateSpec,
        Service, ServiceAccount, ServicePort, ServiceSpec,
    },
    rbac::v1::{ClusterRole, ClusterRoleBinding, PolicyRule, RoleRef, Subject},
};
use k8s_openapi::apimachinery::pkg::{
    apis::meta::v1::{LabelSelector, ObjectMeta},
    util::intstr::IntOrString,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Last release of the wasmCloud v1 operator published under the `wasmcloud-operator` name.
/// Later releases were renamed to `wadm-operator`.
pub const OPERATOR_IMAGE: &str = "ghcr.io/wasmcloud/wasmcloud-operator:0.5.0";

/// Name shared by the operator's Deployment, Service, ServiceAccount and RBAC objects.
/// The operator registers its APIService against a Service with exactly this name.
pub const OPERATOR_NAME: &str = "wasmcloud-operator";
pub const DELEGATOR_BINDING_NAME: &str = "wasmcloud-operator-delegator";
pub const DELEGATOR_CLUSTER_ROLE: &str = "system:auth-delegator";
pub const OPERATOR_PORT: i32 = 8443;

/// Aggregated API the operator serves wadm applications through (core.oam.dev/v1beta1)
pub const API_SERVICE_NAME: &str = "v1beta1.core.oam.dev";

/// Label selector matching the operator's pods
pub fn operator_pod_selector() -> String {
    format!("app={}", OPERATOR_NAME)
}

/// Builds every object the installer applies for one namespace and image
pub struct Manifests {
    namespace: String,
    image: String,
}

impl Manifests {
    pub fn new(namespace: &str, image: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            image: image.to_string(),
        }
    }

    /// All manifests as a multi-document YAML stream, in apply order
    pub fn render_yaml(&self) -> InstallerResult<String> {
        let documents = [
            to_yaml(&self.namespace())?,
            to_yaml(&self.service_account())?,
            to_yaml(&self.cluster_role())?,
            to_yaml(&self.cluster_role_binding())?,
            to_yaml(&self.delegator_binding())?,
            to_yaml(&self.service())?,
            to_yaml(&self.deployment())?,
        ];
        Ok(documents.iter().map(|doc| format!("---\n{}", doc)).collect())
    }

    pub fn namespace(&self) -> Namespace {
        Namespace {
            metadata: ObjectMeta {
                name: Some(self.namespace.clone()),
                labels: Some(common_labels()),
                ..Default::default()
            },
            spec: None,
            status: None,
        }
    }

    pub fn service_account(&self) -> ServiceAccount {
        ServiceAccount {
            metadata: ObjectMeta {
                name: Some(OPERATOR_NAME.to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(common_labels()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Mirrors the upstream ClusterRole in wasmcloud-operator v0.5.0 (deploy/base/deployment.yaml)
    pub fn cluster_role(&self) -> ClusterRole {
        let rule = |group: &str, resources: &[&str], verbs: &[&str]| PolicyRule {
            api_groups: Some(vec![group.to_string()]),
            resources: Some(resources.iter().map(|r| r.to_string()).collect()),
            verbs: verbs.iter().map(|v| v.to_string()).collect(),
            ..Default::default()
        };
        let manage = &["get", "list", "watch", "create", "delete", "patch", "update"];

        ClusterRole {
            metadata: ObjectMeta {
                name: Some(OPERATOR_NAME.to_string()),
                labels: Some(common_labels()),
                ..Default::default()
            },
            rules: Some(vec![
                rule("", &["secrets", "services", "configmaps", "serviceaccounts", "pods"], manage),
                rule("apps", &["deployments", "daemonsets"], manage),
                rule("rbac.authorization.k8s.io", &["rolebindings", "roles"], &["get", "list", "watch", "create", "delete", "patch"]),
                // The operator registers its own CRD on startup
                rule("apiextensions.k8s.io", &["customresourcedefinitions"], &["get", "list", "watch", "create", "delete", "patch"]),
                // ...and the core.oam.dev aggregated API
                rule("apiregistration.k8s.io", &["apiservices"], &["create", "delete", "get", "list", "patch", "update"]),
                rule("discovery.k8s.io", &["endpointslices"], &["create", "delete", "get", "list", "patch", "update"]),
                rule("k8s.wasmcloud.dev", &["wasmcloudhostconfigs", "wasmcloudhostconfigs/status"], &["*"]),
            ]),
            ..Default::default()
        }
    }

    pub fn cluster_role_binding(&self) -> ClusterRoleBinding {
        self.binding(OPERATOR_NAME, OPERATOR_NAME)
    }

    /// Lets the operator's aggregated API server delegate authn/authz to the kube-apiserver
    pub fn delegator_binding(&self) -> ClusterRoleBinding {
        self.binding(DELEGATOR_BINDING_NAME, DELEGATOR_CLUSTER_ROLE)
    }

    fn binding(&self, name: &str, cluster_role: &str) -> ClusterRoleBinding {
        ClusterRoleBinding {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                labels: Some(common_labels()),
                ..Default::default()
            },
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".to_string(),
                kind: "ClusterRole".to_string(),
                name: cluster_role.to_string(),
            },
            subjects: Some(vec![Subject {
                kind: "ServiceAccount".to_string(),
                name: OPERATOR_NAME.to_string(),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            }]),
        }
    }

    pub fn deployment(&self) -> Deployment {
        let mut labels = common_labels();
        labels.insert("app".to_string(), OPERATOR_NAME.to_string());

        Deployment {
            metadata: ObjectMeta {
                name: Some(OPERATOR_NAME.to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(1),
                selector: LabelSelector {
                    match_labels: Some(app_selector()),
                    ..Default::default()
                },
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(labels),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        service_account_name: Some(OPERATOR_NAME.to_string()),
                        containers: vec![Container {
                            name: "operator".to_string(),
                            image: Some(self.image.clone()),
                            image_pull_policy: Some("IfNotPresent".to_string()),
                            ports: Some(vec![ContainerPort {
                                container_port: OPERATOR_PORT,
                                name: Some("https".to_string()),
                                protocol: Some("TCP".to_string()),
                                ..Default::default()
                            }]),
                            env: Some(vec![
                                EnvVar {
                                    name: "RUST_LOG".to_string(),
                                    value: Some("info".to_string()),
                                    ..Default::default()
                                },
                                // The operator registers its APIService against the Service in this namespace
                                EnvVar {
                                    name: "POD_NAMESPACE".to_string(),
                                    value_from: Some(EnvVarSource {
                                        field_ref: Some(ObjectFieldSelector {
                                            field_path: "metadata.namespace".to_string(),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                            ]),
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

    /// The operator's APIService points at this Service by name on port 8443, so neither can change
    pub fn service(&self) -> Service {
        Service {
            metadata: ObjectMeta {
                name: Some(OPERATOR_NAME.to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(common_labels()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: Some(app_selector()),
                ports: Some(vec![ServicePort {
                    name: Some("https".to_string()),
                    port: OPERATOR_PORT,
                    target_port: Some(IntOrString::String("https".to_string())),
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            status: None,
        }
    }
}

/// Labels on every object the installer creates
pub fn common_labels() -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert("app.kubernetes.io/name".to_string(), "wasmcloud-operator".to_string());
    labels.insert("app.kubernetes.io/component".to_string(), "operator".to_string());
    labels.insert("app.kubernetes.io/part-of".to_string(), "wasmcloud".to_string());
    labels.insert("app.kubernetes.io/managed-by".to_string(), "forge".to_string());
    labels
}

fn app_selector() -> BTreeMap<String, String> {
    let mut selector = BTreeMap::new();
    selector.insert("app".to_string(), OPERATOR_NAME.to_string());
    selector
}

fn to_yaml<T: Serialize>(obj: &T) -> InstallerResult<String> {
    Ok(serde_yaml::to_string(obj)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_yaml_emits_every_object_with_type_info() {
        let yaml = Manifests::new("demo", OPERATOR_IMAGE).render_yaml().unwrap();

        assert_eq!(yaml.matches("---\n").count(), 7);
        for kind in ["Namespace", "ServiceAccount", "ClusterRole", "ClusterRoleBinding", "Service", "Deployment"] {
            assert!(yaml.contains(&format!("kind: {}\n", kind)), "missing kind {}", kind);
        }
        assert!(yaml.contains(OPERATOR_IMAGE));
        assert!(yaml.contains("namespace: demo"));
    }

    #[test]
    fn bindings_target_the_install_namespace() {
        let manifests = Manifests::new("demo", OPERATOR_IMAGE);
        for binding in [manifests.cluster_role_binding(), manifests.delegator_binding()] {
            let subject = &binding.subjects.unwrap()[0];
            assert_eq!(subject.namespace.as_deref(), Some("demo"));
            assert_eq!(subject.name, OPERATOR_NAME);
        }
    }
}
