//! The pure part of dispatch: which tasks are ready, which are blocked by
//! a failed parent, and which member takes the next one. The service
//! applies what this decides, inside a transaction.

use crate::domain::*;
use std::collections::HashMap;

/// A pending task is ready when every dependency is completed; it is
/// blocked when a dependency failed or was cancelled.
pub fn readiness(tasks: &[Task]) -> Vec<(String, TaskState, Option<String>)> {
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut changes = vec![];
    for t in tasks.iter().filter(|t| t.state == TaskState::Pending || (t.state == TaskState::Blocked && t.block_reason.as_deref().map(|r| r.starts_with("dependency")).unwrap_or(false))) {
        let mut all_done = true;
        let mut failed: Option<String> = None;
        for d in &t.depends_on {
            match by_id.get(d.as_str()).map(|p| (p.state, p.title.clone())) {
                Some((TaskState::Completed, _)) => {}
                Some((TaskState::Failed, title)) | Some((TaskState::Cancelled, title)) => failed = Some(title),
                _ => all_done = false,
            }
        }
        if let Some(title) = failed {
            if t.state != TaskState::Blocked {
                changes.push((t.id.clone(), TaskState::Blocked, Some(format!("dependency failed: {title}"))));
            }
        } else if all_done && t.state != TaskState::Ready {
            changes.push((t.id.clone(), TaskState::Ready, None));
        } else if !all_done && t.state == TaskState::Blocked {
            // The failed parent was retried: wait for it again.
            changes.push((t.id.clone(), TaskState::Pending, None));
        }
    }
    changes
}

/// Ready tasks in dispatch order: priority first, then age, then id.
pub fn ready_in_order<'a>(tasks: &'a [Task], running_quests: &[String]) -> Vec<&'a Task> {
    let mut ready: Vec<&Task> = tasks.iter().filter(|t| t.state == TaskState::Ready && running_quests.contains(&t.quest_id)).collect();
    ready.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.created_at.cmp(&b.created_at)).then(a.id.cmp(&b.id)));
    ready
}

/// The member to give a task to: the one it names, or the first enabled
/// member with its role that is free and whose harness is available.
pub fn pick_member<'a>(task: &Task, members: &'a [Member], busy: &[String], available: &[HarnessKind]) -> Option<&'a Member> {
    let fits = |m: &&Member| m.enabled && !busy.contains(&m.id) && available.contains(&m.harness) && m.roles.iter().any(|r| r == &task.role);
    if let Some(id) = &task.member_id {
        return members.iter().find(|m| &m.id == id).filter(fits);
    }
    members.iter().find(fits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, deps: &[&str], state: TaskState) -> Task {
        Task {
            id: id.into(), quest_id: "q".into(), title: id.into(), instructions: String::new(), acceptance: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(), role: "implementer".into(), member_id: None, model_id: None,
            priority: 0, state, block_reason: None, attempts_made: 0, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, notes: vec![], granted: Default::default(), evidence: vec![], spike: false, version: 1, created_at: now(), updated_at: now(),
        }
    }

    #[test]
    fn children_wait_for_every_parent_and_block_on_a_failed_one() {
        let tasks = vec![task("t1", &[], TaskState::Completed), task("t2", &[], TaskState::Running), task("t3", &["t1", "t2"], TaskState::Pending), task("t4", &["t1"], TaskState::Pending)];
        let changes = readiness(&tasks);
        assert_eq!(changes, vec![("t4".to_string(), TaskState::Ready, None)]);
        let tasks = vec![task("t1", &[], TaskState::Failed), task("t3", &["t1"], TaskState::Pending)];
        let changes = readiness(&tasks);
        assert_eq!(changes[0].1, TaskState::Blocked);
        assert!(changes[0].2.as_deref().unwrap().starts_with("dependency failed"));
    }

    #[test]
    fn a_busy_member_is_skipped_and_a_named_one_is_honoured() {
        let m = |id: &str, role: &str| Member { id: id.into(), name: id.into(), roles: vec![role.into()], harness: HarnessId::named("mock"), model_id: None, mode: None, role_prompt: String::new(), enabled: true, switched: None };
        let members = vec![m("a", "implementer"), m("b", "implementer"), m("r", "reviewer")];
        let t = task("t", &[], TaskState::Ready);
        assert_eq!(pick_member(&t, &members, &["a".into()], &[HarnessId::named("mock")]).unwrap().id, "b");
        let mut named = t.clone();
        named.member_id = Some("r".into());
        assert!(pick_member(&named, &members, &[], &[HarnessId::named("mock")]).is_none());
        assert!(pick_member(&t, &members, &[], &[HarnessId::named("kiro")]).is_none());
    }
}
