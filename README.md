# forge

A Rust-based command-line tool for automated deployment of the wasmCloud v1 operator ([wasmcloud-operator](https://github.com/wasmCloud/wasmcloud-operator)) on Kubernetes clusters.

> **Note:** forge targets wasmCloud **v1**. The upstream `wasmcloud-operator` is archived and wasmCloud 1.x is deprecated. wasmCloud v2 is installed with the official [`runtime-operator` Helm chart](https://wasmcloud.com/docs/kubernetes-operator/).

## Overview

This tool automates the installation of the wasmCloud operator by performing the following operations:
- Creates the target namespace and configures RBAC (Role-Based Access Control) permissions
- Applies every resource with server-side apply, so re-running is safe
- Deploys the operator and waits until it has rolled out, registered its CRD, and served its API
- Reports container state, recent events, and log lines when the operator fails to start
- Performs pre-installation validation to ensure cluster compatibility
- Supports any standard Kubernetes cluster with appropriate kubeconfig authentication

## Quick Start

### Prerequisites

- Kubernetes cluster version 1.20 or later
- kubeconfig with permissions to create namespaces and configure cluster-level RBAC

### Installation

#### Build from Source

```bash
git clone https://github.com/rabelmervin/forge
cd forge
cargo build --release
forge --help
```

### Create a Kubernetes Cluster

```bash
# Create a kind cluster for wasmCloud
kind create cluster --name wasmcloud-cluster

# With kube config file in action
kind create cluster --name demo --kubeconfig kubeconfig 

# Verify cluster is running
kubectl cluster-info --context wasmcloud-cluster
```

## Usage

### Installation Examples

```bash
# Basic installation with default settings
forge

# Specify custom kubeconfig
forge --kubeconfig ~/.kube/config

# Define custom namespace
forge --namespace wasmcloud-system

# Preview installation without applying changes
forge --dry-run

# Enable verbose logging for troubleshooting
forge --verbose

# Complete example with all options
forge --kubeconfig ~/.kube/config --namespace wasmcloud-demo
```

### Install Everything (NATS, wadm and the operator)

```bash
# One command: NATS (JetStream + leafnodes), wadm, then the operator, then a doctor run
forge --kubeconfig ~/.kube/config --with-deps

# Preview what would be installed or skipped
forge --kubeconfig ~/.kube/config --with-deps --dry-run
```

`--with-deps` needs helm 3.13 or newer on your PATH. forge passes helm the same kubeconfig and context it
uses itself, so the charts always land in the same cluster as the operator. NATS and wadm go into the
`default` namespace, which is where the operator's default NATS address points.

If NATS or wadm is already running and forge didn't install it, forge leaves it as is. Releases forge
installs are labelled `managed-by=forge`. Re-running `--with-deps` upgrades them, and
`--uninstall --with-deps` removes only those.

### Uninstall Examples

```bash
# Remove wasmCloud operator from default namespace
forge --uninstall

# Remove from custom namespace
forge --uninstall --namespace wasmcloud-system

# Uninstall with custom kubeconfig
forge --uninstall --kubeconfig ~/.kube/prod-config

# Also remove the NATS and wadm releases forge installed
forge --uninstall --with-deps
```

### Upgrade Examples

```bash
# Upgrade existing installation
forge --upgrade

# Upgrade installation in custom namespace
forge --upgrade --namespace wasmcloud-system

# Upgrade with different kubeconfig (for cluster migration)
forge --upgrade --kubeconfig ~/.kube/new-cluster
```

### Status Check Examples

```bash
# Check status in default namespace
forge --status

# Check status in custom namespace
forge --status --namespace wasmcloud-system

# Check status with custom kubeconfig
forge --status --kubeconfig ~/.kube/prod-config

# Machine-readable status for CI
forge --status -o json | jq -e '.overall_status == "installed_and_ready"'
```

### Doctor Examples

```bash
# Diagnose an installation (exits 1 if any check fails)
forge --doctor

# JSON report for CI or bug reports
forge --doctor -o json
```

### Render Manifests (GitOps)

```bash
# Print the exact manifests forge applies. No cluster or kubeconfig needed
forge --dry-run -o yaml > wasmcloud-operator.yaml

# Inspect, commit to a GitOps repo, or apply directly
forge --dry-run -o yaml --namespace wasmcloud-system | kubectl apply --server-side -f -
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
| `--doctor` | | Diagnose an installation and suggest fixes | `false` |
| `--output` | `-o` | `text`, `json` (with `--status`/`--doctor`) or `yaml` (with `--dry-run`) | `text` |
| `--help` | `-h` | Show help information | |
| `--version` | `-V` | Show version information | |

**Note**: The `--uninstall`, `--upgrade`, `--status`, and `--doctor` flags cannot be used together with `--dry-run` or each other. With `-o json` or `-o yaml`, stdout contains only the document and logs go to stderr.

## Installation Components

The installer deploys the following Kubernetes resources:

### Namespace
The specified namespace (default: `wasmcloud-operator`) is created if not present.

### RBAC Configuration
Mirrors the upstream manifests in wasmcloud-operator v0.5.0:
- ServiceAccount for operator identity
- ClusterRole defining required permissions
- ClusterRoleBinding linking the ServiceAccount to the ClusterRole
- ClusterRoleBinding `wasmcloud-operator-delegator` to `system:auth-delegator`, used by the operator's aggregated API server

### Operator Deployment
- Kubernetes Deployment running the wasmCloud operator
- Service `wasmcloud-operator` on port 8443, which the operator's APIService points at

### Registered by the operator
forge does not write CRD schemas. On startup the operator registers:
- **CRD** `wasmcloudhostconfigs.k8s.wasmcloud.dev` (`WasmCloudHostConfig`, k8s.wasmcloud.dev/v1alpha1)
- **APIService** `v1beta1.core.oam.dev`, which serves wadm applications through the Kubernetes API

The installer waits for the CRD to be Established and the APIService to be Available.

### Operator Deployment Configuration
- **Container Image**: `ghcr.io/wasmcloud/wasmcloud-operator:0.5.0` (last release under this name; later releases are `wadm-operator`)
- **Listening Port**: 8443 (HTTPS)

### Runtime prerequisites
The operator installs cleanly on its own, but to run wasmCloud hosts it needs a NATS cluster with JetStream and wadm. See the [upstream quickstart](https://github.com/wasmCloud/wasmcloud-operator/tree/main/examples/quickstart).

## Operations

### Installation (Default)

Standard installation creates all resources and deploys the operator:

```bash
forge --namespace wasmcloud-operator
```

### Uninstallation

The `--uninstall` flag removes all wasmCloud operator resources from the specified cluster:

```bash
# Remove all wasmCloud operator resources
forge --uninstall --namespace wasmcloud-operator

# Remove from a different namespace
forge --uninstall --namespace my-wasmcloud
```

**What gets removed during uninstall:**
- Operator deployment and pods (waits until they have stopped)
- APIService `v1beta1.core.oam.dev`, only if it points at this installation's Service
- Service exposing operator endpoints
- ClusterRole and both ClusterRoleBindings (RBAC resources)
- ServiceAccount
- The `WasmCloudHostConfig` CRD, plus CRDs created by older forge versions
- Namespace (only if it was created by the installer and is empty)

**Important Notes:**
- Uninstall will remove the CRD, which will also delete any existing `WasmCloudHostConfig` resources
- The namespace is only deleted if it was created by the installer and contains no other resources
- Uninstall is a destructive operation and cannot be undone

### Upgrade

The `--upgrade` flag updates an existing wasmCloud operator installation:

```bash
# Upgrade to latest operator version
forge --upgrade --namespace wasmcloud-operator

# Upgrade with custom kubeconfig
forge --upgrade -k /path/to/kubeconfig
```

Upgrade uses the same server-side apply path as installation, so every resource is brought to the desired state:
- ClusterRole and ClusterRoleBindings are updated (including the subject namespace)
- Deployment is updated with the pinned container image and configuration, triggering a rolling update
- Service configuration is updated if needed
- The operator re-registers its CRD on startup, so its schema always matches the operator version

**Upgrade Process:**
1. Applies RBAC, Service, and Deployment
2. Waits for the rollout to complete (failing fast on crash loops or image pull errors)
3. Waits for the operator's CRD and APIService to be ready

**Use Cases:**
- Switching to a new kubeconfig file/cluster context
- Updating to a newer version of the operator
- Applying configuration changes
- Fixing resource permission issues

### Status Check

The `--status` flag checks the current installation status of the wasmCloud operator:

```bash
# Check status of installation
forge --status --namespace wasmcloud-operator

# Check with custom kubeconfig
forge --status -k /path/to/kubeconfig
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

### Doctor

`--doctor` inspects a live installation without changing anything, and reports each check as pass, warning or failure with a suggested fix:

| Area | What it checks |
|------|----------------|
| Operator | Deployment exists, image matches the pinned version, rollout complete |
| Pods | Crash loops, image pull errors, restarts with last exit reason, plus log tails |
| Service | Port 8443 exposed and backed by ready endpoints |
| RBAC | Live ClusterRole compared rule by rule against what the operator needs; bindings target the right namespace |
| CRDs | `WasmCloudHostConfig` Established with v1alpha1 served and stored; leftovers from older forge versions; wasmCloud v2 CRDs on the same cluster |
| Aggregated API | `v1beta1.core.oam.dev` points at this install and is Available, with the failure reason if not |
| NATS | For every `WasmCloudHostConfig` (or the default address if none exist): Service exists, client and leafnode ports exposed, ready endpoints |
| wadm | A wadm Deployment is running |

NATS is checked through Kubernetes objects, not a live connection, so JetStream availability is not verified. Doctor exits with code 1 if any check fails.

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
forge --namespace wasmcloud-alternate
```

### Installation Timeout

For slow clusters, increase the timeout duration (default: 300 seconds):

```bash
forge --timeout 600
```

### Verbose Logging

Enable debug output for detailed troubleshooting:

```bash
forge --verbose
```

### Manual Resource Cleanup

If installation fails, remove resources manually:

```bash
kubectl delete apiservice v1beta1.core.oam.dev
kubectl delete namespace wasmcloud-operator
kubectl delete clusterrolebinding wasmcloud-operator wasmcloud-operator-delegator
kubectl delete clusterrole wasmcloud-operator
kubectl delete crd wasmcloudhostconfigs.k8s.wasmcloud.dev
```

## Architecture

This installer utilizes the following Rust libraries:

- **Tokio**: Asynchronous runtime for concurrent operations
- **kube-rs**: Kubernetes API client library
- **Clap**: Command-line argument parsing framework
- **Serde**: YAML/JSON serialization and deserialization

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

