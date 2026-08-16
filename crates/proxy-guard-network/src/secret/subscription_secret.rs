use proxy_guard_core::SubscriptionId;

use crate::NetworkError;

#[cfg(windows)]
const SERVICE: &str = "CodexProxyGuard.Subscription";

/// Secret boundary for subscription credentials.
pub trait SecretStore: Send + Sync {
    /// Store a subscription URL in the platform credential store.
    ///
    /// # Errors
    ///
    /// Returns a redacted credential operation error.
    fn set_subscription_url(&self, id: SubscriptionId, url: &str) -> Result<(), NetworkError>;
    /// Retrieve a subscription URL for internal fetch use only.
    ///
    /// # Errors
    ///
    /// Returns a redacted credential operation error.
    fn get_subscription_url(&self, id: SubscriptionId) -> Result<String, NetworkError>;
    /// Delete one subscription credential.
    ///
    /// # Errors
    ///
    /// Returns a redacted credential operation error.
    fn delete_subscription_url(&self, id: SubscriptionId) -> Result<(), NetworkError>;
}

/// Windows Credential Manager-backed secret store under a fixed namespace.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    #[cfg(windows)]
    fn entry(id: SubscriptionId) -> Result<keyring_core::Entry, NetworkError> {
        use std::sync::OnceLock;

        static INITIALIZED: OnceLock<Result<(), ()>> = OnceLock::new();
        INITIALIZED
            .get_or_init(|| {
                windows_native_keyring_store::Store::new()
                    .map(|store| {
                        let store: std::sync::Arc<keyring_core::CredentialStore> = store;
                        keyring_core::set_default_store(store);
                    })
                    .map_err(|_| ())
            })
            .map_err(|()| NetworkError::Credential)?;
        keyring_core::Entry::new(SERVICE, &id.to_string()).map_err(|_| NetworkError::Credential)
    }
}

#[cfg(windows)]
impl SecretStore for KeyringSecretStore {
    fn set_subscription_url(&self, id: SubscriptionId, url: &str) -> Result<(), NetworkError> {
        Self::entry(id)?
            .set_password(url)
            .map_err(|_| NetworkError::Credential)
    }

    fn get_subscription_url(&self, id: SubscriptionId) -> Result<String, NetworkError> {
        Self::entry(id)?
            .get_password()
            .map_err(|_| NetworkError::Credential)
    }

    fn delete_subscription_url(&self, id: SubscriptionId) -> Result<(), NetworkError> {
        Self::entry(id)?
            .delete_credential()
            .map_err(|_| NetworkError::Credential)
    }
}

#[cfg(not(windows))]
impl SecretStore for KeyringSecretStore {
    fn set_subscription_url(&self, _: SubscriptionId, _: &str) -> Result<(), NetworkError> {
        Err(NetworkError::Credential)
    }

    fn get_subscription_url(&self, _: SubscriptionId) -> Result<String, NetworkError> {
        Err(NetworkError::Credential)
    }

    fn delete_subscription_url(&self, _: SubscriptionId) -> Result<(), NetworkError> {
        Err(NetworkError::Credential)
    }
}
