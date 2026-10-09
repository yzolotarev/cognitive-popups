"""Only what this moment needs: the selection plus the few passages taken just before."""
import json

from cognitive_popups import context
from cognitive_popups.prompts import clarify_universe_prompt

A = "Абзац первый: функция VLOOKUP ищет код в левом столбце справочника."
B = "Абзац второй: номер столбца говорит, откуда взять ответ, считая слева."
C = "Абзац третий: FALSE в конце требует точного совпадения кода целиком."
D = "Абзац четвёртый: XLOOKUP отдельно задаёт, где искать и откуда брать."


def test_remember_skips_words_dedupes_and_caps(tmp_path):
    context.remember("слово", "clarify", root=tmp_path, now=1)
    assert not (tmp_path / context.RING_FILE).exists()
    for index in range(20):
        context.remember(f"{A} {index}", "seed", root=tmp_path, now=index)
    context.remember(f"{A} 5", "note", root=tmp_path, now=100)   # taken again: moves to the end
    items = json.loads((tmp_path / context.RING_FILE).read_text())
    assert len(items) == context.RING_SIZE and items[-1]["text"] == f"{A} 5"
    assert sum(item["text"] == f"{A} 5" for item in items) == 1


def test_pack_is_recent_ordered_and_without_the_selection(tmp_path):
    for moment, text in ((0, A), (1500, B), (1700, C), (1800, D)):
        context.remember(text, "seed", root=tmp_path, now=moment)
    pack = context.pack(D, root=tmp_path, now=1900)
    # A is older than half an hour; D is the selection itself.
    assert [item["text"] for item in pack.recent] == [B, C]
    assert pack.recent_block().startswith("1) 6 мин назад: Абзац второй")
    assert pack.passages() == [B, C, D]
    material = pack.material()
    assert material.index("РАНЬШЕ") < material.index("СЕЙЧАС ВЫДЕЛЕНО") < material.index("четвёртый")


def test_a_larger_paragraph_around_the_selection_stays_as_surroundings(tmp_path):
    context.remember(B, "seed", root=tmp_path, now=10)
    assert [item["text"] for item in context.pack("номер столбца", root=tmp_path, now=20).recent] == [B]


def test_during_a_step_only_its_own_passages_count(tmp_path):
    (tmp_path / "focus.json").write_text(json.dumps({"id": "s1", "status": "running", "goal": "разберу X"}))
    context.remember(A, "seed", root=tmp_path, now=10)
    (tmp_path / "focus.json").write_text(json.dumps({"id": "s2", "status": "running", "goal": "разберу Y"}))
    context.remember(B, "seed", root=tmp_path, now=20)
    pack = context.pack(C, root=tmp_path, now=30)
    assert [item["text"] for item in pack.recent] == [B] and pack.step == "разберу Y"


def test_alt_c_prompt_puts_surroundings_first_and_the_point_last(tmp_path):
    user = clarify_universe_prompt("ВЫДЕЛЕНО", "вопрос?", [], recent="1) раньше", step="шаг")[1]["content"]
    assert user.index("<RECENT>") < user.index("<STEP>") < user.index("<SOURCE>") < user.index("<QUESTION>")
    bare = clarify_universe_prompt("ВЫДЕЛЕНО", "вопрос?", [])[1]["content"]
    assert "<RECENT>" not in bare and "<STEP>" not in bare


def test_hypothesis_reads_the_pack_not_the_whole_buffer(tmp_path):
    from cognitive_popups.service import CognitiveService
    reply = ('{"one_delta":"","status":"confirmed","mismatch":"",'
             '"evidence":"точного совпадения кода","subject_note":""}')

    class Client:
        calls = []

        def complete(self, messages, **kwargs):
            self.calls.append(messages)
            return reply
    service = CognitiveService(Client())
    service.add_fragment_with_cues("Совсем другой текст из переписки о разработке программы.",
                                   [{"simple": "a", "term": "b", "meaning": "c"}] * 4)
    context.remember(B, "seed", root=tmp_path, now=10)
    check = service.check_prediction("FALSE значит точно", context.pack(C, root=tmp_path, now=20))
    content = Client.calls[0][1]["content"]
    assert "СЕЙЧАС ВЫДЕЛЕНО" in content and "Абзац второй" in content
    assert "переписки о разработке" not in content
    assert check.evidence == "точного совпадения кода" and check.buffer_fragment_ids == []


def test_a_paragraph_taken_twice_with_small_differences_is_kept_once(tmp_path):
    context.remember(B, "seed", root=tmp_path, now=10)
    context.remember(B + " И ещё одно слово.", "seed", root=tmp_path, now=20)
    items = json.loads((tmp_path / context.RING_FILE).read_text())
    assert [item["at"] for item in items] == [20]


def test_hypothesis_reply_is_shown_as_written_and_only_the_quote_is_checked(tmp_path):
    import json
    from cognitive_popups.service import CognitiveService, hypothesis_answer
    hypothesis = "col значит количество, то есть какой по счёту"
    source = "col_index_num - из какого по счёту столбца выбранного диапазона вернуть результат."
    data = {"verdict": "partly", "reply": "Да, «какой по счёту» - верно: col это column, номер столбца.",
            "quote": "из какого по счёту столбца"}
    shown = hypothesis_answer(data, hypothesis, [source.lower()])
    assert shown["display"] == data["reply"] and shown["look"] == "из какого по счёту столбца"
    assert shown["ask"] == ""
    fake = dict(data, quote="нет такой фразы")
    assert hypothesis_answer(fake, hypothesis, [source.lower()])["look"] == ""

    class Client:
        def complete(self, messages, **kwargs):
            return json.dumps(data, ensure_ascii=False)
    check = CognitiveService(Client()).check_prediction(hypothesis, context.pack(source, root=tmp_path, now=1))
    assert check.status == "partially_confirmed" and check.text == data["reply"] and not check.ask
