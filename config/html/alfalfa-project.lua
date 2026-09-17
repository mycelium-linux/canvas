local function esc(value)
  return tostring(value or ""):gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;")
end
local goals = data.goals or {}
local beads = data.beads or {}
local agents = data.agents or {}
local reviewers = data.reviewers or {}
local projects = data.project_ids or {}
local available = data.activity_state == "observed"
local function metric(label, value, detail, tone)
  return '<article class="metric ' .. tone .. '"><div class="label">' .. esc(label)
    .. '</div><div class="value">' .. esc(value) .. '</div><div class="detail">' .. esc(detail) .. '</div></article>'
end
local cards = metric("Active goals", goals.active or 0, tostring(goals.queued or 0) .. " queued", "cyan")
  .. metric("Open beads", available and (beads.open or 0) or "—", available and (tostring(beads.in_progress or 0) .. " moving") or "activity unavailable", "amber")
  .. metric("Running agents", available and (agents.running or 0) or "—", available and (tostring(agents.total or 0) .. " observed") or "activity unavailable", "green")
  .. metric("Reviewers", available and (reviewers.running or 0) or "—", available and (tostring(reviewers.total or 0) .. " observed") or "activity unavailable", "violet")
local chips = ""
for i = 1, #projects do chips = chips .. '<span>' .. esc(projects[i]) .. '</span>' end
return {
  html = '<main><header><div><div class="eyebrow">ALFALFA PROJECT</div><h1>' .. esc(data.name or data.workspace_id or "Project") .. '</h1><div class="projects">' .. chips .. '</div></div><div class="state">' .. (available and "LIVE" or "PARTIAL") .. '</div></header><section>' .. cards .. '</section></main>',
  css = [[
    * { box-sizing: border-box; } html, body { margin: 0; width: 100%; height: 100%; background: #11151b; color: #edf4ff; font-family: system-ui, sans-serif; }
    main { padding: 42px; min-height: 100vh; background: radial-gradient(circle at 85% 0%, #173244 0%, #11151b 42%); }
    header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 38px; }
    .eyebrow { color: #65d9ee; font-size: 15px; letter-spacing: 4px; font-weight: 700; } h1 { font-size: 54px; margin: 6px 0 0; font-weight: 620; }
    .projects { margin-top: 14px; display: flex; flex-wrap: wrap; gap: 7px; } .projects span { color: #aeb8c7; border: 1px solid #35404e; border-radius: 999px; padding: 5px 9px; font-size: 12px; }
    .state { border: 1px solid #d6a94e; color: #ffd477; border-radius: 999px; padding: 9px 15px; font-size: 14px; letter-spacing: 2px; }
    section { display: grid; grid-template-columns: 1fr 1fr; gap: 18px; }
    .metric { min-height: 178px; padding: 24px; background: rgba(255,255,255,.045); border: 1px solid #35404e; border-radius: 18px; }
    .label { color: #aeb8c7; font-size: 17px; } .value { font-size: 58px; line-height: 1.05; margin: 16px 0 8px; font-weight: 650; } .detail { color: #7f8a99; font-size: 15px; }
    .cyan .value { color: #65d9ee; } .amber .value { color: #ffd477; } .green .value { color: #73df9c; } .violet .value { color: #b9a0ff; }
  ]],
}
