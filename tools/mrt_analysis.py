#!/usr/bin/env python3
"""MRT analysis over the journal database (P8, OD-06, B12, B13).

    python3 tools/mrt_analysis.py journal.db
    python3 tools/mrt_analysis.py --self-test

Reads `mrt_decisions` and `nudge_shows` written by the `nudges` crate.
Prints, per arm, the verified result (correct among checked), missing,
engagement; then the diagnostic-vs-silence difference with a 95 % interval.
It refuses a conclusion until an A/A check has passed and each arm has the
sample computed for the effect size below. Engagement and the verified result
stay separate (B13): the trial is judged on the verified result only.
"""
import argparse
import math
import random
import sqlite3
import sys

# Smallest effect worth acting on: +15 points of correct answers on a base of 60 %.
BASE, EFFECT, ALPHA_Z, POWER_Z = 0.60, 0.15, 1.96, 0.84


def needed_per_arm(base=BASE, effect=EFFECT):
    p1, p2 = base, base + effect
    pbar = (p1 + p2) / 2
    n = (ALPHA_Z * math.sqrt(2 * pbar * (1 - pbar))
         + POWER_Z * math.sqrt(p1 * (1 - p1) + p2 * (1 - p2))) ** 2 / effect ** 2
    return math.ceil(n)


def compare(a, b):
    """Two-proportion z and a Wald 95 % interval for rate(a) - rate(b)."""
    na, nb = a["correct"] + a["incorrect"], b["correct"] + b["incorrect"]
    if not na or not nb:
        return None
    pa, pb = a["correct"] / na, b["correct"] / nb
    pooled = (a["correct"] + b["correct"]) / (na + nb)
    se0 = math.sqrt(pooled * (1 - pooled) * (1 / na + 1 / nb)) or float("inf")
    se = math.sqrt(pa * (1 - pa) / na + pb * (1 - pb) / nb)
    diff = pa - pb
    return {"diff": diff, "z": diff / se0, "low": diff - 1.96 * se, "high": diff + 1.96 * se}


def arms(db):
    rows = db.execute(
        """SELECT arm, COUNT(*), SUM(outcome='correct'), SUM(outcome='incorrect'),
                  SUM(outcome='missing'), SUM(engaged=1)
           FROM mrt_decisions WHERE eligible = 1 GROUP BY arm""").fetchall()
    return {r[0]: {"n": r[1], "correct": r[2] or 0, "incorrect": r[3] or 0,
                   "missing": r[4] or 0, "engaged": r[5] or 0} for r in rows}


def report(db):
    data = arms(db)
    need = needed_per_arm()
    print(f"нужно на ветку: {need} проверенных исходов (база {BASE:.0%}, эффект +{EFFECT:.0%})")
    for name, a in sorted(data.items()):
        checked = a["correct"] + a["incorrect"]
        rate = a["correct"] / checked if checked else float("nan")
        print(f"  {name:10} решений {a['n']:5}  проверено {checked:5}  верно {rate:6.1%}"
              f"  пропуск {a['missing'] / a['n']:6.1%}  открыл {a['engaged']:5}")
    ineligible = db.execute(
        "SELECT silence, COUNT(*) FROM mrt_decisions WHERE eligible = 0 GROUP BY silence").fetchall()
    if ineligible:
        print("  вне эксперимента (осмысленное молчание): "
              + ", ".join(f"{s} {n}" for s, n in ineligible))
    shows = db.execute("SELECT COUNT(*), SUM(response='not_now') FROM nudge_shows").fetchone()
    if shows and shows[0]:
        print(f"guardrail: «Не сейчас» {shows[1] or 0} из {shows[0]} показов")

    aa = compare(data.get("aa_first", {"correct": 0, "incorrect": 0}),
                 data.get("aa_second", {"correct": 0, "incorrect": 0}))
    if aa is None:
        print("ВЫВОДА НЕТ: A/A-проверка ещё не проводилась.")
        return 2
    print(f"A/A: разница {aa['diff']:+.1%}, z = {aa['z']:.2f}")
    if abs(aa["z"]) >= 1.96:
        print("ВЫВОДА НЕТ: A/A показывает разницу между одинаковыми ветками — сначала найти причину.")
        return 2
    main = compare(data.get("diagnostic", {"correct": 0, "incorrect": 0}),
                   data.get("silence", {"correct": 0, "incorrect": 0}))
    small = [k for k in ("diagnostic", "silence")
             if data.get(k, {}).get("correct", 0) + data.get(k, {}).get("incorrect", 0) < need]
    if main is None or small:
        print(f"ВЫВОДА НЕТ: мало данных в ветках {small or ['diagnostic', 'silence']}.")
        return 2
    miss = {k: data[k]["missing"] / data[k]["n"] for k in ("diagnostic", "silence")}
    if abs(miss["diagnostic"] - miss["silence"]) > 0.10:
        print("ОСТОРОЖНО: доля пропусков в ветках различается больше чем на 10 пунктов.")
    print(f"диагностика − молчание: {main['diff']:+.1%} "
          f"(95 % интервал {main['low']:+.1%} … {main['high']:+.1%}), z = {main['z']:.2f}")
    return 0


def self_test(trials=200, per_arm=400):
    """A/A on synthetic data: identical arms must look different in about 5 % of runs."""
    rng = random.Random(7)
    false_alarms = 0
    for _ in range(trials):
        a = {"correct": 0, "incorrect": 0}
        b = {"correct": 0, "incorrect": 0}
        for arm in (a, b):
            for _ in range(per_arm):
                arm["correct" if rng.random() < BASE else "incorrect"] += 1
        if abs(compare(a, b)["z"]) >= 1.96:
            false_alarms += 1
    rate = false_alarms / trials
    print(f"self-test: ложные срабатывания A/A {rate:.1%} (ожидается около 5 %)")
    return 0 if rate < 0.10 else 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("db", nargs="?")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.db:
        parser.error("give the database path or --self-test")
    with sqlite3.connect(f"file:{args.db}?mode=ro", uri=True) as db:
        return report(db)


if __name__ == "__main__":
    sys.exit(main())
