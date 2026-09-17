//! The git credential mock, compiled only with the `integration-tests`
//! feature.
//!
//! Every value here is obviously fake (rule 3); nothing it hands out would
//! authenticate anywhere.

use std::any::Any;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use uuid::Uuid;

use super::{CommitIdentity, GitCredential, GitCredentialProvider};
use crate::prelude::*;

/// The basic-auth user a GitHub PAT uses.
const TEST_USERNAME: &str = "x-access-token";

/// An obviously fake token. It is not a credential and never was.
const TEST_TOKEN: &str = "ghp_FAKE_TEST_TOKEN_0000000000";

/// The committer identity tests see.
const TEST_BOT_NAME: &str = "Mars Test Bot";

/// The committer address tests see; `.test` is reserved and never resolves.
const TEST_BOT_EMAIL: &str = "bot@example.test";

/// A provider that hands out a fixed fake credential and records who asked.
#[derive(Debug)]
pub struct MockGitCredentialProvider {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    credential: Option<GitCredential>,
    identity: CommitIdentity,
    requested: Vec<Uuid>,
}

impl Default for MockGitCredentialProvider {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                credential: Some(GitCredential {
                    username: TEST_USERNAME.to_string(),
                    token: TEST_TOKEN.to_string(),
                }),
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
    /// A fresh mock with the default fake credential and identity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand out this credential instead, or `None` to model a project with no
    /// stored `GIT_CREDENTIAL`.
    pub fn set_credential(&self, credential: Option<GitCredential>) {
        self.lock().credential = credential;
    }

    /// Use this committer identity instead.
    pub fn set_identity(&self, identity: CommitIdentity) {
        self.lock().identity = identity;
    }

    /// The projects a credential was asked for, in order, cloned out of the
    /// mutex so a test never holds the lock across an await.
    pub fn requested(&self) -> Vec<Uuid> {
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
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn credential(&self, project_id: Uuid) -> Result<Option<GitCredential>> {
        let mut state = self.lock();
        state.requested.push(project_id);
        Ok(state.credential.clone())
    }

    fn commit_identity(&self) -> CommitIdentity {
        self.lock().identity.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_default_credential_is_the_fake_pat_and_requests_are_recorded() {
        let provider = MockGitCredentialProvider::new();
        let project = Uuid::new_v4();

        let credential = provider
            .credential(project)
            .await
            .expect("the mock never fails")
            .expect("the default mock has a credential");

        assert_eq!(credential.username, TEST_USERNAME);
        assert_eq!(credential.token, TEST_TOKEN);
        assert_eq!(provider.requested(), vec![project]);
        assert_eq!(
            provider.commit_identity(),
            CommitIdentity {
                name: TEST_BOT_NAME.to_string(),
                email: TEST_BOT_EMAIL.to_string(),
            }
        );
    }

    #[tokio::test]
    async fn the_credential_and_identity_are_configurable() {
        let provider = MockGitCredentialProvider::new();
        provider.set_credential(None);
        provider.set_identity(CommitIdentity {
            name: "Other Bot".to_string(),
            email: "other@example.test".to_string(),
        });

        assert!(
            provider
                .credential(Uuid::new_v4())
                .await
                .expect("the mock never fails")
                .is_none()
        );
        assert_eq!(provider.commit_identity().name, "Other Bot");
    }
}
