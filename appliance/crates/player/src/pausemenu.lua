-- The playback overlay, drawn by mpv itself (the UI is a separate process and cannot draw on
-- top of the video). Replaces mpv's OSC.
--
-- Paused: title, progress and a Settings entry. Settings pick the audio and subtitle track.
-- Playing: a progress bar for a couple of seconds after a seek.
--
-- Remote buttons arrive from the player as `script-message-to pausemenu key <name>` (see
-- keys.rs), because the player reads the input devices itself; mpv hears no keyboard. So this
-- script owns what each button does, in the overlay and out of it.

local mp = require "mp"

local W, H = 1280, 720
local SHOW_AFTER_SEEK = 2.5

local ACCENT = "2EA1E6" -- amber (ASS colors are BGR)
local PANEL = "1A1A1A"
local TRACK = "6E6E6E"

local LANGS = {
    eng = "English", spa = "Spanish", fre = "French", fra = "French", ger = "German",
    deu = "German", ita = "Italian", jpn = "Japanese", por = "Portuguese", rus = "Russian",
    kor = "Korean", chi = "Chinese", zho = "Chinese", dut = "Dutch", nld = "Dutch",
    swe = "Swedish", nor = "Norwegian", dan = "Danish", fin = "Finnish", pol = "Polish",
    ara = "Arabic", hin = "Hindi", tur = "Turkish", heb = "Hebrew", cze = "Czech",
    ces = "Czech", hun = "Hungarian", gre = "Greek", ell = "Greek", und = "Unknown",
}

local CHANNELS = { [1] = "mono", [2] = "stereo", [6] = "5.1", [8] = "7.1" }

-- hidden | bar (after a seek) | paused | menu | list
local mode = "hidden"
local settings_focus = false
local menu_cursor = 1
local list_kind, list_items, list_cursor, list_top
local bar_timer

local function esc(s)
    return (tostring(s):gsub("\\", "\\\239\187\191"):gsub("{", "\\{"):gsub("\n", " "))
end

-- Cut to `n` characters, not bytes, so a multibyte title is not split.
local function clip(s, n)
    local out, count = {}, 0
    for ch in s:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
        count = count + 1
        if count > n then
            out[#out] = "..."
            break
        end
        out[#out + 1] = ch
    end
    return table.concat(out)
end

local function clock(t)
    t = math.max(0, math.floor(t or 0))
    if t >= 3600 then
        return string.format("%d:%02d:%02d", math.floor(t / 3600), math.floor(t % 3600 / 60), t % 60)
    end
    return string.format("%d:%02d", math.floor(t / 60), t % 60)
end

local ass = {}

local function rect(x, y, w, h, color, alpha)
    ass[#ass + 1] = string.format(
        "{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H%s&\\1a&H%02X&\\p1}m %d %d l %d %d %d %d %d %d{\\p0}",
        color, alpha or 0, x, y, x + w, y, x + w, y + h, x, y + h)
end

local function text(x, y, size, s, color, an)
    ass[#ass + 1] = string.format("{\\an%d\\pos(%d,%d)\\fs%d\\bord2\\shad0\\1c&H%s&\\3c&H000000&}%s",
        an or 7, x, y, size, color or "FFFFFF", esc(s))
end

-- Tracks ----------------------------------------------------------------------------------

local function tracks_of(kind)
    local out = {}
    for _, t in ipairs(mp.get_property_native("track-list") or {}) do
        if t.type == kind then
            out[#out + 1] = t
        end
    end
    return out
end

local function label(t, kind)
    local parts = {}
    if t.lang then
        parts[#parts + 1] = LANGS[t.lang] or t.lang:upper()
    end
    if t.title and t.title ~= "" and t.title ~= parts[1] then
        parts[#parts + 1] = t.title
    end
    if kind == "audio" then
        local detail = {}
        if t.codec then
            detail[#detail + 1] = t.codec:upper()
        end
        local n = t["demux-channel-count"]
        if n then
            detail[#detail + 1] = CHANNELS[n] or (n .. "ch")
        end
        if #detail > 0 then
            parts[#parts + 1] = table.concat(detail, " ")
        end
    else
        if t.forced then
            parts[#parts + 1] = "forced"
        end
        if t.external then
            parts[#parts + 1] = "external"
        end
    end
    if #parts == 0 then
        return "Track " .. t.id
    end
    return table.concat(parts, " - ")
end

-- The choices for a kind, and which one is playing. Subtitles can also be switched off.
local function options(kind)
    local items, current = {}, 1
    if kind == "sub" then
        items[1] = { id = "no", label = "Off" }
    end
    for _, t in ipairs(tracks_of(kind)) do
        items[#items + 1] = { id = t.id, label = label(t, kind) }
        if t.selected then
            current = #items
        end
    end
    return items, current
end

local function current_label(kind)
    local items, current = options(kind)
    if #items == 0 then
        return "None"
    end
    return items[current].label
end

-- Drawing ---------------------------------------------------------------------------------

local function draw_bar(with_controls)
    local pos = mp.get_property_number("time-pos") or 0
    local dur = mp.get_property_number("duration") or 0
    rect(0, 540, W, 180, PANEL, 0x30)
    if with_controls then
        text(60, 560, 24, "PAUSED", ACCENT)
    end
    text(60, 590, 34, clip(mp.get_property("media-title") or "", 60))
    local left, width = 60, W - 120
    rect(left, 648, width, 8, TRACK)
    local frac = dur > 0 and math.min(1, math.max(0, pos / dur)) or 0
    rect(left, 648, math.floor(width * frac), 8, ACCENT)
    rect(left + math.floor(width * frac) - 8, 642, 16, 20, "FFFFFF")
    text(60, 668, 26, clock(pos) .. " / " .. clock(dur))
    if with_controls then
        local bx, by, bw, bh = W - 60 - 200, 566, 200, 48
        rect(bx, by, bw, bh, settings_focus and ACCENT or "3A3A3A")
        text(bx + bw / 2, by + bh / 2, 28, "Settings", settings_focus and "000000" or "FFFFFF", 5)
        local hint = settings_focus and "OK open settings    Up back" or "OK resume    Down settings    Left/Right seek"
        text(W - 60, 676, 24, hint, "B0B0B0", 9)
    end
end

local function draw_panel(title)
    rect(0, 0, W, H, "000000", 0x80)
    local x, y, w, h = 300, 120, 680, 480
    rect(x, y, w, h, PANEL, 0x10)
    text(x + 40, y + 28, 36, title, ACCENT)
    return x, y, w, h
end

local function delay_label()
    local d = mp.get_property_number("sub-delay") or 0
    if math.abs(d) < 0.05 then
        return "0.0 s"
    end
    return string.format("%+.1f s", d)
end

local function draw_menu()
    local x, y, w = draw_panel("Settings")
    local rows = {
        { "Audio", current_label("audio") },
        { "Subtitles", current_label("sub") },
        { "Subtitle delay", "<  " .. delay_label() .. "  >" },
    }
    for i, row in ipairs(rows) do
        local ry = y + 100 + (i - 1) * 80
        if i == menu_cursor then
            rect(x + 20, ry - 8, w - 40, 64, ACCENT)
        end
        local fg = i == menu_cursor and "000000" or "FFFFFF"
        text(x + 40, ry + 24, 30, row[1], fg, 4)
        text(x + w - 40, ry + 24, 28, clip(row[2], 34), fg, 6)
    end
    local hint = menu_cursor == 3 and "Left/Right adjust 0.1 s    OK reset    Back close" or "OK change    Back close"
    text(x + 40, y + 440, 24, hint, "B0B0B0", 4)
end

local VISIBLE = 7

local function draw_list()
    local x, y, w = draw_panel(list_kind == "audio" and "Audio" or "Subtitles")
    if #list_items == 0 then
        text(x + 40, y + 120, 30, "No tracks", "B0B0B0")
    end
    for row = 1, math.min(VISIBLE, #list_items - list_top + 1) do
        local i = list_top + row - 1
        local ry = y + 90 + (row - 1) * 52
        if i == list_cursor then
            rect(x + 20, ry - 4, w - 40, 48, ACCENT)
        end
        text(x + 40, ry + 20, 28, clip(list_items[i].label, 46), i == list_cursor and "000000" or "FFFFFF", 4)
    end
    if #list_items > VISIBLE then
        text(x + w - 40, y + 440, 24, list_cursor .. " / " .. #list_items, "B0B0B0", 6)
    end
    text(x + 40, y + 440, 24, "OK select    Back return", "B0B0B0", 4)
end

local function render()
    ass = {}
    if mode == "bar" then
        draw_bar(false)
    elseif mode == "paused" then
        draw_bar(true)
    elseif mode == "menu" then
        draw_bar(true)
        draw_menu()
    elseif mode == "list" then
        draw_bar(true)
        draw_list()
    end
    mp.set_osd_ass(W, H, table.concat(ass, "\n"))
end

local function set_mode(m)
    mode = m
    if bar_timer then
        bar_timer:kill()
        bar_timer = nil
    end
    if m == "bar" then
        bar_timer = mp.add_timeout(SHOW_AFTER_SEEK, function()
            if mode == "bar" then
                set_mode("hidden")
            end
        end)
    end
    render()
end

-- Behavior --------------------------------------------------------------------------------

local function open_list(kind)
    list_kind = kind
    local current
    list_items, current = options(kind)
    list_cursor = current
    list_top = math.max(1, math.min(list_cursor - 2, #list_items - VISIBLE + 1))
    set_mode("list")
end

local function move(cursor, n, delta)
    return math.max(1, math.min(n, cursor + delta))
end

-- Subtitle delay moves in tenths of a second; positive shows subtitles later. Rounding keeps
-- repeated steps from drifting (0.1 is not exact in floating point).
local function nudge_sub_delay(tenths)
    local d = math.floor((mp.get_property_number("sub-delay") or 0) * 10 + 0.5) + tenths
    mp.set_property_number("sub-delay", d / 10)
end

local function menu_key(name)
    if name == "up" then
        menu_cursor = move(menu_cursor, 3, -1)
    elseif name == "down" then
        menu_cursor = move(menu_cursor, 3, 1)
    elseif name == "left" and menu_cursor == 3 then
        nudge_sub_delay(-1)
    elseif name == "right" and menu_cursor == 3 then
        nudge_sub_delay(1)
    elseif name == "ok" then
        if menu_cursor == 3 then
            mp.set_property_number("sub-delay", 0)
        else
            open_list(menu_cursor == 1 and "audio" or "sub")
            return
        end
    elseif name == "back" then
        set_mode("paused")
        return
    end
    render()
end

local function list_key(name)
    if name == "up" or name == "down" then
        list_cursor = move(list_cursor, #list_items, name == "up" and -1 or 1)
        if list_cursor < list_top then
            list_top = list_cursor
        elseif list_cursor >= list_top + VISIBLE then
            list_top = list_cursor - VISIBLE + 1
        end
    elseif name == "ok" then
        local choice = list_items[list_cursor]
        if choice then
            mp.set_property(list_kind == "audio" and "aid" or "sid", tostring(choice.id))
            if list_kind == "sub" then
                mp.set_property("sub-visibility", "yes")
            end
        end
        set_mode("menu")
        return
    elseif name == "back" then
        set_mode("menu")
        return
    end
    render()
end

local function seek(secs)
    mp.commandv("seek", tostring(secs), "relative")
    if mode ~= "paused" then
        set_mode("bar")
    end
end

local function playback_key(name)
    local paused = mp.get_property_native("pause")
    if name == "back" or name == "stop" then
        mp.commandv("quit")
    elseif name == "playpause" then
        mp.set_property_native("pause", not paused)
    elseif name == "ok" then
        if paused and settings_focus then
            menu_cursor = 1
            set_mode("menu")
        else
            mp.set_property_native("pause", not paused)
        end
    elseif name == "left" then
        seek(-10)
    elseif name == "right" then
        seek(30)
    elseif name == "down" then
        if paused and not settings_focus then
            settings_focus = true
            render()
        else
            seek(-60)
        end
    elseif name == "up" then
        if paused and settings_focus then
            settings_focus = false
            render()
        else
            seek(60)
        end
    end
end

mp.register_script_message("key", function(name)
    if name == "stop" then
        mp.commandv("quit")
    elseif mode == "menu" then
        menu_key(name)
    elseif mode == "list" then
        list_key(name)
    else
        playback_key(name)
    end
end)

mp.observe_property("pause", "bool", function(_, paused)
    settings_focus = false
    set_mode(paused and "paused" or "hidden")
end)

-- Keep the progress and the shown choices current while the overlay is up.
mp.observe_property("time-pos", "number", function()
    if mode ~= "hidden" then
        render()
    end
end)
mp.observe_property("sub-delay", "number", function()
    if mode == "menu" then
        render()
    end
end)
mp.observe_property("track-list", "native", function()
    if mode == "menu" then
        render()
    end
end)
