//! `agents-pixel-dungeon-guild`: the guild service and its command line.
//! `serve` runs the service; every other command talks to it over the
//! socket, starting it in the background when it is not up.

use guild::api::{ApiError, Client, StreamMessage};
use guild::domain::*;
use guild::harness::kiro_acp::KiroAcp;
use guild::harness::mock::{MockBehaviour, MockHarness};
use guild::service::{self, Service, Settings};
use guild::store::Store;
use guild::{config, paths};
use serde_json::{json, Value};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

const USAGE: &str = "\
agents-pixel-dungeon-guild — the guild that plans, builds, verifies and integrates

USAGE
  guild serve [--daemon] [--concurrency N] [--attempt-timeout MIN]
  guild status                       the guild at a glance
  guild snapshot                     the whole state as JSON
  guild stop                         stop the service

  guild project add <path> [--id ID] [--name NAME]
  guild member add <id> --harness kiro|codex|claude|mock [--role R]... [--model M] [--mode MODE] [--name N]
  guild models [kiro|mock]           the models the harness announces

  guild quest new <project> <title> [--description TEXT]
  guild task new <quest> <title> [--instructions TEXT] [--accept TEXT]... [--depends ID,ID]
                                     [--role R] [--member ID] [--model M] [--priority N] [--max-attempts N] [--id ID]
  guild start <quest>  |  guild pause <quest>  |  guild resume <quest>  |  guild cancel-quest <quest>

  guild accept <task> [--integrate]  the latest candidate passes; --integrate squashes it into the quest branch
  guild reject <task> [--reason TEXT]
  guild integrate <task>
  guild retry <task>  |  guild cancel-task <task>  |  guild cancel <attempt>
  guild interrupt <attempt>          end the turn now; what the worktree has becomes the candidate
  guild permission <attempt> <option-id|deny>
  guild attempt <attempt>            the attempt, its candidate, verifiers and transcript
  guild events [--follow] [--after SEQ] [--attempt ID]

ENVIRONMENT
  HPD_GUILD_HOME     data directory (database, worktrees, artifacts, socket)
  HPD_GUILD_SOCKET   socket path
  KIRO_CLI_BIN       kiro-cli binary
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" || args[0] == "help" {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args[0] == "--version" {
        println!("agents-pixel-dungeon-guild {}", service::SERVICE_VERSION);
        return ExitCode::SUCCESS;
    }
    let result = match args[0].as_str() {
        "serve" => serve(&args[1..]),
        other => client_command(other, &args[1..]),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ----- argument helpers -----

struct Args<'a> {
    positional: Vec<&'a str>,
    flags: Vec<(&'a str, Option<&'a str>)>,
}

fn parse(args: &[String]) -> Args<'_> {
    let mut positional = vec![];
    let mut flags = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(name) = a.strip_prefix("--") {
            if let Some((k, v)) = name.split_once('=') {
                flags.push((k, Some(v)));
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") && !is_boolean_flag(name) {
                flags.push((name, Some(args[i + 1].as_str())));
                i += 1;
            } else {
                flags.push((name, None));
            }
        } else {
            positional.push(a);
        }
        i += 1;
    }
    Args { positional, flags }
}

fn is_boolean_flag(name: &str) -> bool { matches!(name, "daemon" | "follow" | "integrate" | "json" | "foreground") }

impl Args<'_> {
    fn flag(&self, name: &str) -> Option<&str> { self.flags.iter().rev().find(|(k, _)| *k == name).and_then(|(_, v)| *v) }
    fn has(&self, name: &str) -> bool { self.flags.iter().any(|(k, _)| *k == name) }
    fn all(&self, name: &str) -> Vec<String> { self.flags.iter().filter(|(k, _)| *k == name).filter_map(|(_, v)| v.map(str::to_string)).collect() }
    fn pos(&self, i: usize, what: &str) -> Result<&str, String> { self.positional.get(i).copied().ok_or_else(|| format!("missing {what}\n\n{USAGE}")) }
}

// ----- serve -----

fn serve(args: &[String]) -> Result<(), String> {
    let a = parse(args);
    if a.has("daemon") {
        return daemonize();
    }
    paths::ensure_home().map_err(|e| e.to_string())?;
    let lock = acquire_lock()?;
    let mut settings = Settings::default();
    if let Some(n) = a.flag("concurrency").and_then(|s| s.parse().ok()) { settings.concurrency = n; }
    if let Some(m) = a.flag("attempt-timeout").and_then(|s| s.parse::<u64>().ok()) { settings.attempt_timeout = Duration::from_secs(m * 60); }
    let store = Store::open(&paths::db_path())?;
    let service = Service::new(store, settings);
    service.log_to(&paths::log_path());
    service.register(Arc::new(KiroAcp::new()));
    service.register(Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "GUILD_MOCK.md".into(), content: "The mock harness was here.\n".into() })));
    service.log(&format!("guild service {} starting, home {}", service::SERVICE_VERSION, paths::home().display()));
    let result = service::run(service.clone(), &paths::socket_path()).map_err(|e| e.to_string());
    service.log("guild service stopped");
    drop(lock);
    result
}

/// One service per home: an exclusive flock on a lock file.
fn acquire_lock() -> Result<std::fs::File, String> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().create(true).write(true).open(paths::lock_path()).map_err(|e| e.to_string())?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(format!("another guild service already owns {}", paths::home().display()));
    }
    Ok(file)
}

fn daemonize() -> Result<(), String> {
    paths::ensure_home().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_path()).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::from(log.try_clone().map_err(|e| e.to_string())?)).stderr(std::process::Stdio::from(log));
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map_err(|e| e.to_string())?;
    let client = Client::default_socket();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if client.is_up() {
            println!("guild service running (socket {})", paths::socket_path().display());
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("the service did not answer; see {}", paths::log_path().display()))
}

// ----- client commands -----

fn ensure_running() -> Result<Client, String> {
    let client = Client::default_socket();
    if client.is_up() { return Ok(client); }
    daemonize()?;
    Ok(client)
}

fn call(client: &Client, method: &str, params: Value) -> Result<Value, String> { client.call(method, params).map_err(|e: ApiError| e.to_string()) }

fn client_command(command: &str, args: &[String]) -> Result<(), String> {
    let a = parse(args);
    match command {
        "stop" => {
            let client = Client::default_socket();
            if !client.is_up() {
                println!("the service is not running");
                return Ok(());
            }
            call(&client, "guild.stop", json!({}))?;
            println!("stopping");
            Ok(())
        }
        "status" => status(&ensure_running()?),
        "snapshot" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            println!("{}", serde_json::to_string_pretty(&snap).unwrap());
            Ok(())
        }
        "models" => {
            let harness = a.positional.first().copied().unwrap_or("kiro");
            let models = call(&ensure_running()?, "models.list", json!({"harness": harness}))?;
            for m in models.as_array().into_iter().flatten() {
                println!("{:<24} {}", m["id"].as_str().unwrap_or(""), m["description"].as_str().unwrap_or(""));
            }
            Ok(())
        }
        "project" => {
            if a.pos(0, "subcommand")? != "add" { return Err(USAGE.into()); }
            let path = a.pos(1, "project path")?;
            let mut params = json!({"repoPath": path});
            if let Some(id) = a.flag("id") { params["id"] = json!(id); }
            if let Some(name) = a.flag("name") { params["name"] = json!(name); }
            let r = call(&ensure_running()?, "project.add", params)?;
            println!("project {} at {} ({} members from .guild/guild.toml)", r["project"]["id"].as_str().unwrap_or(""), r["project"]["repo_path"].as_str().unwrap_or(""), r["membersFromConfig"]);
            Ok(())
        }
        "member" => {
            if a.pos(0, "subcommand")? != "add" { return Err(USAGE.into()); }
            let id = a.pos(1, "member id")?;
            let harness = a.flag("harness").ok_or("missing --harness")?;
            let roles = a.all("role");
            let r = call(&ensure_running()?, "member.add", json!({"id": id, "harness": harness, "roles": roles, "model": a.flag("model"), "mode": a.flag("mode"), "name": a.flag("name"), "prompt": a.flag("prompt")}))?;
            println!("member {} · {} · model {}", r["id"].as_str().unwrap_or(""), r["harness"].as_str().unwrap_or(""), r["model_id"].as_str().unwrap_or("(harness default)"));
            Ok(())
        }
        "quest" => {
            if a.pos(0, "subcommand")? != "new" { return Err(USAGE.into()); }
            let r = call(&ensure_running()?, "quest.create", json!({"projectId": a.pos(1, "project")?, "title": a.pos(2, "title")?, "description": a.flag("description")}))?;
            println!("quest {} · {}", r["id"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or(""));
            Ok(())
        }
        "task" => {
            if a.pos(0, "subcommand")? != "new" { return Err(USAGE.into()); }
            let depends: Vec<String> = a.flag("depends").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            let params = json!({
                "questId": a.pos(1, "quest")?, "title": a.pos(2, "title")?, "instructions": a.flag("instructions"), "acceptance": a.all("accept"),
                "dependsOn": depends, "role": a.flag("role"), "memberId": a.flag("member"), "modelId": a.flag("model"), "id": a.flag("id"),
                "priority": a.flag("priority").and_then(|s| s.parse::<i64>().ok()), "maxAttempts": a.flag("max-attempts").and_then(|s| s.parse::<u64>().ok()),
            });
            let r = call(&ensure_running()?, "task.create", params)?;
            println!("task {} · {} ({})", r["id"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or(""), r["state"].as_str().unwrap_or(""));
            Ok(())
        }
        "start" | "pause" | "resume" | "cancel-quest" => {
            let method = match command { "start" => "quest.start", "pause" => "quest.pause", "resume" => "quest.resume", _ => "quest.cancel" };
            let r = call(&ensure_running()?, method, json!({"questId": a.pos(0, "quest")?}))?;
            println!("{}", r.get("state").and_then(Value::as_str).map(|s| format!("quest {s}")).unwrap_or_else(|| r.to_string()));
            Ok(())
        }
        "accept" => {
            let r = call(&ensure_running()?, "task.accept", json!({"taskId": a.pos(0, "task")?, "integrate": a.has("integrate")}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "reject" => {
            let r = call(&ensure_running()?, "task.reject", json!({"taskId": a.pos(0, "task")?, "reason": a.flag("reason")}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "integrate" => {
            let r = call(&ensure_running()?, "task.integrate", json!({"taskId": a.pos(0, "task")?}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "retry" => {
            let r = call(&ensure_running()?, "task.retry", json!({"taskId": a.pos(0, "task")?}))?;
            println!("task {} · attempt {} of {}", r["state"].as_str().unwrap_or(""), r["attempts_made"], r["max_attempts"]);
            Ok(())
        }
        "cancel-task" => {
            let r = call(&ensure_running()?, "task.cancel", json!({"taskId": a.pos(0, "task")?}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "cancel" => {
            let r = call(&ensure_running()?, "attempt.cancel", json!({"attemptId": a.pos(0, "attempt")?}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "interrupt" => {
            let r = call(&ensure_running()?, "attempt.interrupt", json!({"attemptId": a.pos(0, "attempt")?}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "permission" => {
            let attempt = a.pos(0, "attempt")?;
            let option = a.pos(1, "option id or deny")?;
            let client = ensure_running()?;
            let got = call(&client, "attempt.get", json!({"attemptId": attempt}))?;
            let request = got["attempt"]["pending_permission"].clone();
            if request.is_null() { return Err("the attempt is not waiting for a permission".into()); }
            let request_id = request["request_id"].as_str().unwrap_or("").to_string();
            let option_id = if option == "deny" {
                request["options"].as_array().into_iter().flatten().find(|o| o["kind"].as_str().unwrap_or("").starts_with("reject")).and_then(|o| o["id"].as_str()).map(str::to_string)
            } else {
                Some(option.to_string())
            };
            let r = call(&client, "permission.respond", json!({"attemptId": attempt, "requestId": request_id, "optionId": option_id}))?;
            println!("{}", summarize(&r));
            Ok(())
        }
        "attempt" => show_attempt(&ensure_running()?, a.pos(0, "attempt")?),
        "events" => events(&ensure_running()?, &a),
        _ => Err(format!("unknown command {command}\n\n{USAGE}")),
    }
}

fn summarize(v: &Value) -> String {
    match v.as_object() {
        Some(o) => o.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>().join(" · "),
        None => v.to_string(),
    }
}

fn short(id: &str) -> &str { if id.len() > 8 { &id[id.len() - 8..] } else { id } }

fn status(client: &Client) -> Result<(), String> {
    let snap: Snapshot = serde_json::from_value(call(client, "guild.snapshot", json!({}))?).map_err(|e| e.to_string())?;
    println!("guild {} · {} events · {}", snap.service_version, snap.last_seq, paths::home().display());
    for h in &snap.harnesses {
        println!("  harness {:<6} {} {}", h.kind, if h.available { "ready" } else { "unavailable" }, if h.available { h.runtime_version.clone() } else { h.detail.clone() });
    }
    if snap.members.is_empty() { println!("  no members (guild member add …)"); }
    for m in &snap.members {
        let active = snap.attempts.iter().find(|a| a.member_id == m.id && a.state.is_active());
        let task = active.and_then(|a| snap.tasks.iter().find(|t| t.id == a.task_id));
        println!("  member {:<10} {:<6} {:<22} {:<12} {}", m.name, m.harness, m.model_id.clone().unwrap_or_else(|| "(default)".into()), m.roles.join(","), match (active, task) {
            (Some(a), Some(t)) => format!("{:?} on {}", a.state, t.title),
            _ => "free".into(),
        });
    }
    for p in &snap.projects {
        println!("project {} · {} · {} verifiers", p.id, p.repo_path, p.verifiers.len());
    }
    for q in &snap.quests {
        let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
        let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
        println!("quest {} · {:?} · {}/{} tasks · {}", q.id, q.state, done, tasks.len(), q.title);
        for t in tasks {
            let attempt = snap.attempts.iter().filter(|a| a.task_id == t.id).max_by(|a, b| a.created_at.cmp(&b.created_at));
            let detail = match attempt {
                Some(a) => {
                    let mut d = format!("attempt {} {:?}", short(&a.id), a.state);
                    if let Some(c) = &a.candidate { d.push_str(&format!(" · {} files +{} -{}", c.files_changed, c.insertions, c.deletions)); }
                    if let Some(p) = &a.pending_permission { d.push_str(&format!(" · asks: {} [{}]", p.title, p.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>().join("|"))); }
                    for v in &a.verifier_results { d.push_str(&format!(" · {} {}", v.id, v.status)); }
                    if let Some(f) = &a.failure { d.push_str(&format!(" · {f}")); }
                    d
                }
                None => String::new(),
            };
            let deps = if t.depends_on.is_empty() { String::new() } else { format!(" (after {})", t.depends_on.join(", ")) };
            println!("  task {:<10} {:<12} {}{}{}", t.id, format!("{:?}", t.state).to_lowercase(), t.title, deps, if detail.is_empty() { String::new() } else { format!(" — {detail}") });
            if let Some(r) = &t.block_reason { println!("       {r}"); }
        }
    }
    Ok(())
}

fn show_attempt(client: &Client, id: &str) -> Result<(), String> {
    let got = call(client, "attempt.get", json!({"attemptId": id}))?;
    let attempt: Attempt = serde_json::from_value(got["attempt"].clone()).map_err(|e| e.to_string())?;
    println!("attempt {} · {:?} · member {} · {} · model {} → {}", attempt.id, attempt.state, attempt.member_id, attempt.harness, attempt.model_requested.clone().unwrap_or_else(|| "default".into()), attempt.model_effective.clone().unwrap_or_else(|| "?".into()));
    if let Some(w) = got["workspace"].as_object() { println!("workspace {} on {}", w["path"].as_str().unwrap_or(""), w["branch"].as_str().unwrap_or("")); }
    if let Some(c) = &attempt.candidate { println!("candidate {}..{} · {} files +{} -{}", &c.base_sha[..8], &c.head_sha[..8], c.files_changed, c.insertions, c.deletions); }
    for v in &attempt.verifier_results {
        println!("verifier {} · {} · exit {:?} · {} ms", v.id, v.status, v.exit_code, v.duration_ms);
        for line in v.tail.lines() { println!("    {line}"); }
    }
    if let Some(f) = &attempt.failure { println!("failure: {f}"); }
    if !attempt.summary.is_empty() { println!("summary: {}", attempt.summary); }
    println!("--- transcript ---");
    let events = call(client, "events.attempt", json!({"attemptId": id, "limit": 2000}))?;
    for e in events.as_array().into_iter().flatten() {
        print_event(e);
    }
    Ok(())
}

fn print_event(e: &Value) {
    let kind = e["type"].as_str().unwrap_or("");
    let at = e["occurred_at"].as_str().unwrap_or("").get(11..19).unwrap_or("");
    let p = &e["payload"];
    let line = match kind {
        "turn.text" => p["text"].as_str().unwrap_or("").trim().to_string(),
        "tool.started" => format!("⚒ {} ({})", p["title"].as_str().unwrap_or(""), p["action"].as_str().unwrap_or("")),
        "tool.completed" => format!("  ↳ {} {}", p["title"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or("")),
        "permission.requested" => format!("? {} {}", p["title"].as_str().unwrap_or(""), p["options"].as_array().map(|a| a.iter().map(|o| o["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("|")).unwrap_or_default()),
        "verifier.finished" => format!("✓ verifier {} {}", p["verifierId"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or("")),
        _ => {
            let mut s = p.as_object().map(|o| o.iter().filter(|(k, _)| *k != "text").map(|(k, v)| format!("{k}={}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>().join(" ")).unwrap_or_default();
            s.truncate(160);
            s
        }
    };
    if kind == "turn.text" {
        for l in line.lines() { println!("{at}          {l}"); }
    } else {
        println!("{at} {:<22} {line}", kind);
    }
}

fn events(client: &Client, a: &Args) -> Result<(), String> {
    if let Some(attempt) = a.flag("attempt") {
        let events = call(client, "events.attempt", json!({"attemptId": attempt, "limit": 2000}))?;
        for e in events.as_array().into_iter().flatten() { print_event(e); }
        return Ok(());
    }
    let after: u64 = a.flag("after").and_then(|s| s.parse().ok()).unwrap_or(0);
    if !a.has("follow") {
        let events = call(client, "events.after", json!({"afterSeq": after, "limit": 500}))?;
        for e in events.as_array().into_iter().flatten() {
            print!("{:>6} ", e["seq"]);
            print_event(e);
        }
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _handle = client.subscribe(after, 1, tx).map_err(|e| e.to_string())?;
    loop {
        match rx.recv() {
            Ok((_, StreamMessage::Open { last_seq })) => println!("following from {last_seq}"),
            Ok((_, StreamMessage::Event(ev))) => {
                print!("{:>6} ", ev.seq);
                print_event(&serde_json::to_value(&ev).unwrap());
            }
            Ok((_, StreamMessage::Close)) | Err(_) => return Ok(()),
        }
    }
}

#[allow(dead_code)]
fn _config_is_linked() { let _ = config::default_role_prompt; }
