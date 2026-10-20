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
  guild mcp                          an MCP server on stdio, so Claude Code orchestrates the guild
  guild status                       the guild at a glance
  guild snapshot                     the whole state as JSON
  guild stop                         stop the service

  guild project add <path> [--id ID] [--name NAME] [--target-ref BRANCH]
                                     the branch quests start from and integrate into (default: the checkout's)
  guild found <path> [--name NAME] [--template videogame|blank|none] [--member NAME:ROLE:HARNESS[:MODEL]]...
  guild roles                        the roles of the catalogue
  guild design <project> [get|template|export]   the game design: show it, start from the template, write game-design/ into the repo
  guild member add <id> --harness kiro|codex|claude|mock [--role R]... [--model M] [--mode MODE] [--name N]
  guild models [kiro|codex|claude|mock]  the models the harness announces

  guild quest new <project> <title> [--description TEXT]
  guild task new <quest> <title> [--instructions TEXT] [--accept TEXT]... [--depends ID,ID]
                                     [--role R] [--member ID] [--model M] [--priority N] [--max-attempts N] [--id ID]
                                     [--strategy single|reviewed|race] [--candidates N] [--select policy|judge|user] [--auto-integrate]
                                     a race runs N members at once; the selection picks among the candidates that passed the verifiers
  guild decisions <task>             how each candidate was chosen: by the policy, a judge or you
  guild question new <project> --question TEXT [--context TEXT] [--system ID]
  guild question resolve <project> <Q-id> --decision TEXT [--why TEXT] [--answer TEXT] [--evidence TEXT]... [--rejected TEXT]...
  guild questions <project>          open and resolved questions, with their decisions
  guild decision new <project> --decision TEXT [--why TEXT] [--question Q-id]... [--system ID]... [--evidence TEXT]... [--rejected TEXT]...
  guild design <project> system set <id> --name N [--maturity undefined|concept|hypothesis|prototyping|validated|production|revised] [--intent TEXT] [--current TEXT] [--known TEXT]... [--unknown TEXT]...
                                     [--formula TEXT]... [--knob TEXT]... [--edge TEXT]... [--accept TEXT]... [--depends ID]...
                                     production needs the current design, edge cases, acceptance criteria and no unknowns
  guild hypothesis new <project> --problem TEXT --hypothesis TEXT [--level whole|loop|part] [--evidence TEXT] [--question Q-id]
                                     [--expected TEXT] [--change TEXT] [--measure TEXT]... [--system NAME]...
  guild experiment new <project> <hypothesis> [--title T] --variant id[:name][:key=value,...]... [--instructions TEXT]
                                     [--profiles a,b,c] [--runs N] [--seeds 1,2,3]
  guild experiment start|show|accept|reject|iterate <experiment> [--variant ID] [--note TEXT]
  guild playtests <experiment>       every playtest of the experiment, with its metrics
  guild play <project> [--experiment E --variant V] [--ref REF] [--player NAME]
                                     play the build here; the session ends when the game does
  guild play finish <playtest>       close a session the dungeon or another terminal started
  guild observe <project> --kind confusing|frustrating|interesting|great_moment|bug|comment [--text T] [--playtest P]
  guild observations <project>       what humans noticed, and the questions it raised
  guild observation promote <project> <OBS-id> [--question T] [--system ID]
  guild signals <project> [--experiment E] [--variant V] [--kind human|synthetic]
  guild ai-play <project> <member> [--persona player|novice|optimizer|explorer|exploit_hunter|<free text>]
                [--seed N] [--turns N] [--experiment E --variant V] [--ref REF] [--detach]
                                     a member plays the build through the game's agent protocol ([ai_play])
  guild ai-stop <playtest>           stop an AI player between moves
  guild findings <project>           what the exploit hunter broke, with its replay and status
  guild finding verify <project> <F-id> [--ref REF]   replay its moves on its build (or REF): reproduced, fixed…
  guild finding promote <project> <F-id> [--question T] [--system ID]
  guild finding set <project> <F-id> <unverified|reproduced|not_reproduced|fixed|wont_fix>
  guild records [--project ID] [--task ID]   one row per finished attempt: strategy, member, harness, model, duration, size, verifiers, judge score, winner
  guild start <quest>  |  guild pause <quest>  |  guild resume <quest>  |  guild cancel-quest <quest>

  guild accept <task> [--integrate] [--attempt ID]  the latest (or that) candidate passes; --integrate squashes it into the quest branch
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
  CODEX_BIN          codex binary        CLAUDE_BIN  claude binary
";

fn run_with_env(args: &[String]) -> ExitCode {
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (vars, rest) = args.split_at(split);
    let rest = rest.get(1..).unwrap_or(&[]);
    let Some(program) = rest.first() else {
        eprintln!("usage: env K=V… -- program [args…]");
        return ExitCode::FAILURE;
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&rest[1..]);
    for var in vars {
        if let Some((key, value)) = var.split_once('=') { cmd.env(key, value); }
    }
    match cmd.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("error: cannot start {program}: {e}");
            ExitCode::FAILURE
        }
    }
}

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
    if args[0] == "env" {
        return run_with_env(&args[1..]);
    }
    let result = match args[0].as_str() {
        "serve" => serve(&args[1..]),
        // An MCP server on stdio, for Claude Code: `claude mcp add guild -- <this binary> mcp`.
        "mcp" => ensure_running().map(|client| guild::mcp::serve_stdio(&|method, params| call(&client, method, params))),
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

fn is_boolean_flag(name: &str) -> bool { matches!(name, "daemon" | "follow" | "integrate" | "json" | "foreground" | "detach") }

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
        // The daemon gets the same settings this command was given.
        let mut forward: Vec<String> = vec![];
        for key in ["concurrency", "attempt-timeout"] {
            if let Some(v) = a.flag(key) { forward.push(format!("--{key}")); forward.push(v.to_string()); }
        }
        return daemonize(&forward);
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
    service.register(Arc::new(guild::harness::codex_exec::CodexExec::new()));
    service.register(Arc::new(guild::harness::claude_stream::ClaudeStream::new()));
    service.register(Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "GUILD_MOCK.md".into(), content: "The mock harness was here.\n".into() })));
    service.log(&format!("guild service {} starting, home {}", service::SERVICE_VERSION, paths::home().display()));
    let result = service::run(service.clone(), &paths::socket_path()).map_err(|e| e.to_string());
    service.log("guild service stopped");
    drop(lock);
    result
}

/// One service per home: an exclusive lock on a lock file.
fn acquire_lock() -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new().create(true).write(true).open(paths::lock_path()).map_err(|e| e.to_string())?;
    if file.try_lock().is_err() {
        return Err(format!("another guild service already owns {}", paths::home().display()));
    }
    Ok(file)
}

fn daemonize(forward: &[String]) -> Result<(), String> {
    paths::ensure_home().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_path()).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve").args(forward).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::from(log.try_clone().map_err(|e| e.to_string())?)).stderr(std::process::Stdio::from(log));
    guild::platform::keep_stdio_private();
    guild::platform::detach(&mut cmd);
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
    daemonize(&[])?;
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
            if let Some(r) = a.flag("target-ref") { params["targetRef"] = json!(if r.starts_with("refs/") || r == "HEAD" { r.to_string() } else { format!("refs/heads/{r}") }); }
            let r = call(&ensure_running()?, "project.add", params)?;
            println!("project {} at {} ({} members from .guild/guild.toml)", r["project"]["id"].as_str().unwrap_or(""), r["project"]["repo_path"].as_str().unwrap_or(""), r["membersFromConfig"]);
            Ok(())
        }
        "found" => {
            let members: Vec<Value> = a.all("member").iter().map(|m| {
                let parts: Vec<&str> = m.splitn(4, ':').collect();
                json!({"name": parts.first().unwrap_or(&""), "role": parts.get(1).unwrap_or(&"implementer"), "harness": parts.get(2).unwrap_or(&"kiro"), "model": parts.get(3)})
            }).collect();
            let mut params = json!({"repoPath": a.pos(0, "project path")?, "template": a.flag("template").unwrap_or("videogame"), "members": members});
            if let Some(name) = a.flag("name") { params["name"] = json!(name); }
            let r = call(&ensure_running()?, "guild.found", params)?;
            println!("guild {} · {} members · design {}", r["project"]["id"].as_str().unwrap_or(""), r["members"].as_array().map(Vec::len).unwrap_or(0), if r["design"].is_null() { "none" } else { "from template" });
            Ok(())
        }
        "roles" => {
            let r = call(&ensure_running()?, "roles.list", json!({}))?;
            for role in r.as_array().into_iter().flatten() {
                println!("{:<18} {:<13} {}", role["id"].as_str().unwrap_or(""), role["guild"].as_str().unwrap_or(""), role["name"].as_str().unwrap_or(""));
            }
            Ok(())
        }
        "design" => {
            let project = a.pos(0, "project")?;
            match a.positional.get(1).copied().unwrap_or("get") {
                "system" => {
                    if a.positional.get(2).copied() != Some("set") { return Err(USAGE.into()); }
                    let id = a.pos(3, "system id")?;
                    let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
                    let mut system = snap["designs"].as_array().into_iter().flatten().find(|d| d["project_id"] == project).and_then(|d| d["systems"].as_array()).into_iter().flatten().find(|s| s["id"] == id).cloned().unwrap_or(json!({"id": id, "name": id}));
                    if let Some(n) = a.flag("name") { system["name"] = json!(n); }
                    if let Some(m) = a.flag("maturity") { system["maturity"] = json!(m); }
                    if let Some(t) = a.flag("intent") { system["intent"] = json!(t); }
                    if let Some(t) = a.flag("current") { system["current"] = json!(t); }
                    if !a.all("known").is_empty() { system["known"] = json!(a.all("known")); }
                    if !a.all("unknown").is_empty() { system["unknown"] = json!(a.all("unknown")); }
                    for (flag, key) in [("formula", "formulas"), ("knob", "tuning"), ("edge", "edge_cases"), ("accept", "acceptance"), ("depends", "depends_on")] {
                        if !a.all(flag).is_empty() { system[key] = json!(a.all(flag)); }
                    }
                    let r = call(&ensure_running()?, "design.system.set", json!({"projectId": project, "system": system}))?;
                    println!("design v{} · {} systems", r["version"], r["systems"].as_array().map(Vec::len).unwrap_or(0));
                }
                "template" => { let r = call(&ensure_running()?, "design.template", json!({"projectId": project, "template": a.flag("template").unwrap_or("videogame")}))?; println!("design v{} · {} loops · {} parts", r["version"], r["loops"].as_array().map(Vec::len).unwrap_or(0), r["parts"].as_array().map(Vec::len).unwrap_or(0)); }
                "export" => { let r = call(&ensure_running()?, "design.export", json!({"projectId": project}))?; for f in r["files"].as_array().into_iter().flatten() { println!("{}", f.as_str().unwrap_or("")); } }
                _ => { let r = call(&ensure_running()?, "design.get", json!({"projectId": project}))?; println!("{}", serde_json::to_string_pretty(&r).unwrap()); }
            }
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
                "candidates": a.flag("candidates").and_then(|s| s.parse::<u64>().ok()), "autoIntegrate": a.has("auto-integrate"),
                "strategy": a.flag("strategy"), "selection": a.flag("select"),
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
            let r = call(&ensure_running()?, "task.accept", json!({"taskId": a.pos(0, "task")?, "integrate": a.has("integrate"), "attemptId": a.flag("attempt")}))?;
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
        "records" => {
            let rows = call(&ensure_running()?, "records.list", json!({"projectId": a.flag("project"), "taskId": a.flag("task")}))?;
            println!("{:<10} {:<22} {:<16} {:<8} {:<7} {:<22} {:>7} {:>5} {:>4} {:>4} {:>5} {:>5} {:>5} {:<8}", "kind", "task", "strategy", "member", "harness", "model", "ms", "turns", "res", "files", "±", "pass", "score", "result");
            for r in rows.as_array().into_iter().flatten() {
                let size = format!("{}/{}", r["insertions"].as_u64().unwrap_or(0), r["deletions"].as_u64().unwrap_or(0));
                let win = match r["winner"] { Value::Bool(true) => " ★", Value::Bool(false) => " ✗", _ => "" };
                println!("{:<10} {:<22} {:<16} {:<8} {:<7} {:<22} {:>7} {:>5} {:>4} {:>4} {:>5} {:>5} {:>5} {:<8}{}", r["kind"].as_str().unwrap_or(""), r["task_title"].as_str().unwrap_or("").chars().take(22).collect::<String>(), format!("{}/{}", r["strategy"].as_str().unwrap_or(""), r["selection"].as_str().unwrap_or("")), r["member_id"].as_str().unwrap_or(""), r["harness"].as_str().unwrap_or(""), r["model"].as_str().unwrap_or("-").chars().take(22).collect::<String>(), r["duration_ms"].as_i64().unwrap_or(0), r["turns"], r["resumed"], r["files_changed"].as_u64().unwrap_or(0), size, r["verifiers_passed"], r["judge_score"].as_f64().map(|s| format!("{s:.0}")).unwrap_or("-".into()), r["result"].as_str().unwrap_or(""), win);
            }
            Ok(())
        }
        "question" => {
            match a.pos(0, "subcommand")? {
                "new" => {
                    let r = call(&ensure_running()?, "question.create", json!({"projectId": a.pos(1, "project")?, "question": a.flag("question"), "context": a.flag("context"), "systemId": a.flag("system")}))?;
                    println!("{} · {}", r["id"].as_str().unwrap_or(""), r["question"].as_str().unwrap_or(""));
                    Ok(())
                }
                "resolve" => {
                    let r = call(&ensure_running()?, "question.resolve", json!({"projectId": a.pos(1, "project")?, "questionId": a.pos(2, "question id")?, "decision": a.flag("decision"), "why": a.flag("why"), "answer": a.flag("answer"), "evidence": a.all("evidence"), "rejectedAlternatives": a.all("rejected")}))?;
                    println!("{} resolved by {} · {}", r["question"]["id"].as_str().unwrap_or(""), r["decision"]["id"].as_str().unwrap_or(""), r["decision"]["decision"].as_str().unwrap_or(""));
                    Ok(())
                }
                _ => Err(USAGE.into()),
            }
        }
        "questions" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            let project = a.pos(0, "project")?;
            for q in snap["questions"].as_array().into_iter().flatten().filter(|q| q["project_id"] == project) {
                println!("{} [{}] {}{}", q["id"].as_str().unwrap_or(""), q["status"].as_str().unwrap_or(""), q["question"].as_str().unwrap_or(""), q["decision_id"].as_str().map(|d| format!(" → {d}")).unwrap_or_default());
            }
            for d in snap["design_decisions"].as_array().into_iter().flatten().filter(|d| d["project_id"] == project) {
                println!("{} {} · {}", d["id"].as_str().unwrap_or(""), d["decision"].as_str().unwrap_or(""), d["why"].as_str().unwrap_or(""));
            }
            Ok(())
        }
        "decision" => {
            if a.pos(0, "subcommand")? != "new" { return Err(USAGE.into()); }
            let r = call(&ensure_running()?, "decision.create", json!({"projectId": a.pos(1, "project")?, "decision": a.flag("decision"), "why": a.flag("why"), "questionIds": a.all("question"), "systemIds": a.all("system"), "evidence": a.all("evidence"), "rejectedAlternatives": a.all("rejected")}))?;
            println!("{} · {}", r["id"].as_str().unwrap_or(""), r["decision"].as_str().unwrap_or(""));
            Ok(())
        }
        "hypothesis" => {
            if a.pos(0, "subcommand")? != "new" { return Err(USAGE.into()); }
            let r = call(&ensure_running()?, "hypothesis.create", json!({"projectId": a.pos(1, "project")?, "problem": a.flag("problem"), "hypothesis": a.flag("hypothesis"), "level": a.flag("level").unwrap_or("loop"),
                "evidence": a.flag("evidence"), "expectedEffect": a.flag("expected"), "proposedChange": a.flag("change"), "measurements": a.all("measure"), "relatedSystems": a.all("system"), "questionId": a.flag("question")}))?;
            println!("hypothesis {} · {} · {}", r["id"].as_str().unwrap_or(""), r["level"].as_str().unwrap_or(""), r["problem"].as_str().unwrap_or(""));
            Ok(())
        }
        "experiment" => {
            let sub = a.pos(0, "subcommand")?;
            match sub {
                "new" => {
                    let variants: Vec<Value> = a.all("variant").iter().map(|spec| {
                        let mut parts = spec.splitn(3, ':');
                        let id = parts.next().unwrap_or("").to_string();
                        let name = parts.next().unwrap_or("").to_string();
                        let params: serde_json::Map<String, Value> = parts.next().unwrap_or("").split(',').filter(|kv| kv.contains('=')).map(|kv| {
                            let (k, v) = kv.split_once('=').unwrap();
                            (k.trim().to_string(), v.trim().parse::<f64>().map(Value::from).unwrap_or_else(|_| Value::from(v.trim())))
                        }).collect();
                        json!({"id": id, "name": if name.is_empty() { id.clone() } else { name }, "params": params, "instructions": a.flag("instructions").unwrap_or("")})
                    }).collect();
                    let mut plan = json!({});
                    if let Some(p) = a.flag("profiles") { plan["profiles"] = json!(p.split(',').map(str::trim).collect::<Vec<_>>()); }
                    if let Some(n) = a.flag("runs").and_then(|s| s.parse::<u64>().ok()) { plan["runs_per_profile"] = json!(n); }
                    if let Some(s) = a.flag("seeds") { plan["seeds"] = json!(s.split(',').filter_map(|x| x.trim().parse::<u64>().ok()).collect::<Vec<_>>()); }
                    let mut params = json!({"projectId": a.pos(1, "project")?, "hypothesisId": a.pos(2, "hypothesis")?, "variants": variants, "plan": plan});
                    if let Some(t) = a.flag("title") { params["title"] = json!(t); }
                    let r = call(&ensure_running()?, "experiment.create", params)?;
                    println!("experiment {} · {} · {} variants", r["id"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or(""), r["variants"].as_array().map(Vec::len).unwrap_or(0));
                    Ok(())
                }
                "start" => { let r = call(&ensure_running()?, "experiment.start", json!({"experimentId": a.pos(1, "experiment")?}))?; println!("experiment {} · quest {}", r["status"].as_str().unwrap_or(""), r["quest_id"].as_str().unwrap_or("")); Ok(()) }
                "accept" | "reject" | "iterate" => {
                    let r = call(&ensure_running()?, &format!("experiment.{sub}"), json!({"experimentId": a.pos(1, "experiment")?, "variantId": a.flag("variant"), "note": a.flag("note")}))?;
                    println!("experiment {} · {}", r["experiment"]["status"].as_str().unwrap_or(""), r["integrated"]);
                    Ok(())
                }
                _ => {
                    let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
                    let id = a.pos(1, "experiment")?;
                    let e = snap["experiments"].as_array().into_iter().flatten().find(|e| e["id"] == id || e["id"].as_str().map(|s| s.ends_with(id)).unwrap_or(false)).ok_or_else(|| format!("no experiment {id}"))?;
                    println!("{}", serde_json::to_string_pretty(e).unwrap());
                    Ok(())
                }
            }
        }
        "play" => {
            let client = ensure_running()?;
            if a.positional.first().copied() == Some("finish") {
                let r = call(&client, "play.finish", json!({"playtestId": a.pos(1, "playtest")?}))?;
                println!("playtest {} · {} events · {}", r["id"].as_str().unwrap_or(""), r["event_count"], r["status"].as_str().unwrap_or(""));
                return Ok(());
            }
            let r = call(&client, "play.start", json!({"projectId": a.pos(0, "project")?, "experimentId": a.flag("experiment"), "variantId": a.flag("variant"), "ref": a.flag("ref"), "player": a.flag("player")}))?;
            let argv: Vec<String> = serde_json::from_value(r["argv"].clone()).map_err(|e| e.to_string())?;
            let pid = r["playtest"]["id"].as_str().unwrap_or("").to_string();
            eprintln!("playing {} in {} (playtest {pid}); `guild observe` from another terminal notes what you see", r["playtest"]["build_sha"].as_str().map(|s| &s[..s.len().min(8)]).unwrap_or("?"), r["cwd"].as_str().unwrap_or(""));
            let status = std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(r["cwd"].as_str().unwrap_or(".")).env("GUILD_TELEMETRY", r["env"]["GUILD_TELEMETRY"].as_str().unwrap_or("")).env("GUILD_PLAYTEST", &pid).status();
            if let Err(e) = status { eprintln!("could not start the game: {e}"); }
            let done = call(&client, "play.finish", json!({"playtestId": pid}))?;
            println!("playtest {pid} · {} events · {}", done["event_count"], done["status"].as_str().unwrap_or(""));
            Ok(())
        }
        "observe" => {
            let r = call(&ensure_running()?, "observation.create", json!({"projectId": a.pos(0, "project")?, "kind": a.flag("kind").unwrap_or("comment"), "text": a.flag("text"), "playtestId": a.flag("playtest")}))?;
            println!("{} [{}] {}{}", r["id"].as_str().unwrap_or(""), r["kind"].as_str().unwrap_or(""), r["text"].as_str().unwrap_or(""), r["round"].as_u64().map(|n| format!(" · round {n}")).unwrap_or_default());
            Ok(())
        }
        "observations" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            let project = a.pos(0, "project")?;
            for o in snap["observations"].as_array().into_iter().flatten().filter(|o| o["project_id"] == project) {
                println!("{} [{}] {}{}{}", o["id"].as_str().unwrap_or(""), o["kind"].as_str().unwrap_or(""), o["text"].as_str().unwrap_or(""), o["round"].as_u64().map(|n| format!(" · round {n}")).unwrap_or_default(), o["question_id"].as_str().map(|q| format!(" → {q}")).unwrap_or_default());
            }
            Ok(())
        }
        "observation" => {
            if a.pos(0, "subcommand")? != "promote" { return Err(USAGE.into()); }
            let r = call(&ensure_running()?, "observation.promote", json!({"projectId": a.pos(1, "project")?, "observationId": a.pos(2, "observation")?, "question": a.flag("question"), "systemId": a.flag("system")}))?;
            println!("{} → {} · {}", r["observation"]["id"].as_str().unwrap_or(""), r["question"]["id"].as_str().unwrap_or(""), r["question"]["question"].as_str().unwrap_or(""));
            Ok(())
        }
        "ai-play" => {
            let socket = ensure_running()?;
            let seed = a.flag("seed").map(|s| s.parse::<u64>().map_err(|_| "--seed takes a number".to_string())).transpose()?;
            let turns = a.flag("turns").map(|s| s.parse::<u64>().map_err(|_| "--turns takes a number".to_string())).transpose()?;
            let pt = call(&socket, "ai.play", json!({"projectId": a.pos(0, "project")?, "memberId": a.pos(1, "member")?, "persona": a.flag("persona"), "seed": seed, "maxTurns": turns, "experimentId": a.flag("experiment"), "variantId": a.flag("variant"), "ref": a.flag("ref")}))?;
            let id = pt["id"].as_str().unwrap_or("").to_string();
            println!("{} plays {} as {} (seed {}, playtest {id})", pt["player"].as_str().unwrap_or("?"), pt["build_sha"].as_str().map(|s| &s[..s.len().min(8)]).unwrap_or("?"), pt["profile"].as_str().unwrap_or(""), pt["seed"]);
            if a.has("detach") { return Ok(()); }
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let snap = call(&socket, "guild.snapshot", json!({}))?;
                let Some(p) = snap["playtests"].as_array().into_iter().flatten().find(|p| p["id"] == id.as_str()) else { continue };
                if p["status"] == "running" { continue; }
                println!("{} · {} events · {} moves{}", p["status"].as_str().unwrap_or(""), p["event_count"], p["metrics"]["ai"]["turns"], p["failure"].as_str().map(|f| format!(" · {f}")).unwrap_or_default());
                if let Some(r) = p["metrics"]["runs"].as_object() { println!("runs: {} completed of {}", r.get("completed").cloned().unwrap_or_default(), r.get("started").cloned().unwrap_or_default()); }
                if let Some(t) = p["observations"].as_str().filter(|t| !t.is_empty()) { println!("it says: {t}"); }
                for f in snap["findings"].as_array().into_iter().flatten().filter(|f| f["playtest_id"] == id.as_str()) {
                    println!("{} [{}] {} · round {} · replay: seed {} moves {}", f["id"].as_str().unwrap_or(""), f["severity"].as_str().unwrap_or(""), f["title"].as_str().unwrap_or(""), f["round"], f["seed"], f["actions"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default());
                }
                return Ok(());
            }
        }
        "ai-stop" => {
            let r = call(&ensure_running()?, "ai.stop", json!({"playtestId": a.pos(0, "playtest")?}))?;
            println!("{}", if r["stopping"] == true { "stopping after the current move" } else { "it is not playing" });
            Ok(())
        }
        "findings" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            let project = a.pos(0, "project")?;
            for f in snap["findings"].as_array().into_iter().flatten().filter(|f| f["project_id"] == project) {
                println!("{} [{} · {}] {}{}", f["id"].as_str().unwrap_or(""), f["severity"].as_str().unwrap_or(""), f["status"].as_str().unwrap_or(""), f["title"].as_str().unwrap_or(""), f["question_id"].as_str().map(|q| format!(" → {q}")).unwrap_or_default());
                if let Some(i) = f["impact"].as_str().filter(|i| !i.is_empty()) { println!("    impact: {i}"); }
                println!("    replay: build {} seed {} moves {}", f["build_sha"].as_str().map(|s| &s[..s.len().min(8)]).unwrap_or("?"), f["seed"], f["actions"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default());
                if let Some(v) = f["verification"].as_str() { println!("    {v}"); }
            }
            Ok(())
        }
        "finding" => {
            let socket = ensure_running()?;
            let (project, finding) = (a.pos(1, "project")?, a.pos(2, "finding")?);
            let f = match a.pos(0, "subcommand")? {
                "verify" => call(&socket, "finding.verify", json!({"projectId": project, "findingId": finding, "ref": a.flag("ref")}))?,
                "promote" => {
                    let r = call(&socket, "finding.promote", json!({"projectId": project, "findingId": finding, "question": a.flag("question"), "systemId": a.flag("system")}))?;
                    println!("{} → {} · {}", finding, r["question"]["id"].as_str().unwrap_or(""), r["question"]["question"].as_str().unwrap_or(""));
                    return Ok(());
                }
                "set" => call(&socket, "finding.set", json!({"projectId": project, "findingId": finding, "status": a.pos(3, "status")?}))?,
                _ => return Err(USAGE.into()),
            };
            println!("{} · {}{}", f["id"].as_str().unwrap_or(""), f["status"].as_str().unwrap_or(""), f["verification"].as_str().map(|v| format!("\n{v}")).unwrap_or_default());
            Ok(())
        }
        "signals" => {
            let r = call(&ensure_running()?, "signals", json!({"projectId": a.pos(0, "project")?, "experimentId": a.flag("experiment"), "variantId": a.flag("variant"), "kind": a.flag("kind")}))?;
            println!("{} playtests ({} human, {} synthetic)", r["playtests"], r["human"], r["synthetic"]);
            for s in r["signals"].as_array().into_iter().flatten() { println!("· {}\n    → {}", s["statement"].as_str().unwrap_or(""), s["suggested_question"].as_str().unwrap_or("")); }
            for d in r["desired"].as_array().into_iter().flatten() { println!("  desired: {}", d.as_str().unwrap_or("")); }
            for d in r["avoid"].as_array().into_iter().flatten() { println!("  avoid:   {}", d.as_str().unwrap_or("")); }
            Ok(())
        }
        "playtests" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            let id = a.pos(0, "experiment")?;
            for p in snap["playtests"].as_array().into_iter().flatten().filter(|p| p["experiment_id"] == id || p["experiment_id"].as_str().map(|s| s.ends_with(id)).unwrap_or(false)) {
                println!("{:<10} {:<10} seed {:<3} {:<9} {:>4} events · runs {}/{} · top {} ({:.0}%) · {}", p["variant_id"].as_str().unwrap_or(""), p["profile"].as_str().unwrap_or(""), p["seed"], p["status"].as_str().unwrap_or(""), p["event_count"], p["metrics"]["runs"]["completed"], p["metrics"]["runs"]["started"], p["metrics"]["choices"]["top"].as_str().unwrap_or("-"), p["metrics"]["choices"]["top_share"].as_f64().unwrap_or(0.0) * 100.0, p["failure"].as_str().unwrap_or(""));
            }
            Ok(())
        }
        "decisions" => {
            let snap = call(&ensure_running()?, "guild.snapshot", json!({}))?;
            let task = a.pos(0, "task")?;
            for d in snap["decisions"].as_array().into_iter().flatten().filter(|d| d["task_id"] == task) {
                println!("{} by {} · winner {} · confidence {} · {}", d["created_at"].as_str().unwrap_or(""), d["by"].as_str().unwrap_or(""), d["winner_attempt_id"].as_str().unwrap_or("none"), d["confidence"], d["reason"].as_str().unwrap_or(""));
                for c in d["candidates"].as_array().into_iter().flatten() {
                    println!("  {} {:<10} gates {} · +{} -{} · score {}", c["label"].as_str().unwrap_or(""), c["member_id"].as_str().unwrap_or(""), c["verifiers_passed"], c["insertions"], c["deletions"], c["score"]);
                }
                if d["human_override"] == true { println!("  (the user chose against an earlier decision)"); }
            }
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
            guild::domain::cut(&mut s, 160);
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
