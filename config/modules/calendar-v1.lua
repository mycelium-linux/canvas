-- A wall calendar: month, week and day, drawn from the host's civil date.
--
-- Everything about the grid lives here rather than in a Rust widget kind, because
-- a calendar is date arithmetic and layout, and both change more often than a
-- renderer should. `module.now` is stamped by Canvas: this script has no clock, so
-- the same inputs always produce the same wall, and the day is part of the key the
-- host retains the scene under, which is what rolls it over at midnight.
--
-- The event payload is one plain agenda: `|`-separated `YYYY-MM-DD HH:MM Title`
-- entries. A pipe rather than a newline because a Canvas text input must be
-- printable, so a multi-line blob would be refused by the host before it ever
-- reached here. One shape serves all three views, so a producer formats events
-- instead of learning three encodings, and a day with nothing on it still draws a
-- calendar rather than a lie.

local theme = module.theme
local now = module.now

-- Scene text is placed in a normalized box where x and y each span -4..4 and y
-- grows upward. The renderer multiplies `scale` by the theme's body size and by a
-- perspective factor that is 1.25 at z = 0, so `size` turns a theme role into the
-- scale that draws at it: an authored grid sizes itself with the same names a
-- native widget uses, and moves when the theme does.
local Z_PERSPECTIVE = 1.25
local function size(px)
  return px / (theme.body * Z_PERSPECTIVE)
end

local LEFT = -3.78
local SPAN = 7.56
local TOP = 3.42
local BOTTOM = -3.6
local COLS = 7
local COL_W = SPAN / COLS

local MONTHS = {
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
}
local DOW = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" }

local function label(id, text, x, y, scale, ink)
  canvas.text({
    id = id,
    text = text,
    translation = { x, y, 0 },
    scale = { scale, scale, 1 },
    ink = ink,
  })
end

-- Sakamoto's algorithm returns 0 = Sunday through 6 = Saturday. The host stamps
-- the weekday of today; a grid needs the weekday of every other day, so that
-- arithmetic is here rather than asked for.
local SAKAMOTO = { 0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4 }
local function weekday_of(year, month, day)
  local y = year
  if month < 3 then
    y = y - 1
  end
  return (y + math.floor(y / 4) - math.floor(y / 100) + math.floor(y / 400)
    + SAKAMOTO[month] + day) % 7
end

-- 1 = Monday through 7 = Sunday, the convention `module.now.weekday` uses.
local function iso_weekday_of(year, month, day)
  local w = weekday_of(year, month, day)
  if w == 0 then
    return 7
  end
  return w
end

local DAYS = { 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 }
local function days_in_month(year, month)
  if month == 2 and ((year % 4 == 0 and year % 100 ~= 0) or year % 400 == 0) then
    return 29
  end
  return DAYS[month]
end

local function iso(year, month, day)
  return string.format("%04d-%02d-%02d", year, month, day)
end

-- One day at a time, in both directions, so no second calendar appears here.
local function next_day(year, month, day)
  if day < days_in_month(year, month) then
    return year, month, day + 1
  end
  if month < 12 then
    return year, month + 1, 1
  end
  return year + 1, 1, 1
end

local function back_day(year, month, day)
  if day > 1 then
    return year, month, day - 1
  end
  if month > 1 then
    return year, month - 1, days_in_month(year, month - 1)
  end
  return year - 1, 12, days_in_month(year - 1, 12)
end

local function back_days(year, month, day, count)
  local y, m, d = year, month, day
  for _ = 1, count do
    y, m, d = back_day(y, m, d)
  end
  return y, m, d
end

-- Monday of the week containing the date. Both views need it, and for the same
-- reason: a reader looks for a week, not for an arbitrary seven-day window.
local function week_start(year, month, day)
  return back_days(year, month, day, iso_weekday_of(year, month, day) - 1)
end

local today = iso(now.year, now.month, now.day)

-- The anchor is where the widget is looking, which is not necessarily today: a
-- person pins the wall to next month to plan it.
local ay, am, ad = now.year, now.month, now.day
local anchor = module.inputs.anchor
if type(anchor) == "string" then
  local y, m, d = anchor:match("^(%d%d%d%d)%-(%d%d)%-(%d%d)$")
  if y then
    ay, am, ad = tonumber(y), tonumber(m), tonumber(d)
  end
end

local events = {}
local agenda = module.inputs.agenda
if type(agenda) == "string" then
  -- `|` rather than a newline: the host validates every input as visible text,
  -- which by definition has no control characters in it. A separator the contract
  -- allows is the only honest way to put an agenda in one field.
  for entry in (agenda .. "|"):gmatch("([^|]*)|") do
    local day, hm, title = entry:match("^%s*(%d%d%d%d%-%d%d%-%d%d)%s+(%d%d:%d%d)%s+(.+)$")
    if day then
      events[day] = events[day] or {}
      table.insert(events[day], { time = hm, title = title })
    end
  end
end

-- Sorted on first read rather than up front: the grid asks about a handful of
-- dates, and an agenda longer than a month should not cost a sort of everything.
local function on(day)
  local list = events[day]
  if list == nil then
    return nil
  end
  if not list.sorted then
    table.sort(list, function(a, b)
      return a.time < b.time
    end)
    list.sorted = true
  end
  return list
end

-- A view that is not one of the three is a typo in a spec, not a request for a
-- different view, and the month is the shape a wall is read from — so the domain
-- of this function is closed rather than permissive. Erroring the whole widget for
-- a mistyped string would leave a dead panel on a wall someone is standing in front
-- of, which is a worse outcome than the intended default.
local view = "month"
if module.inputs.view == "week" or module.inputs.view == "day" then
  view = module.inputs.view
end

local function title_text()
  if view == "day" then
    return string.format(
      "%s %d %s %d",
      DOW[iso_weekday_of(ay, am, ad)],
      ad,
      MONTHS[am],
      ay
    )
  end
  if view == "week" then
    local wy, wm, wd = week_start(ay, am, ad)
    -- Six steps from Monday, not seven: the Monday itself is day one.
    local sy, sm, sd = wy, wm, wd
    for _ = 2, COLS do
      sy, sm, sd = next_day(sy, sm, sd)
    end
    if wm == sm then
      return string.format("%s %d – %d, %d", MONTHS[wm], wd, sd, wy)
    end
    return string.format("%s %d – %s %d", MONTHS[wm], wd, MONTHS[sm], sd)
  end
  return string.format("%s %d", MONTHS[am], ay)
end

-- A click chooses a day. The choice belongs to the view, not to the Spore: it
-- has to survive the re-render the click itself causes, and it should not
-- outlive the window. `module.select` is that memory; the pointer event is what
-- fills it, and the highlight below is what reads it back.
local selected = module.view and module.view.selected or nil
local press_x, press_y = nil, nil
local event = module.event
if event and event.kind == "pointer" and event.phase ~= "move" then
  -- Scene space is -4..4 on both axes with y growing upward; a pointer arrives
  -- normalized to the surface, top-left origin. One conversion, one place.
  press_x = (event.x - 0.5) * 8
  press_y = (0.5 - event.y) * 8
end

local function hits(x, y, width, height)
  return press_x ~= nil
    and press_x >= x
    and press_x <= x + width
    and press_y >= y
    and press_y <= y + height
end

local function highlight(id, key, x, y, width, height)
  if key ~= selected then
    return
  end
  canvas.rect({
    id = id,
    ink = "primary",
    opacity = 0.16,
    radius = theme.radius,
    translation = { x + width * 0.5, y + height * 0.5, -0.4 },
    scale = { width, height, 1 },
  })
end

label("title", title_text(), LEFT, TOP, size(theme.title), "text")

local header_y = TOP - 0.66

if view == "month" then
  for column = 1, COLS do
    label(
      string.format("h%d", column),
      DOW[column],
      LEFT + (column - 1) * COL_W + 0.06,
      header_y,
      size(theme.small),
      "muted"
    )
  end

  -- The grid starts on the Monday of the week containing the 1st, so the first
  -- row is a whole week and the month is never cut mid-stride.
  local gy, gm, gd = week_start(ay, am, 1)

  local row_top = header_y - 0.4
  local row_h = (row_top - BOTTOM) / 6
  for row = 0, 5 do
    for column = 1, COLS do
      local key = iso(gy, gm, gd)
      local own = (gm == am)
      local x = LEFT + (column - 1) * COL_W + 0.06
      local y = row_top - row * row_h

      -- Today is the one cell a reader is looking for, so it takes the accent;
      -- days outside the anchor month recede instead of competing with it.
      local ink = "text"
      if key == today then
        ink = "primary"
      elseif not own then
        ink = "muted"
      end
      local cell_x = LEFT + (column - 1) * COL_W
      if hits(cell_x, y - row_h, COL_W, row_h) then
        selected = key
        module.select(key)
      end
      highlight("s" .. key, key, cell_x, y - row_h, COL_W, row_h)
      label(string.format("d%s", key), string.format("%d", gd), x, y, size(theme.display), ink)

      local list = on(key)
      if list then
        local dots = "•"
        if #list > 5 then
          dots = "•••"
        elseif #list > 1 then
          dots = "••"
        end
        label(
          string.format("m%s", key),
          dots,
          x,
          y - row_h * 0.34,
          size(theme.small),
          own and "primary" or "muted"
        )
      end

      gy, gm, gd = next_day(gy, gm, gd)
    end
  end
else
  -- Week and day share one rule: a column per date, its plans beneath it, and a
  -- day with nothing on it still drawn, because an open Tuesday is information.
  local dates = {}
  if view == "week" then
    local wy, wm, wd = week_start(ay, am, ad)
    dates[1] = { year = wy, month = wm, day = wd }
    for index = 2, COLS do
      local previous = dates[index - 1]
      local ny, nm, nd = next_day(previous.year, previous.month, previous.day)
      dates[index] = { year = ny, month = nm, day = nd }
    end
  else
    dates[1] = { year = ay, month = am, day = ad }
  end

  local column_w = SPAN / #dates
  local row_y = header_y - 0.3
  local line_h = view == "day" and 0.34 or 0.4
  for index, date in ipairs(dates) do
    local key = iso(date.year, date.month, date.day)
    local column_x = LEFT + (index - 1) * column_w
    if hits(column_x, BOTTOM, column_w, row_y + 0.34 - BOTTOM) then
      selected = key
      module.select(key)
    end
    highlight("s" .. key, key, column_x, BOTTOM, column_w, row_y + 0.34 - BOTTOM)
    local x = column_x + 0.06
    label(
      string.format("c%d", index),
      string.format("%s %d", DOW[iso_weekday_of(date.year, date.month, date.day)], date.day),
      x,
      row_y,
      size(theme.body),
      key == today and "primary" or "muted"
    )

    local list = on(key)
    local shown = 0
    if list then
      local limit = view == "day" and 18 or 5
      for _, event in ipairs(list) do
        if shown >= limit then
          break
        end
        local line = string.format("%s  %s", event.time, event.title)
        if #line > 46 then
          line = string.sub(line, 1, 45) .. "…"
        end
        label(
          string.format("e%s-%d", key, shown + 1),
          line,
          x,
          row_y - (shown + 1) * line_h,
          size(view == "day" and theme.body or theme.small),
          "text"
        )
        shown = shown + 1
      end
      if shown < #list then
        label(
          string.format("o%s", key),
          string.format("+%d more", #list - shown),
          x,
          row_y - (shown + 1) * line_h,
          size(theme.micro),
          "muted"
        )
      end
    else
      label(string.format("n%s", key), "open", x, row_y - line_h, size(theme.small), "muted")
    end
  end
end
