#!/usr/bin/env python3
"""cmp.py [--strict] A.out.tsv B.out.tsv CASES.tsv
Compares two xcmp outputs. Both files must cover exactly the cases of CASES.tsv, in order, with the same number of
replies per case. Four independent counts:
  end    cases whose final composition, segments or joined commits differ
  steps  replies that differ (handled, composing, list open, commit, composition, cursor; candidate lists excluded)
  first  cases whose first candidate differs
  later  cases whose later candidates differ (first equal)
--strict (same system, e.g. a Windows regression): all four must be 0.
default (across systems): end = 0, first = 0, steps <= 0.01% of all replies, later <= 0.1% of the cases; every case with a
differing reply is listed. Exit 0 = pass, 1 = fail, 2 = the inputs are not comparable."""
import collections, sys

args = [x for x in sys.argv[1:] if x != "--strict"]
strict = len(args) != len(sys.argv) - 1
a = [l.rstrip("\n").split("\t") for l in open(args[0], encoding="utf-8")]
b = [l.rstrip("\n").split("\t") for l in open(args[1], encoding="utf-8")]
cases = [l.rstrip("\n").split("\t") for l in open(args[2], encoding="utf-8") if l.strip()]
ids = [c[0] for c in cases]
problems = []
if not cases:
    problems.append("no cases")
for name, rows in (("A", a), ("B", b)):
    if [r[0] for r in rows] != ids:
        problems.append(f"{name}: the ids do not match the case file ({len(rows)} rows, {len(ids)} cases)")
    if any(len(r) != 6 for r in rows):
        problems.append(f"{name}: a row does not have 6 fields")
if not problems and any(len(x[5].split()) != len(y[5].split()) or not x[5] for x, y in zip(a, b)):
    problems.append("a case has no replies or a different number of replies in the two files")
if problems:
    print("\n".join(problems))
    sys.exit(2)
sets = {c[0]: c[1] for c in cases}
per, tot = collections.Counter(), collections.Counter()
end = steps = first = later = replies = shown = 0
step_cases = []
for x, y in zip(a, b):
    tot[sets[x[0]]] += 1
    sx, sy = x[5].split(), y[5].split()
    replies += len(sx)
    d = sum(1 for p, q in zip(sx, sy) if p != q)
    e = x[1] != y[1] or x[2] != y[2] or x[4] != y[4]
    f = x[3].split("/")[0] != y[3].split("/")[0]
    l = not f and x[3] != y[3]
    end += e; steps += d; first += f; later += l
    if d:
        step_cases.append(f"{x[0]} ({d} replies)")
    if e or d or f or l:
        per[sets[x[0]]] += 1
        if shown < 8:
            shown += 1
            for i, n in ((1, "comp"), (2, "segs"), (3, "cands"), (4, "commit")):
                if x[i] != y[i]:
                    print(x[0], n, "\n  A:", x[i][:160], "\n  B:", y[i][:160])
print("cases", len(a), "replies", replies, "cases with any difference per set", dict(per), "of", dict(tot))
print(f"end={end} steps={steps} ({steps / replies:.5%} of replies) first={first} later={later} ({later / len(a):.4%} of cases)")
if step_cases:
    print("cases with differing replies:", ", ".join(step_cases))
ok = end == 0 and first == 0 and ((steps == 0 and later == 0) if strict else (steps <= 0.0001 * replies and later <= 0.001 * len(a)))
print(("PASS" if ok else "FAIL") + (" (strict)" if strict else ""))
sys.exit(0 if ok else 1)
