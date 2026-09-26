from pathlib import Path

from cognitive_popups.nodes import (
    APPLICATION,
    DEFINITION,
    EXAMPLE,
    EXERCISE,
    MAX_NODE_CHARS,
    METHOD,
    MODE_KINDS,
    ORIENTATION,
    PROOF,
    STATEMENT,
    TASKABLE_KINDS,
    TaskNode,
    clean_fragment,
    find_source_proof,
    parse_nodes,
    render_nodes,
    select_node,
    strip_wrapper,
    unmet_deps,
)

FIXTURE = Path(__file__).parent / "data" / "synthetic_study.txt"


def fixture_text() -> str:
    return FIXTURE.read_text(encoding="utf-8")


def test_synthetic_fragment_yields_all_relevant_kinds():
    kinds = {node.kind for node in parse_nodes(fixture_text())}
    assert {DEFINITION, METHOD, PROOF, STATEMENT, EXAMPLE, APPLICATION} <= kinds


def test_nodes_are_ordered_non_overlapping_and_inside_the_fragment():
    text = fixture_text()
    nodes = parse_nodes(text)
    assert nodes
    previous_end = 0
    for node in nodes:
        assert 0 <= node.start < node.end <= len(text)
        assert node.start >= previous_end
        assert node.text == clean_fragment(text)[node.start:node.end]
        previous_end = node.end


def test_node_ids_are_stable_for_the_same_fragment():
    text = fixture_text()
    assert [n.id for n in parse_nodes(text)] == [n.id for n in parse_nodes(text)]


def test_theorem_statement_and_its_proof_are_separate_nodes():
    text = (
        "Теорема 1. Две системы эквивалентны. "
        "Доказательство. Достаточно установить это для одного преобразования."
    )
    nodes = parse_nodes(text)
    assert [node.kind for node in nodes] == [STATEMENT, PROOF]
    assert nodes[0].title == "Теорема 1"
    assert nodes[1].title == "Доказательство"


def test_a_definition_is_recognised_by_its_phrasing():
    nodes = parse_nodes("Матрицей размера m×n называется прямоугольная таблица.")
    assert len(nodes) == 1
    assert nodes[0].kind == DEFINITION
    assert "distinguish" in nodes[0].ops


def test_naming_a_new_object_opens_a_node():
    # A definition can be introduced by "Назовём …" without an explicit heading.
    nodes = parse_nodes(
        "Введём обозначения. Назовём элементом множества любой его объект."
    )
    definitions = [node for node in nodes if node.kind == DEFINITION]
    assert len(definitions) == 1
    assert definitions[0].text.startswith("Назовём элементом")


def test_ocr_hyphenation_is_joined_without_touching_formulas():
    # Synthetic OCR repeats the trailing syllable: joining must not double it.
    assert clean_fragment("ко-\nкоробка") == "коробка"
    assert clean_fragment("цвет-\nцветной") == "цветной"
    assert clean_fragment("ко-\nробка") == "коробка"
    assert clean_fragment("x³ + ax²\n  + bx + c = 0") == "x³ + ax²\n  + bx + c = 0"


def test_named_cross_references_become_deps():
    nodes = parse_nodes("Доказательство. По теореме 1 и § 3 это следует.")
    assert nodes[0].kind == PROOF
    assert any("теорем" in dep for dep in nodes[0].deps)
    assert "§ 3" in nodes[0].deps


def test_a_node_does_not_depend_on_itself():
    nodes = parse_nodes("Теорема 4. Совместная система определена тогда и только тогда, когда г = п.")
    assert nodes[0].title == "Теорема 4"
    assert nodes[0].deps == ()


def test_a_long_block_is_split_at_sentence_boundaries():
    nodes = parse_nodes("Это достаточно длинное предложение про линейные системы. " * 60)
    assert len(nodes) > 1
    assert all(len(node.text) <= MAX_NODE_CHARS for node in nodes)


def test_application_overviews_are_labelled_application_not_method():
    nodes = parse_nodes(fixture_text())
    overviews = [node for node in nodes if node.title.startswith("4. Задача о")]
    assert overviews
    assert all(node.kind == APPLICATION for node in overviews)


def test_orientation_and_exercises_are_not_taskable():
    nodes = parse_nodes(fixture_text())
    kinds = {node.kind for node in nodes}
    assert ORIENTATION in kinds
    assert EXERCISE in kinds
    for node in nodes:
        if node.kind in (ORIENTATION, EXERCISE):
            assert node.kind not in TASKABLE_KINDS


def test_exercise_items_after_the_heading_stay_exercises():
    nodes = parse_nodes(fixture_text())
    items = [node for node in nodes if node.title.startswith("1. Формулу")]
    assert items
    assert items[0].kind == EXERCISE


def test_render_nodes_lists_every_node():
    nodes = parse_nodes(fixture_text())
    rendered = render_nodes(nodes)
    assert rendered.count("ops:") == len(nodes)
    assert rendered.count("deps:") == len(nodes)


def make_node(kind: str, title: str, deps: tuple[str, ...] = ()) -> TaskNode:
    return TaskNode(
        id=title, kind=kind, title=title, text=title,
        start=0, end=len(title), deps=deps, ops=(),
    )


def test_select_node_respects_the_mode():
    nodes = parse_nodes(fixture_text())
    justify = select_node(nodes, "justify")
    assert justify is not None and justify.kind in MODE_KINDS["justify"]
    apply_node = select_node(nodes, "apply")
    assert apply_node is not None and apply_node.kind in MODE_KINDS["apply"]


def test_select_node_returns_none_for_an_unknown_mode():
    assert select_node(parse_nodes(fixture_text()), "invent") is None


def test_select_node_skips_a_node_whose_dependency_is_missing():
    orphan = make_node(STATEMENT, "Теорема 9", deps=("теорема 8",))
    assert unmet_deps(orphan, []) == ("теорема 8",)
    assert select_node([orphan], "justify") is None


def test_select_node_accepts_a_resolved_dependency():
    earlier = make_node(STATEMENT, "Теорема 8")
    later = make_node(STATEMENT, "Теорема 9", deps=("теорема 8",))
    assert unmet_deps(later, [earlier]) == ()
    assert select_node([earlier, later], "justify") is earlier


def test_structural_references_are_always_available():
    node = make_node(STATEMENT, "Теорема 1", deps=("§ 3", "гл. 4"))
    assert unmet_deps(node, []) == ()


def test_find_source_proof_returns_the_following_proof():
    nodes = parse_nodes(fixture_text())
    statement = next(node for node in nodes if node.title == "Теорема 1")
    proof = find_source_proof(statement, nodes)
    assert proof is not None and proof.kind == PROOF


def test_find_source_proof_stops_at_the_next_claim():
    # Теорема 3 is followed by Теорема 4, not by its proof.
    nodes = parse_nodes(fixture_text())
    statement = next(node for node in nodes if node.title == "Теорема 3")
    assert find_source_proof(statement, nodes) is None


def test_strip_wrapper_removes_context_tags():
    wrapped = (
        '<READING_SESSION id="x">\n<SOURCE_FRAGMENT>\nТекст.\n'
        "</SOURCE_FRAGMENT>\n</READING_SESSION>"
    )
    stripped = strip_wrapper(wrapped)
    assert "SOURCE_FRAGMENT" not in stripped
    assert "Текст." in stripped
