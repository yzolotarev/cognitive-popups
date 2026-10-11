"""Мгновенный черновик «4 слов» без модели (миллисекунды).

Что умеет:
  * выбрать до 4 понятий ИЗ текста (слой term), разных, а не похожих;
  * вытащить словарь обозначений, которые текст сам определяет («где ψ это ...»);
  * найти связи между выбранными понятиями (встречаются в одних предложениях).

Чего НЕ умеет (и не должен): слой simple (категория) и meaning (смысл),
восстановление испорченных распознаванием формул. Это остаётся за моделью.

Формула веса понятия:
    вес = частота_в_выделении^0.7 * неожиданность * (1 + бонус_определения) * (1 + бонус_имени)
    неожиданность = log((N_фона + V) / (частота_в_фоне + 1))
Фон лучше строить из уже прочитанных пользователем текстов: тогда «редкое»
значит «новое для него». Выбор четвёрки: жадное покрытие предложений с
убывающей отдачей (у такого выбора есть гарантия качества около 63 % от лучшего).

Только стандартная библиотека.
"""
from __future__ import annotations

import math
import re
from collections import Counter, defaultdict

DASH = "\u2014"  # длинное тире в исходных текстах
NDASH = "\u2013"

STOP = set("""
и в во не что он на я с со как а то все она так его но да ты к у же вы за бы по только ее мне было вот от меня еще нет о из ему теперь когда даже ну вдруг ли если уже или ни быть был него до вас нибудь опять уж вам ведь там потом себя ничего ей может они тут где есть надо ней для мы тебя их чем была сам чтоб без будто чего раз тоже себе под будет ж тогда кто этот того потому этого какой совсем ним здесь этом один почти мой тем чтобы нее сейчас были куда зачем всех никогда можно при наконец два об другой хоть после над больше тот через эти нас про всего них какая много разве три эту моя впрочем хорошо свою этой перед иногда лучше чуть том нельзя такой им более всегда конечно всю между это эта также такие который которые которая которое которых является являются может могут каждый данный данного
the a an of to in and or is are be was were for on with as by that this these those it its from at which can not we our their such may also into than then there here where when what how
""".split())

RU_ENDINGS = sorted("""
ами ями ого его ому ему ыми ими ах ях ой ей ий ый ая яя ое ее ую юю ов ев ом ем ам ям ую ия ья ие ье ий ость ости остью ение ения ению ением ении ание ания анию анием ании ться тся ешь ет ют ут ит ат ят ал ала али ило ыл ыла ыли ть а я о е ы и у ю ь
""".split(), key=len, reverse=True)

WORD_RE = re.compile(r"[A-Za-zА-Яа-яЁё][A-Za-zА-Яа-яЁё\-]*|[Α-Ωα-ωħℏ∂∇]")
SENT_RE = re.compile(r"(?<=[.!?…])\s+|\n{2,}")


def stem(word: str) -> str:
    w = word.lower().replace("ё", "е")
    if not re.match(r"[а-я]", w):
        return w[:-1] if len(w) > 4 and w.endswith("s") else w
    for end in RU_ENDINGS:
        if w.endswith(end) and len(w) - len(end) >= 4:
            return w[: -len(end)]
    return w


def sentences(text: str) -> list[str]:
    return [s.strip() for s in SENT_RE.split(text) if s and s.strip()]


def build_background(texts) -> Counter:
    """Фон: частоты основ по уже прочитанным текстам (или любому корпусу)."""
    c: Counter = Counter()
    for t in texts:
        for w in WORD_RE.findall(t):
            if w.lower() not in STOP:
                c[stem(w)] += 1
    return c


DEF_PATTERNS = [
    re.compile(r"(?:называется|называют|называем|is called|we call)\s+([A-Za-zА-Яа-яЁё][\w\-]*(?:\s+[A-Za-zА-Яа-яЁё][\w\-]*)?)", re.I),
    re.compile(r"([A-Za-zА-Яа-яЁё][\w\-]*(?:\s+[A-Za-zА-Яа-яЁё][\w\-]*)?)\s+(?:называется|называют)", re.I),
    re.compile(r"([A-Za-zА-Яа-яЁё][\w\-]*)\s*[" + DASH + NDASH + r"-]\s*это\b", re.I),
    re.compile(r"(?:обозначим|denote)\s+[^.]{0,40}?(?:через|by)\s+([^\s,.;]+)", re.I),
]
SYMBOL_DEF = re.compile(
    r"(?:где|here|where)?\s*([A-Za-zΑ-Ωα-ωħℏ∂∇][A-Za-z0-9_'^]{0,3})\s*(?:[" + DASH + NDASH +
    r"]|\s-\s|\bэто\b|\bis\b|\bdenotes\b)\s+([^,.;:\n]{3,60})")


def extract_symbols(text: str) -> list[dict]:
    """Словарь обозначений, которые текст определяет сам."""
    out, seen = [], set()
    for m in SYMBOL_DEF.finditer(text):
        sym, meaning = m.group(1).strip(), m.group(2).strip()
        if len(sym) > 4 or sym.lower() in STOP or sym in seen:
            continue
        if len(sym) > 1 and sym.isalpha() and sym[0].islower() and not re.match(r"[α-ω]", sym):
            continue  # обычное слово, а не обозначение
        seen.add(sym)
        out.append({"symbol": sym, "meaning": meaning})
    return out


def draft_cues(text: str, background: Counter | None = None, k: int = 4) -> dict:
    sents = sentences(text) or [text]
    bg = background or Counter()
    bg_total = sum(bg.values()) + 1
    bg_vocab = len(bg) + 1

    tf: Counter = Counter()
    forms: dict[str, Counter] = defaultdict(Counter)
    sent_of: dict[str, set] = defaultdict(set)
    capital: Counter = Counter()
    for si, s in enumerate(sents):
        toks = WORD_RE.findall(s)
        for ti, w in enumerate(toks):
            if w.lower() in STOP or (len(w) < 3 and not re.match(r"[Α-Ωα-ωħℏ∂∇]", w)):
                continue
            st = stem(w)
            tf[st] += 1
            forms[st][w] += 1
            sent_of[st].add(si)
            if ti > 0 and w[0].isupper():
                capital[st] += 1
        # двусловные термины: соседние значимые слова
        sig = [w for w in toks if w.lower() not in STOP and len(w) >= 3]
        for a, b in zip(sig, sig[1:]):
            key = stem(a) + " " + stem(b)
            tf[key] += 0  # регистрируем, частоту считаем ниже
            forms[key][a + " " + b] += 1
            sent_of[key].add(si)

    defined = set()
    for pat in DEF_PATTERNS:
        for m in pat.finditer(text):
            for w in m.group(1).split():
                defined.add(stem(w))

    scores: dict[str, float] = {}
    for st, f in tf.items():
        if " " in st:
            a, b = st.split(" ")
            cnt = sum(forms[st].values())
            if cnt < 2 and not (capital[b] or capital[a]):
                continue
            base = (scores.get(a, 0) + scores.get(b, 0)) * 0.55 if a in scores and b in scores else 0
            scores[st] = base * (1.15 if cnt >= 2 else 1.0)
            continue
        surpr = math.log((bg_total + bg_vocab) / (bg.get(st, 0) + 1))
        bonus_def = 0.8 if st in defined else 0.0
        bonus_cap = 0.3 if capital[st] else 0.0
        scores[st] = (f ** 0.7) * surpr * (1 + bonus_def) * (1 + bonus_cap)
    # пересчёт двусловных после одиночных
    for st in list(scores):
        if " " in st:
            a, b = st.split(" ")
            cnt = sum(forms[st].values())
            scores[st] = (scores.get(a, 0) + scores.get(b, 0)) * 0.55 * (1.15 if cnt >= 2 else 1.0)

    covered: set = set()
    chosen: list[str] = []
    cand = sorted(scores, key=scores.get, reverse=True)[:60]
    while len(chosen) < k and cand:
        best, best_gain = None, 0.0
        for st in cand:
            if any(_too_close(st, c) for c in chosen):
                continue
            ss = sent_of[st]
            new = len(ss - covered) / max(1, len(ss))
            gain = scores[st] * (0.35 + 0.65 * new)
            if gain > best_gain:
                best, best_gain = st, gain
        if best is None:
            break
        chosen.append(best)
        covered |= sent_of[best]
        cand.remove(best)

    cues = [{"term": min(forms[st].items(), key=lambda kv: (-kv[1], len(kv[0])))[0], "score": round(scores[st], 3),
             "sentences": sorted(sent_of[st])} for st in chosen]
    edges = []
    for i in range(len(chosen)):
        for j in range(i + 1, len(chosen)):
            shared = len(sent_of[chosen[i]] & sent_of[chosen[j]])
            if shared:
                edges.append({"a": i, "b": j, "weight": shared})
    return {"cues": cues, "edges": edges, "symbols": extract_symbols(text)}


def _too_close(a: str, b: str) -> bool:
    pa, pb = set(a.split(" ")), set(b.split(" "))
    if pa & pb:
        return True
    return any(x[:5] == y[:5] for x in pa for y in pb if len(x) >= 5 and len(y) >= 5)


if __name__ == "__main__":
    import json, sys
    src = open(sys.argv[1], encoding="utf-8").read() if len(sys.argv) > 1 else (
        "Энтропия " + DASH + " мера беспорядка в системе. Второй закон термодинамики утверждает, "
        "что в замкнутой системе энтропия со временем не убывает и задаёт направление стрелы времени. "
        "В статистической физике энтропия связана с числом микросостояний, где W " + DASH +
        " число микросостояний.")
    print(json.dumps(draft_cues(src), ensure_ascii=False, indent=2))

