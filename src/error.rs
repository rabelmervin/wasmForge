use thiserror::Error;

#[derive(Error, Debug)]
pub enum InstallerError {
    #[error("Kubeconfig error: {0}")]
    KubeconfigError(String),

    #[error("Kubernetes connection error: {0}")]
    KubeConnectionError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Installation error: {0}")]
    InstallationError(String),

    #[error("Timeout waiting for deployment: {0}")]
    TimeoutError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_yaml::Error),

    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),
}

pub type InstallerResult<T> = Result<T, InstallerError>;
