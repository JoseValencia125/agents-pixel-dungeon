#!/usr/bin/env python3
"""A tiny word-combat game, headless, for the guild's experiments.

Each round the player gets a hand of words and picks one to hit the enemy.
A word's power is its letter power times a length multiplier times a
semantic multiplier when the word matches the enemy's weakness. Rare
letters refill slowly, so spending them has a cost. The balance lives in
balance.json next to this file; an experiment's variants change it.

Profiles (how a synthetic player chooses):
  greedy     always the longest word
  novice     a random word
  optimizer  the word with the highest immediate power
  explorer   prefers a semantic match, then the shortest word that still hits
  human      you: the hand is printed, you type the number of a word

Telemetry: one JSON object per line,
  {"type": "game.event", "runId": "...", "event": "...", "data": {...}}
on stdout, or appended to the file $GUILD_TELEMETRY names (always for a
human, whose screen is the game).
"""
import json, os, random, sys

HERE = os.path.dirname(os.path.abspath(__file__))

WORDS = [
    # word, category
    ("ember", "fire"), ("blaze", "fire"), ("inferno", "fire"), ("spark", "fire"), ("ash", "fire"),
    ("frost", "ice"), ("glacier", "ice"), ("ice", "ice"), ("snowfall", "ice"), ("chill", "ice"),
    ("gale", "wind"), ("tempest", "wind"), ("breeze", "wind"), ("whirlwind", "wind"), ("air", "wind"),
    ("quartz", "stone"), ("boulder", "stone"), ("pebble", "stone"), ("granite", "stone"), ("ore", "stone"),
]
RARE = set("qzxjkvw")
WEAKNESS = ["fire", "ice", "wind", "stone"]


def letter_power(word):
    return sum(3 if c in RARE else 1 for c in word)


def power(word, category, weakness, balance):
    p = letter_power(word) * (balance["length_multiplier"] ** max(0, len(word) - 3))
    if category == weakness:
        p *= balance["semantic_multiplier"]
    return p


def ask_human(hand, weakness, hp, rounds, rare_budget, balance):
    print()
    print(f"Round {rounds} · enemy HP {hp:.0f} · weak to {weakness.upper()} · rare letters left {rare_budget}")
    for i, (word, category) in enumerate(hand, 1):
        mark = "*" if category == weakness else " "
        print(f"  {i}. {word:<10} {category:<6}{mark}")
    while True:
        try:
            raw = input("Your word (1-5, q to quit): ").strip().lower()
        except EOFError:
            return None
        if raw == "q":
            return None
        if raw.isdigit() and 1 <= int(raw) <= len(hand):
            return hand[int(raw) - 1]
        print("Type a number from the list.")


def choose(profile, hand, weakness, balance, rng):
    if profile == "greedy":
        return max(hand, key=lambda w: (len(w[0]), w[0]))
    if profile == "novice":
        return rng.choice(hand)
    if profile == "optimizer":
        return max(hand, key=lambda w: power(w[0], w[1], weakness, balance))
    if profile == "explorer":
        matching = [w for w in hand if w[1] == weakness]
        pool = matching or hand
        return min(pool, key=lambda w: (len(w[0]), w[0]))
    return rng.choice(hand)


def run(profile, seed, balance, emit):
    rng = random.Random(seed)
    run_id = f"{profile}-{seed}"
    emit(run_id, "run.started", {"profile": profile, "seed": seed, **balance})
    rare_budget = 6
    hp = 100.0
    rounds = 0
    score = 0.0
    while hp > 0 and rounds < 12:
        rounds += 1
        weakness = rng.choice(WEAKNESS)
        hand = rng.sample(WORDS, 5)
        emit(run_id, "round.started", {"round": rounds, "weakness": weakness, "hp": hp})
        emit(run_id, "choice.presented", {"options": [w for w, _ in hand], "weakness": weakness})
        if profile == "human":
            picked = ask_human(hand, weakness, hp, rounds, rare_budget, balance)
            if picked is None:
                emit(run_id, "run.failed", {"rounds": rounds, "score": round(score, 2), "reason": "quit"})
                return
            word, category = picked
        else:
            word, category = choose(profile, hand, weakness, balance, rng)
        rare = sum(1 for c in word if c in RARE)
        if rare > rare_budget:
            # Not enough rare letters left: the word fizzles.
            dmg = 1.0
        else:
            rare_budget -= rare
            dmg = power(word, category, weakness, balance)
        rare_budget = min(6, rare_budget + 1)
        hp -= dmg
        score += dmg
        emit(run_id, "choice.selected", {"word": word, "length": len(word), "semantic": category == weakness, "category": category, "rare_letters": rare})
        emit(run_id, "damage.dealt", {"amount": round(dmg, 2), "hp_left": round(max(hp, 0.0), 2)})
        if profile == "human":
            print(f"  {word} hits for {dmg:.1f}" + (" (semantic match!)" if category == weakness else ""))
        if hp <= 0:
            emit(run_id, "enemy.defeated", {"round": rounds})
    if profile == "human":
        print("\nYou won!" if hp <= 0 else "\nThe enemy survived.")
    if hp <= 0:
        emit(run_id, "run.completed", {"rounds": rounds, "score": round(score, 2), "won": True})
    else:
        emit(run_id, "run.failed", {"rounds": rounds, "score": round(score, 2), "reason": "enemy survived"})


def main(argv):
    args = dict(a.split("=", 1) for a in argv[1:] if "=" in a)
    profile = args.get("profile", "novice")
    runs = int(args.get("runs", "1"))
    seed = int(args.get("seed", "1"))
    with open(os.path.join(HERE, "balance.json")) as f:
        balance = json.load(f)

    path = os.environ.get("GUILD_TELEMETRY")
    sink = open(path, "a") if path else (None if profile == "human" else sys.stdout)

    def emit(run_id, event, data):
        if sink is None:
            return
        sink.write(json.dumps({"type": "game.event", "runId": run_id, "event": event, "data": data}) + "\n")
        sink.flush()

    for i in range(runs):
        run(profile, seed + i, balance, emit)
    sys.stdout.flush()


if __name__ == "__main__":
    main(sys.argv)
