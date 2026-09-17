//! Domain types and their validation (`User`, `Project`, `Session`, `Task`,
//! `Secret`, ...). Models never contain SQL; each carries its own error enum.

pub mod task;
pub mod task_comment;
pub mod task_dependency;
pub mod task_event;
pub mod task_handoff;
pub mod task_session;
pub mod task_state;
pub mod user;

pub use task::{Label, MAX_TITLE_CHARS, NewTask, Priority, Task, TaskError, TaskResult, TaskTitle};
pub use task_comment::{NewTaskComment, TaskComment};
pub use task_dependency::{TaskDependency, TaskDependencyKind};
pub use task_event::{NewTaskEvent, TaskEventRow, kind as task_event_kind};
pub use task_handoff::{NewTaskHandoff, ReviewStatus, TaskHandoff, is_commit_id};
pub use task_session::TaskSession;
pub use task_state::{
    DEFAULT_TASK_STATES, MAX_STATE_NAME_CHARS, NewTaskState, TaskState, TaskStateKind,
    TaskStateName, is_state_name,
};
pub use user::{
    Email, MAX_PASSWORD_CHARS, MAX_USERNAME_CHARS, MIN_PASSWORD_CHARS, MIN_USERNAME_CHARS, NewUser,
    Password, PasswordResetToken, RefreshToken, User, UserError, UserInvite, UserResult,
    UserUpdate, Username,
};
