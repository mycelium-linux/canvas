local function esc(value)
  return tostring(value or ""):gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;")
end

local goals = data.goals or {}
local beads = data.beads or {}
local agents = data.agents or {}
local reviewers = data.reviewers or {}
local projects = data.project_ids or {}
local items = goals.items or {}
local live = data.activity_state == "observed"
local theme = data.theme or {}
local palette = theme.palette or {}
local function color(name, fallback)
  local value = palette[name]
  if type(value) == "string" and value:match("^#[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]$") then return value end
  return fallback
end
local theme_css = ':root {'
  .. '--co-background:' .. color("background", "#0c1016") .. ';'
  .. '--co-background-strong:' .. color("background_strong", "#111821") .. ';'
  .. '--co-surface:' .. color("surface", "#121821") .. ';'
  .. '--co-surface-muted:' .. color("surface_muted", "#18212c") .. ';'
  .. '--co-text:' .. color("text", "#f2f5f9") .. ';'
  .. '--co-text-muted:' .. color("text_muted", "#a9b3c0") .. ';'
  .. '--co-primary:' .. color("primary", "#62d5e9") .. ';'
  .. '--co-success:' .. color("success", "#72dfa0") .. ';'
  .. '--co-warning:' .. color("warning", "#f0c76f") .. ';'
  .. '--co-danger:' .. color("danger", "#ff8f9f") .. ';'
  .. '--co-info:' .. color("info", "#69d7e9") .. ';'
  .. '--co-border:' .. color("border", "#293340") .. ';'
  .. '--co-border-strong:' .. color("border_strong", "#394759") .. ';}'

local function metric(label, value, detail, tone)
  return '<article class="metric ' .. tone .. '"><div class="metric-label">' .. esc(label)
    .. '</div><div class="metric-value">' .. esc(value) .. '</div><div class="metric-detail">' .. esc(detail) .. '</div></article>'
end

local cards = metric("Goals", goals.total or 0, tostring(goals.active or 0) .. " active · " .. tostring(goals.ideas or 0) .. " ideas", "cyan")
  .. metric("Linked marbles", beads.linked or 0, live and (tostring(beads.open or 0) .. " currently open") or "catalog references", "amber")
  .. metric("Agents", live and (agents.running or 0) or "—", live and (tostring(agents.total or 0) .. " observed") or "live activity unavailable", "green")
  .. metric("Reviewers", live and (reviewers.running or 0) or "—", live and (tostring(reviewers.total or 0) .. " observed") or "live activity unavailable", "violet")

local chips = ""
for i = 1, math.min(#projects, 12) do chips = chips .. '<span>' .. esc(projects[i]) .. '</span>' end

local rows = ""
for i = 1, math.min(#items, 5) do
  local goal = items[i]
  rows = rows .. '<li><span class="goal-copy">' .. esc(goal.outcome or goal.id or "Goal")
    .. '</span><span class="goal-state">' .. esc(goal.status or "unknown") .. '</span></li>'
end
if rows == "" then rows = '<li class="empty">No tracked goals in this workspace yet.</li>' end

return {
  html = '<main><header><div class="identity"><div class="eyebrow">ALFALFA / WORKSPACE</div><h1>'
    .. esc(data.name or data.workspace_id or "Project") .. '</h1><div class="projects">' .. chips
    .. '</div></div><div class="state ' .. (live and "live" or "partial") .. '"><i></i>'
    .. (live and "LIVE" or "CATALOG") .. '</div></header><section class="metrics">' .. cards
    .. '</section><section class="work"><div class="section-head"><h2>Tracked goals</h2><span>'
    .. esc(#items) .. ' shown</span></div><ul>' .. rows .. '</ul></section><footer><span>'
    .. esc(data.project_count or #projects) .. ' projects</span><span>' .. (live and "Realtime activity connected" or "Waiting for activity snapshot")
    .. '</span></footer></main>',
  css = theme_css .. [[
    * { box-sizing: border-box; }
    html, body { margin: 0; width: 100%; height: 100%; overflow: hidden; background: var(--co-background); color: var(--co-text); font-family: system-ui, sans-serif; }
    body { border-top: 3px solid var(--co-primary); }
    main { width: 100vw; height: 100vh; padding: 42px 42px 30px; display: flex; flex-direction: column; background: var(--co-background); }
    header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; padding-bottom: 28px; border-bottom: 1px solid var(--co-border); }
    .identity { min-width: 0; }
    .eyebrow { color: var(--co-primary); font-size: 14px; letter-spacing: 3.4px; font-weight: 750; }
    h1 { font-size: 58px; line-height: .98; letter-spacing: -2px; margin: 10px 0 0; font-weight: 640; }
    .projects { margin-top: 20px; display: flex; flex-wrap: wrap; gap: 7px; max-height: 64px; overflow: hidden; }
    .projects span { color: var(--co-text-muted); background: var(--co-surface); border: 1px solid var(--co-border); border-radius: 7px; padding: 6px 9px; font-size: 12px; }
    .state { flex: 0 0 auto; display: flex; align-items: center; gap: 9px; color: var(--co-text-muted); border: 1px solid var(--co-border-strong); border-radius: 7px; padding: 10px 13px; font-size: 12px; font-weight: 750; letter-spacing: 1.8px; }
    .state i { width: 8px; height: 8px; border-radius: 50%; background: var(--co-warning); }
    .state.live i { background: var(--co-success); }
    .metrics { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-top: 24px; }
    .metric { min-height: 150px; padding: 20px 22px; background: var(--co-surface); border: 1px solid var(--co-border); border-left-width: 3px; border-radius: 8px; }
    .metric-label { color: var(--co-text-muted); font-size: 15px; font-weight: 560; }
    .metric-value { font-size: 50px; line-height: 1; margin: 15px 0 10px; font-weight: 680; letter-spacing: -1.5px; }
    .metric-detail { color: var(--co-text-muted); font-size: 13px; }
    .cyan { border-left-color: var(--co-info); } .cyan .metric-value { color: var(--co-info); }
    .amber { border-left-color: var(--co-warning); } .amber .metric-value { color: var(--co-warning); }
    .green { border-left-color: var(--co-success); } .green .metric-value { color: var(--co-success); }
    .violet { border-left-color: var(--co-primary); } .violet .metric-value { color: var(--co-primary); }
    .work { min-height: 0; flex: 1 1 auto; margin-top: 24px; background: var(--co-surface); border: 1px solid var(--co-border); border-radius: 8px; padding: 20px 22px; overflow: hidden; }
    .section-head { display: flex; align-items: baseline; justify-content: space-between; margin-bottom: 10px; }
    h2 { margin: 0; font-size: 16px; font-weight: 650; } .section-head span { color: var(--co-text-muted); font-size: 12px; }
    ul { list-style: none; margin: 0; padding: 0; }
    li { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: 42px; border-top: 1px solid var(--co-border); font-size: 13px; }
    .goal-copy { min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--co-text); }
    .goal-state { flex: 0 0 auto; color: var(--co-warning); background: var(--co-background-strong); border: 1px solid var(--co-border); border-radius: 5px; padding: 3px 7px; font-size: 10px; text-transform: uppercase; letter-spacing: 1px; }
    .empty { color: var(--co-text-muted); }
    footer { display: flex; justify-content: space-between; gap: 20px; padding-top: 17px; color: var(--co-text-muted); font-size: 11px; letter-spacing: .3px; }
    @media (max-aspect-ratio: 4/5) {
      main { padding: 34px 30px 24px; } h1 { font-size: 50px; }
      .metrics { grid-template-columns: 1fr; } .metric { min-height: 112px; }
      .metric-value { font-size: 40px; margin: 10px 0 7px; }
    }
    @media (min-aspect-ratio: 3/2) {
      main { padding: 34px 42px 24px; } header { padding-bottom: 20px; }
      .metrics { grid-template-columns: repeat(4, 1fr); } .metric { min-height: 138px; }
      .work { margin-top: 16px; } h1 { font-size: 52px; }
    }
  ]],
}
