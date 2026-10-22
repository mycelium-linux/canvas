-- Canvas presentation only. Input is an authenticated, normalized owner snapshot.
local M = {}
local last = nil
local function valid(value)
    assert(type(value)=="table" and value.schema_version==1, "invalid agent contract")
    assert(value.mode=="snapshot", "dashboard requires complete owner snapshots")
    assert(type(value.host_id)=="string" and type(value.provider)=="string"
        and type(value.instance_id)=="string", "missing owner scope")
    assert(type(value.sequence)=="number" and type(value.observed_at_unix_ms)=="number", "missing generation")
    assert(type(value.sessions)=="table" and #value.sessions<=512, "invalid roster")
end
function M.receive(value, now_ms)
    valid(value)
    assert(value.observed_at_unix_ms <= now_ms+5000, "future observation")
    -- This compact view follows one declared owner; it is not a fleet census.
    if last and last.instance_id==value.instance_id and value.sequence<=last.sequence then return end
    if last and value.observed_at_unix_ms<last.observed_at_unix_ms then return end
    last=value
end
function M.render(now_ms)
    local active,idle,unknown=0,0,0
    local seen={}
    local fresh=last and now_ms-last.observed_at_unix_ms<=30000
        and now_ms-last.observed_at_unix_ms>=-5000
    if last then
        for _,session in ipairs(last.sessions) do
            assert(type(session.id)=="string", "missing session identity")
            if not seen[session.id] then
                seen[session.id]=true
                if session.state=="ended" then
                elseif not fresh then unknown=unknown+1
                elseif session.state=="active" then active=active+1
                elseif session.state=="idle" then idle=idle+1
                else unknown=unknown+1 end
            end
        end
    end
    local function row(label,count,state)
        return {label=label,detail=last and tostring(count) or "—",state=state}
    end
    return {widget={kind="status_list",title="Agents",nodes={
        row("Active",active,"good"),row("Idle",idle,"neutral"),
        row("Unknown",unknown,unknown>0 and "warning" or "neutral")}},
        data={state=not last and "loading" or not fresh and "stale" or unknown>0 and "partial" or "ready",
            provenance="live",source="unibus:agent.sessions.v1",
            -- Host converts numeric timestamp to RFC3339; Lua does not own wall-clock IO.
            detail=last and (last.host_id .. " · " .. last.provider) or "Waiting for owner telemetry"},
        observed_at_unix_ms=last and last.observed_at_unix_ms or 0}
end
return M
