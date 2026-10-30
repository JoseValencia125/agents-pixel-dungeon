//! SQLite in WAL mode. Entities are rows of JSON with the columns the
//! scheduler filters on; events are an append-only table with a global
//! sequence. A state change and its event go in one transaction.

use crate::domain::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

pub struct Store {
    conn: Connection,
    /// Open savepoints: state and its events commit together at depth 0.
    depth: std::cell::Cell<u32>,
}

pub type StoreResult<T> = Result<T, String>;

fn e(err: impl std::fmt::Display) -> String { err.to_string() }

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS quests (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, state TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, quest_id TEXT NOT NULL, state TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS members (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS attempts (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, member_id TEXT NOT NULL, state TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS workspaces (id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL, state TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS artifacts (id TEXT PRIMARY KEY, attempt_id TEXT, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, type TEXT NOT NULL, attempt_id TEXT, task_id TEXT, quest_id TEXT, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS commands (idempotency_key TEXT PRIMARY KEY, result TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS designs (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS decisions (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS records (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, task_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS hypotheses (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS questions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS design_decisions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS experiments (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS playtests (id TEXT PRIMARY KEY, experiment_id TEXT, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS observations (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS findings (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS feedback (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, status TEXT NOT NULL, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS stalls (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, json TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS tasks_quest ON tasks(quest_id, state);
CREATE INDEX IF NOT EXISTS attempts_task ON attempts(task_id, state);
CREATE INDEX IF NOT EXISTS events_attempt ON events(attempt_id);
";

impl Store {
    pub fn open(path: &Path) -> StoreResult<Store> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir).map_err(e)?; }
        let conn = Connection::open(path).map_err(e)?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> StoreResult<Store> { Store::init(Connection::open_in_memory().map_err(e)?) }

    fn init(conn: Connection) -> StoreResult<Store> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.pragma_update(None, "busy_timeout", 5000).ok();
        conn.execute_batch(SCHEMA).map_err(e)?;
        conn.execute("INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', ?1)", params![SCHEMA_VERSION.to_string()]).map_err(e)?;
        Ok(Store { conn, depth: std::cell::Cell::new(0) })
    }

    // ----- generic helpers -----

    fn put<T: Serialize>(&self, table: &str, id: &str, extra: &[(&str, String)], value: &T) -> StoreResult<()> {
        let json = serde_json::to_string(value).map_err(e)?;
        let mut cols = vec!["id".to_string(), "json".to_string()];
        let mut vals: Vec<String> = vec![id.to_string(), json];
        for (c, v) in extra {
            cols.push(c.to_string());
            vals.push(v.clone());
        }
        let placeholders: Vec<String> = (1..=vals.len()).map(|i| format!("?{i}")).collect();
        let sql = format!("INSERT OR REPLACE INTO {table}({}) VALUES ({})", cols.join(", "), placeholders.join(", "));
        self.conn.execute(&sql, rusqlite::params_from_iter(vals.iter())).map_err(e)?;
        Ok(())
    }

    fn get<T: DeserializeOwned>(&self, table: &str, id: &str) -> StoreResult<Option<T>> {
        let sql = format!("SELECT json FROM {table} WHERE id = ?1");
        let row: Option<String> = self.conn.query_row(&sql, params![id], |r| r.get(0)).optional().map_err(e)?;
        match row {
            Some(json) => Ok(Some(serde_json::from_str(&json).map_err(e)?)),
            None => Ok(None),
        }
    }

    fn list<T: DeserializeOwned>(&self, sql: &str, args: &[&str]) -> StoreResult<Vec<T>> {
        let mut stmt = self.conn.prepare(sql).map_err(e)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = vec![];
        for row in rows {
            out.push(serde_json::from_str(&row.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    /// A setting the guild keeps between runs (`concurrency`, …).
    pub fn meta(&self, key: &str) -> StoreResult<Option<String>> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().map_err(e)
    }
    pub fn set_meta(&self, key: &str, value: &str) -> StoreResult<()> {
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value]).map_err(e)?;
        Ok(())
    }

    // ----- entities -----

    pub fn put_project(&self, p: &Project) -> StoreResult<()> { self.put("projects", &p.id, &[], p) }
    pub fn project(&self, id: &str) -> StoreResult<Option<Project>> { self.get("projects", id) }
    pub fn projects(&self) -> StoreResult<Vec<Project>> { self.list("SELECT json FROM projects ORDER BY id", &[]) }

    pub fn put_quest(&self, q: &Quest) -> StoreResult<()> {
        self.put("quests", &q.id, &[("project_id", q.project_id.clone()), ("state", state_name(&q.state))], q)
    }
    pub fn quest(&self, id: &str) -> StoreResult<Option<Quest>> { self.get("quests", id) }
    pub fn quests(&self) -> StoreResult<Vec<Quest>> { self.list("SELECT json FROM quests ORDER BY id", &[]) }

    pub fn put_task(&self, t: &Task) -> StoreResult<()> {
        self.put("tasks", &t.id, &[("quest_id", t.quest_id.clone()), ("state", state_name(&t.state))], t)
    }
    pub fn task(&self, id: &str) -> StoreResult<Option<Task>> { self.get("tasks", id) }
    pub fn tasks(&self) -> StoreResult<Vec<Task>> { self.list("SELECT json FROM tasks ORDER BY id", &[]) }
    pub fn tasks_of_quest(&self, quest_id: &str) -> StoreResult<Vec<Task>> {
        self.list("SELECT json FROM tasks WHERE quest_id = ?1 ORDER BY id", &[quest_id])
    }

    pub fn put_member(&self, m: &Member) -> StoreResult<()> { self.put("members", &m.id, &[], m) }
    pub fn put_design(&self, d: &crate::design::GameDesign) -> StoreResult<()> { self.put("designs", &d.project_id, &[], d) }
    pub fn put_decision(&self, d: &RaceDecision) -> StoreResult<()> { self.put("decisions", &d.id, &[("task_id", d.task_id.clone())], d) }
    pub fn put_question(&self, q: &crate::design::DesignQuestion) -> StoreResult<()> { self.put("questions", &format!("{}:{}", q.project_id, q.id), &[("project_id", q.project_id.clone())], q) }
    pub fn question(&self, project_id: &str, id: &str) -> StoreResult<Option<crate::design::DesignQuestion>> { self.get("questions", &format!("{project_id}:{id}")) }
    pub fn questions(&self) -> StoreResult<Vec<crate::design::DesignQuestion>> { self.list("SELECT json FROM questions ORDER BY id", &[]) }
    pub fn questions_of(&self, project_id: &str) -> StoreResult<Vec<crate::design::DesignQuestion>> { self.list("SELECT json FROM questions WHERE project_id = ?1 ORDER BY id", &[project_id]) }
    pub fn put_design_decision(&self, d: &crate::design::DesignDecision) -> StoreResult<()> { self.put("design_decisions", &format!("{}:{}", d.project_id, d.id), &[("project_id", d.project_id.clone())], d) }
    pub fn design_decisions(&self) -> StoreResult<Vec<crate::design::DesignDecision>> { self.list("SELECT json FROM design_decisions ORDER BY id", &[]) }
    pub fn design_decisions_of(&self, project_id: &str) -> StoreResult<Vec<crate::design::DesignDecision>> { self.list("SELECT json FROM design_decisions WHERE project_id = ?1 ORDER BY id", &[project_id]) }
    pub fn put_hypothesis(&self, h: &crate::experiment::Hypothesis) -> StoreResult<()> { self.put("hypotheses", &h.id, &[("project_id", h.project_id.clone())], h) }
    pub fn hypothesis(&self, id: &str) -> StoreResult<Option<crate::experiment::Hypothesis>> { self.get("hypotheses", id) }
    pub fn hypotheses(&self) -> StoreResult<Vec<crate::experiment::Hypothesis>> { self.list("SELECT json FROM hypotheses ORDER BY id", &[]) }
    pub fn put_experiment(&self, e: &crate::experiment::Experiment) -> StoreResult<()> { self.put("experiments", &e.id, &[("project_id", e.project_id.clone())], e) }
    pub fn experiment(&self, id: &str) -> StoreResult<Option<crate::experiment::Experiment>> { self.get("experiments", id) }
    pub fn experiments(&self) -> StoreResult<Vec<crate::experiment::Experiment>> { self.list("SELECT json FROM experiments ORDER BY id", &[]) }
    pub fn put_playtest(&self, p: &crate::experiment::Playtest) -> StoreResult<()> { self.put("playtests", &p.id, &[("experiment_id", p.experiment_id.clone().unwrap_or_default())], p) }
    pub fn playtest(&self, id: &str) -> StoreResult<Option<crate::experiment::Playtest>> { self.get("playtests", id) }
    pub fn put_observation(&self, o: &crate::experiment::Observation) -> StoreResult<()> { self.put("observations", &format!("{}:{}", o.project_id, o.id), &[("project_id", o.project_id.clone())], o) }
    pub fn observation(&self, project_id: &str, id: &str) -> StoreResult<Option<crate::experiment::Observation>> { self.get("observations", &format!("{project_id}:{id}")) }
    pub fn observations(&self) -> StoreResult<Vec<crate::experiment::Observation>> { self.list("SELECT json FROM observations ORDER BY id", &[]) }
    pub fn put_finding(&self, f: &crate::aiplay::Finding) -> StoreResult<()> { self.put("findings", &format!("{}:{}", f.project_id, f.id), &[("project_id", f.project_id.clone())], f) }
    pub fn finding(&self, project_id: &str, id: &str) -> StoreResult<Option<crate::aiplay::Finding>> { self.get("findings", &format!("{project_id}:{id}")) }
    pub fn findings(&self) -> StoreResult<Vec<crate::aiplay::Finding>> { self.list("SELECT json FROM findings ORDER BY id", &[]) }
    pub fn findings_of(&self, project_id: &str) -> StoreResult<Vec<crate::aiplay::Finding>> { self.list("SELECT json FROM findings WHERE project_id = ?1 ORDER BY id", &[project_id]) }
    pub fn put_stall(&self, s: &crate::sentinel::Stall) -> StoreResult<()> { self.put("stalls", &s.id, &[("project_id", s.project_id.clone())], s) }
    pub fn stalls(&self) -> StoreResult<Vec<crate::sentinel::Stall>> { self.list("SELECT json FROM stalls ORDER BY id", &[]) }
    pub fn stall(&self, id: &str) -> StoreResult<Option<crate::sentinel::Stall>> { self.get("stalls", id) }
    pub fn delete_stall(&self, id: &str) -> StoreResult<()> {
        self.conn.execute("DELETE FROM stalls WHERE id = ?1", params![id]).map_err(e)?;
        Ok(())
    }
    pub fn put_feedback(&self, f: &crate::feedback::Feedback) -> StoreResult<()> { self.put("feedback", &f.id, &[("project_id", f.project_id.clone().unwrap_or_default()), ("status", f.status.clone())], f) }
    pub fn feedback(&self, id: &str) -> StoreResult<Option<crate::feedback::Feedback>> { self.get("feedback", id) }
    pub fn feedback_all(&self) -> StoreResult<Vec<crate::feedback::Feedback>> { self.list("SELECT json FROM feedback ORDER BY id", &[]) }
    pub fn observations_of(&self, project_id: &str) -> StoreResult<Vec<crate::experiment::Observation>> { self.list("SELECT json FROM observations WHERE project_id = ?1 ORDER BY id", &[project_id]) }
    pub fn playtests(&self) -> StoreResult<Vec<crate::experiment::Playtest>> { self.list("SELECT json FROM playtests ORDER BY id", &[]) }
    pub fn playtests_of_experiment(&self, id: &str) -> StoreResult<Vec<crate::experiment::Playtest>> { self.list("SELECT json FROM playtests WHERE experiment_id = ?1 ORDER BY id", &[id]) }
    pub fn put_record(&self, r: &AttemptRecord) -> StoreResult<()> { self.put("records", &r.attempt_id, &[("project_id", r.project_id.clone()), ("task_id", r.task_id.clone())], r) }
    pub fn record(&self, attempt_id: &str) -> StoreResult<Option<AttemptRecord>> { self.get("records", attempt_id) }
    pub fn records(&self, project_id: Option<&str>, task_id: Option<&str>) -> StoreResult<Vec<AttemptRecord>> {
        match (project_id, task_id) {
            (_, Some(t)) => self.list("SELECT json FROM records WHERE task_id = ?1 ORDER BY id", &[t]),
            (Some(p), None) => self.list("SELECT json FROM records WHERE project_id = ?1 ORDER BY id", &[p]),
            (None, None) => self.list("SELECT json FROM records ORDER BY id", &[]),
        }
    }
    pub fn decisions(&self) -> StoreResult<Vec<RaceDecision>> { self.list("SELECT json FROM decisions ORDER BY id", &[]) }
    pub fn decisions_of_task(&self, task_id: &str) -> StoreResult<Vec<RaceDecision>> { self.list("SELECT json FROM decisions WHERE task_id = ?1 ORDER BY id", &[task_id]) }
    pub fn design(&self, project_id: &str) -> StoreResult<Option<crate::design::GameDesign>> { self.get("designs", project_id) }
    pub fn designs(&self) -> StoreResult<Vec<crate::design::GameDesign>> { self.list("SELECT json FROM designs ORDER BY id", &[]) }
    pub fn member(&self, id: &str) -> StoreResult<Option<Member>> { self.get("members", id) }
    pub fn members(&self) -> StoreResult<Vec<Member>> { self.list("SELECT json FROM members ORDER BY id", &[]) }
    pub fn delete_member(&self, id: &str) -> StoreResult<()> {
        self.conn.execute("DELETE FROM members WHERE id = ?1", params![id]).map_err(e)?;
        Ok(())
    }

    pub fn put_attempt(&self, a: &Attempt) -> StoreResult<()> {
        self.put("attempts", &a.id, &[("task_id", a.task_id.clone()), ("member_id", a.member_id.clone()), ("state", state_name(&a.state))], a)
    }
    pub fn attempt(&self, id: &str) -> StoreResult<Option<Attempt>> { self.get("attempts", id) }
    pub fn attempts(&self) -> StoreResult<Vec<Attempt>> { self.list("SELECT json FROM attempts ORDER BY id", &[]) }
    pub fn attempts_of_task(&self, task_id: &str) -> StoreResult<Vec<Attempt>> {
        self.list("SELECT json FROM attempts WHERE task_id = ?1 ORDER BY id", &[task_id])
    }
    pub fn active_attempts(&self) -> StoreResult<Vec<Attempt>> {
        Ok(self.attempts()?.into_iter().filter(|a| a.state.is_active()).collect())
    }

    pub fn put_workspace(&self, w: &Workspace) -> StoreResult<()> {
        self.put("workspaces", &w.id, &[("attempt_id", w.attempt_id.clone()), ("state", state_name(&w.state))], w)
    }
    pub fn workspace(&self, id: &str) -> StoreResult<Option<Workspace>> { self.get("workspaces", id) }
    pub fn workspaces(&self) -> StoreResult<Vec<Workspace>> { self.list("SELECT json FROM workspaces ORDER BY id", &[]) }

    pub fn put_session(&self, s: &Session) -> StoreResult<()> { self.put("sessions", &s.id, &[("attempt_id", s.attempt_id.clone())], s) }
    pub fn session(&self, id: &str) -> StoreResult<Option<Session>> { self.get("sessions", id) }

    pub fn put_artifact(&self, a: &Artifact) -> StoreResult<()> {
        self.put("artifacts", &a.id, &[("attempt_id", a.attempt_id.clone().unwrap_or_default())], a)
    }
    pub fn artifact(&self, id: &str) -> StoreResult<Option<Artifact>> { self.get("artifacts", id) }
    pub fn artifacts_of_attempt(&self, attempt_id: &str) -> StoreResult<Vec<Artifact>> {
        self.list("SELECT json FROM artifacts WHERE attempt_id = ?1 ORDER BY id", &[attempt_id])
    }

    // ----- events -----

    /// Append an event and return it with its sequence number.
    pub fn append(&self, mut ev: Event) -> StoreResult<Event> {
        ev.occurred_at = if ev.occurred_at.is_empty() { now() } else { ev.occurred_at };
        let json = serde_json::to_string(&ev).map_err(e)?;
        self.conn
            .execute(
                "INSERT INTO events(id, type, attempt_id, task_id, quest_id, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![ev.id, ev.kind, ev.attempt_id, ev.task_id, ev.quest_id, json],
            )
            .map_err(e)?;
        ev.seq = self.conn.last_insert_rowid() as u64;
        // Rewrite with the seq inside the JSON, so a replay carries it.
        let json = serde_json::to_string(&ev).map_err(e)?;
        self.conn.execute("UPDATE events SET json = ?1 WHERE seq = ?2", params![json, ev.seq as i64]).map_err(e)?;
        Ok(ev)
    }

    pub fn last_seq(&self) -> StoreResult<u64> {
        let v: Option<i64> = self.conn.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0)).map_err(e)?;
        Ok(v.unwrap_or(0) as u64)
    }

    pub fn events_after(&self, after_seq: u64, limit: usize) -> StoreResult<Vec<Event>> {
        let mut stmt = self.conn.prepare("SELECT json FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2").map_err(e)?;
        let rows = stmt.query_map(params![after_seq as i64, limit as i64], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = vec![];
        for row in rows {
            out.push(serde_json::from_str(&row.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    /// The last `limit` events whose type starts with `prefix`, oldest first.
    pub fn events_of_kind(&self, prefix: &str, limit: usize) -> StoreResult<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM (SELECT seq, json FROM events WHERE type LIKE ?1 ORDER BY seq DESC LIMIT ?2) ORDER BY seq")
            .map_err(e)?;
        let rows = stmt.query_map(params![format!("{prefix}%"), limit as i64], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = vec![];
        for row in rows {
            out.push(serde_json::from_str(&row.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    pub fn events_of_attempt(&self, attempt_id: &str, limit: usize) -> StoreResult<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM (SELECT seq, json FROM events WHERE attempt_id = ?1 ORDER BY seq DESC LIMIT ?2) ORDER BY seq")
            .map_err(e)?;
        let rows = stmt.query_map(params![attempt_id, limit as i64], |r| r.get::<_, String>(0)).map_err(e)?;
        let mut out = vec![];
        for row in rows {
            out.push(serde_json::from_str(&row.map_err(e)?).map_err(e)?);
        }
        Ok(out)
    }

    // ----- commands -----

    /// The stored result of a command already run with this key, if any.
    pub fn command_result(&self, key: &str) -> StoreResult<Option<serde_json::Value>> {
        let row: Option<String> = self.conn.query_row("SELECT result FROM commands WHERE idempotency_key = ?1", params![key], |r| r.get(0)).optional().map_err(e)?;
        Ok(match row {
            Some(s) => Some(serde_json::from_str(&s).map_err(e)?),
            None => None,
        })
    }

    pub fn remember_command(&self, key: &str, result: &serde_json::Value) -> StoreResult<()> {
        self.conn
            .execute("INSERT OR REPLACE INTO commands(idempotency_key, result, at) VALUES (?1, ?2, ?3)", params![key, result.to_string(), now()])
            .map_err(e)?;
        Ok(())
    }

    // ----- transactions -----

    pub fn transaction<T>(&mut self, f: impl FnOnce(&Store) -> StoreResult<T>) -> StoreResult<T> {
        self.begin()?;
        match f(self) {
            Ok(v) => { self.commit()?; Ok(v) }
            Err(err) => { self.rollback(); Err(err) }
        }
    }

    /// Open a unit of work; nested calls become savepoints, so a helper
    /// that is atomic on its own stays part of a larger atomic change.
    pub fn begin(&self) -> StoreResult<()> {
        let d = self.depth.get();
        let sql = if d == 0 { "BEGIN IMMEDIATE".to_string() } else { format!("SAVEPOINT sp{d}") };
        self.conn.execute_batch(&sql).map_err(e)?;
        self.depth.set(d + 1);
        Ok(())
    }

    pub fn commit(&self) -> StoreResult<()> {
        let d = self.depth.get().saturating_sub(1);
        let sql = if d == 0 { "COMMIT".to_string() } else { format!("RELEASE sp{d}") };
        self.conn.execute_batch(&sql).map_err(e)?;
        self.depth.set(d);
        Ok(())
    }

    pub fn rollback(&self) {
        let d = self.depth.get().saturating_sub(1);
        let sql = if d == 0 { "ROLLBACK".to_string() } else { format!("ROLLBACK TO sp{d}; RELEASE sp{d}") };
        let _ = self.conn.execute_batch(&sql);
        self.depth.set(d);
    }

    /// True while a unit of work is open.
    pub fn in_transaction(&self) -> bool { self.depth.get() > 0 }
    pub fn depth(&self) -> u32 { self.depth.get() }

    pub fn snapshot(&self, service_version: &str, harnesses: Vec<HarnessStatus>) -> StoreResult<Snapshot> {
        Ok(Snapshot {
            schema_version: SCHEMA_VERSION,
            last_seq: self.last_seq()?,
            service_version: service_version.into(),
            projects: self.projects()?,
            quests: self.quests()?,
            tasks: self.tasks()?,
            members: self.members()?,
            attempts: self.attempts()?,
            workspaces: self.workspaces()?,
            harnesses,
            decisions: self.decisions()?,
            hypotheses: self.hypotheses()?,
            questions: self.questions()?,
            design_decisions: self.design_decisions()?,
            experiments: self.experiments()?,
            playtests: self.playtests()?,
            observations: self.observations()?,
            findings: self.findings()?,
            feedback: self.feedback_all()?,
            designs: self.designs()?,
            roles: vec![],
            conversation: vec![],
            orchestrator: None,
            orchestrator_busy: false,
            concurrency: 0,
            overlaps: vec![],
            disk_low: None,
            disk_note: None,
            paused_members: vec![],
            stalls: self.stalls()?,
        })
    }
}

fn state_name<T: Serialize>(s: &T) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_get_increasing_sequences_and_replay_after_a_cursor() {
        let store = Store::open_in_memory().unwrap();
        let a = store.append(Event::new("quest.created", "guild")).unwrap();
        let b = store.append(Event::new("task.ready", "guild")).unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));
        let after = store.events_after(1, 10).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].kind, "task.ready");
        assert_eq!(after[0].seq, 2);
        assert_eq!(store.last_seq().unwrap(), 2);
    }

    #[test]
    fn a_failed_transaction_rolls_back_the_entity_and_the_event() {
        let mut store = Store::open_in_memory().unwrap();
        let result: StoreResult<()> = store.transaction(|s| {
            s.put_member(&Member { id: "m".into(), name: "M".into(), roles: vec![], harness: HarnessId::named("mock"), model_id: None, mode: None, role_prompt: String::new(), enabled: true, switched: None })?;
            s.append(Event::new("x", "guild"))?;
            Err("boom".into())
        });
        assert!(result.is_err());
        assert!(store.member("m").unwrap().is_none());
        assert_eq!(store.last_seq().unwrap(), 0);
    }

    #[test]
    fn repeated_commands_return_the_remembered_result() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.command_result("k").unwrap().is_none());
        store.remember_command("k", &serde_json::json!({"ok": true})).unwrap();
        assert_eq!(store.command_result("k").unwrap().unwrap()["ok"], true);
    }
}
