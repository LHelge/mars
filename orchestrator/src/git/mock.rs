//! The git credential mock, compiled only with the `integration-tests`
//! feature.
//!
//! Every value here is obviously fake (rule 3); nothing it hands out would
//! authenticate anywhere.
//!
//! The default is *no* credential, which is the shape of a project whose
//! remote is public: a test that wants an authenticated command says so with
//! [`MockGitCredentialProvider::set_credential`] or
//! [`MockGitCredentialProvider::set_credential_for_all`], and a test that only
//! wants to know who asked reads
//! [`MockGitCredentialProvider::requested`].

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{CommitIdentity, GitActor, GitCredential, GitCredentialProvider};
use crate::prelude::*;

/// An obviously fake header value. It is not a credential and never was.
const TEST_HEADER_VALUE: &str = "Authorization: Basic FAKE-NOT-A-CREDENTIAL-0000";

/// The committer identity tests see.
const TEST_BOT_NAME: &str = "Mars Bot";

/// The committer address tests see; `.invalid` is reserved and never resolves.
const TEST_BOT_EMAIL: &str = "bot@example.invalid";

/// A provider that hands out whatever a test configured and records who asked.
#[derive(Debug)]
pub struct MockGitCredentialProvider {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// What a project with no entry of its own gets. `None` — no credential —
    /// is the default, matching a public remote.
    fallback: Option<GitCredential>,
    /// Per-project answers, which win over [`State::fallback`].
    per_project: HashMap<Uuid, Option<GitCredential>>,
    identity: CommitIdentity,
    requested: Vec<(Uuid, GitActor)>,
}

impl Default for MockGitCredentialProvider {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                fallback: None,
                per_project: HashMap::new(),
                identity: CommitIdentity {
                    name: TEST_BOT_NAME.to_string(),
                    email: TEST_BOT_EMAIL.to_string(),
                },
                requested: Vec::new(),
            }),
        }
    }
}

impl MockGitCredentialProvider {
    /// A fresh mock: no credential for any project, and the fixed identity.
    pub fn new() -> Self {
        Self::default()
    }

    /// The obviously fake credential a test hands to `set_credential`.
    pub fn fake_credential() -> GitCredential {
        GitCredential::from_header_value(Zeroizing::new(TEST_HEADER_VALUE.to_string()))
    }

    /// Answer this project with `credential`, whatever the fallback is.
    pub fn set_credential(&self, project_id: Uuid, credential: Option<GitCredential>) {
        self.lock().per_project.insert(project_id, credential);
    }

    /// Answer every project without an entry of its own with `credential`.
    pub fn set_credential_for_all(&self, credential: Option<GitCredential>) {
        self.lock().fallback = credential;
    }

    /// Use this committer identity instead of the fixed one.
    pub fn set_identity(&self, identity: CommitIdentity) {
        self.lock().identity = identity;
    }

    /// Every `credential_for` call, in order, cloned out of the mutex so a
    /// test never holds the lock across an await.
    pub fn requested(&self) -> Vec<(Uuid, GitActor)> {
        self.lock().requested.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Test-only code: a poisoned lock means another test thread already
        // panicked, which is a failure in its own right.
        self.state.lock().expect("the mock git lock is healthy")
    }
}

#[async_trait]
impl GitCredentialProvider for MockGitCredentialProvider {
    async fn credential_for(
        &self,
        project_id: Uuid,
        actor: &GitActor,
        _min_ttl: Duration,
    ) -> Result<Option<GitCredential>> {
        let mut state = self.lock();
        state.requested.push((project_id, *actor));

        Ok(match state.per_project.get(&project_id) {
            Some(configured) => configured.clone(),
            None => state.fallback.clone(),
        })
    }

    async fn commit_identity(&self, _project_id: Uuid) -> Result<CommitIdentity> {
        Ok(self.lock().identity.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `min_ttl` every test passes; the mock ignores it, as the PAT
    /// provider does.
    const ANY_TTL: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn a_fresh_mock_has_no_credential_and_the_fixed_identity() {
        let provider = MockGitCredentialProvider::new();
        let project = Uuid::new_v4();
        let actor = GitActor::System;

        assert!(
            provider
                .credential_for(project, &actor, ANY_TTL)
                .await
                .expect("the mock never fails")
                .is_none(),
            "the default is a project with no stored GIT_CREDENTIAL"
        );
        assert_eq!(provider.requested(), vec![(project, actor)]);
        assert_eq!(
            provider
                .commit_identity(project)
                .await
                .expect("the mock never fails"),
            CommitIdentity {
                name: TEST_BOT_NAME.to_string(),
                email: TEST_BOT_EMAIL.to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_per_project_credential_wins_over_the_fallback() {
        let provider = MockGitCredentialProvider::new();
        let with = Uuid::new_v4();
        let without = Uuid::new_v4();

        provider.set_credential_for_all(Some(MockGitCredentialProvider::fake_credential()));
        provider.set_credential(without, None);

        let user = GitActor::User(Uuid::new_v4());
        let found = provider
            .credential_for(with, &user, ANY_TTL)
            .await
            .expect("the mock never fails")
            .expect("the fallback applies");
        assert_eq!(found.header_value(), TEST_HEADER_VALUE);

        assert!(
            provider
                .credential_for(without, &user, ANY_TTL)
                .await
                .expect("the mock never fails")
                .is_none(),
            "an explicit None wins over the fallback"
        );

        assert_eq!(provider.requested(), vec![(with, user), (without, user)]);
    }

    #[tokio::test]
    async fn the_identity_is_configurable() {
        let provider = MockGitCredentialProvider::new();
        provider.set_identity(CommitIdentity {
            name: "Other Bot".to_string(),
            email: "other@example.invalid".to_string(),
        });

        assert_eq!(
            provider
                .commit_identity(Uuid::new_v4())
                .await
                .expect("the mock never fails")
                .name,
            "Other Bot"
        );
    }

    #[tokio::test]
    async fn the_mock_is_reachable_through_the_trait_object() {
        let provider: Arc<dyn GitCredentialProvider> = Arc::new(MockGitCredentialProvider::new());
        let project = Uuid::new_v4();

        provider
            .credential_for(project, &GitActor::Session(Uuid::new_v4()), ANY_TTL)
            .await
            .expect("the mock never fails");

        let mock = provider
            .as_any()
            .downcast_ref::<MockGitCredentialProvider>()
            .expect("the trait object is the mock");
        assert_eq!(mock.requested().len(), 1);
    }
}
