from cognitive_popups.popup_helper import estimate_text_size


def test_prediction_result_uses_expanded_text_geometry():
    text = (
        "Гипотеза: цветная метка на коробке всегда позволяет найти нужную деталь, "
        "даже когда коробки переставили на другую полку\n\n"
        "Результат: частично подтверждено\n\n"
        "В тексте: Each box has a coloured label, but the inventory number is needed "
        "when two boxes share the same colour.\n\n"
        "Расхождение: цвет помогает сузить поиск, но при совпадении меток нужно "
        "дополнительно проверить номер коробки"
    )

    compact_width, _ = estimate_text_size(text)
    expanded_width, expanded_height = estimate_text_size(text, expanded=True)

    assert compact_width <= 560
    assert 420 <= expanded_width < 760
    assert expanded_height < 526


def test_prediction_size_shrinks_for_shorter_text():
    short_width, short_height = estimate_text_size(
        "Гипотеза: A\n\nРезультат: частично подтверждено\n\nВ тексте: B\n\nРасхождение: C",
        expanded=True,
    )
    long_width, long_height = estimate_text_size(
        "Гипотеза: " + "длинная формулировка " * 30 +
        "\n\nРезультат: частично подтверждено\n\nВ тексте: " +
        "source evidence " * 30 + "\n\nРасхождение: " + "объяснение " * 30,
        expanded=True,
    )

    assert short_width <= long_width
    assert short_height <= long_height


def test_prediction_size_respects_small_monitor():
    width, height = estimate_text_size(
        "Гипотеза: " + "длинный текст " * 100,
        expanded=True,
        available_width=900,
        available_height=500,
    )

    assert width <= 680
    assert height + 34 <= 500 - 48
