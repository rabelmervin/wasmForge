//! NATS and wadm, installed with helm into the same cluster as the operator (--with-deps)

use crate::error::{InstallerError, InstallerResult};
use colored::*;
use k8s_openapi::api::{apps::v1::Deployment, core::v1::Service};
use kube::{
    api::{Api, ListParams},
    Client,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// The operator's default NATS address is nats://nats.default.svc.cluster.local, so NATS goes here
pub const DEPS_NAMESPACE: &str = "default";

/// Helm release label that marks a release as installed by forge, so uninstall only removes those
const MANAGED_LABEL_KEY: &str = "managed-by";
const MANAGED_LABEL_VALUE: &str = "forge";

/// Release labels (--labels) arrived in helm 3.13
const MIN_HELM: (u32, u32) = (3, 13);

const WADM_SELECTOR: &str = "app.kubernetes.io/name=wadm";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dependency {
    Nats,
    Wadm,
}

impl Dependency {
    fn label(self) -> &'static str {
        match self {
            Dependency::Nats => "NATS",
            Dependency::Wadm => "wadm",
        }
    }

    /// Release name. For NATS it is also the Service name, which the operator's default address expects
    fn release(self) -> &'static str {
        match self {
            Dependency::Nats => "nats",
            Dependency::Wadm => "wadm",
        }
    }

    fn chart_args(self) -> Vec<&'static str> {
        match self {
            Dependency::Nats => vec![
                "nats",
                "--repo", "https://nats-io.github.io/k8s/helm/charts/",
                // Chart 1.2.11 ships NATS 2.10. The v1 operator gives every host a nats:2.10 leaf
                // sidecar, and 2.10 leaf nodes can't connect to a 2.15 server ("wrong port")
                "--version", "1.2.11",
                // wadm keeps its state in JetStream, and hosts join NATS as leaf nodes
                "--set", "config.jetstream.enabled=true",
                "--set", "config.leafnodes.enabled=true",
                // Hosts reach this JetStream through their leaf sidecar as domain "default"
                // (WASMCLOUD_JS_DOMAIN, the operator's jetstreamDomain default)
                "--set", "config.jetstream.merge.domain=default",
            ],
            Dependency::Wadm => vec![
                "oci://ghcr.io/wasmcloud/charts/wadm",
                "--version", "0.2.10",
            ],
        }
    }
}

/// Install order: wadm needs NATS to start. Uninstall runs in reverse
const DEPENDENCIES: [Dependency; 2] = [Dependency::Nats, Dependency::Wadm];

#[derive(Debug, PartialEq)]
enum Action {
    Install,
    /// Our own release: upgrade --install re-applies forge's settings
    Upgrade,
    /// Present but not installed by forge, so it isn't ours to change
    Skip,
}

pub struct Dependencies {
    client: Client,
    kubeconfig: PathBuf,
    context: String,
    timeout: u64,
}

impl Dependencies {
    /// helm gets the same kubeconfig and context as forge, so everything lands in one cluster
    pub fn new(client: &Client, kubeconfig: &Path, context: &str, timeout: u64) -> Self {
        Self {
            client: client.clone(),
            kubeconfig: kubeconfig.to_path_buf(),
            context: context.to_string(),
            timeout,
        }
    }

    pub async fn install(&self, dry_run: bool) -> InstallerResult<()> {
        println!("{}", "🧩 Installing dependencies (NATS, wadm) with helm...".blue().bold());
        self.check_helm().await?;
        println!("   Target: context {} via {}", self.context.green(), self.kubeconfig.display().to_string().yellow());

        let ours = self.forge_releases().await?;
        for dep in DEPENDENCIES {
            let action = match (ours.contains(dep.release()), self.present(dep).await) {
                (true, _) => Action::Upgrade,
                (false, true) => Action::Skip,
                (false, false) => Action::Install,
            };

            match (&action, dry_run) {
                (Action::Skip, _) => println!(
                    "   {} {}: already present, not installed by forge, leaving it as is",
                    "•".yellow(), dep.label()
                ),
                (_, true) => println!(
                    "   {} {}: would {} release {} in namespace {}",
                    "•".blue(), dep.label(),
                    if action == Action::Install { "install" } else { "upgrade" },
                    dep.release(), DEPS_NAMESPACE
                ),
                (_, false) => {
                    println!("   {} {}: {}... (downloads the chart, can take a few minutes)", "→".blue(), dep.label(),
                        if action == Action::Install { "installing" } else { "upgrading" });
                    self.helm(&self.upgrade_install_args(dep)).await?;
                    println!("   {} {}: ready", "✓".green(), dep.label());
                }
            }
        }

        Ok(())
    }

    /// Remove only the releases forge installed; anything else is left in place
    pub async fn uninstall(&self) -> InstallerResult<()> {
        println!();
        println!("{}", "🧩 Removing dependencies installed by forge...".red().bold());
        self.check_helm().await?;

        let ours = self.forge_releases().await?;
        for dep in DEPENDENCIES.iter().rev() {
            if ours.contains(dep.release()) {
                self.helm(&self.uninstall_args(*dep)).await?;
                println!("   {} {}: release {} removed", "✓".green(), dep.label(), dep.release());
            } else if self.present(*dep).await {
                println!("   {} {}: not installed by forge, leaving it in place", "•".yellow(), dep.label());
            } else {
                println!("   {} {}: not found", "•".dimmed(), dep.label());
            }
        }

        Ok(())
    }

    async fn check_helm(&self) -> InstallerResult<()> {
        let output = Command::new("helm").args(["version", "--short"]).output().await.map_err(|_| {
            InstallerError::ValidationError("--with-deps needs helm on PATH, see https://helm.sh/docs/intro/install/".to_string())
        })?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("helm version: {}", version);

        match parse_helm_version(&version) {
            Some(v) if v >= MIN_HELM => Ok(()),
            _ => Err(InstallerError::ValidationError(format!(
                "--with-deps needs helm {}.{} or newer, found {}",
                MIN_HELM.0, MIN_HELM.1, if version.is_empty() { "an unknown version" } else { &version }
            ))),
        }
    }

    /// Releases in the dependency namespace carrying forge's label
    async fn forge_releases(&self) -> InstallerResult<HashSet<String>> {
        let mut args = vec!["list".to_string(), "--short".to_string(), "--selector".to_string(),
            format!("{}={}", MANAGED_LABEL_KEY, MANAGED_LABEL_VALUE)];
        args.extend(self.target_args());
        let stdout = self.helm(&args).await?;
        Ok(stdout.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
    }

    /// Whether the dependency already runs in the cluster, however it was installed
    async fn present(&self, dep: Dependency) -> bool {
        match dep {
            Dependency::Nats => {
                let services: Api<Service> = Api::namespaced(self.client.clone(), DEPS_NAMESPACE);
                matches!(services.get_opt(dep.release()).await, Ok(Some(_)))
            }
            Dependency::Wadm => {
                let deployments: Api<Deployment> = Api::all(self.client.clone());
                deployments.list(&ListParams::default().labels(WADM_SELECTOR)).await
                    .map(|l| !l.items.is_empty())
                    .unwrap_or(false)
            }
        }
    }

    fn target_args(&self) -> Vec<String> {
        vec![
            "--kubeconfig".to_string(), self.kubeconfig.display().to_string(),
            "--kube-context".to_string(), self.context.clone(),
            "--namespace".to_string(), DEPS_NAMESPACE.to_string(),
        ]
    }

    fn upgrade_install_args(&self, dep: Dependency) -> Vec<String> {
        let mut args = vec!["upgrade".to_string(), "--install".to_string(), dep.release().to_string()];
        args.extend(dep.chart_args().into_iter().map(String::from));
        args.extend([
            "--labels".to_string(), format!("{}={}", MANAGED_LABEL_KEY, MANAGED_LABEL_VALUE),
            "--wait".to_string(),
            "--timeout".to_string(), format!("{}s", self.timeout),
        ]);
        args.extend(self.target_args());
        args
    }

    fn uninstall_args(&self, dep: Dependency) -> Vec<String> {
        let mut args = vec!["uninstall".to_string(), dep.release().to_string(), "--wait".to_string()];
        args.extend(self.target_args());
        args
    }

    async fn helm(&self, args: &[String]) -> InstallerResult<String> {
        debug!("helm {}", args.join(" "));
        let output = Command::new("helm").args(args).output().await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(InstallerError::InstallationError(format!(
                "helm {} failed: {}",
                args.first().map(String::as_str).unwrap_or(""),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

/// "v3.17.3+ge4da497" -> (3, 17)
fn parse_helm_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().trim_start_matches('v').split(['.', '+', '-']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_helm_versions() {
        assert_eq!(parse_helm_version("v3.17.3+ge4da497"), Some((3, 17)));
        assert_eq!(parse_helm_version("v3.13.0"), Some((3, 13)));
        assert_eq!(parse_helm_version("v4.0.1"), Some((4, 0)));
        assert_eq!(parse_helm_version(""), None);
        assert!(parse_helm_version("v3.12.3").unwrap() < MIN_HELM);
    }

    #[tokio::test]
    async fn helm_targets_the_same_cluster_as_forge() {
        // Building a Client needs no live cluster as long as nothing is sent
        let config = kube::Config::new("http://127.0.0.1:1".parse().unwrap());
        let client = Client::try_from(config).unwrap();
        let deps = Dependencies::new(&client, Path::new("kubeconfig"), "kind-wasmcloud-demo", 300);

        let args = deps.upgrade_install_args(Dependency::Nats).join(" ");
        assert!(args.starts_with("upgrade --install nats nats --repo"));
        assert!(args.contains("--kubeconfig kubeconfig --kube-context kind-wasmcloud-demo --namespace default"));
        assert!(args.contains("config.leafnodes.enabled=true"));
        assert!(args.contains("config.jetstream.merge.domain=default"));
        assert!(args.contains("--labels managed-by=forge"));
        assert!(args.contains("--timeout 300s"));

        let args = deps.uninstall_args(Dependency::Wadm).join(" ");
        assert_eq!(args, "uninstall wadm --wait --kubeconfig kubeconfig --kube-context kind-wasmcloud-demo --namespace default");
    }
}
