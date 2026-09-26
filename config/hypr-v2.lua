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
-- Проверка понимания методом Фейнмана.
hl.bind("ALT + F", hl.dsp.exec_cmd(signal .. " feynman"))
-- Добровольно посмотреть на ту же связь под другим углом.
hl.bind("ALT + R", hl.dsp.exec_cmd(signal .. " reframe"))
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
-- Боковая панель: те же действия мышью и тихая подсветка готовой задачи.
-- Первое нажатие запускает панель, следующие показывают и скрывают её.
hl.bind("ALT + H", hl.dsp.exec_cmd(hud))


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
