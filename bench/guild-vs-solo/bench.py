#!/usr/bin/env python3
"""Guild vs solo Claude on HARNESS: Directive.

Three arms per task and replica, all from the same commit of a dedicated
clone (so nobody else's work moves the base):

  solo    one Claude Code session does the whole task;
  review  the solo result, reviewed by a second read-only session, then
          fixed by the solo session itself (resumed): isolates what review adds;
  guild   a Claude orchestrator plans and reviews over the guild's MCP tools;
          Orin (Kiro, Opus) and Lira (Kiro, Sonnet) build in their worktrees.

Every arm is verified by the same script (verify.sh) and judged blind by a
third session that sees three diffs as P, Q, R in two shuffled orders.

  bench.py run <task> <rep> solo|guild    run one arm (solo also produces review)
  bench.py judge <task> <rep>             blind judging, two orders
  bench.py all [--reps N]                 every task × replica × arm, then judges
  bench.py report                         the table, as Markdown

Results go to $BENCH_HOME/results (default ~/guild-bench/results).
"""
import json, os, random, re, shutil, subprocess, sys, time, datetime as dt
from pathlib import Path

HERE = Path(__file__).resolve().parent
HOME = Path(os.environ.get("BENCH_HOME", Path.home() / "guild-bench"))
REPO = HOME / "harness-directive"          # the clone; its main is the base
WORK = HOME / "work"
RESULTS = HOME / "results"
GUILD = os.environ.get("GUILD_BIN", "/Users/nachovalencia/agents-pixel-dungeon/build/Agents Pixel Dungeon.app/Contents/MacOS/agents-pixel-dungeon-guild")
PROJECT = "hd-bench"
MODEL = os.environ.get("BENCH_MODEL", "claude-opus-5-5")
TASKS = sorted(p.stem for p in (HERE / "tasks").glob("*.md"))
ARMS = ["solo", "review", "guild"]
RUN_TIMEOUT = 90 * 60

SOLO_TOOLS = "Read,Edit,Write,Glob,Grep,TodoWrite,Bash(godot:*),Bash(sh:*),Bash(git status:*),Bash(git diff:*),Bash(ls:*),Bash(cat:*),Bash(grep:*),Bash(rg:*)"
READ_TOOLS = "Read,Glob,Grep,Bash(git status:*),Bash(git diff:*),Bash(sh:*),Bash(godot:*)"
ORCH_TOOLS = "mcp__guild__guild_status,mcp__guild__guild_start_quest,mcp__guild__guild_wait,mcp__guild__guild_task,mcp__guild__guild_diff,mcp__guild__guild_decide,mcp__guild__guild_permission,Read,Glob,Grep,TodoWrite"


def now(): return dt.datetime.now(dt.timezone.utc).isoformat()
def log(*a): print(f"[{dt.datetime.now():%H:%M:%S}]", *a, flush=True)
def git(*a, cwd=REPO): return subprocess.run(["git", *a], cwd=cwd, capture_output=True, text=True, check=True).stdout
def prompt(name, **kw):
    text = (HERE / "prompts" / f"{name}.md").read_text()
    for k, v in kw.items(): text = text.replace("{" + k + "}", str(v))
    return text
def task_text(task): return (HERE / "tasks" / f"{task}.md").read_text().strip()
def base_sha(): return git("rev-parse", "main").strip()


def clean_env():
    """No herdr pane, no parent Claude session: each run is a fresh, top-level session."""
    env = {k: v for k, v in os.environ.items() if not (k.startswith("HERDR") or k.startswith("CLAUDE") or k == "CLAUDECODE")}
    env["BENCH_BASE"] = base_sha()
    return env


def claude(cwd, text, out, tools, extra=(), resume=None):
    """One headless Claude Code session; the transcript is kept as stream-json."""
    argv = ["claude", "-p", text, "--model", MODEL, "--output-format", "stream-json", "--verbose",
            "--permission-mode", "acceptEdits", "--allowedTools", tools, *extra]
    if "--mcp-config" not in extra: argv.append("--strict-mcp-config")   # no personal MCP servers in any arm
    if resume: argv += ["--resume", resume]
    t0 = time.time()
    with open(out, "w") as f:
        try:
            subprocess.run(argv, cwd=cwd, env=clean_env(), stdout=f, stderr=subprocess.STDOUT, timeout=RUN_TIMEOUT)
        except subprocess.TimeoutExpired:
            log("timeout", out)
    result, tool_calls = {}, {}
    for line in open(out):
        try: ev = json.loads(line)
        except ValueError: continue
        if ev.get("type") == "result": result = ev
        if ev.get("type") == "assistant":
            for c in ev.get("message", {}).get("content", []):
                if c.get("type") == "tool_use":
                    name = c["name"]
                    if name == "mcp__guild__guild_decide": name += ":" + str(c.get("input", {}).get("decision", "?"))
                    tool_calls[name] = tool_calls.get(name, 0) + 1
    return {"seconds": round(time.time() - t0), "cost_usd": result.get("total_cost_usd", 0.0), "turns": result.get("num_turns"),
            "session_id": result.get("session_id"), "is_error": result.get("is_error", True) if result else True,
            "result": result.get("result", ""), "tool_calls": tool_calls}


def worktree(name, ref="main"):
    path = WORK / name
    if path.exists():
        subprocess.run(["git", "worktree", "remove", "--force", str(path)], cwd=REPO, capture_output=True)
        shutil.rmtree(path, ignore_errors=True)
    WORK.mkdir(parents=True, exist_ok=True)
    git("worktree", "add", "--detach", str(path), ref)
    subprocess.run(["godot", "--headless", "--path", ".", "--import"], cwd=path, capture_output=True)
    return path


def snapshot_diff(path, out):
    """The whole change against the base, new files included, as one diff."""
    subprocess.run(["git", "add", "-A"], cwd=path, check=True)
    diff = git("diff", "--cached", base_sha(), "--", ".", ":(exclude).godot", cwd=path)
    subprocess.run(["git", "reset", "-q"], cwd=path, check=True)
    Path(out).write_text(diff)
    files = len(re.findall(r"^diff --git", diff, re.M))
    plus = len(re.findall(r"^\+(?!\+\+)", diff, re.M)); minus = len(re.findall(r"^-(?!--)", diff, re.M))
    return {"files": files, "added": plus, "removed": minus}


def verify(path, out):
    t0 = time.time()
    try:
        r = subprocess.run(["sh", str(HERE / "verify.sh")], cwd=path, env=clean_env(), capture_output=True, text=True, timeout=900)
        text = r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        text = "VERIFY FAILED: timeout"
    Path(out).write_text(text)
    return {"ok": "VERIFY OK" in text, "seconds": round(time.time() - t0), "last": text.strip().splitlines()[-1] if text.strip() else ""}


def kiro_credits(t0, t1):
    r = subprocess.run([sys.executable, str(HERE / "kiro_credits.py"), t0, t1], capture_output=True, text=True)
    try: return json.loads(r.stdout)
    except ValueError: return {"kiro_credits": None}


def save(dirpath, arm, meta):
    (dirpath / f"{arm}.json").write_text(json.dumps(meta, indent=2, ensure_ascii=False))
    log(arm, "done:", {k: meta.get(k) for k in ("verify_ok", "minutes", "cost_usd", "kiro_credits")})


def run_solo(task, rep):
    d = RESULTS / task / f"r{rep}"; d.mkdir(parents=True, exist_ok=True)
    path = worktree(f"{task}-r{rep}-solo")
    vs = HERE / "verify.sh"
    log("solo", task, rep)
    t0 = now()
    s = claude(path, prompt("solo", TASK=task_text(task), VERIFY=vs), d / "solo.transcript.jsonl", SOLO_TOOLS)
    size = snapshot_diff(path, d / "solo.diff")
    v = verify(path, d / "solo.verify.txt")
    save(d, "solo", {"arm": "solo", "task": task, "rep": rep, "base": base_sha(), "started": t0, "minutes": round(s["seconds"] / 60, 1),
                     "cost_usd": round(s["cost_usd"], 2), "kiro_credits": 0, "turns": s["turns"], "tool_calls": s["tool_calls"],
                     "verify_ok": v["ok"], "verify_last": v["last"], "diff": size, "summary": s["result"], "human_interventions": 0})
    # The review arm continues from this exact result.
    log("review", task, rep)
    rv = claude(path, prompt("review", TASK=task_text(task), VERIFY=vs), d / "review.reviewer.jsonl", READ_TOOLS,
                extra=["--disallowedTools", "Edit,Write,NotebookEdit"])
    review = rv["result"].strip()
    (d / "review.review.md").write_text(review)
    if review == "SIN CAMBIOS" or not s["session_id"]:
        fx = {"seconds": 0, "cost_usd": 0.0, "turns": 0, "tool_calls": {}, "result": "(sin cambios pedidos)"}
    else:
        fx = claude(path, prompt("fix", REVIEW=review, VERIFY=vs), d / "review.fix.jsonl", SOLO_TOOLS, resume=s["session_id"])
    size = snapshot_diff(path, d / "review.diff")
    v = verify(path, d / "review.verify.txt")
    secs = s["seconds"] + rv["seconds"] + fx["seconds"]
    save(d, "review", {"arm": "review", "task": task, "rep": rep, "base": base_sha(), "started": t0, "minutes": round(secs / 60, 1),
                       "cost_usd": round(s["cost_usd"] + rv["cost_usd"] + fx["cost_usd"], 2), "kiro_credits": 0,
                       "turns": (s["turns"] or 0) + (rv["turns"] or 0) + (fx["turns"] or 0),
                       "review_points": review_points(review),
                       "verify_ok": v["ok"], "verify_last": v["last"], "diff": size, "summary": fx["result"], "human_interventions": 0})


def review_points(review):
    """The reviewer's numbered points; nested bullets explain a point, they are not points."""
    return 0 if review.strip() == "SIN CAMBIOS" else len(re.findall(r"^\s{0,3}(?:\*\*)?\d+[.)]", review, re.M))


def guild_quest_after(t0):
    snap = json.loads(subprocess.run([GUILD, "snapshot"], capture_output=True, text=True).stdout)
    qs = [q for q in snap["quests"] if q["project_id"] == PROJECT and q["created_at"] >= t0]
    return (sorted(qs, key=lambda q: q["created_at"])[-1] if qs else None), snap


def run_guild(task, rep):
    d = RESULTS / task / f"r{rep}"; d.mkdir(parents=True, exist_ok=True)
    cwd = WORK / f"{task}-r{rep}-orch"; cwd.mkdir(parents=True, exist_ok=True)
    mcp = cwd / "mcp.json"
    mcp.write_text(json.dumps({"mcpServers": {"guild": {"command": GUILD, "args": ["mcp"]}}}))
    log("guild", task, rep)
    t0 = now()
    env_extra = ["--mcp-config", str(mcp), "--strict-mcp-config", "--add-dir", str(REPO), "--disallowedTools", "Edit,Write,Bash,NotebookEdit"]
    os.environ["MCP_TOOL_TIMEOUT"] = "600000"
    o = claude(cwd, prompt("orchestrator", TASK=task_text(task), REPO=REPO), d / "guild.transcript.jsonl", ORCH_TOOLS, extra=env_extra)
    t1 = now()
    quest, snap = guild_quest_after(t0)
    meta = {"arm": "guild", "task": task, "rep": rep, "base": base_sha(), "started": t0, "minutes": round(o["seconds"] / 60, 1),
            "cost_usd": round(o["cost_usd"], 2), "turns": o["turns"], "tool_calls": o["tool_calls"], "summary": o["result"],
            "human_interventions": 0, "orchestrator_permissions": o["tool_calls"].get("mcp__guild__guild_permission", 0),
            "orchestrator_rejections": o["tool_calls"].get("mcp__guild__guild_decide:reject", 0)}
    meta.update(kiro_credits(t0, t1))
    if quest:
        ref = quest["integration_ref"]
        tasks = [t for t in snap["tasks"] if t.get("quest_id") == quest["id"]]
        atts = [a for a in snap["attempts"] if a.get("task_id") in {t["id"] for t in tasks}]
        meta.update({"quest": quest["id"], "quest_state": quest["state"], "ref": ref, "guild_tasks": len(tasks), "guild_attempts": len(atts)})
        path = worktree(f"{task}-r{rep}-guild", ref)
        meta["diff"] = snapshot_diff(path, d / "guild.diff")
        v = verify(path, d / "guild.verify.txt")
        meta.update({"verify_ok": v["ok"], "verify_last": v["last"]})
    else:
        (d / "guild.diff").write_text("")
        meta.update({"quest": None, "verify_ok": False, "verify_last": "no quest", "diff": {"files": 0, "added": 0, "removed": 0}})
    save(d, "guild", meta)


def judge(task, rep):
    d = RESULTS / task / f"r{rep}"
    metas = {a: json.loads((d / f"{a}.json").read_text()) for a in ARMS if (d / f"{a}.json").exists()}
    if len(metas) < 3: log("judge skipped, missing arms", task, rep, list(metas)); return
    rng = random.Random(f"{task}-{rep}")
    out = []
    for order in range(2):
        arms = ARMS[:]; rng.shuffle(arms)
        key = dict(zip("PQR", arms))
        jd = d / f"judge{order + 1}"; jd.mkdir(exist_ok=True)
        for label, arm in key.items(): shutil.copy(d / f"{arm}.diff", jd / f"{label}.diff")
        ver = "\n".join(f"- {label}: {'pasa' if metas[arm]['verify_ok'] else 'falla'} ({metas[arm]['verify_last']})" for label, arm in key.items())
        log("judge", task, rep, order + 1, key)
        j = claude(REPO, prompt("judge", TASK=task_text(task), DIR=jd, REPO=REPO, VERIFY=ver), jd / "transcript.jsonl", "Read,Glob,Grep",
                   extra=["--add-dir", str(jd), "--disallowedTools", "Edit,Write,Bash,NotebookEdit"])
        m = re.findall(r"```json\s*(\{.*?\})\s*```", j["result"], re.S)
        verdict = json.loads(m[-1]) if m else None
        out.append({"key": key, "verdict": verdict, "cost_usd": round(j["cost_usd"], 2)})
    (d / "judge.json").write_text(json.dumps(out, indent=2, ensure_ascii=False))


def report():
    crit = ["correctness", "spec_coverage", "test_quality", "simplicity", "fit_with_codebase", "design_alignment", "ux"]
    rows, per_arm = [], {a: {"n": 0, "ok": 0, "min": 0, "usd": 0, "kiro": 0, "score": 0, "scored": 0, "rank": 0, "wins": 0, "verdicts": 0} for a in ARMS}
    for task in TASKS:
        for d in sorted((RESULTS / task).glob("r*")) if (RESULTS / task).exists() else []:
            judged = json.loads((d / "judge.json").read_text()) if (d / "judge.json").exists() else []
            for arm in ARMS:
                f = d / f"{arm}.json"
                if not f.exists(): continue
                m = json.loads(f.read_text())
                scores, ranks = [], []
                for jg in judged:
                    v = jg.get("verdict")
                    if not v: continue
                    label = next(l for l, a in jg["key"].items() if a == arm)
                    scores.append(sum(v[label].get(c, 0) for c in crit) / len(crit))
                    ranks.append(v["ranking"].index(label) + 1)
                score = sum(scores) / len(scores) if scores else None
                rank = sum(ranks) / len(ranks) if ranks else None
                rows.append((task, d.name, arm, m["verify_ok"], m["minutes"], m["cost_usd"], m.get("kiro_credits"), m["diff"]["files"], m["diff"]["added"], m["diff"]["removed"], score, rank))
                a = per_arm[arm]; a["n"] += 1; a["ok"] += m["verify_ok"]; a["min"] += m["minutes"]; a["usd"] += m["cost_usd"]; a["kiro"] += m.get("kiro_credits") or 0
                if score is not None: a["score"] += score; a["rank"] += rank; a["scored"] += 1; a["wins"] += sum(r == 1 for r in ranks); a["verdicts"] += len(ranks)
    fmt = lambda x, p=1: "—" if x is None else (f"{x:.{p}f}" if isinstance(x, float) else str(x))
    print("| Tarea | Rép. | Brazo | Verifica | Min | USD | Kiro | Archivos | +/− | Puntaje juez (0–5) | Puesto medio |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for r in rows:
        print(f"| {r[0]} | {r[1]} | {r[2]} | {'sí' if r[3] else 'no'} | {fmt(r[4])} | {fmt(r[5], 2)} | {fmt(r[6])} | {r[7]} | +{r[8]} −{r[9]} | {fmt(r[10], 2)} | {fmt(r[11], 2)} |")
    print("\n| Brazo | Corridas | Verifica | Min medio | USD medio | Kiro medio | Puntaje medio | Puesto medio | 1.er puesto (veredictos) |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for arm, a in per_arm.items():
        if not a["n"]: continue
        n, s = a["n"], a["scored"]
        print(f"| {arm} | {n} | {a['ok']}/{n} | {a['min'] / n:.1f} | {a['usd'] / n:.2f} | {a['kiro'] / n:.1f} | {fmt(a['score'] / s if s else None, 2)} | {fmt(a['rank'] / s if s else None, 2)} | {a['wins']}/{a['verdicts']} |")


def main():
    args = sys.argv[1:]
    if not args: print(__doc__); return
    cmd = args[0]
    if cmd == "run":
        task, rep, arm = args[1], int(args[2]), args[3]
        (run_solo if arm == "solo" else run_guild)(task, rep)
    elif cmd == "judge": judge(args[1], int(args[2]))
    elif cmd == "all":
        reps = int(args[args.index("--reps") + 1]) if "--reps" in args else 2
        for rep in range(1, reps + 1):
            for i, task in enumerate(TASKS):
                # Alternate which arm goes first, so time of day and load do not favour one.
                order = ["solo", "guild"] if (rep + i) % 2 else ["guild", "solo"]
                for arm in order:
                    if (RESULTS / task / f"r{rep}" / f"{arm}.json").exists(): continue
                    try: (run_solo if arm == "solo" else run_guild)(task, rep)
                    except Exception as e: log("FAILED", task, rep, arm, repr(e))
                if not (RESULTS / task / f"r{rep}" / "judge.json").exists():
                    try: judge(task, rep)
                    except Exception as e: log("JUDGE FAILED", task, rep, repr(e))
        report()
    elif cmd == "report": report()
    else: print(__doc__)


if __name__ == "__main__":
    main()
