//! Filing a plan: a parent, its sub-tasks and the `blocks` edges between them,
//! as one tracker mutation (`SPEC.md`, "MCP tool contracts" → `create_plan`;
//! ADR 0056).
//!
//! A planner that files a graph one `create_task` at a time commits it one
//! task at a time, and every commit wakes the dispatcher ("Dispatcher"). A
//! dependant filed before its prerequisite, or before the edge to it, is a
//! ready, unblocked task for as long as the rest of the graph is missing, and
//! the dispatcher launches exactly such tasks. A plan closes that window by
//! construction: every task, edge, `blocked` flag and event of the batch is
//! written under one project lock and committed together, so the first read
//! any other transaction can make — the dispatcher's candidate read included —
//! already sees the finished graph.
//!
//! Nothing about a single task is decided here. Each task is
//! [`create_task_with`]'s, the one creation path REST and `create_task` share,
//! so the state, number, parent, edge, `blocked` and event rules are the same
//! ones; what this module adds is only what a *batch* has:
//!
//! - **local refs.** A sub-task carries a caller-chosen `ref`, and another
//!   sub-task names it in `depends_on`. A ref is 1-64 letters, digits, `.`,
//!   `_` or `-`, and is never something that would also read as a task
//!   reference (all digits, or a UUID), so an entry of `depends_on` means one
//!   thing whichever way it is read. A duplicated ref, or an entry naming a
//!   ref no sub-task has, refuses the whole plan.
//! - **order.** The sub-tasks are created — and numbered — in input order,
//!   except that each is created after every sub-task it depends on; a batch
//!   whose local edges close a cycle cannot be ordered and is refused before
//!   the lock. Edges to existing tasks cannot close a cycle at all: nothing
//!   that exists can already depend on a task that does not.
//! - **one provenance.** [`resolve_origin`] is asked once, for the plan. A new
//!   parent records it and its sub-tasks reach it through the parent link; with
//!   an existing parent or none, each sub-task records it, unless the origin is
//!   that existing parent (ADR 0023).
//! - **a bound.** At most [`MAX_PLAN_TASKS`] sub-tasks, so one call holds the
//!   project lock for a bounded time.

use std::collections::{BTreeMap, HashMap};

use uuid::Uuid;

use crate::models::TaskRef;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::PARENT_NOT_TOP_LEVEL;
use crate::tracker::graph::CYCLE;
use crate::tracker::provenance::resolve_origin;
use crate::tracker::tasks::{CreateTaskInput, CreatedBy, Provenance, create_task_with};
use crate::tracker::{TaskDto, TrackerMutation};

/// The most sub-tasks one plan may carry (`SPEC.md`, `create_plan`).
pub const MAX_PLAN_TASKS: usize = 50;

/// The longest a local ref may be.
pub const MAX_REF_LEN: usize = 64;

/// What a plan with no sub-tasks, or too many, is told.
pub const PLAN_SIZE: &str = "tasks must list 1 to 50 sub-tasks";

/// A whole plan, un-validated.
#[derive(Debug, Clone)]
pub struct PlanInput {
    /// The parent every sub-task is created under, if any.
    pub parent: Option<PlanParent>,
    /// The sub-tasks, in the caller's order.
    pub tasks: Vec<PlanTask>,
    /// The held task the plan was discovered from; inferred when omitted.
    pub discovered_from: Option<TaskRef>,
    /// The session filing the plan: the creator of every task in it.
    pub session_id: Uuid,
}

/// The plan's parent: one it creates, or one that is already there.
#[derive(Debug, Clone)]
pub enum PlanParent {
    /// An existing top-level task of the project, such as the task a planner
    /// was launched for.
    Existing(TaskRef),
    /// A new task, created first.
    New(PlanTaskFields),
}

/// The fields a task of the plan is created with, `create_task`'s own.
#[derive(Debug, Clone, Default)]
pub struct PlanTaskFields {
    pub title: String,
    pub description: Option<String>,
    pub state: Option<String>,
    pub priority: Option<i16>,
    pub labels: Vec<String>,
    /// `blocks` prerequisites among existing tasks. A sub-task's local
    /// prerequisites are [`PlanTask::depends_on`].
    pub depends_on: Vec<TaskRef>,
}

/// One sub-task.
#[derive(Debug, Clone)]
pub struct PlanTask {
    /// The caller's name for it within this plan.
    pub local_ref: String,
    /// Its fields; `fields.depends_on` must be empty, the prerequisites are
    /// `depends_on` below.
    pub fields: PlanTaskFields,
    /// Its prerequisites, local or existing, in the caller's order.
    pub depends_on: Vec<PlanDependency>,
}

/// A prerequisite of a sub-task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDependency {
    /// Another sub-task of this plan, by its ref.
    Local(String),
    /// A task that already exists in the project.
    Task(TaskRef),
}

/// What a filed plan produced.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanOutcome {
    /// The parent — the new one, or the existing one as the plan left it.
    pub parent: Option<TaskDto>,
    /// The sub-tasks, in the caller's order.
    pub tasks: Vec<TaskDto>,
    /// Each ref and the id of the task it became.
    pub refs: BTreeMap<String, Uuid>,
}

/// Is `value` usable as a local ref?
///
/// Letters, digits, `.`, `_` and `-`, at most [`MAX_REF_LEN`] of them, and
/// not something that is also a task reference: all digits is a task number
/// and a UUID is a task id, and a `#` is not in the alphabet at all.
pub fn is_valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REF_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<Uuid>().is_err()
}

/// What a malformed ref is told.
fn invalid_ref(value: &str) -> Error {
    Error::BadRequest(format!(
        r#"ref "{value}" must be 1-64 letters, digits, ".", "_" or "-", and not a task number or UUID"#
    ))
}

/// What a `depends_on` entry naming no ref of the plan is told.
pub fn unknown_ref(value: &str) -> Error {
    Error::BadRequest(format!(
        r#"depends_on names "{value}", which is neither a ref of this plan nor a task reference"#
    ))
}

/// The rules of a plan that need no database, and the order its sub-tasks are
/// created in.
///
/// The bound, the refs — well formed, unique, and every local prerequisite
/// naming one of them — and the acyclicity of the local edges. The answer is
/// the indices of `input.tasks` in creation order: input order, except that a
/// sub-task waits for every sub-task it depends on (Kahn's algorithm, taking
/// the lowest ready index each time). Every failure is [`Error::BadRequest`];
/// the MCP layer calls this before it takes the project lock, and
/// [`create_plan`] calls it again so the verb is whole on its own.
pub fn validate_plan(input: &PlanInput) -> Result<Vec<usize>> {
    if input.tasks.is_empty() || input.tasks.len() > MAX_PLAN_TASKS {
        return Err(Error::BadRequest(PLAN_SIZE.into()));
    }

    let mut index: HashMap<&str, usize> = HashMap::with_capacity(input.tasks.len());
    for (position, task) in input.tasks.iter().enumerate() {
        if !is_valid_ref(&task.local_ref) {
            return Err(invalid_ref(&task.local_ref));
        }
        if index.insert(&task.local_ref, position).is_some() {
            return Err(Error::BadRequest(format!(
                r#"ref "{}" is used by more than one task"#,
                task.local_ref
            )));
        }
    }

    // `prerequisites[i]`: the sub-tasks `i` waits for, deduplicated.
    let mut prerequisites: Vec<Vec<usize>> = Vec::with_capacity(input.tasks.len());
    for task in &input.tasks {
        let mut local = Vec::new();
        for dependency in &task.depends_on {
            if let PlanDependency::Local(name) = dependency {
                let position = *index.get(name.as_str()).ok_or_else(|| unknown_ref(name))?;
                if !local.contains(&position) {
                    local.push(position);
                }
            }
        }
        prerequisites.push(local);
    }

    let mut created = vec![false; input.tasks.len()];
    let mut order = Vec::with_capacity(input.tasks.len());
    while order.len() < input.tasks.len() {
        let next = (0..input.tasks.len()).find(|&position| {
            !created[position] && prerequisites[position].iter().all(|&p| created[p])
        });

        match next {
            Some(position) => {
                created[position] = true;
                order.push(position);
            }
            None => return Err(cycle_error(input, &prerequisites, &created)),
        }
    }

    Ok(order)
}

/// The refusal of a batch whose local edges close a cycle, naming the refs on
/// it.
///
/// What Kahn's algorithm leaves behind is every sub-task on a cycle *and*
/// every sub-task downstream of one; peeling off the ones nothing left waits
/// for leaves the cycles themselves, which is what the caller has to change.
/// The text starts with the tracker's own [`CYCLE`], the words `update` gives
/// the same mistake.
fn cycle_error(input: &PlanInput, prerequisites: &[Vec<usize>], created: &[bool]) -> Error {
    let mut left: Vec<bool> = created.iter().map(|done| !done).collect();

    loop {
        let waited_for = |position: usize, left: &[bool]| {
            (0..left.len()).any(|other| left[other] && prerequisites[other].contains(&position))
        };
        let peel: Vec<usize> = (0..left.len())
            .filter(|&position| left[position] && !waited_for(position, &left))
            .collect();
        if peel.is_empty() {
            break;
        }
        for position in peel {
            left[position] = false;
        }
    }

    let refs: Vec<&str> = (0..left.len())
        .filter(|&position| left[position])
        .map(|position| input.tasks[position].local_ref.as_str())
        .collect();

    Error::BadRequest(format!("{CYCLE} among refs {}", refs.join(", ")))
}

/// File a plan inside an open mutation.
///
/// The order, after [`validate_plan`]:
///
/// ```text
/// resolve the provenance once (400/404)
///   → the parent: resolve an existing one (400), or create the new one
///     with the provenance edge
///   → each sub-task in creation order, under the parent, its local
///     prerequisites resolved to the ids just created: create_task's own
///     state, number, parent, edge, blocked and event rules
///   → read every task once more for the answer
/// ```
///
/// Every event of every creation goes into this mutation's one batch, so the
/// caller's commit announces the whole plan with one notification, and any
/// failure — a state name, a parent that is not top-level, a prerequisite of
/// another project — rolls back every task, edge, event and number the plan
/// had written.
pub async fn create_plan(m: &mut TrackerMutation<'_>, input: PlanInput) -> Result<PlanOutcome> {
    let order = validate_plan(&input)?;
    let project_id = m.project_id();
    let created_by = CreatedBy::Session(input.session_id);

    let existing_parent = match &input.parent {
        Some(PlanParent::Existing(reference)) => Some(
            TaskRepository::new(m.pool())
                .find_task_for_update(m.conn(), project_id, *reference)
                .await?
                .ok_or_else(|| Error::BadRequest(PARENT_NOT_TOP_LEVEL.into()))?
                .id,
        ),
        _ => None,
    };

    // Asked once for the whole plan, with the parent it names when that parent
    // already exists, so an origin that *is* that parent records nothing more.
    let origin =
        resolve_origin(m, input.session_id, input.discovered_from, existing_parent).await?;

    let (parent_id, sub_task_origin) = match input.parent {
        Some(PlanParent::New(fields)) => {
            let parent = create_task_with(
                m,
                creation(fields, None, Vec::new(), created_by),
                Provenance::Resolved(origin),
            )
            .await?;
            // The sub-tasks' parent link leads to the origin already.
            (Some(parent.id), None)
        }
        Some(PlanParent::Existing(_)) => (existing_parent, origin),
        None => (None, origin),
    };

    let mut ids: Vec<Option<Uuid>> = vec![None; input.tasks.len()];
    let mut refs: BTreeMap<String, Uuid> = BTreeMap::new();
    for position in order {
        let task = &input.tasks[position];

        let mut depends_on = Vec::with_capacity(task.depends_on.len());
        for dependency in &task.depends_on {
            depends_on.push(match dependency {
                PlanDependency::Task(reference) => *reference,
                PlanDependency::Local(name) => {
                    let id = refs.get(name).copied().ok_or_else(|| unknown_ref(name))?;
                    TaskRef::Id(id)
                }
            });
        }

        let created = create_task_with(
            m,
            creation(task.fields.clone(), parent_id, depends_on, created_by),
            Provenance::Resolved(sub_task_origin),
        )
        .await?;

        ids[position] = Some(created.id);
        refs.insert(task.local_ref.clone(), created.id);
    }

    info!(
        project_id = %project_id,
        session_id = %input.session_id,
        tasks = refs.len(),
        parent = parent_id.is_some(),
        "plan created",
    );

    // Read once more: a later sub-task's creation flips the parent's `blocked`
    // after the parent's own answer was taken, and the caller answers with
    // these.
    let parent = match parent_id {
        Some(id) => Some(load(m, id).await?),
        None => None,
    };
    let mut tasks = Vec::with_capacity(ids.len());
    for id in ids.into_iter().flatten() {
        tasks.push(load(m, id).await?);
    }

    Ok(PlanOutcome {
        parent,
        tasks,
        refs,
    })
}

/// One task of the plan as [`create_task_with`]'s input.
///
/// A sub-task's local prerequisites arrive in `local` already resolved to the
/// ids just created, after the existing ones its fields name.
fn creation(
    fields: PlanTaskFields,
    parent: Option<Uuid>,
    local: Vec<TaskRef>,
    created_by: CreatedBy,
) -> CreateTaskInput {
    let mut depends_on = fields.depends_on;
    depends_on.extend(local);

    CreateTaskInput {
        title: fields.title,
        description: fields.description,
        state: fields.state,
        priority: fields.priority,
        labels: fields.labels,
        parent,
        depends_on,
        // Resolved once for the plan and passed as `Provenance::Resolved`.
        discovered_from: None,
        created_by,
    }
}

/// A task of this project under this mutation's lock, which must be there.
async fn load(m: &mut TrackerMutation<'_>, task_id: Uuid) -> Result<TaskDto> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .load_task_dto_in(m.conn(), project_id, task_id)
        .await?
        .ok_or(Error::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str, depends_on: &[&str]) -> PlanTask {
        PlanTask {
            local_ref: name.into(),
            fields: PlanTaskFields {
                title: name.into(),
                ..PlanTaskFields::default()
            },
            depends_on: depends_on
                .iter()
                .map(|name| PlanDependency::Local((*name).into()))
                .collect(),
        }
    }

    fn plan(tasks: Vec<PlanTask>) -> PlanInput {
        PlanInput {
            parent: None,
            tasks,
            discovered_from: None,
            session_id: Uuid::nil(),
        }
    }

    fn refused(input: &PlanInput) -> String {
        match validate_plan(input) {
            Err(Error::BadRequest(message)) => message,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn input_order_is_kept_until_a_prerequisite_comes_later() {
        let order = validate_plan(&plan(vec![
            task("api", &["schema"]),
            task("docs", &[]),
            task("schema", &[]),
            task("ui", &["api", "api"]),
        ]))
        .expect("the plan is acyclic");

        assert_eq!(order, [1, 2, 0, 3]);
    }

    #[test]
    fn an_empty_or_oversized_plan_is_refused() {
        assert_eq!(refused(&plan(Vec::new())), PLAN_SIZE);

        let many = (0..=MAX_PLAN_TASKS)
            .map(|n| task(&format!("t{n}"), &[]))
            .collect();
        assert_eq!(refused(&plan(many)), PLAN_SIZE);

        let most = (0..MAX_PLAN_TASKS)
            .map(|n| task(&format!("t{n}"), &[]))
            .collect();
        assert!(validate_plan(&plan(most)).is_ok());
    }

    #[test]
    fn a_ref_that_reads_as_a_task_reference_or_has_odd_characters_is_refused() {
        for name in [
            "",
            "12",
            "#12",
            "a b",
            "schéma",
            "0f8fad5b-d9cb-469f-a165-70867728950e",
            &"r".repeat(MAX_REF_LEN + 1),
        ] {
            let message = refused(&plan(vec![task(name, &[])]));
            assert!(
                message.starts_with(&format!(r#"ref "{name}" must be"#)),
                "{message}"
            );
        }

        for name in [
            "api",
            "step-1",
            "1a",
            "v2.schema",
            "_x",
            &"r".repeat(MAX_REF_LEN),
        ] {
            assert!(is_valid_ref(name), "{name}");
        }
    }

    #[test]
    fn a_duplicated_or_unknown_ref_is_refused() {
        assert_eq!(
            refused(&plan(vec![task("a", &[]), task("a", &[])])),
            r#"ref "a" is used by more than one task"#,
        );
        assert_eq!(
            refused(&plan(vec![task("a", &["b"])])),
            r#"depends_on names "b", which is neither a ref of this plan nor a task reference"#,
        );
    }

    #[test]
    fn a_cycle_names_the_refs_on_it_and_not_the_ones_downstream() {
        let message = refused(&plan(vec![
            task("a", &["c"]),
            task("b", &["a"]),
            task("c", &["b"]),
            task("d", &["c"]),
            task("e", &[]),
        ]));
        assert_eq!(message, format!("{CYCLE} among refs a, b, c"));

        assert_eq!(
            refused(&plan(vec![task("self", &["self"])])),
            format!("{CYCLE} among refs self"),
        );
    }
}
