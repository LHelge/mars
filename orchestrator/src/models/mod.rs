//! Domain types and their validation (`User`, `Project`, `Session`, `Task`,
//! `Secret`, ...). Models never contain SQL; each carries its own error enum.

pub mod agent_profile;
pub mod event;
pub mod git;
pub mod project;
pub mod secret;
pub mod session;
pub mod shared_dir;
pub mod task;
pub mod task_comment;
pub mod task_dependency;
pub mod task_event;
pub mod task_handoff;
pub mod task_session;
pub mod task_state;
pub mod token;
pub mod user;

pub use agent_profile::{
    AgentBackend, AgentProfile, DEFAULT_IDLE_TIMEOUT_SECS, MAX_SECRET_NAME_CHARS, NewAgentProfile,
    PERMISSION_MODE_BYPASS, ProfileError, ProfileKind, ProfileResult, ProfileUpdate,
    is_secret_name,
};
pub use event::{EventRow, INTERNAL_FIELD_PREFIX, NewEvent, OFFSET_FIELD};
pub use git::{
    Branch, BranchKind, Diff, DiffFile, DiffStatus, GitMergeDetail, GitPushDetail, GitRebaseDetail,
    GitSyncDetail, SessionBranch, SyncOutcome,
};
pub use project::{
    BranchName, DEFAULT_MAX_ATTEMPTS, MAX_MAX_ATTEMPTS, MAX_PROJECT_NAME_CHARS, MIN_MAX_ATTEMPTS,
    MaxAttempts, NewProject, Project, ProjectError, ProjectName, ProjectResult, ProjectStatus,
    ProjectUpdate, RemoteUrl, is_branch_name,
};
pub use secret::{
    EncryptedValue, KeyVersionSample, MAX_SECRET_VALUE_BYTES, NewSecret, ScopeRef, Secret,
    SecretError, SecretMeta, SecretName, SecretResult, SecretScope, SecretUse, SecretUsePurpose,
    validate_secret_value,
};
pub use session::{
    NewSession, Session, SessionError, SessionResult, SessionState, SessionTitle, StateChange,
    session_branch, state_change_payload,
};
pub use shared_dir::{
    ContainerPath, MAX_SHARED_DIR_NAME_CHARS, NewSharedDir, RESERVED_PATHS, SharedDir,
    SharedDirError, SharedDirName, SharedDirResult, is_shared_dir_name,
};
pub use task::{
    Label, MAX_TITLE_CHARS, NewTask, Priority, Task, TaskError, TaskRef, TaskResult, TaskTitle,
    TaskUpdate,
};
pub use task_comment::{NewTaskComment, TaskComment};
pub use task_dependency::{TaskDependency, TaskDependencyKind};
pub use task_event::{NewTaskEvent, TaskEventRow, kind as task_event_kind};
pub use task_handoff::{NewTaskHandoff, ReviewStatus, TaskHandoff, is_commit_id};
pub use task_session::TaskSession;
pub use task_state::{
    DEFAULT_TASK_STATES, MAX_STATE_NAME_CHARS, NewTaskState, TaskState, TaskStateKind,
    TaskStateName, is_state_name,
};
pub use token::{OpaqueToken, TOKEN_HASH_CHARS};
pub use user::{
    Email, MAX_PASSWORD_CHARS, MAX_USERNAME_CHARS, MIN_PASSWORD_CHARS, MIN_USERNAME_CHARS, NewUser,
    Password, PasswordResetToken, RefreshToken, User, UserError, UserInvite, UserResult,
    UserUpdate, Username,
};
