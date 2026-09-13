-- Cognitive Popups v2 bindings.
-- The service must be running before these bindings have an effect.
local home = os.getenv("HOME") or ""
local signal = home .. "/projects/cognitive-popups/scripts/cognitive-popups-signal.sh"

-- Основное действие: четыре слова из выделенного текста.
hl.bind("ALT + W", hl.dsp.exec_cmd(signal .. " seed"))
-- Проверка понимания методом Фейнмана.
hl.bind("ALT + F", hl.dsp.exec_cmd(signal .. " feynman"))


hl.window_rule({
    name = "cognitive-ask",
    match = { class = "cognitive-ask" },
    float = true,
    no_anim = true,
})
