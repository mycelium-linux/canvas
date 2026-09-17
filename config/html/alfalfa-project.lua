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
  css = [[
    * { box-sizing: border-box; }
    html, body { margin: 0; width: 100%; height: 100%; overflow: hidden; background: #0c1016; color: #f2f5f9; font-family: system-ui, sans-serif; }
    body { border-top: 3px solid #51c6dc; }
    main { width: 100vw; height: 100vh; padding: 42px 42px 30px; display: flex; flex-direction: column; background: #0c1016; }
    header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; padding-bottom: 28px; border-bottom: 1px solid #29313d; }
    .identity { min-width: 0; }
    .eyebrow { color: #62d5e9; font-size: 14px; letter-spacing: 3.4px; font-weight: 750; }
    h1 { font-size: 58px; line-height: .98; letter-spacing: -2px; margin: 10px 0 0; font-weight: 640; }
    .projects { margin-top: 20px; display: flex; flex-wrap: wrap; gap: 7px; max-height: 64px; overflow: hidden; }
    .projects span { color: #aeb8c5; background: #131922; border: 1px solid #303947; border-radius: 7px; padding: 6px 9px; font-size: 12px; }
    .state { flex: 0 0 auto; display: flex; align-items: center; gap: 9px; color: #b7c0cc; border: 1px solid #394351; border-radius: 7px; padding: 10px 13px; font-size: 12px; font-weight: 750; letter-spacing: 1.8px; }
    .state i { width: 8px; height: 8px; border-radius: 50%; background: #e3b659; }
    .state.live i { background: #67dc96; }
    .metrics { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-top: 24px; }
    .metric { min-height: 150px; padding: 20px 22px; background: #121821; border: 1px solid #293340; border-left-width: 3px; border-radius: 8px; }
    .metric-label { color: #a9b3c0; font-size: 15px; font-weight: 560; }
    .metric-value { font-size: 50px; line-height: 1; margin: 15px 0 10px; font-weight: 680; letter-spacing: -1.5px; }
    .metric-detail { color: #778391; font-size: 13px; }
    .cyan { border-left-color: #5ccfe3; } .cyan .metric-value { color: #69d7e9; }
    .amber { border-left-color: #dfb35d; } .amber .metric-value { color: #f0c76f; }
    .green { border-left-color: #64d891; } .green .metric-value { color: #72dfa0; }
    .violet { border-left-color: #a890ea; } .violet .metric-value { color: #b9a6ee; }
    .work { min-height: 0; flex: 1 1 auto; margin-top: 24px; background: #10161e; border: 1px solid #293340; border-radius: 8px; padding: 20px 22px; overflow: hidden; }
    .section-head { display: flex; align-items: baseline; justify-content: space-between; margin-bottom: 10px; }
    h2 { margin: 0; font-size: 16px; font-weight: 650; } .section-head span { color: #687482; font-size: 12px; }
    ul { list-style: none; margin: 0; padding: 0; }
    li { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: 42px; border-top: 1px solid #242c37; font-size: 13px; }
    .goal-copy { min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: #cbd3dd; }
    .goal-state { flex: 0 0 auto; color: #dcb55f; background: #211d14; border: 1px solid #493b20; border-radius: 5px; padding: 3px 7px; font-size: 10px; text-transform: uppercase; letter-spacing: 1px; }
    .empty { color: #737f8c; }
    footer { display: flex; justify-content: space-between; gap: 20px; padding-top: 17px; color: #65717f; font-size: 11px; letter-spacing: .3px; }
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
