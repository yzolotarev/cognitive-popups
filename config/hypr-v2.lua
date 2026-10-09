-- Cognitive Popups v2 bindings.
-- The service must be running before these bindings have an effect.
local home = os.getenv("HOME") or ""
-- Prefer an explicit checkout, then the public clone, then the legacy folder.
-- If both exist, set COGNITIVE_PROJECT to select the one whose service you use.
local project = os.getenv("COGNITIVE_PROJECT")
if not project or project == "" then
    project = home .. "/projects/cognitive-popups"
    local script = io.open(project .. "/scripts/cognitive-popups-signal.sh", "r")
    if script then
        script:close()
    else
        project = home .. "/projects/cognitive-exoskeleton"
    end
end
local function shell_quote(value)
    return "'" .. value:gsub("'", "'\\''") .. "'"
end
local signal = shell_quote(project .. "/scripts/cognitive-popups-signal.sh")
local tasks = shell_quote(project .. "/scripts/cognitive-tasks.sh")
local hud = shell_quote(project .. "/scripts/cognitive-hud.sh")

-- Основное действие: четыре слова из выделенного текста.
hl.bind("ALT + W", hl.dsp.exec_cmd(signal .. " seed"))
-- Три ракурса 4 слов с паузой на mindmap между окнами и итоговым тезисом.
hl.bind("CTRL + ALT + W", hl.dsp.exec_cmd(signal .. " seed-batch"))
-- Проверка понимания методом Фейнмана.
hl.bind("ALT + F", hl.dsp.exec_cmd(signal .. " feynman"))
-- Explicit 15-minute block; Ctrl+Alt+G was checked against live/config bindings.
-- Ctrl+Alt+G retired 2026-10-07: the 15-minute step now lives on Alt+I.
-- Input windows keep the keyboard until closed: with follow_mouse a mouse twitch
-- past the edge sent typed letters to the app underneath (2026-10-07).
hl.window_rule({
    name = "cognitive-input-keeps-focus",
    match = { class = "^cognitive-input$" },
    float = true,
    pin = true,
    stay_focused = true,
})
hl.window_rule({
    name = "cognitive-focus-timer",
    match = { class = "cognitive-focus-timer" },
    float = true,
    -- GTK already refuses keyboard focus; no_focus in Hyprland would also block
    -- the hover that opens the whole step text.
    no_focus = false,
    no_anim = true,
})
-- Добровольно посмотреть на ту же связь под другим углом: сразу ответ.
-- Alt+R frozen 2026-10-07 ("другой ракурс"); the daemon ignores the request.
-- То же, но со своим вопросом к ракурсу.
-- Alt+Shift+R frozen 2026-10-07 together with Alt+R.
-- Заметка о своей ошибке в чтении; якорь — текущее выделение.
hl.bind("ALT + E", hl.dsp.exec_cmd(signal .. " note"))
-- Спросить или объяснить: слова из буфера, которые не понял, — вручную; вопрос —
-- свободной строкой. Один хоткей закрывает оба случая, режим выбирается по вводу.
hl.bind("ALT + C", hl.dsp.exec_cmd(signal .. " clarify"))
-- Сжать выделенный текст до короткой сути; вход и ответ пишутся в лог.
hl.bind("CTRL + Q", hl.dsp.exec_cmd(signal .. " summary"))
-- Закладка намерения: одна фраза «сейчас хочу…», вернуть её после перерыва.
hl.bind("ALT + I", hl.dsp.exec_cmd(signal .. " intent"))
-- Покажи на примере: один маленький конкретный случай к выделенному тексту.
hl.bind("ALT + G", hl.dsp.exec_cmd(signal .. " example"))
-- То же, но со своим запросом или с вставкой материала, если выделения нет.
hl.bind("ALT + SHIFT + G", hl.dsp.exec_cmd(signal .. " example-ask"))
-- Сгенерировать тренировочную задачу по тому, что читаешь: меню типов.
hl.bind("ALT + T", hl.dsp.exec_cmd(tasks))
-- По своему решению выбрать старый фрагмент и получить новую задачу на применение.
hl.bind("ALT + SHIFT + T", hl.dsp.exec_cmd(tasks .. " --revisit-menu"))
-- Явно перейти к попытке последней сгенерированной задачи и получить обратную связь.
hl.bind("ALT + Y", hl.dsp.exec_cmd(tasks .. " --practice"))
-- Справка по клавишам: минималистичное окно «клавиша — что делает».
hl.bind("ALT + K", hl.dsp.exec_cmd(signal .. " keys"))
-- Боковая панель: те же действия мышью и тихая подсветка готовой задачи.
-- Первое нажатие запускает панель, следующие показывают и скрывают её.
-- Alt+H frozen 2026-10-07: the panel does not start while var/hud.frozen exists.
hl.bind("ALT + H", hl.dsp.exec_cmd(hud))


-- Custom shadow is rendered by the popup helper, never by a global setting.
-- All helper windows explicitly use cognitive-popup; the unmanaged companion
-- uses cognitive-shadow (also exclude decorations if it is ever managed).
hl.window_rule({
    name = "cognitive-popup-shadow",
    match = { class = "^(cognitive-popup|cognitive-shadow)$" },
    float = true,
    no_anim = true,
    no_shadow = true,
    -- HyprGlass replaces native blur with its own rectangular decoration.
    tag = "+hyprglass_disabled",
    no_blur = true,
    opaque = false,
    border_size = 0,
})

hl.window_rule({
    name = "cognitive-ask",
    match = { class = "cognitive-ask" },
    float = true,
    no_anim = true,
})

-- Панель: висит поверх рабочего окна, но не забирает фокус — набранный текст
-- и выделение остаются у читаемой страницы. Позицию панель задаёт сама.
hl.window_rule({
    name = "cognitive-hud",
    match = { class = "cognitive-hud" },
    float = true,
    -- GTK запрещает клавиатурный фокус; no_focus в Hyprland блокирует и клики.
    no_focus = false,
    no_anim = true,
})
