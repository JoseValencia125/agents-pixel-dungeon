import json, sys, glob, os, datetime as dt
# Credits of Kiro turns that ended inside [t0, t1]: the v3 engine's sessions
# (what the guild runs over ACP) keep a usage_summary per turn in messages.jsonl.
t0, t1 = (dt.datetime.fromisoformat(x) for x in sys.argv[1:3])
total = 0.0; turns = 0; by_model = {}
for f in glob.glob(os.path.expanduser("~/.kiro/sessions/*/sess_*/messages.jsonl")):
    try: model = json.load(open(os.path.join(os.path.dirname(f), "session.json"))).get("modelId", "?")
    except Exception: model = "?"
    for line in open(f):
        if '"usage_summary"' not in line: continue
        m = json.loads(line)
        at = dt.datetime.fromisoformat(m["timestamp"].replace("Z", "+00:00"))
        if not (t0 <= at <= t1): continue
        used = sum(s.get("usage", 0) for s in m["payload"].get("promptTurnSummaries", []))
        turns += 1; total += used; by_model[model] = by_model.get(model, 0) + used
print(json.dumps({"kiro_credits": round(total, 2), "kiro_turns": turns, "by_model": {k: round(v, 2) for k, v in by_model.items()}}))
