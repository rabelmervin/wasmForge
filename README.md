# wasmCloud Operator Installer

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
./target/release/wasmcloud-installer --help
```

## Usage

### Installation Examples

```bash
# Basic installation with default settings
./target/release/wasmcloud-installer

# Specify custom kubeconfig
./target/release/wasmcloud-installer --kubeconfig ~/.kube/prod-cluster

# Define custom namespace
./target/release/wasmcloud-installer --namespace wasmcloud-system

# Preview installation without applying changes
./target/release/wasmcloud-installer --dry-run

# Enable verbose logging for troubleshooting
./target/release/wasmcloud-installer --verbose

# Complete example with all options
./target/release/wasmcloud-installer \
  --kubeconfig ~/.kube/my-cluster \
  --namespace wasmcloud-prod \
  --timeout 600 \
  --verbose
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
| `--help` | `-h` | Show help information | |
| `--version` | `-V` | Show version information | |

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

