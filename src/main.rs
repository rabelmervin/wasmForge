use anyhow::Result;
use clap::{Arg, Command, ArgAction};
use colored::*;
use std::path::PathBuf;
use tracing::{info, warn, debug};

mod config;
mod crds;
mod doctor;
mod error;
mod manifests;
mod install;
mod kubernetes;
mod validator;

use config::KubeConfig;
use doctor::Doctor;
use error::InstallerError;
use install::WasmcloudInstaller;
use manifests::{Manifests, OPERATOR_IMAGE};
use kubernetes::KubeClient;
use validator::Validator;

#[tokio::main]
async fn main() -> Result<()> {
    let app = Command::new("wasmForge")
        .version("0.1.0")
        .author("Your Name <your.email@example.com>")
        .about("Install the wasmCloud v1 operator (wasmcloud-operator) on Kubernetes")
        .long_about("A CLI tool that reads your kubeconfig and installs the wasmCloud v1 operator (wasmcloud-operator) on your Kubernetes cluster")
        .arg(
            Arg::new("kubeconfig")
                .short('k')
                .long("kubeconfig")
                .value_name("FILE")
                .help("Path to kubeconfig file")
                .action(ArgAction::Set)
        )
        .arg(
            Arg::new("namespace")
                .short('n')
                .long("namespace")
                .value_name("NAMESPACE")
                .help("Kubernetes namespace to install into")
                .default_value("wasmcloud-operator")
                .action(ArgAction::Set)
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .help("Show what would be installed without actually installing")
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging")
                .action(ArgAction::Count)
        )
        .arg(
            Arg::new("skip-validation")
                .long("skip-validation")
                .help("Skip pre-installation validation checks")
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("timeout")
                .short('t')
                .long("timeout")
                .value_name("SECONDS")
                .help("Timeout for installation operations in seconds")
                .default_value("300")
                .action(ArgAction::Set)
        )
        .arg(
            Arg::new("uninstall")
                .long("uninstall")
                .help("Remove all wasmCloud operator resources from the cluster")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(&["dry-run", "upgrade", "doctor"])
        )
        .arg(
            Arg::new("upgrade")
                .long("upgrade")
                .help("Update existing wasmCloud operator installation with new configuration")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(&["dry-run", "uninstall", "status", "doctor"])
        )
        .arg(
            Arg::new("status")
                .long("status")
                .help("Check the current installation status of wasmCloud operator")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(&["dry-run", "uninstall", "upgrade", "doctor"])
        )
        .arg(
            Arg::new("doctor")
                .long("doctor")
                .help("Diagnose an installation: pod crashes, RBAC gaps, CRD versions, aggregated API, NATS and wadm")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(&["dry-run", "uninstall", "upgrade", "status"])
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FORMAT")
                .help("Output format: json for --status/--doctor, yaml for --dry-run (no cluster needed)")
                .value_parser(["text", "json", "yaml"])
                .default_value("text")
                .action(ArgAction::Set)
        );

    let matches = app.get_matches();

    // Handle verbose flag for additional logging before initializing tracing
    let verbose = matches.get_count("verbose");
    if verbose > 0 {
        std::env::set_var("RUST_LOG", "debug");
    }

    // Initialize tracing with proper log level
    init_logging()?;

    run(matches).await
}

async fn run(matches: clap::ArgMatches) -> Result<()> {
    // Get command line arguments
    let kubeconfig_path = matches.get_one::<String>("kubeconfig");
    let namespace = matches.get_one::<String>("namespace").unwrap();
    let dry_run = matches.get_flag("dry-run");
    let skip_validation = matches.get_flag("skip-validation");
    let timeout = matches.get_one::<String>("timeout").unwrap().parse::<u64>().unwrap_or(300);
    let uninstall = matches.get_flag("uninstall");
    let upgrade = matches.get_flag("upgrade");
    let status = matches.get_flag("status");
    let doctor = matches.get_flag("doctor");
    let output = matches.get_one::<String>("output").unwrap().as_str();

    match output {
        "json" if !(status || doctor) => {
            return Err(InstallerError::ValidationError("--output json is only supported with --status or --doctor".to_string()).into());
        }
        "yaml" if !dry_run => {
            return Err(InstallerError::ValidationError("--output yaml is only supported with --dry-run".to_string()).into());
        }
        _ => {}
    }

    // Rendering manifests needs no cluster, so GitOps pipelines can run this offline
    if dry_run && output == "yaml" {
        print!("{}", Manifests::new(namespace, OPERATOR_IMAGE).render_yaml()?);
        return Ok(());
    }

    // Machine-readable output keeps stdout free of progress text
    let quiet = output != "text";
    if !quiet {
        println!("{}", "🚀 Wasmcloud Operator Installer".cyan().bold());
        println!("{}", "================================".cyan());
        println!();
    }

    info!("Starting Wasmcloud operator operation");
    debug!("Configuration: namespace={}, dry_run={}, skip_validation={}, timeout={}s, uninstall={}, upgrade={}, status={}", 
           namespace, dry_run, skip_validation, timeout, uninstall, upgrade, status);

    // Step 1: Load kubeconfig
    let config = load_kubeconfig(kubeconfig_path, quiet).await?;
    
    // Step 2: Create Kubernetes client
    let kube_client = create_kube_client(&config, kubeconfig_path, quiet).await?;

    // Step 3: Determine operation mode
    if uninstall {
        // Uninstall operation
        uninstall_operator(&kube_client, namespace).await?;
        return Ok(());
    } else if upgrade {
        // Upgrade operation
        upgrade_operator(&kube_client, namespace, timeout).await?;
        return Ok(());
    } else if status {
        // Status check operation
        check_operator_status(&kube_client, namespace, quiet).await?;
        return Ok(());
    } else if doctor {
        let healthy = run_doctor(&kube_client, namespace, quiet).await?;
        if !healthy {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Default installation operation continues below

    // Step 4: Run validation (unless skipped)
    if !skip_validation {
        run_validation(&kube_client, namespace).await?;
    } else {
        warn!("Skipping validation checks as requested");
    }

    // Step 5: Install Wasmcloud operator
    let installer = install_operator(&kube_client, namespace, dry_run, timeout).await?;

    // Step 6: Show installation summary
    if !dry_run {
        installer.print_installation_summary().await?;
    } else {
        println!();
        println!("{}", "✓ Dry run completed successfully!".green().bold());
        println!("{}", "No resources were created.".dimmed());
    }

    Ok(())
}

async fn load_kubeconfig(kubeconfig_path: Option<&String>, quiet: bool) -> Result<KubeConfig> {
    if !quiet {
        println!("{}", "📋 Loading kubeconfig...".blue().bold());
    }

    let config = match kubeconfig_path {
        Some(path) => {
            info!("Loading kubeconfig from: {}", path);
            if !quiet {
                println!("   Using kubeconfig: {}", path.yellow());
            }
            KubeConfig::from_file(path)?
        }
        None => {
            let default_path = default_kubeconfig_path()?;
            info!("Loading kubeconfig from default location: {:?}", default_path);
            if !quiet {
                println!("   Using default kubeconfig: {}", default_path.display().to_string().yellow());
            }
            KubeConfig::from_file(default_path)?
        }
    };

    let cluster = config.get_current_cluster()?;
    if !quiet {
        println!("   Cluster: {}", cluster.cluster.server.green());
        println!("   Context: {}", config.current_context.green());
    }
    
    Ok(config)
}

fn default_kubeconfig_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| InstallerError::KubeconfigError("Could not find home directory".to_string()))?;
    
    Ok(home.join(".kube").join("config"))
}

async fn create_kube_client(config: &KubeConfig, kubeconfig_path: Option<&String>, quiet: bool) -> Result<KubeClient> {
    if !quiet {
        println!("{}", "🔌 Connecting to Kubernetes...".blue().bold());
    }

    let client = match kubeconfig_path {
        Some(path) => KubeClient::with_kubeconfig_path(path).await?,
        None => KubeClient::new(config).await?,
    };

    // Test the connection
    client.test_connection().await?;
    
    if !quiet {
        println!("   Connection: {}", "✓ Connected".green());
    }
    
    Ok(client)
}

async fn run_validation(client: &KubeClient, namespace: &str) -> Result<()> {
    println!("{}", "🔍 Running validation checks...".blue().bold());

    let validator = Validator::new(client.client());
    
    // Run all validation checks
    validator.validate_cluster_access().await?;
    validator.validate_permissions(namespace).await?;
    validator.validate_prerequisites().await?;
    
    println!("   Validation: {}", "✓ All checks passed".green());
    
    Ok(())
}

async fn install_operator(client: &KubeClient, namespace: &str, dry_run: bool, timeout: u64) -> Result<WasmcloudInstaller> {
    if dry_run {
        println!("{}", "🔍 Dry run - showing what would be installed...".blue().bold());
    } else {
        println!("{}", "📦 Installing Wasmcloud operator...".blue().bold());
    }

    let installer = WasmcloudInstaller::new(client.client(), namespace, timeout, OPERATOR_IMAGE);
    
    if dry_run {
        installer.dry_run().await?;
        println!("   Dry run: {}", "✓ Installation plan validated".green());
    } else {
        installer.install().await?;
        println!("   Installation: {}", "✓ Operator deployed".green());
        
        // Wait for deployment to be ready
        println!("{}", "⏳ Waiting for operator to be ready...".blue().bold());
        installer.wait_for_ready().await?;
        println!("   Status: {}", "✓ Operator is running".green());
    }
    
    Ok(installer)
}

fn init_logging() -> Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_writer(std::io::stderr)
        .compact()
        .init();
    
    Ok(())
}

async fn uninstall_operator(client: &KubeClient, namespace: &str) -> Result<()> {
    println!("{}", " Uninstalling Wasmcloud operator...".red().bold());
    println!("{}", "===================================".red());
    println!();

    let installer = WasmcloudInstaller::new(client.client(), namespace, 300, "");
    
    installer.uninstall().await?;
    
    println!();
    println!("{}", "Uninstallation completed successfully!".green().bold());
    println!("{}", "All wasmCloud operator resources have been removed.".dimmed());
    
    Ok(())
}

async fn upgrade_operator(client: &KubeClient, namespace: &str, timeout: u64) -> Result<()> {
    println!("{}", " Upgrading Wasmcloud operator...".yellow().bold());
    println!("{}", "=================================".yellow());
    println!();

    let installer = WasmcloudInstaller::new(client.client(), namespace, timeout, OPERATOR_IMAGE);
    
    installer.upgrade().await?;
    
    // Wait for deployment to be ready
    println!("{}", "⏳ Waiting for upgraded operator to be ready...".blue().bold());
    installer.wait_for_ready().await?;
    println!("   Status: {}", "✓ Upgraded operator is running".green());
    
    println!();
    println!("{}", "Upgrade completed successfully!".green().bold());
    
    Ok(())
}
async fn check_operator_status(client: &KubeClient, namespace: &str, json: bool) -> Result<()> {
    let installer = WasmcloudInstaller::new(client.client(), namespace, 300, "");

    if json {
        let status = installer.check_installation_status().await?;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("{}", " Checking wasmCloud operator status...".blue().bold());
    println!("{}", "======================================".blue());
    println!();

    let status = installer.check_installation_status().await?;
    
    status.print_status_report();
    
    Ok(())
}

/// Run every diagnostic check and print the report. Returns whether nothing failed.
async fn run_doctor(client: &KubeClient, namespace: &str, json: bool) -> Result<bool> {
    let report = Doctor::new(client.client(), namespace, OPERATOR_IMAGE).run().await;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!();
        report.print();
    }

    Ok(report.healthy)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_kubeconfig_path() {
        let path = default_kubeconfig_path();
        assert!(path.is_ok());
        
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains(".kube"));
        assert!(path.to_string_lossy().contains("config"));
    }
}