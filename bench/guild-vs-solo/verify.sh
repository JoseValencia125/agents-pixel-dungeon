#!/bin/sh
# Verifies a HARNESS: Directive checkout the same way for every arm: the
# project's headless Godot tests plus every tools/test_*.gd the change added.
# Run from the checkout. BENCH_BASE is the commit the change started from.
base="${BENCH_BASE:-main}"
added=$( { git diff --name-only --diff-filter=A "$base" -- 'tools/test_*.gd'; git ls-files --others --exclude-standard -- 'tools/test_*.gd'; } | sed 's|tools/||; s|\.gd$||' | sort -u)
[ -d .godot ] || godot --headless --path . --import >/dev/null 2>&1
for t in test_stages test_save test_ui test_learn $added; do
	out=$(godot --headless --path . -s "tools/$t.gd" 2>&1)
	code=$?
	if [ $code -ne 0 ] || echo "$out" | grep -q 'SCRIPT ERROR'; then
		echo "$out" | tail -40
		echo "VERIFY FAILED: $t (exit $code)"
		exit 1
	fi
	echo "$t: $(echo "$out" | tail -1)"
done
echo "VERIFY OK"
