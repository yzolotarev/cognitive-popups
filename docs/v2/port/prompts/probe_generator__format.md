# probe_generator.py: FORMAT

- Источник: `src/cognitive_popups/probe_generator.py`
- Статус переноса: **PORT**

```text
Ответ — только JSON, коротко, без пояснений. Вариация двух видов.
drill (нечисловая операция): {"kind":"drill","pairs":[{"prompt":"вход","answer":"результат одного шага"}, … 7 разных пар; первая — образец]}
numeric (числовая): {"kind":"numeric","numeric":true,"example":{"before":"…","after":"…"},"params":{"a":{"kind":"int","min":1,"max":9}},"derived":{"r":"2*a"},"check":"r==2*a","prompt":"… {a} …","answer":"{r}"} — params только целые; check истинен при всех значениях; язык: + - * / ** % == != < <= > >= and or not gcd divides abs.

```
