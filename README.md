# wasmForge

A Rust-based command-line tool for automated deployment of the wasmCloud operator on Kubernetes clusters.

## Overview

This tool automates the installation of the wasmCloud operator by performing the following operations:
- Creates the target namespace and configures RBAC (Role-Based Access Control) permissions
- Installs all required Custom Resource Definitions (CRDs) for both runtime and legacy compatibility
- Deploys the operator and verifies successful initialization
- Performs pre-installation validation to ensure cluster compatibility
- Supports any standard Kubernetes cluster with appropriate kubeconfig authentication

## Quick Start

### Prerequisites

- Kubernetes cluster version 1.20 or later
- kubeconfig with permissions to create namespaces and configure cluster-level RBAC

### Installation

#### Build from Source

```bash
git clone https://github.com/rabelmervin/wasmcloud-operator-installer
cd wasmcloud-operator-installer
cargo build --release
./target/r okelease/wasmcloud-installer --help
```

### Create a Kubernetes Cluster

```bash
# Create a kind cluster for wasmCloud
kind create cluster --name wasmcloud-cluster

# Verify cluster is running
kubectl cluster-info --context wasmcloud-cluster
```

## Usage

### Installation Examples

```bash
# Basic installation with default settings
./target/release/wasmcloud-installer

# Specify custom kubeconfig
./target/release/wasmcloud-installer --kubeconfig ~/.kube/config

# Define custom namespace
./target/release/wasmcloud-installer --namespace wasmcloud-system

# Preview installation without applying changes
./target/release/wasmcloud-installer --dry-run

# Enable verbose logging for troubleshooting
./target/release/wasmcloud-installer --verbose

# Complete example with all options
./target/release/wasmcloud-installer --kubeconfig ~/.kube/config --namespace wasmcloud-demo
```

### Uninstall Examples

```bash
# Remove wasmCloud operator from default namespace
./target/release/wasmcloud-installer --uninstall

# Remove from custom namespace
./target/release/wasmcloud-installer --uninstall --namespace wasmcloud-system

# Uninstall with custom kubeconfig
./target/release/wasmcloud-installer --uninstall --kubeconfig ~/.kube/prod-config
```

### Upgrade Examples

```bash
# Upgrade existing installation
./target/release/wasmcloud-installer --upgrade

# Upgrade installation in custom namespace
./target/release/wasmcloud-installer --upgrade --namespace wasmcloud-system

# Upgrade with different kubeconfig (for cluster migration)
./target/release/wasmcloud-installer --upgrade --kubeconfig ~/.kube/new-cluster
```

### Status Check Examples

```bash
# Check status in default namespace
./target/release/wasmcloud-installer --status

# Check status in custom namespace
./target/release/wasmcloud-installer --status --namespace wasmcloud-system

# Check status with custom kubeconfig
./target/release/wasmcloud-installer --status --kubeconfig ~/.kube/prod-config
```

## CLI Options

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--kubeconfig` | `-k` | Path to kubeconfig file | `~/.kube/config` |
| `--namespace` | `-n` | Target namespace | `wasmcloud-operator` |
| `--dry-run` | | Show installation plan without executing | `false` |
| `--skip-validation` | | Skip pre-installation checks | `false` |
| `--timeout` | `-t` | Installation timeout in seconds | `300` |
| `--verbose` | `-v` | Enable verbose logging | `false` |
| `--uninstall` | | Remove all wasmCloud operator resources from the cluster | `false` |
| `--upgrade` | | Update existing wasmCloud operator installation with new configuration | `false` |
| `--status` | | Check the current installation status of wasmCloud operator | `false` |
| `--help` | `-h` | Show help information | |
| `--version` | `-V` | Show version information | |

**Note**: The `--uninstall`, `--upgrade`, and `--status` flags cannot be used together with `--dry-run` or each other.

## Installation Components

The installer deploys the following Kubernetes resources:

### Namespace
The specified namespace (default: `wasmcloud-operator`) is created if not present.

### RBAC Configuration
- ServiceAccount for operator identity
- ClusterRole defining required permissions
- ClusterRoleBinding linking the ServiceAccount to the ClusterRole

### Operator Deployment
- Kubernetes Deployment running the wasmCloud operator
- Service exposing operator endpoints

### Custom Resource Definitions (CRDs)
**Runtime CRDs** (runtime.wasmcloud.dev/v1alpha1):
- **Artifact**: Container for WebAssembly component images and OCI artifacts
- **Host**: Defines wasmCloud runtime execution environments
- **Workload**: Application specifications including components and configuration
- **WorkloadDeployment**: Primary deployment interface analogous to Kubernetes Deployment
- **WorkloadReplicaSet**: Manages workload replicas (controlled by WorkloadDeployment)

**Legacy CRDs** (runtime.wasmcloud.dev/v1alpha1 - for backward compatibility):
- **WasmCloudHostConfig**: Legacy host configuration definitions
- **WasmCloudApplication**: Legacy application definitions

### Operator Deployment Configuration
- **Container Image**: `ghcr.io/wasmcloud/wasmcloud-operator:0.4.0`
- **Resource Requests**: 100m CPU, 128Mi memory
- **Resource Limits**: 200m CPU, 256Mi memory
- **Listening Port**: 8080

## Operations

### Installation (Default)

Standard installation creates all resources and deploys the operator:

```bash
./target/release/wasmcloud-installer --namespace wasmcloud-operator
```

### Uninstallation

The `--uninstall` flag removes all wasmCloud operator resources from the specified cluster:

```bash
# Remove all wasmCloud operator resources
./target/release/wasmcloud-installer --uninstall --namespace wasmcloud-operator

# Remove from a different namespace
./target/release/wasmcloud-installer --uninstall --namespace my-wasmcloud
```

**What gets removed during uninstall:**
- Operator deployment and pods
- Service exposing operator endpoints
- ClusterRole and ClusterRoleBinding (RBAC resources)
- ServiceAccount
- All Custom Resource Definitions (CRDs)
- Namespace (only if it was created by the installer and is empty)

**Important Notes:**
- Uninstall will remove CRDs, which will also delete any existing custom resources (WasmCloudApplications, etc.)
- The namespace is only deleted if it was created by the installer and contains no other resources
- Uninstall is a destructive operation and cannot be undone

### Upgrade

The `--upgrade` flag updates an existing wasmCloud operator installation:

```bash
# Upgrade to latest operator version
./target/release/wasmcloud-installer --upgrade --namespace wasmcloud-operator

# Upgrade with custom kubeconfig
./target/release/wasmcloud-installer --upgrade -k /path/to/kubeconfig
```

**What gets updated during upgrade:**
- Custom Resource Definitions (CRDs) are updated to latest versions
- ClusterRole permissions are updated
- ClusterRoleBinding is updated to reference current namespace
- Deployment is updated with latest container image and configuration
- Service configuration is updated if needed

**Upgrade Process:**
1. Updates CRDs with latest schema definitions
2. Updates RBAC resources (ClusterRole and ClusterRoleBinding)
3. Performs rolling update of the operator deployment
4. Verifies the upgraded operator is running correctly

**Use Cases:**
- Switching to a new kubeconfig file/cluster context
- Updating to a newer version of the operator
- Applying configuration changes
- Fixing resource permission issues

### Status Check

The `--status` flag checks the current installation status of the wasmCloud operator:

```bash
# Check status of installation
./target/release/wasmcloud-installer --status --namespace wasmcloud-operator

# Check with custom kubeconfig
./target/release/wasmcloud-installer --status -k /path/to/kubeconfig
```

**Status Information Provided:**
- Overall installation status (Not Installed, Partially Installed, Installed Not Ready, Installed and Ready)
- Namespace existence and resources
- Deployment status and pod health
- Service and ServiceAccount status
- RBAC resources (ClusterRole, ClusterRoleBinding)
- Custom Resource Definitions (CRDs) status
- Recommendations for next steps

**Use Cases:**
- Verify installation before performing operations
- Troubleshoot installation issues
- Check health status of running operator
- Audit what components are installed

## Pre-Installation Validation

The installer performs the following validation checks before deployment:
- Cluster API server connectivity and accessibility
- User permissions for namespace and RBAC resource creation
- Kubernetes version compatibility (1.20 or later)
- CRD API support and creation permissions
- Cluster node availability and resource capacity
- Detection of existing or conflicting wasmCloud installations

## Troubleshooting

### Authorization Errors

If "Forbidden" errors occur, verify that the kubeconfig user has permissions to create namespaces and cluster roles:

```bash
kubectl auth can-i create namespaces
```

### Terminating Namespace

If a namespace remains in terminating state, specify an alternate namespace name:

```bash
./target/release/wasmcloud-installer --namespace wasmcloud-alternate
```

### Installation Timeout

For slow clusters, increase the timeout duration (default: 300 seconds):

```bash
./target/release/wasmcloud-installer --timeout 600
```

### Verbose Logging

Enable debug output for detailed troubleshooting:

```bash
./target/release/wasmcloud-installer --verbose
```

### Manual Resource Cleanup

If installation fails, remove resources manually:

```bash
kubectl delete namespace wasmcloud-operator
kubectl delete clusterrolebinding wasmcloud-operator
kubectl delete clusterrole wasmcloud-operator
```

## Architecture

This installer utilizes the following Rust libraries:

- **Tokio**: Asynchronous runtime for concurrent operations
- **kube-rs**: Kubernetes API client library
- **Clap**: Command-line argument parsing framework
- **Serde**: YAML/JSON serialization and deserialization

### Module Structure

```
src/
├── main.rs          # CLI orchestration and entry point
├── config.rs        # Kubeconfig parsing and validation
├── kubernetes.rs    # Kubernetes API client wrapper
├── validator.rs     # Pre-installation validation checks
├── install.rs       # Operator deployment logic
├── crds.rs          # CRD installation management
└── error.rs         # Error type definitions
```

## Development

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release
```

### Testing

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture
```

### Code Quality

```bash
# Run linter
cargo clippy --all-targets

# Format code
cargo fmt
```

