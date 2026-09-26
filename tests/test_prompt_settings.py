from cognitive_popups import prompts
from cognitive_popups.prompt_settings import PromptSettings, built_in_defaults


def test_prompt_override_and_reset(tmp_path):
    settings = PromptSettings(tmp_path / "prompts.json")
    default = settings.get("four_words")

    settings.set("four_words", "custom prompt")
    assert settings.get("four_words") == "custom prompt"
    assert settings.is_custom("four_words")

    reloaded = PromptSettings(tmp_path / "prompts.json")
    assert reloaded.get("four_words") == "custom prompt"

    reloaded.reset("four_words")
    assert reloaded.get("four_words") == default
    assert not reloaded.is_custom("four_words")


def test_clarify_override_migrates_the_known_legacy_rules(tmp_path):
    path = tmp_path / "prompts.json"
    settings = PromptSettings(path)
    legacy = (
        "Правила:\n"
        "2. Начни с доступного смысла, затем добавь один конкретный пример.\n"
        "4. Но не ограничивайся им, если читатель явно спрашивает об общем термине.\n"
    )
    settings.set("clarify", legacy)
    original = path.read_bytes()
    settings.reload()

    assert settings.get("clarify") == prompts.CLARIFY_SYSTEM
    assert settings.is_custom("clarify")
    assert path.read_bytes() == original

    settings.set("clarify", "Мой собственный промпт без старых правил.")
    assert settings.get("clarify") == "Мой собственный промпт без старых правил."


def test_get_returns_the_current_default(tmp_path):
    # No override: the text comes from prompts.py as it is on disk right now, not
    # from a snapshot taken when the daemon imported the module.
    settings = PromptSettings(tmp_path / "prompts.json")

    assert settings.get("four_words") == prompts.SEED_SYSTEM
    assert settings.default("prediction") == prompts.PREDICTION_SYSTEM
    assert settings.default("example") == prompts.EXAMPLE_SYSTEM


def test_example_prompt_is_configurable(tmp_path):
    settings = PromptSettings(tmp_path / "prompts.json")

    assert settings.get("example") == prompts.EXAMPLE_SYSTEM
    settings.set("example", "Мой собственный промпт для примера.")

    assert settings.get("example") == "Мой собственный промпт для примера."
    assert settings.is_custom("example")


def test_reframe_prompt_is_editable_without_changing_other_prompts(tmp_path):
    settings = PromptSettings(tmp_path / "prompts.json")
    default = settings.get("reframe")
    assert default == prompts.REFRAME_SYSTEM
    original_prediction = settings.get("prediction")

    settings.set("reframe", "Мой промпт для ракурса")
    assert PromptSettings(settings.path).get("reframe") == "Мой промпт для ракурса"
    assert settings.get("prediction") == original_prediction
    settings.reset("reframe")
    assert settings.get("reframe") == default


def test_defaults_are_read_from_the_prompt_source(tmp_path):
    source = tmp_path / "prompts_like.py"
    source.write_text('SEED_SYSTEM = "новый дефолт"\n', encoding="utf-8")
    assert built_in_defaults(source) == {"four_words": "новый дефолт"}

    # Half-written file: a compile error yields nothing rather than replacing the
    # text the caller already had.
    source.write_text("SEED_SYSTEM = \n", encoding="utf-8")
    assert built_in_defaults(source) == {}

    source.write_text('QUESTION_SYSTEM = "вопрос"\n', encoding="utf-8")
    assert built_in_defaults(source) == {"feynman_question": "вопрос"}
