//! The checks `--self-test` and `cargo test` run: everything that works
//! without a window.

use crate::alerts::*;
use crate::assets::{decode_png, HEROES_PNG, ROOM_FILES};
use crate::herdr::*;
use crate::l10n;
use crate::monitor::Monitor;
use crate::prefs::Prefs;
use crate::scene::{columns_fitting, width_for, STYLES};
use crate::standalone::{claude_project_dir, claude_view, codex_view, identify};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

fn agent(id: &str, name: &str, status: &str, project: &str, activity: &str, cwd: &str) -> Agent {
    Agent::new(id, name, status, project, activity, cwd)
}

pub fn run() {
    let sample = br#"{"result":{"snapshot":{"agents":[{"pane_id":"a","agent":"claude","agent_status":"idle","workspace_id":"w"},{"pane_id":"b","agent":"codex","agent_status":"blocked"},{"pane_id":"shell"}],"workspaces":[{"workspace_id":"w","label":"Example"}]}}}"#;
    let agents = decode_snapshot(sample).expect("snapshot");
    assert!(agents.len() == 2 && agents[0].status == "blocked" && agents[1].project == "Example");
    assert!(decode_snapshot(b"{}").is_err(), "Invalid snapshot accepted");
    assert!(decode_snapshot(br#"{"result":{"snapshot":{"agents":[]}}}"#).unwrap().is_empty());
    let mut monitor = Monitor::new("default".into(), StatusFilter::All);
    monitor.apply(demo_agents(0));
    monitor.apply(demo_agents(10));
    assert!(monitor.events.iter().any(|e| e.text.contains(&l10n::tr("Necesita atención"))));
    monitor.apply(vec![]);
    assert!(monitor.agents.is_empty());
    for (name, bytes) in ROOM_FILES { assert!(decode_png(bytes).is_some(), "Unreadable asset: {name}"); }
    let heroes = decode_png(HEROES_PNG).expect("heroes sheet");
    assert_eq!(heroes.size, [48, HERO_ORDER.len() * 32], "Hero sheet layout mismatch");
    for style in STYLES { assert!(ROOM_FILES.iter().any(|(n, _)| *n == style.background), "Missing room art: {}", style.background); }
    let busy = agent("x", "claude", "idle", "p", "a", "~").with_subagents(2);
    assert!(busy.status == "working" && busy.subagents == 2);
    assert_eq!(agent("x", "claude", "blocked", "p", "a", "~").with_subagents(1).status, "blocked");
    let repo = std::env::temp_dir().join(format!("hpd-selftest-{}", std::process::id()));
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    assert_eq!(git_branch(&repo.join("src").to_string_lossy()).as_deref(), Some("feature/x"));
    let _ = std::fs::remove_dir_all(&repo);
    assert!(harness_for("claude") == "claude" && harness_for("Codex CLI") == "codex" && harness_for("opencode") == "hero");
    let with_session = decode_snapshot(br#"{"result":{"snapshot":{"agents":[{"pane_id":"a","agent":"claude","agent_session":{"kind":"id","value":"s-1"}}]}}}"#).unwrap();
    assert!(with_session[0].session.as_deref() == Some("s-1") && with_session[0].subagents == 0);
    assert!(tool_action("WebSearch") == "read" && tool_action("Edit") == "forge" && tool_action("Bash") == "brew" && tool_action("Task") == "summon" && tool_action("TodoWrite") == "plan" && tool_action("mcp__x") == "type");
    let tail = concat!(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"Grep","input":{}}]}}"#, "\n", r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#);
    assert_eq!(last_action(tail).as_deref(), Some("read"));
    assert_eq!(last_action(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#).as_deref(), Some("type"));
    assert_eq!(last_action(""), None);
    for (spanish, row) in l10n::TABLE.iter() {
        assert!(row.iter().all(|t| !t.is_empty()), "Missing translation: {spanish}");
        for language in l10n::LANGUAGES {
            let t = l10n::text(spanish, language);
            assert_eq!(t.matches("{}").count(), spanish.matches("{}").count(), "Format mismatch: {spanish} [{language}]");
        }
    }
    assert!(l10n::text("LISTO", "fr") == "TERMINÉ" && l10n::text("LISTO", "es") == "LISTO");
    let mut before: HashMap<String, Agent> = HashMap::new();
    before.insert("a".into(), agent("a", "claude", "working", "p", "a", "~"));
    assert_eq!(alert_for(&before, &[agent("a", "claude", "idle", "p", "a", "~")]), Some(AlertKind::Finished));
    assert_eq!(alert_for(&before, &[agent("a", "claude", "blocked", "p", "a", "~"), agent("b", "x", "done", "p", "a", "~")]), Some(AlertKind::NeedsHelp));
    before.insert("a".into(), agent("a", "claude", "idle", "p", "a", "~"));
    assert_eq!(alert_for(&before, &[agent("a", "claude", "idle", "p", "a", "~")]), None);
    let mut watcher = Monitor::new("default".into(), StatusFilter::All);
    watcher.apply(demo_agents(0));
    watcher.apply(demo_agents(10));
    assert_eq!(watcher.alerts.iter().map(|a| a.kind).collect::<Vec<_>>(), vec![AlertKind::NeedsHelp]);
    // Background shells: read from ps and the transcript; one that ends rings.
    let line = "69963 84548 Sun Oct 11 10:19:17 2026 /bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && eval 'cd /p && python3 emol.py fetch > data/fetch.log 2>&1' < /dev/null && pwd -P >| /tmp/claude-4330-cwd";
    let process = crate::shells::parse_ps_line(line).expect("ps line not read");
    assert_eq!((process.pid, process.ppid), (69963, 84548));
    assert_eq!(crate::shells::shell_command(&process.command).as_deref(), Some("cd /p && python3 emol.py fetch > data/fetch.log 2>&1"));
    assert_eq!(crate::shells::shell_command("node /x/cli.js serve"), None, "An MCP server was taken for a shell");
    assert_eq!(crate::shells::short_command("cd /p && python3 emol.py fetch"), "python3 emol.py fetch");
    let transcript = [
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 100","description":"Wait a while","run_in_background":true}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Command running in background with ID: b123. Output is being written to: /tmp/tasks/b123.output. You will be notified."}]},"toolUseResult":{"backgroundTaskId":"b123"}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t9","content":"<task-notification><task-id>bquoted</task-id><status>failed</status></task-notification>"}]}}"#,
        r#"{"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>\n<task-id>b123</task-id>\n<status>failed</status>\n<summary>Background command \"Wait a while\" failed (exit code 1)</summary>\n</task-notification>"}}"#,
    ].join("
");
    let launches = crate::shells::parse_launches(&transcript);
    assert_eq!(launches.by_command.get("sleep 100"), Some(&(Some("b123".into()), Some("Wait a while".into()), Some("/tmp/tasks/b123.output".into()))), "A background launch was not read");
    assert_eq!(launches.results.iter().map(|r| (r.task.as_str(), r.ok)).collect::<Vec<_>>(), vec![("b123", false)], "A quoted notification counted, or the real one was missed");
    let shell = crate::shells::Shell { pid: 7, command: "sleep 100".into(), label: "Wait a while".into(), started: 0, task: Some("b123".into()), last_line: None, output: None };
    let mut brewing = agent("s", "claude", "idle", "p", "a", "~");
    brewing.shells = vec![shell];
    let mut ended = agent("s", "claude", "idle", "p", "a", "~");
    ended.shell_results = launches.results.clone();
    let mut shells_watch = Monitor::new("default".into(), StatusFilter::All);
    shells_watch.apply(vec![brewing]);
    shells_watch.apply(vec![ended]);
    assert_eq!(shells_watch.alerts.iter().map(|a| (a.kind, a.shell.clone())).collect::<Vec<_>>(), vec![(AlertKind::NeedsHelp, Some(("Wait a while".to_string(), "Background command \"Wait a while\" failed (exit code 1)".to_string())))], "A failed shell did not ring");
    watcher.select(Some("demo:1".into()));
    watcher.apply(vec![]);
    assert!(watcher.selected.is_none());
    let path = std::env::temp_dir().join(format!("hpd-prefs-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut prefs = Prefs::load(path.clone());
    assert!(wants_sound(&prefs, AlertKind::NeedsHelp) && wants_sound(&prefs, AlertKind::Finished));
    prefs.sound_finished = false;
    prefs.save();
    let reread = Prefs::load(path.clone());
    assert!(wants_sound(&reread, AlertKind::NeedsHelp) && !wants_sound(&reread, AlertKind::Finished));
    prefs.sound_enabled = false;
    prefs.save();
    assert!(!wants_sound(&Prefs::load(path.clone()), AlertKind::NeedsHelp));
    let _ = std::fs::remove_file(&path);
    let mut notes = Prefs::load(path.clone());
    assert!(wants_notification(&notes, AlertKind::NeedsHelp) && !wants_notification(&notes, AlertKind::Finished));
    notes.notify_finished = true;
    notes.notify_needs_help = false;
    notes.save();
    let reread = Prefs::load(path.clone());
    assert!(wants_notification(&reread, AlertKind::Finished) && !wants_notification(&reread, AlertKind::NeedsHelp));
    notes.notify_enabled = false;
    notes.save();
    assert!(!wants_notification(&Prefs::load(path.clone()), AlertKind::Finished));
    let _ = std::fs::remove_file(&path);
    let mut previous = HashMap::new();
    previous.insert("a".to_string(), agent("a", "x", "working", "p", "", "~"));
    previous.insert("b".to_string(), agent("b", "y", "working", "p", "", "~"));
    let both = alerts_for(&previous, &[agent("a", "x", "done", "p", "", "~"), agent("b", "y", "blocked", "p", "", "~")]);
    assert_eq!(both.iter().map(|a| a.kind).collect::<Vec<_>>(), vec![AlertKind::NeedsHelp, AlertKind::Finished]);
    assert_eq!(both.iter().map(|a| a.agent.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
    assert!(jingle(&[(440.0, 0.1)], SAMPLE_RATE, 0.12).len() > 1000, "Jingle did not render");
    let screen = "✻ Worked for 17s · done 12:00 AM\n※ recap: Estamos mejorando el dungeon y ya sale un cuadro\n  nueva: subir tus commits de main en un PR\n────────\n❯ la 1, con PR borrador\n────────\n  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents";
    assert!(meaningful_lines(screen).is_empty(), "Chrome left in terminal tail: {:?}", meaningful_lines(screen));
    assert_eq!(meaningful_lines(" Do you want to proceed?\n❯ 1. Yes\n  2. No\n❯ hola").len(), 3, "Permission options were dropped");
    let prompt = crate::ansi::parse("❯ /model\n● listo\n────────\n❯ hola\n────────");
    assert_eq!((crate::ui::prompt_line(&prompt), crate::ui::picker_open(&prompt)), (Some(3), false), "Prompt line not found");
    // Kiro asking for a tool mid-conversation: its highlighted option has a ❯.
    for footer in ["esc to close · Enter to see more options", "esc to close · ↑↓ to navigate · ↵ to select · Tab to edit"] {
        let kiro = format!("• Read /Users/me/build_table.py\n  esc to cancel\n shell requires approval\n ❯ Yes, single permission\n   No (Tab to edit)\n {footer}");
        let lines = meaningful_lines(&kiro);
        let menu = crate::herdr::MenuOptions::new(&lines);
        assert_eq!(menu.options.iter().map(|o| o.text.as_str()).collect::<Vec<_>>(), vec!["Yes, single permission", "No (Tab to edit)"], "Kiro's approval options not read: {lines:?}");
        assert_eq!((menu.highlighted, menu.is_menu()), (Some(0), true));
    }
    // Terminal titles that say nothing give way to the session's title.
    assert!(["kiro-cli", "codex", "zsh", "~/Verbrawl", ""].iter().all(|t| crate::herdr::generic_title(t, if t.contains("codex") { "codex" } else { "kiro" })), "A bare harness or shell title was kept");
    assert!(!crate::herdr::generic_title("Guion promocional herdr dungeon", "claude"), "A real title was replaced");
    let picker = crate::ansi::parse("❯ /model\n  Select model\n    1. Default (recommended)\n  ❯ 2. Opus 5.5 ✔\n    3. Fable 5.1\n  Enter to set as default");
    assert_eq!((crate::ui::prompt_line(&picker), crate::ui::picker_open(&picker)), (None, true), "A picker's highlight was taken for the prompt");
    // Claude Code's trust dialog: unnumbered options under a menu footer.
    let trust = " Accessing workspace:\n\n /Users/me/rock-tower\n\n Quick safety check: Is this a project you created or one you trust?\n\n Security guide\n\n ❯ Yes, I trust this folder\n   No, exit\n\n Enter to confirm · Esc to cancel";
    let lines = meaningful_lines(trust);
    assert!(lines.iter().any(|l| l.contains("❯ Yes, I trust this folder")), "The trust dialog's highlighted option was dropped: {lines:?}");
    let menu = crate::herdr::MenuOptions::new(&lines);
    assert_eq!(menu.options.iter().map(|o| o.text.as_str()).collect::<Vec<_>>(), vec!["Yes, I trust this folder", "No, exit"], "Unnumbered options not read: {lines:?}");
    assert_eq!((menu.highlighted, menu.is_menu(), menu.keys_choosing(1)), (Some(0), true, vec!["down".to_string(), "enter".to_string()]));
    assert!(meaningful_lines("● listo\n────\n❯ hola\n────").iter().all(|l| !l.contains("hola")), "The input prompt was kept");
    let rule = "──────────────────────────────────────────";
    let quoted = crate::ansi::parse(&format!("  assert!(x, \"Esc to cancel\");\n● done\n\n{rule}\n❯\u{a0}\n{rule}"));
    assert_eq!((crate::ui::prompt_line(&quoted), crate::ui::picker_open(&quoted)), (Some(4), false), "Text quoting a picker's footer was taken for a picker");
    let kiro = crate::ansi::parse(&format!("› /model\nSelect model:   type to search\n\n❯ auto                 1.00x credits    Models chosen by task [active]\n  claude-opus-5.5      2.00x credits    Claude Opus 5.5\n(+13 more)\n{rule}\n Settings for selected model: auto      tab to switch panels\n{rule}\n esc to close · ↑↓ to navigate · ↵ to select · tab to switch panels"));
    assert_eq!((crate::ui::prompt_line(&kiro), crate::ui::picker_open(&kiro)), (None, true), "Kiro's /model picker was taken for a prompt");
    // A new shell that asks something first eats the agent's command.
    let omz = "Restored session: Sun Oct 11 18:22:00 -03 2026\n[oh-my-zsh] Would you like to update? [Y/n] kiro-cli\n\n➜  agents-pixel-dungeon iro-cli\nzsh: correct 'iro-cli' to 'kiro-cli' [nyae]?";
    assert!(shell_blocker(omz).is_some_and(|w| w.contains("omz update")), "oh-my-zsh's update prompt not recognised");
    assert!(shell_blocker("[oh-my-zsh] Would you like to update? [Y/n] ").is_some(), "A waiting oh-my-zsh prompt not recognised");
    assert!(shell_blocker("➜  x iro-cli\nzsh: correct 'iro-cli' to 'kiro-cli' [nyae]?").is_some_and(|w| w.contains("iro-cli")), "zsh's correction prompt not recognised");
    assert_eq!(shell_blocker("Restored session: Sun Oct 11\n➜  agents-pixel-dungeon "), None, "A plain shell prompt taken for a question");
    // Codex (0.162) marks its highlight, its prompt and past messages with ›.
    let approval: Vec<String> = ["› Run the shell command: touch b.txt", "• Running touch b.txt", "  Would you like to run the following command?", "  $ touch b.txt", "› 1. Yes, proceed (y)", "  2. Yes, and don't ask again for commands that start with `touch b.txt;",  "     touch c.txt` (p)", "  3. No, and tell Codex what to do differently (esc)", "  Press enter to confirm or esc to cancel"].iter().map(|l| l.to_string()).collect();
    let menu = MenuOptions::new(&approval);
    assert_eq!((menu.options.len(), menu.highlighted, menu.options[0].text.as_str()), (3, Some(0), "Yes, proceed (y)"), "Codex's approval not read as a menu: {menu:?}");
    assert_eq!(menu.keys_choosing(2), vec!["down", "down", "enter"], "Codex's approval would be answered with text, not arrows");
    assert_eq!(menu.tell_instead(), Some(2), "Words typed at Codex's approval would hit its y/p shortcuts");
    assert_eq!(menu.options[1].text, "Yes, and don't ask again for commands that start with `touch b.txt; touch c.txt` (p)", "A wrapped option lost its second line");
    let wrapped: Vec<String> = ["  Update Model Permissions", "› 1. Ask for approval (current)  Read and edit workspace files, with approval required for internet access", "                                 or edits outside the workspace", "  2. Read Only                   Read workspace files", "  enter select · esc back"].iter().map(|l| l.to_string()).collect();
    let menu = MenuOptions::new(&wrapped);
    assert!(menu.options.len() == 2 && menu.options[0].text.ends_with("access or edits outside the workspace"), "A wrapped option lost its second line: {menu:?}");
    assert!(MenuOptions::new(&["› fix the tests".to_string(), "› and the docs".to_string()]).options.is_empty(), "Codex's past messages taken for a menu");
    assert!(is_menu_footer("enter select · esc back") && is_menu_footer("Press enter to confirm or esc to cancel"), "Codex's menu footers not recognised");
    let picker = crate::ansi::parse("  Select Model and Effort\n  1. GPT-6.1-Sol            Latest model.\n› 2. GPT-6-Astra (current)  Frontier model.\n  enter select · esc back");
    assert_eq!((crate::ui::prompt_line(&picker), crate::ui::picker_open(&picker), crate::ui::composer_text(&picker)), (None, true, None), "Codex's /model picker taken for typed text");
    let idle = "› Run the shell command: touch a.txt\n• Done. Created a.txt.\n\n› Ask Codex to do anything\n\n  GPT-6-Astra medium · ~/proj\n  ← for agents · ? for shortcuts";
    assert_eq!(meaningful_lines(idle), vec!["› Run the shell command: touch a.txt", "• Done. Created a.txt."], "Codex's prompt, model line or past message mishandled");
    let leftover = crate::ansi::parse("• Unrecognized command '/approvals'.\n› /approvals\n  GPT-6-Astra medium · ~/proj");
    let empty = crate::ansi::parse("› \x1b[2mAsk Codex to do anything\x1b[0m\n  GPT-6-Astra medium · ~/proj");
    assert_eq!((crate::ui::composer_text(&leftover), crate::ui::composer_text(&empty)), (Some("/approvals".to_string()), None), "Codex's leftover input not seen, or its placeholder taken for it");
    // Kiro (2.29) paints its placeholder grey, not dim, after the cursor.
    let kiro_empty = crate::ansi::parse("kiro_default · auto · ◔ 2%\n› \x1b[0m\x1b[7m \x1b[0m\x1b[38;2;158;158;158mask a question or describe a task ↵\x1b[0m");
    let kiro_typed = crate::ansi::parse("› hello there\x1b[0m\x1b[7m \x1b[0m");
    assert_eq!((crate::ui::composer_text(&kiro_empty), crate::ui::composer_text(&kiro_typed)), (None, Some("hello there".to_string())), "Kiro's placeholder taken for typing");
    let kiro_approval: Vec<String> = ["↓ Shell touch kiro_a.txt", " shell requires approval", " ❯ Yes, single permission", "   Trust, always allow in this session", "   No (Tab to edit)", " esc to close · ↑↓ to navigate · ↵ to select · Tab to edit"].map(String::from).to_vec();
    let menu = MenuOptions::new(&kiro_approval);
    assert_eq!((menu.options.len(), menu.highlighted, menu.affirmative()), (3, Some(0), None), "Kiro's approval not read as a menu");
    // Claude Code (2.1.296): a description under each option is not part of it.
    let ask: Vec<String> = [" ☐ Fix scope", "Should the fix for calc.py be a patch or a full rewrite?", "❯ 1. Patch", "     Make minimal, targeted changes to the existing calc.py and keep its current structure.", "  2. Full rewrite", "     Replace calc.py with a new implementation designed from scratch.", "  3. Type something.", "  4. Chat about this", "Enter to select · ↑/↓ to navigate · Esc to cancel"].map(String::from).to_vec();
    let menu = MenuOptions::new(&ask);
    assert_eq!(menu.options.iter().map(|o| o.text.as_str()).collect::<Vec<_>>(), vec!["Patch", "Full rewrite", "Type something.", "Chat about this"], "Claude's option descriptions joined to the options");
    // A long list keeps its first option, and its scroll arrow is no part of a number.
    let models: Vec<String> = ["  Select model", "    1.  Default (recommended)  Opus 5.5 · Best for everyday, complex tasks", "  ❯ 2.  Opus 5.5 ✔             For complex work and everyday tasks", "    3.  Fable 5.1              For your toughest challenges", "    4.  Sonnet 5.5             Most efficient for simpler tasks", "    5.  Haiku 5.5              Fastest for quick answers", "    6.  Haiku 4.5              Fastest for quick answers", "    7.  Sonnet 5               Efficient for routine tasks", "    8.  Opus 5                 Best for everyday, complex tasks", "    9.  Fable 5                Most capable for your hardest and longest-running tasks", "  ↓ 10. Opus 4.8               Best for everyday, complex tasks", "     … +3 models", "  ◐ Medium effort (default) ←/→ to adjust", "  Enter to set as default · s to use this session only · Esc to cancel"].map(String::from).to_vec();
    let head = crate::herdr::menu_head(&models, models.len() - 12);
    let menu = MenuOptions::new(&models[head..]);
    assert_eq!((menu.options.len(), menu.highlighted, menu.options[9].text.split_whitespace().take(2).collect::<Vec<_>>().join(" ")), (10, Some(1), "Opus 4.8".to_string()), "A long menu lost its first or last option");
    // Folder trust starts on "No, exit": accepting picks the yes.
    let trust: Vec<String> = [" Claude Code'll be able to read, edit, and execute files here.", " ❯ No, exit", "   Yes, I trust this folder", " Enter to confirm · Esc to cancel"].map(String::from).to_vec();
    assert_eq!(MenuOptions::new(&trust).affirmative(), Some(1), "Accepting Claude's folder trust would exit it");
    let codex = crate::commands::slash_commands("codex", "/nonexistent", std::path::Path::new("/nonexistent"));
    assert!(codex.iter().any(|c| c.name == "permissions") && codex.iter().any(|c| c.name == "exit") && !codex.iter().any(|c| c.name == "approvals" || c.name == "quit"), "Codex offered commands it no longer knows");
    let typed = crate::ansi::parse(&format!("● done\n{rule}\n❯ fix the tests\n{rule}\n  ? for shortcuts"));
    assert_eq!((crate::ui::prompt_line(&typed), crate::ui::picker_open(&typed)), (Some(2), false), "A prompt with typed text was taken for a picker");
    let empty = crate::ansi::parse(&format!("● Interrupted\n{rule}\n❯ \x1b[2mTry \"fix the tests\"\x1b[0m\n{rule}"));
    assert_eq!((crate::ui::composer_text(&empty), crate::ui::erase_count(&empty, "fix it")), (None, 0), "The input's placeholder was taken for typed text");
    let back = crate::ansi::parse(&format!("● Interrupted\n{rule}\n❯ fix the failing tests in src\n{rule}"));
    assert_eq!(crate::ui::erase_count(&back, "fix the failing tests in src and the docs"), 41, "The message put back in the input was not erased whole");
    let narrow = crate::ansi::parse(&format!("  ❯ 3.  Fable 5.1 ✔          For your \n                             toughest \n\n  Enter to set as default · s to use\n  this session only · Esc to cancel\n\n{rule}\n❯\u{a0}\n{rule}"));
    assert_eq!((crate::ui::prompt_line(&narrow), crate::ui::picker_open(&narrow)), (None, true), "An empty prompt under a picker hid the picker");
    let texts = |s: &[Vec<crate::ansi::Span>]| s.iter().map(|l| l.iter().map(|x| x.text.as_str()).collect::<String>()).collect::<Vec<_>>();
    let flowed = texts(&crate::ansi::reflow(narrow));
    assert_eq!(flowed[0], "  ❯ 3.  Fable 5.1 ✔          For your toughest", "Wrapped picker line not joined: {flowed:?}");
    assert_eq!(flowed[2], "  Enter to set as default · s to use this session only · Esc to cancel", "Footer not joined: {flowed:?}");
    assert_eq!(flowed[4], rule, "A rule was joined to the footer: {flowed:?}");
    assert_eq!(crate::ui::draft_rows("one line", 40), 1);
    assert_eq!(crate::ui::draft_rows("a\nb\nc\nd", 40), 3, "The box grows past three rows");
    assert_eq!(crate::ui::draft_rows(&"x".repeat(100), 40), 3, "A long line wraps into rows");
    let wrapped = crate::ansi::parse(&format!("  Sources: Room 8, Who Needs An Art Bible?\n  (https://room8studio.com/news/who-needs\n  -an-art-bible-game-art-direction-from-in\n  die-to-aaa) · Juego Studio\n\n{rule}"));
    let flowed = texts(&crate::ansi::reflow(wrapped));
    assert_eq!(flowed[0], "  Sources: Room 8, Who Needs An Art Bible? (https://room8studio.com/news/who-needs-an-art-bible-game-art-direction-from-indie-to-aaa) · Juego Studio", "A URL cut at the column was not glued: {flowed:?}");
    let same = crate::ansi::parse("● short\n  ⎿ ok\n");
    assert_eq!(crate::ansi::reflow(same.clone()), same, "A narrow screen was re-flowed");
    let config: serde_json::Value = serde_json::from_str(r#"{"cachedUsageUtilization":{"utilization":{"limits":[{"kind":"session","group":"session","percent":100,"resets_at":"2099-01-01T00:00:00+00:00"},{"kind":"weekly_all","group":"weekly","percent":78,"resets_at":null},{"kind":"weekly_scoped","group":"weekly","percent":85,"resets_at":null}]}}}"#).unwrap();
    let claude = crate::usage::claude_from(&config).expect("Claude usage not read");
    assert_eq!(claude.meters.iter().map(|m| (m.window, m.used)).collect::<Vec<_>>(), vec![("5h", 100.0), ("sem", 78.0)], "Claude HP is the general weekly limit, not the per-model one");
    assert_eq!(claude.meters[0].hp(0), 0.0);
    let codex = crate::usage::codex_from_tail(r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":93.0,"window_minutes":300,"resets_at":1},"secondary":{"used_percent":22.0,"window_minutes":10080,"resets_at":4102444800}}}}"#).expect("Codex usage not read");
    assert_eq!(codex.meters.iter().map(|m| (m.window, m.hp(100))).collect::<Vec<_>>(), vec![("5h", 100.0), ("sem", 78.0)], "Codex meters (a reset window is full)");
    let kiro: serde_json::Value = serde_json::from_str(r#"{"session_state":{"conversation_metadata":{"user_turn_metadatas":[{"end_timestamp":"2026-10-08T12:00:00Z","metering_usage":[{"value":1.5},{"value":0.5}]},{"end_timestamp":"2026-09-08T12:00:00Z","metering_usage":[{"value":9.0}]}]}}}"#).unwrap();
    assert_eq!(crate::usage::kiro_month_credits(&kiro, "2026-10"), 2.0, "Kiro credits of the month");
    {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 10, 9, 21, 0, 0).unwrap();
        let at = |d, h| chrono::Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap().timestamp();
        assert_eq!(crate::usage::reset_label(at(9, 22), now, true).as_deref(), Some("22:00"), "A reset today shows its hour");
        assert_eq!(crate::usage::reset_label(at(11, 12), now, true), Some(format!("{} 11 12:00", l10n::tr("dom"))), "A later reset shows its day");
    }
    let line = "  ⎿ Abre http://localhost:5173/, o mira (src/ui.rs:42). Total 3.5 y v1.2";
    let at = |needle: &str| line.chars().count() - line[line.find(needle).unwrap()..].chars().count() + 2;
    assert_eq!(link_at(line, at("http")).as_deref(), Some("http://localhost:5173/"), "URL not found");
    assert_eq!(link_at(line, at("src/")).as_deref(), Some("src/ui.rs:42"), "Path not found");
    assert_eq!(link_at(line, at("3.5")), None, "A number is not a file");
    assert_eq!(link_at(line, at("Abre")), None, "A word is not a link");
    let cwd = env!("CARGO_MANIFEST_DIR");
    assert_eq!(resolve_link("src/ui.rs:42:7", cwd), Some(format!("{cwd}/src/ui.rs")), "Path with line not resolved");
    assert_eq!(resolve_link("src/nope.rs", cwd), None, "A missing file must not open");
    let ask = "Lo que veo:\n- algo\n\nneeds input: ¿para qué es la rama nueva? Opciones:\n\n1. **Rama de PR** desde `main`\n2. Renombrar";
    assert_eq!(question_lines(ask, Some("¿para qué es la rama nueva? Opciones:")), vec!["¿para qué es la rama nueva? Opciones:", "1. Rama de PR desde main", "2. Renombrar"], "Question not cut at the ask");
    let split = concat!(r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Primera parte"}]}}"#, "\n",
                        r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#, "\n",
                        r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Segunda parte"}]}}"#);
    assert_eq!(last_assistant_text(split).as_deref(), Some("Primera parte\nSegunda parte"), "Split assistant message not joined");
    assert_eq!(last_assistant_text(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"solo"}]}}"#).as_deref(), Some("solo"));
    let wrapped: Vec<String> = ["● Bash(ls)", "  ⎿ ok", "Tipografía. Propongo pixel font solo para títulos, nombres y números grandes, y Manrope", "para texto pequeño. Una pixel font a 12 px no se lee en móvil. Se decide con capturas.", "  - Primer paso. Un vertical slice de 3 a 4 días: marco del bestiario, un goblin con sus", "tags, el ataque de rayo completo y la barra de HP.", "❯ 1. Yes"].iter().map(|s| s.to_string()).collect();
    let flowed = paragraphs(&wrapped);
    assert_eq!(flowed.len(), 5, "Hard-wrapped lines not joined: {flowed:?}");
    assert!(flowed[2].starts_with("Tipografía") && flowed[2].ends_with("capturas.") && flowed[3].starts_with("  - Primer paso") && flowed[3].ends_with("de HP.") && flowed[4] == "❯ 1. Yes");
    assert!(job_state("blocked", Some("active"), false) == "working" && job_state("blocked", Some("idle"), false) == "idle" && job_state("blocked", Some("idle"), true) == "blocked" && job_state("working", Some("blocked"), false) == "blocked" && job_state("blocked", None, false) == "blocked", "Job tempo must decide its state");
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(identify(&args(&["claude"])), Some("claude"));
    assert_eq!(identify(&args(&["/Users/me/.local/share/claude/versions/2.1.295", "--resume", "x"])), Some("claude"));
    assert_eq!(identify(&args(&["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"])), Some("codex"));
    assert_eq!(identify(&args(&["/opt/homebrew/bin/kiro-cli", "chat"])), Some("kiro"));
    assert_eq!(identify(&args(&["claude", "--chrome-native-host"])), None);
    assert_eq!(identify(&args(&["claude", "daemon", "run"])), None);
    assert_eq!(identify(&args(&["claude", "-p", "hola"])), None);
    assert_eq!(identify(&args(&["zsh"])), None);
    assert_eq!(identify(&args(&["node", "/x/aseprite-ai-artist.mjs", "serve"])), None);
    assert_eq!(claude_project_dir("/Users/me/my.app"), "-Users-me-my-app");
    let pending = concat!(r#"{"type":"user","message":{"role":"user","content":"arregla el bug"}}"#, "\n", r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#);
    assert_eq!(claude_view(pending, Duration::from_secs(30), false).status, "blocked", "A pending tool call with an idle agent waits for permission");
    assert_eq!(claude_view(pending, Duration::from_secs(30), true).status, "working", "A running child means the tool is executing");
    assert_eq!(claude_view(pending, Duration::from_secs(1), false).status, "working");
    let view = claude_view(pending, Duration::from_secs(30), false);
    assert_eq!(view.prompt.as_deref(), Some("arregla el bug"));
    let replied = concat!(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#, "\n", r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Listo."}]}}"#);
    assert_eq!(claude_view(replied, Duration::from_secs(30), false).status, "idle");
    assert_eq!(claude_view(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#, Duration::from_secs(5), false).status, "working");
    let codex_pending = concat!(r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"corre los tests"}]}}"#, "\n", r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{}"}}"#);
    let cv = codex_view(codex_pending, Duration::from_secs(30), false);
    assert!(cv.status == "blocked" && cv.prompt.as_deref() == Some("corre los tests"));
    assert_eq!(codex_view(r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[]}}"#, Duration::from_secs(30), false).status, "idle");
    let job = BackgroundJob { state: "blocked".into(), needs: Some("¿Sí?".into()), transcript: Some("/t.jsonl".into()), updated: SystemTime::now(), resets: None };
    let asked = agent("q", "claude", "idle", "p", "a", "~").with_jobs(&[job]);
    assert!(asked.status == "blocked" && asked.question.as_deref() == Some("¿Sí?") && asked.question_transcript.as_deref() == Some("/t.jsonl"));
    let lines: Vec<String> = ["Bash(rm -rf build)", " Do you want to proceed?", "❯ 1. Yes", "  2. Yes, and don't ask again", "  3. No, and tell Claude what to do"].iter().map(|s| s.to_string()).collect();
    let menu = MenuOptions::new(&lines);
    assert!(menu.options.iter().map(|o| o.number).collect::<Vec<_>>() == vec![1, 2, 3] && menu.highlighted == Some(0) && menu.is_menu() && menu.options[1].text == "Yes, and don't ask again" && menu.options[2].line == 4);
    assert!(menu.keys_choosing(2) == vec!["down", "down", "enter"] && menu.keys_choosing(0) == vec!["enter"]);
    let plain_lines: Vec<String> = ["¿Qué rama?", "1. Rama de PR", "2) Renombrar", "texto", "1. Otra pregunta", "2. Sí"].iter().map(|s| s.to_string()).collect();
    let plain = MenuOptions::new(&plain_lines);
    assert!(!plain.is_menu() && plain.options.iter().map(|o| o.text.as_str()).collect::<Vec<_>>() == vec!["Otra pregunta", "Sí"] && MenuOptions::new(&["hola".to_string()]).options.is_empty());
    let pool = vec![agent("1", "claude", "blocked", "Web Shop", "Arreglando el carrito", "~/code/shop"), agent("2", "codex", "working", "API", "Tests", "~/code/api")];
    let mut branched = pool[1].clone();
    branched.branch = Some("feature/pagos".into());
    let ids = |v: Vec<Agent>| v.into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert!(filter_agents(&pool, StatusFilter::All, "").len() == 2 && ids(filter_agents(&pool, StatusFilter::Blocked, "")) == vec!["1"] && filter_agents(&pool, StatusFilter::Idle, "").is_empty());
    assert!(ids(filter_agents(&pool, StatusFilter::All, "CARRITO")) == vec!["1"] && ids(filter_agents(&pool, StatusFilter::All, "codex")) == vec!["2"] && ids(filter_agents(&pool, StatusFilter::All, "web shop")) == vec!["1"]);
    assert!(ids(filter_agents(&[pool[0].clone(), branched], StatusFilter::All, "pagos")) == vec!["2"] && ids(filter_agents(&pool, StatusFilter::All, "cárrito shop")) == vec!["1"] && filter_agents(&pool, StatusFilter::Working, "shop").is_empty());
    let now = Instant::now();
    assert!(connection_note(None, Some(now), now, 5.0).is_none() && connection_note(None, None, now, 5.0).is_none());
    let stale = connection_note(None, Some(now - Duration::from_secs(14)), now, 5.0).unwrap();
    assert!(!stale.1 && stale.0.contains("14"));
    let lost = connection_note(Some("x"), Some(now - Duration::from_secs(200)), now, 5.0).unwrap();
    assert!(lost.1 && lost.0.contains('3'));
    assert_eq!(connection_note(Some("x"), None, now, 5.0).unwrap().0, l10n::tr("Herdr desconectado"));
    for n in 1..=6 { assert!(columns_fitting(width_for(n)) == n && columns_fitting(width_for(n) + 100.0) == n, "Flex wrap columns: {n}"); }
    assert!(columns_fitting(10.0) == 1 && width_for(2) == 458.0);
    let listed = decode_sessions(br#"{"sessions":[{"default":true,"name":"default","running":true},{"name":"work","running":false}]}"#);
    assert!(listed == vec![HerdrSession { name: "default".into(), running: true }, HerdrSession { name: "work".into(), running: false }] && decode_sessions(b"nope").is_empty());
    // An agent's folder is kept as ~/…: the repo behind it is still read.
    let here = std::env::current_dir().unwrap();
    if let Ok(inside) = here.strip_prefix(home_dir()) {
        if here.join(".git").exists() {
            let meta = crate::identity::read_meta(&format!("~/{}", inside.display()));
            assert!(meta.kind != crate::identity::Kind::Unknown || meta.owner.is_some(), "Repo behind ~/…: {meta:?}");
        }
    }
    for (column, files) in [(500.0, 1), (500.0, 2), (100.0, 40), (900.0, 40)] {
        let h = crate::ui::changes_list_height(column, files);
        assert!(h > 0.0 && h <= 21.0 * files as f32 + 4.0, "changes list height {h} for {files} files in {column}");
    }
    let named = agent_name("codex", "Mi Proyecto_2");
    assert!(named.starts_with("codex-mi-proyecto-2-") && named.len() == 24, "Agent name: {named}");
    let long = agent_name("claude", "saludtech-android-app");
    let accented = agent_name("opencode", "Diseño Ñandú");
    assert!(long.len() <= 32 && long.starts_with("claude-saludtech-android-") && accented.starts_with("opencode-dise-o-and-"), "Agent names: {long} {accented}");
    assert!(root_pane(br#"{"result":{"root_pane":{"pane_id":"w3:p1"},"tab":{}}}"#).as_deref() == Some("w3:p1") && root_pane(b"{}").is_none());
    let screen = crate::ansi::parse("\x1b[0m\x1b[1m\x1b[38;5;2mok\x1b[0m plain\r\n\x1b[38;2;215;119;87m●\x1b[0m\x1b[48;2;1;2;3m x\x1b[0m\n\n");
    assert!(screen.len() == 2 && crate::ansi::plain(&screen) == ["ok plain", "● x"], "ANSI lines: {:?}", crate::ansi::plain(&screen));
    assert!(screen[0][0].bold && screen[0][0].fg == Some(egui::Color32::from_rgb(56, 196, 56)) && screen[0][1].fg.is_none() && !screen[0][1].bold);
    assert!(screen[1][0].fg == Some(egui::Color32::from_rgb(215, 119, 87)) && screen[1][1].bg == Some(egui::Color32::from_rgb(1, 2, 3)));
    let commands = crate::commands::slash_commands("claude", "/nonexistent", std::path::Path::new("/nonexistent"));
    assert!(commands.iter().any(|c| c.name == "help") && commands.iter().any(|c| c.name == "compact"));
    let names = |draft: &str| crate::commands::matching(&commands, draft).iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert!(names("/co").contains(&"compact".to_string()) && names("/co").iter().all(|n| n.starts_with("co")) && names("/compact x").is_empty() && names("hi").is_empty());
    let kiro = crate::commands::slash_commands("kiro", "/nonexistent", std::path::Path::new("/nonexistent"));
    assert!(kiro.iter().any(|c| c.name == "exit") && kiro.iter().any(|c| c.name == "chat") && !kiro.iter().any(|c| c.name == "save" || c.name == "tangent"), "Kiro offered commands it no longer knows");
    let created = br#"{"result":{"root_pane":{"pane_id":"w3:p1"},"workspace":{"workspace_id":"w3"}}}"#;
    assert!(created_workspace(created).as_deref() == Some("w3") && created_workspace(b"{}").is_none());
    let codex = [
        r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rg --files app\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"npm test\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin"}}"#,
        r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#,
    ];
    assert!(codex_action(codex[0]).as_deref() == Some("read") && codex_action(codex[1]).as_deref() == Some("brew") && codex_action(&codex[0..3].join("\n")).as_deref() == Some("forge"));
    assert!(codex_action(&codex.join("\n")).as_deref() == Some("type") && codex_action(r#"{"type":"event_msg","payload":{"type":"token_count"}}"#).is_none());
    let kiro = r#"{"version":"v1","kind":"AssistantMessage","data":{"content":[{"kind":"text","data":"x"},{"kind":"toolUse","data":{"name":"shell","input":{}}}]}}"#;
    assert!(kiro_action(kiro).as_deref() == Some("brew") && kiro_action(r#"{"kind":"AssistantMessage","data":{"content":[{"kind":"text","data":"hola"}]}}"#).as_deref() == Some("type") && kiro_action(r#"{"kind":"Prompt","data":{}}"#).is_none());
    assert!(shell_action("/usr/bin/sed -n 1,5p x") == "read" && shell_action("swift build") == "brew" && shell_action("") == "brew");
    let mut live = vec![agent("a", "claude", "working", "p", "x", "~"), agent("b", "codex", "idle", "p", "y", "~")];
    let names: HashMap<String, String> = HashMap::new();
    let ev = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
    assert!(apply_herdr_event(&ev(r#"{"event":"pane.agent_status_changed","data":{"pane_id":"b","workspace_id":"w","agent_status":"blocked"}}"#), &mut live, &names) == EventEffect::Changed && live.iter().map(|a| a.id.as_str()).collect::<Vec<_>>() == vec!["b", "a"] && live[0].status == "blocked");
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_agent_status_changed","data":{"pane_id":"b","agent_status":"blocked"}}"#), &mut live, &names) == EventEffect::None);
    assert!(apply_herdr_event(&ev(r#"{"event":"pane.agent_status_changed","data":{"pane_id":"zz","agent_status":"idle"}}"#), &mut live, &names) == EventEffect::Resync);
    let mut web = HashMap::new();
    web.insert("w".to_string(), "Web".to_string());
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_updated","data":{"pane":{"pane_id":"a","agent":"claude","agent_status":"done","workspace_id":"w","terminal_title_stripped":"Nuevo"}}}"#), &mut live, &web) == EventEffect::Changed);
    let a = live.iter().find(|x| x.id == "a").unwrap();
    assert!(a.activity == "Nuevo" && a.project == "Web");
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_updated","data":{"pane":{"pane_id":"a","agent_status":"idle"}}}"#), &mut live, &names) == EventEffect::Changed && live.iter().map(|a| a.id.as_str()).collect::<Vec<_>>() == vec!["b"]);
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_created","data":{"pane":{"pane_id":"n","agent":"kiro"}}}"#), &mut live, &names) == EventEffect::Resync);
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_closed","data":{"pane_id":"b"}}"#), &mut live, &names) == EventEffect::Changed && live.is_empty());
    assert!(apply_herdr_event(&ev(r#"{"event":"pane_agent_detected","data":{"pane_id":"q"}}"#), &mut live, &names) == EventEffect::Resync && apply_herdr_event(&ev(r#"{"event":"pane_focused","data":{}}"#), &mut live, &names) == EventEffect::None);
    let subs = herdr_subscriptions(&["w2:p1".to_string(), "w1:p1".to_string()]);
    assert!(subs.last().and_then(|s| s.get("pane_id")).and_then(|v| v.as_str()) == Some("w2:p1") && subs.iter().any(|s| s.get("type").and_then(|v| v.as_str()) == Some("pane.closed")));
    assert!(herdr_error(br#"{"error":{"code":"agent_blocked","message":"blocked"},"id":"x"}"#).and_then(|e| e.code().map(str::to_string)).as_deref() == Some("agent_blocked") && herdr_error(b"oops").is_none());
    // Usage limit: the refusal row Claude Code writes, its reset time, and the trapped state.
    let refusal = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"You've hit your session limit · resets 2:20pm (America/Santiago)"}]},"quotaLimits":{"status":"rejected","resetsAt":1790094600},"error":"rate_limit","isApiErrorMessage":true}"#;
    assert_eq!(limit_in_tail(&format!("{}\n{refusal}", r#"{"type":"user","message":{"content":"hola"}}"#)), Some(Some(1790094600)), "Limit row not read");
    assert_eq!(limit_in_tail(&format!("{refusal}\n{}", r#"{"type":"user","message":{"content":"sigue"}}"#)), None, "A later prompt ends the limit");
    assert_eq!(limit_in_tail(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#), None);
    assert!(is_limit_text("rate limited — wait and retry · You've hit your session limit") && is_limit_text("Claude usage limit reached.") && !is_limit_text("Done, under the limit"));
    use chrono::TimeZone;
    let noon = chrono::Local.with_ymd_and_hms(2026, 10, 9, 12, 0, 0).unwrap();
    assert_eq!(resets_from_text("You've hit your session limit · resets 2:20pm (America/Santiago)", noon), Some(noon.timestamp() + 2 * 3600 + 20 * 60));
    assert_eq!(resets_from_text("resets 11am", noon), Some(noon.timestamp() + 23 * 3600), "A past time means tomorrow");
    assert!(resets_from_text("resets soon", noon).is_none() && resets_from_text("no time here", noon).is_none());
    let trapped = |state: &str, resets| BackgroundJob { state: state.into(), needs: Some("rate limited".into()), transcript: None, updated: SystemTime::now(), resets };
    let caged = agent("l", "claude", "blocked", "p", "", "~").with_jobs(&[trapped("limited", Some(5000)), trapped("limited", Some(4000))]);
    assert!(caged.status == "limited" && caged.limit_resets == Some(4000) && caged.limit_left(3000) == Some(1000) && caged.limit_left(9000) == Some(0));
    assert_eq!(agent("l", "claude", "idle", "p", "", "~").with_jobs(&[trapped("limited", None), trapped("blocked", None)]).status, "blocked", "A real question outranks the limit");
    assert_eq!(agent("l", "claude", "working", "p", "", "~").with_jobs(&[trapped("limited", None)]).status, "working");
    assert!(agent("s", "claude", "blocked", "p", "", "~").with_limit(Some(Some(7))).limit_resets == Some(7) && agent("s", "claude", "working", "p", "", "~").with_limit(Some(None)).status == "working");
    assert!(crate::scene::countdown(65) == "1:05" && crate::scene::countdown(4385) == "1:13:05" && crate::scene::countdown(97_200) == "1d 3h" && crate::scene::countdown(-4) == "0:00");
    assert!(StatusFilter::from_raw("limited") == StatusFilter::Limited && ids(filter_agents(&[caged.clone(), pool[1].clone()], StatusFilter::Limited, "")) == vec!["l"]);
    assert_eq!(sort_agents(vec![agent("w", "x", "working", "p", "", "~"), caged, agent("b", "x", "blocked", "p", "", "~")]).iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["b", "l", "w"]);
    let mut ordering = Monitor::new("default".into(), StatusFilter::All);
    let urgent = agent("b", "claude", "working", "p", "", "~").with_jobs(&[BackgroundJob { state: "blocked".into(), needs: None, transcript: None, updated: SystemTime::now(), resets: None }]);
    ordering.apply(vec![agent("a", "claude", "idle", "p", "", "~"), urgent]);
    assert_eq!(ordering.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"], "Attention must lead the grid");
    let daemon = std::env::temp_dir().join(format!("hpd-daemon-{}", std::process::id()));
    std::fs::create_dir_all(daemon.join("d1/pty")).unwrap();
    std::fs::write(daemon.join("d1/pty/abc123.sock"), b"").unwrap();
    let ids_found = live_job_ids(&daemon.to_string_lossy()).unwrap();
    assert!(ids_found.len() == 1 && ids_found.contains("abc123") && live_job_ids(&format!("{}-missing", daemon.to_string_lossy())).is_none(), "Live job sockets");
    let _ = std::fs::remove_dir_all(&daemon);
    assert!(natural_cmp("w2:p1", "w10:p1") == std::cmp::Ordering::Less && natural_cmp("a", "A") == std::cmp::Ordering::Equal);
    assert_eq!(l10n::trf("Tú → {}: {}", &[&"a", &"b"]), l10n::text("Tú → {}: {}", &l10n::CURRENT).replacen("{}", "a", 1).replacen("{}", "b", 1));
    window_sizing();
    println!("PASS: snapshot states, filtering, empty/error handling, monitor transitions, room art, heroes, subagent sessions, subagents keep agents busy, tool actions, translations, git branch, sound alerts + prefs, notifications + prefs, chat selection, question options, filters and search, connection notes, flex-wrap columns, sessions, agent creation, codex + kiro actions, herdr events, question extraction, Herdr error codes, live background jobs, usage limits, HP viewer, natural order, window sizing, background shells, {} bundled sprites", ROOM_FILES.len() + 1);
}

/// The widget's size: fitted to the agents until the user resizes it, then
/// kept whatever the agents or the chat panel need.
fn window_sizing() {
    use crate::{layout_height, layout_width, snap_columns, snap_rows};
    use crate::scene::{columns_fitting, GAP, ROW_H};
    use crate::ui::PANEL_HEIGHT;
    let rows = |n: f32| n * ROW_H + (n - 1.0) * GAP;
    let (chrome, screen) = (60.0, 2000.0);
    // Automatic: never more columns than agents, nor rows than they fill.
    assert_eq!(layout_width(4, 2, false, None), width_for(2));
    assert_eq!(layout_width(2, 0, false, None), width_for(1));
    assert!((1..=8).all(|n| columns_fitting(width_for(n)) == n));
    assert_eq!(layout_height(2, 4, chrome, None, screen), chrome + rows(2.0));
    assert_eq!(layout_height(9, 4, chrome, None, screen), chrome + rows(4.0));
    assert_eq!(layout_height(9, 9, chrome, None, 900.0), 900.0);
    // A one-room widget widens while a panel is open.
    assert_eq!(layout_width(1, 5, true, None), width_for(2));
    assert_eq!(layout_width(1, 5, true, Some(width_for(1))), width_for(2));
    assert_eq!(layout_width(1, 5, true, Some(900.0)), 900.0);
    // User-sized: the width stays exactly as dragged, however many agents
    // there are, also past one screen; never under one room.
    assert_eq!(layout_width(2, 2, false, Some(1000.0)), 1000.0, "Rounding must not shrink a resized widget");
    assert_eq!(layout_width(8, 9, false, Some(3000.0)), 3000.0, "A widget across two screens keeps its width");
    assert_eq!(layout_width(3, 9, false, Some(10.0)), width_for(1));
    // User-sized: the height stays with the chat panel open or closed,
    // whatever the agents need and past the screen, but never under one row.
    let user = 777.0;
    assert_eq!(layout_height(1, 4, chrome, Some(user), screen), user, "Few agents must not shrink a resized widget");
    assert_eq!(layout_height(1, 4, chrome + PANEL_HEIGHT, Some(user), screen), user, "Opening the chat must not change a resized widget");
    assert_eq!(layout_height(1, 4, chrome, Some(1500.0), 900.0), 1500.0, "The screen's height must not shrink a resized widget");
    assert_eq!(layout_height(1, 1, chrome + PANEL_HEIGHT, Some(chrome + rows(1.0)), screen), chrome + PANEL_HEIGHT + ROW_H, "The chat needs a row of rooms above it");
    // The columns and rows remembered for the next start: the nearest.
    assert!(snap_columns(width_for(3)) == 3 && snap_columns(width_for(3) + 100.0) == 3 && snap_columns(width_for(3) + 130.0) == 4 && snap_columns(10.0) == 1 && snap_columns(9000.0) == 8);
    assert!(snap_rows(chrome + rows(2.0), chrome) == 2 && snap_rows(chrome + rows(2.0) + 0.4 * (ROW_H + GAP), chrome) == 2 && snap_rows(chrome + rows(2.0) + 0.6 * (ROW_H + GAP), chrome) == 3 && snap_rows(chrome, chrome) == 1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn self_test() {
        super::run();
    }
}
