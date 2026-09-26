from cognitive_popups.popup_helper import estimate_keys_size, estimate_text_size


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


def test_keys_reference_grows_with_rows_but_stays_a_reading_column():
    rows = [
        ("Alt+W", "четыре слова из выделенного текста"),
        ("Alt+Shift+T", "новая задача по старому материалу"),
    ]
    width, height = estimate_keys_size(rows)

    assert 320 <= width <= 560
    assert height > 0
    # More rows means a taller window, up to the cap.
    assert estimate_keys_size(rows * 6)[1] > height
    # One very long action wraps instead of widening the window without bound.
    long_width, _ = estimate_keys_size([("Alt+K", "длинный текст " * 40)])
    assert long_width <= 560


def test_keys_size_survives_an_empty_reference():
    width, height = estimate_keys_size([])

    assert width >= 320
    assert height > 0


def test_prediction_size_respects_small_monitor():
    width, height = estimate_text_size(
        "Гипотеза: " + "длинный текст " * 100,
        expanded=True,
        available_width=900,
        available_height=500,
    )

    assert width <= 680
    assert height + 34 <= 500 - 48
