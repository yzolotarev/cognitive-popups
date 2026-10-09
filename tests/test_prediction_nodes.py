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
    assert sure["mark"] == "wrong" and nodes.describe("wrong", ["a", "b", "c"]) == "✗\na · b · c"


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
    assert check.status == "partially_confirmed" and check.display == "≈\nнумерация · порядок · граница · матрица"


def test_the_switch_is_a_file_and_history_is_kept(tmp_path):
    assert nodes.enabled(tmp_path)
    (tmp_path / nodes.OFF_FILE).touch()
    assert not nodes.enabled(tmp_path)
    nodes.remember(tmp_path, ["a", "b", "c"])
    nodes.remember(tmp_path, ["d", "e", "f"])
    assert nodes.recent_words(nodes.load_history(tmp_path), 1) == ["d", "e", "f"]
