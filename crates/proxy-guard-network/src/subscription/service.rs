use std::collections::{HashMap, HashSet};

use chrono::Utc;
use proxy_guard_core::{
    ManagedNode, NodeId, SingBoxOutbound, SubscriptionId, SubscriptionNodeBinding,
    SubscriptionNodeState, SubscriptionPreview, SubscriptionProtocol, SubscriptionProtocolCounts,
    SubscriptionSource, SubscriptionSyncStatus, SubscriptionSyncSummary,
};

use super::{HttpsSubscriptionFetcher, NodeCandidate, SubscriptionFetcher, SubscriptionParser};
use crate::region::RegionHintClassifier;
use crate::storage::{NodeStore, StoredSubscription, SubscriptionStore};
use crate::{ManagedPaths, NetworkError, SecretStore};

pub struct SubscriptionService<S, F> {
    subscriptions: SubscriptionStore,
    nodes: NodeStore,
    secrets: S,
    fetcher: F,
}

impl<S: SecretStore, F: SubscriptionFetcher> SubscriptionService<S, F> {
    /// Open stores and bind explicit credential and fetch implementations.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a managed repository cannot be opened.
    pub fn open(paths: &ManagedPaths, secrets: S, fetcher: F) -> Result<Self, NetworkError> {
        Ok(Self {
            subscriptions: SubscriptionStore::open(paths)?,
            nodes: NodeStore::open(paths)?,
            secrets,
            fetcher,
        })
    }

    /// Validate and store an HTTPS credential before committing URL-free metadata.
    ///
    /// # Errors
    ///
    /// Returns a validation, credential, conflict, or storage error with rollback.
    pub fn add(
        &self,
        name: impl Into<String>,
        url: &str,
    ) -> Result<StoredSubscription, NetworkError> {
        HttpsSubscriptionFetcher::validate_url(url)?;
        let source = SubscriptionSource::new(name).map_err(NetworkError::from)?;
        self.secrets.set_subscription_url(source.id, url)?;
        match self.subscriptions.create(source.clone()) {
            Ok(stored) => Ok(stored),
            Err(error) => {
                let _ = self.secrets.delete_subscription_url(source.id);
                Err(error)
            }
        }
    }

    /// Fetch and parse a subscription URL without persisting anything.
    ///
    /// # Errors
    ///
    /// Returns a redacted validation, fetch, or parse error.
    pub fn inspect(&self, url: &str) -> Result<SubscriptionPreview, NetworkError> {
        HttpsSubscriptionFetcher::validate_url(url)?;
        let bytes = self.fetcher.fetch(SubscriptionId::new(), url)?;
        let parsed = SubscriptionParser::parse(&bytes)?;
        let ignored_region = parsed
            .candidates
            .iter()
            .filter(|candidate| RegionHintClassifier::classify(&candidate.name).is_none())
            .count();
        Ok(SubscriptionPreview {
            fetched: parsed.fetched,
            supported: parsed.candidates.len(),
            unsupported: parsed.unsupported,
            ignored_region,
            failed: parsed.failed,
            protocols: count_protocols(&parsed.candidates),
        })
    }

    /// List URL-free subscription metadata and bindings.
    ///
    /// # Errors
    ///
    /// Returns a storage or validation error.
    pub fn list(&self) -> Result<Vec<StoredSubscription>, NetworkError> {
        self.subscriptions.list()
    }

    /// Resolve one subscription by ID or unique case-insensitive name.
    ///
    /// # Errors
    ///
    /// Returns a not-found error.
    pub fn find(&self, reference: &str) -> Result<StoredSubscription, NetworkError> {
        self.subscriptions.find(reference)
    }

    /// Delete a subscription credential and metadata while retaining imported nodes.
    ///
    /// # Errors
    ///
    /// Returns a credential or storage error; metadata failure restores the credential.
    pub fn delete(&self, reference: &str) -> Result<SubscriptionId, NetworkError> {
        let stored = self.subscriptions.find(reference)?;
        let url = self.secrets.get_subscription_url(stored.source.id)?;
        self.secrets.delete_subscription_url(stored.source.id)?;
        if let Err(error) = self.subscriptions.delete(stored.source.id) {
            let _ = self.secrets.set_subscription_url(stored.source.id, &url);
            return Err(error);
        }
        Ok(stored.source.id)
    }

    /// Fetch, fully validate, and transactionally reconcile one subscription.
    ///
    /// Only JP/SG/US nodes are persisted; everything else is counted as
    /// `ignored_region`. A failed fetch or parse preserves the last known good
    /// node set and records a failed sync status.
    ///
    /// # Errors
    ///
    /// Returns a redacted fetch, parse, validation, credential, or storage error.
    pub fn sync(&self, reference: &str) -> Result<SubscriptionSyncSummary, NetworkError> {
        let stored = self.subscriptions.find(reference)?;
        let result = self.sync_stored(&stored);
        if result.is_err() {
            let mut failed = stored.source.clone();
            failed.last_sync_at = Some(Utc::now());
            failed.last_sync_status = SubscriptionSyncStatus::Failed;
            let _ = self.subscriptions.replace(failed, stored.bindings.clone());
        }
        result
    }

    fn sync_stored(
        &self,
        stored: &StoredSubscription,
    ) -> Result<SubscriptionSyncSummary, NetworkError> {
        let url = self.secrets.get_subscription_url(stored.source.id)?;
        let bytes = self.fetcher.fetch(stored.source.id, &url)?;
        let parsed = SubscriptionParser::parse(&bytes)?;

        let existing_bindings = stored
            .bindings
            .iter()
            .map(|binding| (binding.remote_key.clone(), binding.clone()))
            .collect::<HashMap<_, _>>();
        let current_nodes = self.nodes.list()?;
        let mut used_names = current_nodes
            .iter()
            .map(|node| (node.node.name.to_lowercase(), node.node.id))
            .collect::<HashMap<_, _>>();
        // Stale detection is based on true removal from the subscription, so the
        // full remote-key set is tracked regardless of region filtering.
        let seen_keys = parsed
            .candidates
            .iter()
            .map(|candidate| candidate.remote_key.clone())
            .collect::<HashSet<_>>();

        let mut ignored_region = 0;
        let mut replacements = Vec::new();
        let mut creations = Vec::new();
        let mut bindings = Vec::new();
        let protocols = count_protocols(&parsed.candidates);

        for candidate in parsed.candidates {
            let Some(region) = RegionHintClassifier::classify(&candidate.name) else {
                ignored_region += 1;
                continue;
            };
            let remote_key = candidate.remote_key.clone();
            let outbound = SingBoxOutbound::new(candidate.outbound).map_err(NetworkError::from)?;
            if let Some(binding) = existing_bindings.get(&remote_key) {
                let previous = self.nodes.get(binding.node_id)?;
                let name = unique_name(&candidate.name, binding.node_id, &mut used_names);
                let mut node =
                    ManagedNode::new(name, stored.source.id, region, outbound, remote_key.clone())
                        .map_err(NetworkError::from)?;
                node.id = previous.node.id;
                node.created_at = previous.node.created_at;
                node.updated_at = Utc::now();
                node.validate().map_err(NetworkError::from)?;
                replacements.push((previous, node));
                bindings.push(SubscriptionNodeBinding {
                    subscription_id: stored.source.id,
                    remote_key,
                    node_id: binding.node_id,
                    state: SubscriptionNodeState::Active,
                });
            } else {
                let temporary_id = NodeId::new();
                let name = unique_name(&candidate.name, temporary_id, &mut used_names);
                let node =
                    ManagedNode::new(name, stored.source.id, region, outbound, remote_key.clone())
                        .map_err(NetworkError::from)?;
                let node_id = node.id;
                creations.push(node);
                bindings.push(SubscriptionNodeBinding {
                    subscription_id: stored.source.id,
                    remote_key,
                    node_id,
                    state: SubscriptionNodeState::Active,
                });
            }
        }

        for previous in &stored.bindings {
            if !seen_keys.contains(&previous.remote_key) {
                let mut stale = previous.clone();
                stale.state = SubscriptionNodeState::Stale;
                bindings.push(stale);
            }
        }
        let stale = bindings
            .iter()
            .filter(|binding| binding.state == SubscriptionNodeState::Stale)
            .count();
        let summary = SubscriptionSyncSummary {
            fetched: parsed.fetched,
            imported: creations.len(),
            updated: replacements.len(),
            stale,
            unsupported: parsed.unsupported,
            ignored_region,
            failed: parsed.failed,
            protocols,
        };

        let mut replaced_old = Vec::new();
        let mut created_ids = Vec::new();
        let mutation = (|| {
            for (old, new) in &replacements {
                self.nodes.replace(new.clone())?;
                replaced_old.push(old.node.clone());
            }
            for node in &creations {
                let created = self.nodes.create(node.clone())?;
                created_ids.push(created.node.id);
            }
            let mut source = stored.source.clone();
            source.last_sync_at = Some(Utc::now());
            source.last_sync_status = SubscriptionSyncStatus::Succeeded;
            self.subscriptions.replace(source, bindings)?;
            Ok::<(), NetworkError>(())
        })();
        if let Err(error) = mutation {
            rollback_nodes(&self.nodes, &replaced_old, &created_ids);
            return Err(error);
        }
        Ok(summary)
    }
}

fn count_protocols(candidates: &[NodeCandidate]) -> SubscriptionProtocolCounts {
    let mut counts = SubscriptionProtocolCounts::default();
    for candidate in candidates {
        match candidate.protocol {
            SubscriptionProtocol::Vless => counts.vless += 1,
            SubscriptionProtocol::Trojan => counts.trojan += 1,
            SubscriptionProtocol::Shadowsocks => counts.shadowsocks += 1,
            SubscriptionProtocol::Socks => counts.socks += 1,
        }
    }
    counts
}

fn unique_name(requested: &str, node_id: NodeId, used: &mut HashMap<String, NodeId>) -> String {
    let normalized = requested.to_lowercase();
    if used
        .get(&normalized)
        .is_none_or(|existing| *existing == node_id)
    {
        used.insert(normalized, node_id);
        return requested.to_owned();
    }
    let suffix = node_id.to_string();
    let candidate = format!("{} [{}]", requested, &suffix[..8]);
    used.insert(candidate.to_lowercase(), node_id);
    candidate
}

fn rollback_nodes(nodes: &NodeStore, replaced: &[ManagedNode], created: &[NodeId]) {
    for id in created.iter().rev() {
        let _ = nodes.delete(*id);
    }
    for node in replaced.iter().rev() {
        let _ = nodes.replace(node.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use proxy_guard_core::{CodexRegion, SubscriptionId};
    use tempfile::tempdir;

    use super::SubscriptionService;
    use crate::{ManagedPaths, NetworkError, SecretStore, SubscriptionFetcher};

    #[derive(Clone, Default)]
    struct MemorySecrets(Arc<Mutex<HashMap<SubscriptionId, String>>>);

    impl SecretStore for MemorySecrets {
        fn set_subscription_url(&self, id: SubscriptionId, url: &str) -> Result<(), NetworkError> {
            self.0.lock().expect("lock").insert(id, url.to_owned());
            Ok(())
        }

        fn get_subscription_url(&self, id: SubscriptionId) -> Result<String, NetworkError> {
            self.0
                .lock()
                .expect("lock")
                .get(&id)
                .cloned()
                .ok_or(NetworkError::Credential)
        }

        fn delete_subscription_url(&self, id: SubscriptionId) -> Result<(), NetworkError> {
            self.0.lock().expect("lock").remove(&id);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MemoryFetcher(Arc<Mutex<Vec<u8>>>);

    impl SubscriptionFetcher for MemoryFetcher {
        fn fetch(&self, _: SubscriptionId, _: &str) -> Result<Vec<u8>, NetworkError> {
            Ok(self.0.lock().expect("lock").clone())
        }
    }

    #[test]
    fn sync_imports_only_jp_sg_us_nodes() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let body = Arc::new(Mutex::new(
            b"socks://one.example:1080#JP Tokyo\nsocks://two.example:1080#Singapore 2\nsocks://three.example:1080#US Seattle\nsocks://four.example:1080#Hong Kong\nsocks://five.example:1080#Germany".to_vec(),
        ));
        let service =
            SubscriptionService::open(&paths, MemorySecrets::default(), MemoryFetcher(body))
                .expect("service");
        service
            .add("Airport", "https://example.com/sub?token=secret")
            .expect("add");
        let summary = service.sync("Airport").expect("sync");

        assert_eq!(summary.imported, 3);
        assert_eq!(summary.ignored_region, 2);
        let nodes = crate::NodeStore::open(&paths)
            .expect("nodes")
            .list()
            .expect("list");
        let regions = nodes
            .iter()
            .map(|node| node.node.region_hint)
            .collect::<Vec<_>>();
        assert!(regions.contains(&CodexRegion::JP));
        assert!(regions.contains(&CodexRegion::SG));
        assert!(regions.contains(&CodexRegion::US));
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn sync_preserves_ids_and_marks_removed_nodes_stale() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let body = Arc::new(Mutex::new(
            b"socks://one.example:1080#JP One\nsocks://two.example:1080#JP Two".to_vec(),
        ));
        let service = SubscriptionService::open(
            &paths,
            MemorySecrets::default(),
            MemoryFetcher(body.clone()),
        )
        .expect("service");
        service
            .add("Airport", "https://example.com/sub?token=secret")
            .expect("add");
        let first = service.sync("Airport").expect("sync");
        assert_eq!(first.imported, 2);
        let before = service.list().expect("list")[0].bindings.clone();

        *body.lock().expect("lock") = b"socks://one.example:1080#JP One-Renamed".to_vec();
        let second = service.sync("Airport").expect("sync");
        assert_eq!(second.updated, 1);
        assert_eq!(second.stale, 1);
        let after = service.list().expect("list")[0].bindings.clone();
        let active = after
            .iter()
            .find(|binding| binding.state == proxy_guard_core::SubscriptionNodeState::Active)
            .expect("active");
        assert_eq!(active.node_id, before[0].node_id);
    }

    #[test]
    fn malformed_sync_preserves_last_known_good_nodes() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let body = Arc::new(Mutex::new(b"socks://one.example:1080#JP One".to_vec()));
        let service = SubscriptionService::open(
            &paths,
            MemorySecrets::default(),
            MemoryFetcher(body.clone()),
        )
        .expect("service");
        service
            .add("Airport", "https://example.com/sub?token=secret")
            .expect("add");
        service.sync("Airport").expect("initial sync");
        let before = crate::NodeStore::open(&paths)
            .expect("nodes")
            .list()
            .expect("list");

        *body.lock().expect("lock") = b"not a subscription".to_vec();
        service.sync("Airport").expect_err("malformed");
        let after = crate::NodeStore::open(&paths)
            .expect("nodes")
            .list()
            .expect("list");
        assert_eq!(before, after);
    }

    #[test]
    fn inspect_reports_protocols_without_persisting() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let body = Arc::new(Mutex::new(
            b"socks://one.example:1080#JP One\nvless://11111111-1111-1111-1111-111111111111@example.com:443?security=reality&pbk=public#US Two".to_vec(),
        ));
        let service =
            SubscriptionService::open(&paths, MemorySecrets::default(), MemoryFetcher(body))
                .expect("service");

        let preview = service
            .inspect("https://example.com/sub?token=secret")
            .expect("inspect");
        assert_eq!(preview.fetched, 2);
        assert_eq!(preview.supported, 2);
        assert_eq!(preview.protocols.socks, 1);
        assert_eq!(preview.protocols.vless, 1);

        assert!(service.list().expect("list").is_empty());
        assert!(
            crate::NodeStore::open(&paths)
                .expect("nodes")
                .list()
                .expect("node list")
                .is_empty()
        );
    }

    #[test]
    fn inspect_rejects_non_https_without_echoing_the_url() {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let service = SubscriptionService::open(
            &paths,
            MemorySecrets::default(),
            MemoryFetcher(Arc::new(Mutex::new(Vec::new()))),
        )
        .expect("service");

        let secret = "http://example.com/sub?token=never-log-this";
        let error = service.inspect(secret).expect_err("http must fail");
        assert!(!error.to_string().contains(secret));
    }
}
