"""Offline comparison of draft terms with saved model cues. No model calls."""
import argparse
import json
import sqlite3
from pathlib import Path

from .instant_cues import build_background, draft_cues, stem, WORD_RE, STOP


def compare(path):
    db = sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True)
    try:
        rows = db.execute("SELECT id,source_text FROM fragments WHERE source_text IS NOT NULL ORDER BY created_epoch").fetchall()
        cases = []
        for fid, text in rows:
            terms = [r[0] for r in db.execute("SELECT term FROM cues WHERE fragment_id=? ORDER BY position", (fid,)) if r[0]]
            if not terms:
                continue
            background = build_background(other for _, other in rows if other != text)
            draft = [c["term"] for c in draft_cues(text, background)["cues"]]
            def roots(term):
                return {stem(w) for w in WORD_RE.findall(term) if w.lower() not in STOP}
            matched, available = [], list(draft)
            for term in terms:
                for d in available:
                    a, b = roots(term), roots(d)
                    if a and b and len(a & b) / len(a | b) >= .6:
                        matched.append(term)
                        available.remove(d)
                        break
            cases.append({"fragment_id": fid, "source": text[:500], "model": terms, "draft": draft, "matched": matched,
                          "overlap": len(matched), "total": len(terms)})
        ranked = sorted(cases, key=lambda r: r["overlap"] / r["total"])
        return {"cases": len(cases), "matched": sum(c["overlap"] for c in cases),
                "terms": sum(c["total"] for c in cases), "worst": ranked[:3], "best": ranked[-3:][::-1],
                "meaning": "Approximate stem overlap, not cue quality or learning quality. Duplicate source texts excluded from each background."}
    finally:
        db.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("db")
    args = parser.parse_args()
    print(json.dumps(compare(args.db), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
