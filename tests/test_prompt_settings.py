from cognitive_popups.prompt_settings import PromptSettings


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
