"""Mark + four nodes: the model proposes, the script checks."""
import json

from cognitive_popups import context, prediction_nodes as nodes
from cognitive_popups.service import CognitiveService

SOURCE = "col_index_num - из какого по счёту столбца выбранного диапазона вернуть результат. Нумерация идёт слева."
HYPOTHESIS = "col значит количество, то есть какой по счёту"


def answer(**fields):
    base = {"mark": "partly", "sure": True, "gap_quote": "из какого по счёту столбца", "specific": True,
            "words": ["нумерация", "порядок", "граница", "матрица"]}
    return dict(base, **fields)


def test_nodes_that_echo_the_guess_or_sit_in_the_answer_sentence_are_dropped():
    shown = nodes.assemble(answer(words=["нумерация", "столбец", "счёт", "диапазон"]), HYPOTHESIS, SOURCE, {})
    assert shown["dropped"] == {"столбец": "из фразы с ответом", "счёт": "эхо догадки", "диапазон": "из фразы с ответом"}
    assert shown["words"] == []                       # one left: silence, not a padded set


def test_generic_nodes_issued_many_times_are_dropped_and_three_are_enough():
    history = {"issued": [["структура", "стиль"]] * 3}
    candidates = ["структура", "порядок", "граница", "матрица", "индекс", "сдвиг"]
    shown = nodes.assemble(answer(words=candidates), HYPOTHESIS, SOURCE, history)
    assert shown["dropped"] == {"структура": "шаблон: уже много раз выдавалось"}
    assert shown["words"] == ["порядок", "граница", "матрица", "индекс"]   # first four of what is left
    three = nodes.assemble(answer(words=candidates[:4]), HYPOTHESIS, SOURCE, history)
    assert three["words"] == []                 # three would break the four-cue window: silence


def test_unspecific_set_is_silence_and_a_doubtful_partly_is_withheld():
    shown = nodes.assemble(answer(specific=False, sure=False), HYPOTHESIS, SOURCE, {})
    assert shown["words"] == [] and shown["mark"] == "" and shown["raw_mark"] == "partly"
    assert nodes.describe(shown["mark"], shown["words"]) == "нечего добавить"
    sure = nodes.assemble(answer(mark="wrong"), HYPOTHESIS, SOURCE, {})
    assert sure["mark"] == "wrong" and nodes.describe("wrong", ["a", "b", "c"]) == "?\na · b · c"


def test_check_prediction_nodes_returns_marked_nodes_and_never_an_explanation(tmp_path):
    class Client:
        calls = []

        def complete(self, messages, **kwargs):
            self.calls.append(messages)
            return json.dumps(answer(), ensure_ascii=False)
    service = CognitiveService(Client())
    check = service.check_prediction_nodes(HYPOTHESIS, context.pack(SOURCE, root=tmp_path, now=1),
                                           history={"issued": [["старое"]]}, mine=["моя мысль"])
    request = Client.calls[0][1]["content"]
    assert "<USED>\nстарое" in request and "- моя мысль" in request and "<HYPOTHESIS>" in request
    assert check.nodes == ["нумерация", "порядок", "граница", "матрица"] and check.mark == "partly"
    assert check.status == "partially_confirmed" and check.display == (
        "≈ частично\nсмотри: «из какого по счёту столбца»\nузлы: нумерация · порядок · граница · матрица")
    assert check.one_delta == ""


def test_the_switch_is_a_file_and_history_is_kept(tmp_path):
    assert nodes.enabled(tmp_path)
    (tmp_path / nodes.OFF_FILE).touch()
    assert not nodes.enabled(tmp_path)
    nodes.remember(tmp_path, ["a", "b", "c"])
    nodes.remember(tmp_path, ["d", "e", "f"])
    assert nodes.recent_words(nodes.load_history(tmp_path), 1) == ["d", "e", "f"]


def test_a_vague_guess_stands_only_when_nothing_was_refuted():
    assert nodes.assemble(answer(vague=True, mark="ok"), HYPOTHESIS, SOURCE, {})["vague"]
    refuted = nodes.assemble(answer(vague=True, mark="wrong"), HYPOTHESIS, SOURCE, {})
    assert not refuted["vague"] and refuted["mark"] == "wrong"     # the error is worth more than "bolder"
    question = nodes.assemble(answer(mark="none"), HYPOTHESIS, SOURCE, {})
    assert question["mark"] == "" and question["raw_mark"] == "none"   # a question is not marked wrong


def test_vague_guess_gets_bolder_once_and_is_marked_on_insist(tmp_path):
    class Client:
        def complete(self, messages, **kwargs):
            return json.dumps(answer(vague=True, mark="ok"), ensure_ascii=False)
    service = CognitiveService(Client())
    pack = context.pack(SOURCE, root=tmp_path, now=1)
    first = service.check_prediction_nodes("функция имеет определённые параметры", pack)
    assert first.status == nodes.VAGUE_STATUS and first.display == nodes.BOLDER
    assert first.mark == "" and first.nodes == []
    second = service.check_prediction_nodes("функция имеет определённые параметры", pack, insist=True)
    assert second.status == "confirmed" and second.mark == "ok"
    assert [c.status for c in service.session.prediction_checks] == [nodes.VAGUE_STATUS, "confirmed"]  # both kept


# --- honest verdicts: script gates over the model (swarm C, M1-M3) -------------

PASSAGE = ("Номенклатура дел — это перечень дел организации с указанием сроков хранения. "
           "Каждому делу присваивается индекс, а документ получает номер внутри дела.")


def test_selection_made_of_issued_nodes_is_a_pointer_not_the_ground():
    # 12.10 05:06: the selection was the previous turn's nodes, and a guess built
    # from them was "confirmed" by them.
    ground, focus = nodes.choose_ground("таблица данные структура операция", PASSAGE,
                                        ["таблица", "данные", "структура", "операция"])
    assert ground == PASSAGE and focus == "таблица данные структура операция"


def test_short_selection_points_into_the_last_passage():
    ground, focus = nodes.choose_ground("индекс", PASSAGE, [])
    assert (ground, focus) == (PASSAGE, "индекс")


def test_a_real_passage_is_its_own_ground():
    assert nodes.choose_ground(PASSAGE, "другое", []) == (PASSAGE, "")


def test_a_question_is_never_marked():
    assert nodes.gate("partly", False, "Мы тут юзаем int и trunc для округления вниз или вверх?",
                      PASSAGE, []) == ("none", False, "question")


def test_glued_vague_labels_are_vague_whatever_the_model_said():
    # 8c2b270e, 317e2031: confirmed by the model, nothing anyone could refute.
    assert nodes.gate("ok", False, "задача имеет в себе уравнение определённой формы и метод решения",
                      PASSAGE, [])[1] is True


def test_a_tick_needs_something_of_this_passage():
    # c14b3779: none of these words is in the paragraph; checked against the nodes.
    mark, _, _ = nodes.gate("ok", False, "функция в алгоритме, алгоритм — набор операций",
                            PASSAGE, ["функция", "операция", "алгоритм", "трансформация"])
    assert mark == "partly"


def test_a_concrete_true_guess_keeps_its_tick():
    assert nodes.gate("ok", False, "у каждого дела свой индекс, а документ нумеруется внутри дела",
                      PASSAGE, []) == ("ok", False, "claim")


def test_the_answer_stays_readable_and_never_gives_the_answer_away():
    text = nodes.answer_text("wrong", "дело — это папка", "документ получает номер внутри дела", [])
    assert text.splitlines() == ["? расходится с текстом", "верно у тебя: дело — это папка",
                                 "смотри: «документ получает номер внутри дела»"]
    assert nodes.answer_text("", "", "", [], kind="question") == nodes.QUESTION_REPLY


def test_nodes_check_stores_the_mismatch_not_the_nodes(tmp_path):
    class Client:
        def complete(self, messages, **kwargs):
            return json.dumps({"vague": False, "mark": "wrong", "sure": True,
                               "gap_quote": "документ получает номер внутри дела",
                               "right": "у дела свой индекс", "mismatch": "у него индекс у документа, а не у дела",
                               "specific": True, "words": ["перечень", "срок", "хранение", "организация"]},
                              ensure_ascii=False)
    service = CognitiveService(Client())
    check = service.check_prediction_nodes("индекс есть и у документов, и у папок",
                                           context.pack(PASSAGE, root=tmp_path, now=1))
    assert check.mismatch == "у него индекс у документа, а не у дела"
    assert check.one_delta == "" and check.nodes == ["перечень", "срок", "хранение", "организация"]
    assert "верно у тебя: у дела свой индекс" in check.display


def test_a_question_gets_asked_for_its_answer(tmp_path):
    class Client:
        def complete(self, messages, **kwargs):
            return json.dumps({"vague": False, "mark": "partly", "sure": True, "gap_quote": "",
                               "specific": True, "words": ["a", "b", "c", "d"]})
    check = CognitiveService(Client()).check_prediction_nodes(
        "а индекс у документа есть?", context.pack(PASSAGE, root=tmp_path, now=1))
    assert check.status == "question" and check.mark == "" and check.nodes == []
    assert check.display == nodes.QUESTION_REPLY
